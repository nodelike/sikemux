//! The slash commands Codex's adapter answers itself rather than sending to
//! the model, and the list of them the chat offers.

use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub(super) enum Command {
    Compact,
    Review(Value),
    Rename(String),
    Skills,
    Mcp,
    Status,
    /// A command used without what it needs: the line saying so.
    Usage(String),
}

/// The command a prompt starts with, if it is one handled here. Anything
/// else, `$skill` mentions included, goes to the model as written.
pub(super) fn parse(text: &str) -> Option<Command> {
    let text = text.trim().strip_prefix('/')?.trim();
    let name = text.split_whitespace().next()?;
    let rest = text[name.len()..].trim();
    let name = name.to_lowercase();
    let usage = |hint: &str| {
        Some(Command::Usage(format!(
            "Command \"/{name}\" requires {hint}."
        )))
    };
    match name.as_str() {
        "compact" => Some(Command::Compact),
        "review" if rest.is_empty() => {
            Some(Command::Review(json!({ "type": "uncommittedChanges" })))
        }
        "review" => Some(Command::Review(
            json!({ "type": "custom", "instructions": rest }),
        )),
        "review-branch" if rest.is_empty() => usage("branch name"),
        "review-branch" => Some(Command::Review(
            json!({ "type": "baseBranch", "branch": rest }),
        )),
        "review-commit" if rest.is_empty() => usage("commit sha"),
        "review-commit" => Some(Command::Review(
            json!({ "type": "commit", "sha": rest, "title": null }),
        )),
        "rename" if rest.is_empty() => usage("new name"),
        "rename" => Some(Command::Rename(rest.to_owned())),
        "skills" => Some(Command::Skills),
        "mcp" => Some(Command::Mcp),
        "status" => Some(Command::Status),
        _ => None,
    }
}

/// The commands the chat offers: the ones handled here, then the skills
/// `skills/list` found.
pub(super) fn available(skills: &Value) -> Value {
    let command = |name: &str, description: &str, hint: Option<&str>| {
        json!({
            "name": name,
            "description": description,
            "input": hint.map(|hint| json!({ "hint": hint })),
        })
    };
    let mut commands = vec![
        command(
            "mcp",
            "List configured Model Context Protocol (MCP) tools.",
            None,
        ),
        command("skills", "List available skills.", None),
        command(
            "status",
            "Display session configuration and token usage.",
            None,
        ),
        command(
            "review",
            "Review uncommitted changes, or review with custom instructions.",
            Some("optional review instructions"),
        ),
        command(
            "review-branch",
            "Review changes relative to a base branch.",
            Some("branch name"),
        ),
        command(
            "review-commit",
            "Review a specific commit.",
            Some("commit sha"),
        ),
        command(
            "compact",
            "Summarize conversation to avoid hitting the context limit.",
            None,
        ),
        command("rename", "Rename the current session.", Some("new name")),
    ];
    for skill in skill_list(skills) {
        let name = format!("${}", skill["name"].as_str().unwrap_or_default());
        if commands.iter().any(|known| known["name"] == name.as_str()) {
            continue;
        }
        let description = skill["shortDescription"]
            .as_str()
            .or_else(|| skill["description"].as_str())
            .or_else(|| skill["name"].as_str())
            .unwrap_or_default();
        commands.push(command(&name, description, None));
    }
    Value::Array(commands)
}

fn skill_list(skills: &Value) -> impl Iterator<Item = &Value> {
    skills["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["skills"].as_array().into_iter().flatten())
        .filter(|skill| skill["name"].is_string())
}

/// `/skills`: the skills, one per line.
pub(super) fn skills_text(skills: &Value) -> String {
    let lines: Vec<String> = skill_list(skills)
        .map(|skill| {
            let name = skill["name"].as_str().unwrap_or_default();
            match skill["shortDescription"]
                .as_str()
                .or_else(|| skill["description"].as_str())
                .filter(|description| !description.is_empty())
            {
                Some(description) => format!("- {name}: {description}"),
                None => format!("- {name}"),
            }
        })
        .collect();
    if lines.is_empty() {
        "No skills configured.".to_owned()
    } else {
        format!("Available skills:\n{}", lines.join("\n"))
    }
}

/// `/mcp`: Codex's tool servers and the chat's own.
pub(super) fn servers_text(statuses: &[Value], own: &[String]) -> String {
    let mut lines: Vec<String> = statuses
        .iter()
        .map(|server| {
            let tools = server["tools"].as_object().map_or(0, serde_json::Map::len);
            let resources = server["resources"].as_array().map_or(0, Vec::len);
            format!(
                "- {}: {tools} tools, {resources} resources, auth={}",
                server["name"].as_str().unwrap_or_default(),
                server["authStatus"].as_str().unwrap_or("unknown"),
            )
        })
        .collect();
    lines.extend(own.iter().map(|name| format!("- {name}")));
    if lines.is_empty() {
        "No MCP servers configured.".to_owned()
    } else {
        format!("Configured MCP servers:\n{}", lines.join("\n"))
    }
}

fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}K", tokens as f64 / 1_000.0),
        _ => format!("{:.2}M", tokens as f64 / 1_000_000.0),
    }
}

pub(super) struct Status<'a> {
    pub model: &'a str,
    pub cwd: &'a str,
    pub approval: &'a str,
    pub sandbox: &'a str,
    pub account: &'a Value,
    pub session: &'a str,
    pub token_usage: Option<&'a Value>,
}

/// `/status`: the session's settings and what it has used.
pub(super) fn status_text(status: &Status) -> String {
    let account = match status.account["type"].as_str() {
        None => "not logged in".to_owned(),
        Some("apiKey") => "API key configured".to_owned(),
        Some("chatgpt") => format!(
            "ChatGPT {} ({})",
            status.account["planType"].as_str().unwrap_or("unknown"),
            status.account["email"].as_str().unwrap_or_default()
        ),
        Some("amazonBedrock") => "Amazon Bedrock".to_owned(),
        Some(_) => "unknown".to_owned(),
    };
    let tokens = |usage: &Value, key: &str| usage[key].as_u64().unwrap_or(0);
    let total = status.token_usage.map(|usage| &usage["total"]);
    let used = total.map_or_else(
        || "data not available yet".to_owned(),
        |total| {
            format!(
                "{} total  ({} input + {} cached input, {} output)",
                count(tokens(total, "totalTokens")),
                count(tokens(total, "inputTokens")),
                count(tokens(total, "cachedInputTokens")),
                count(tokens(total, "outputTokens")),
            )
        },
    );
    let window = status
        .token_usage
        .and_then(|usage| {
            let size = usage["modelContextWindow"]
                .as_u64()
                .filter(|size| *size > 0)?;
            let used = usage["last"]["totalTokens"].as_u64()?;
            let left = (size.saturating_sub(used) as f64 / size as f64 * 100.0).round();
            Some(format!(
                "{left}% left ({} used / {})",
                count(used),
                count(size)
            ))
        })
        .unwrap_or_else(|| "data not available yet".to_owned());
    [
        format!("**Model:** {}", status.model),
        format!("**Directory:** {}", status.cwd),
        format!("**Approval:** {}", status.approval),
        format!("**Sandbox:** {}", status.sandbox),
        format!("**Account:** {account}"),
        format!("**Session:** `{}`", status.session),
        String::new(),
        format!("**Token usage:** {used}"),
        format!("**Context window:** {window}"),
    ]
    .join("  \n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_from_the_start_of_a_prompt() {
        assert_eq!(parse("/compact"), Some(Command::Compact));
        assert_eq!(
            parse("  /Review  "),
            Some(Command::Review(json!({ "type": "uncommittedChanges" })))
        );
        assert_eq!(
            parse("/review look at auth"),
            Some(Command::Review(
                json!({ "type": "custom", "instructions": "look at auth" })
            ))
        );
        assert_eq!(
            parse("/review-branch main"),
            Some(Command::Review(
                json!({ "type": "baseBranch", "branch": "main" })
            ))
        );
        assert_eq!(
            parse("/rename"),
            Some(Command::Usage(
                "Command \"/rename\" requires new name.".into()
            ))
        );
        assert_eq!(
            parse("/rename My chat"),
            Some(Command::Rename("My chat".into()))
        );
        assert_eq!(parse("/$deploy now"), None);
        assert_eq!(parse("/plan"), None);
        assert_eq!(parse("fix /compact"), None);
        assert_eq!(parse("/"), None);
    }

    #[test]
    fn skills_join_the_commands_offered() {
        let skills = json!({ "data": [{ "cwd": "/w", "skills": [
            { "name": "deploy", "description": "Ship it", "shortDescription": "Ship" },
            { "name": "lint", "description": "" },
        ], "errors": [] }] });
        let commands = available(&skills);
        let names: Vec<_> = commands
            .as_array()
            .unwrap()
            .iter()
            .map(|command| command["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.last(), Some(&"$lint"));
        assert!(names.contains(&"compact"));
        let deploy = commands
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "$deploy")
            .unwrap();
        assert_eq!(deploy["description"], "Ship");
        assert_eq!(deploy["input"], Value::Null);
        assert_eq!(
            skills_text(&skills),
            "Available skills:\n- deploy: Ship\n- lint"
        );
        assert_eq!(skills_text(&json!({ "data": [] })), "No skills configured.");
    }

    #[test]
    fn status_names_the_settings_and_usage() {
        let usage = json!({
            "total": { "totalTokens": 12_500, "inputTokens": 10_000, "cachedInputTokens": 2_000, "outputTokens": 500 },
            "last": { "totalTokens": 50_000 },
            "modelContextWindow": 200_000,
        });
        let text = status_text(&Status {
            model: "gpt-6-astra",
            cwd: "/w",
            approval: "never",
            sandbox: "danger-full-access",
            account: &json!({ "type": "chatgpt", "planType": "plus", "email": "me@x" }),
            session: "t-1",
            token_usage: Some(&usage),
        });
        assert!(text.contains("**Model:** gpt-6-astra"));
        assert!(text.contains("**Account:** ChatGPT plus (me@x)"));
        assert!(text.contains(
            "**Token usage:** 12.5K total  (10.0K input + 2.0K cached input, 500 output)"
        ));
        assert!(text.contains("**Context window:** 75% left"));
    }
}
