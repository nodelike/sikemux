//! The slash commands the chat answers for Muse rather than sending to the
//! model, Muse's skills, and the list of both the chat offers.

use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub(super) enum Command {
    Compact,
    Rename(String),
    Status,
    Usage,
    /// One of Muse's skills, with what was written after it.
    Skill {
        selector: String,
        arguments: String,
    },
    /// A command used without what it needs: the line saying so.
    Hint(String),
}

/// The command a prompt starts with, if it is one handled here. A skill is
/// one when `skills` names it.
pub(super) fn parse(text: &str, skills: &Value) -> Option<Command> {
    let text = text.trim().strip_prefix('/')?.trim_start();
    let name = text.split_whitespace().next()?;
    let rest = text[name.len()..].trim();
    match name.to_lowercase().as_str() {
        "compact" => Some(Command::Compact),
        "rename" if rest.is_empty() => Some(Command::Hint(
            "Command \"/rename\" requires a new name.".into(),
        )),
        "rename" => Some(Command::Rename(rest.to_owned())),
        "status" => Some(Command::Status),
        "usage" => Some(Command::Usage),
        _ => skill_list(skills)
            .any(|skill| skill["selector"] == name)
            .then(|| Command::Skill {
                selector: name.to_owned(),
                arguments: rest.to_owned(),
            }),
    }
}

fn skill_list(skills: &Value) -> impl Iterator<Item = &Value> {
    skills["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|skill| skill["selector"].is_string())
}

/// The commands the chat offers: the ones handled here, then Muse's skills.
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
            "compact",
            "Summarize the conversation to free up context.",
            None,
        ),
        command("rename", "Rename the current session.", Some("new name")),
        command(
            "status",
            "Show the session's model, settings and token usage.",
            None,
        ),
        command("usage", "Show how much of your Muse plan is used.", None),
    ];
    for skill in skill_list(skills) {
        let name = skill["selector"].as_str().unwrap_or_default();
        if commands.iter().any(|known| known["name"] == name) {
            continue;
        }
        let description = skill["description"]
            .as_str()
            .or_else(|| skill["displayName"].as_str())
            .unwrap_or(name);
        let description = description.lines().next().unwrap_or(description);
        commands.push(command(name, description, Some("optional instructions")));
    }
    Value::Array(commands)
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
    pub effort: Option<&'a str>,
    pub cwd: &'a str,
    pub approval: &'a str,
    pub sandbox: &'a str,
    pub session: &'a str,
    /// The latest `session/tokenUsage` running totals.
    pub totals: Option<&'a Value>,
    /// The latest `session/contextUsage`.
    pub context: Option<&'a Value>,
}

/// `/status`: the session's settings and what it has used.
pub(super) fn status_text(status: &Status) -> String {
    let tokens = |usage: &Value, key: &str| usage[key].as_u64().unwrap_or(0);
    let used = status.totals.map_or_else(
        || "data not available yet".to_owned(),
        |totals| {
            format!(
                "{} total  ({} prompt, {} output)",
                count(tokens(totals, "totalTokens")),
                count(tokens(totals, "promptTokens")),
                count(tokens(totals, "outputTokens")),
            )
        },
    );
    let window = status
        .context
        .and_then(|context| {
            let size = context["windowTokens"].as_u64().filter(|size| *size > 0)?;
            let used = context["usedTokens"].as_u64()?;
            let left = (size.saturating_sub(used) as f64 / size as f64 * 100.0).round();
            Some(format!(
                "{left}% left ({} used / {})",
                count(used),
                count(size)
            ))
        })
        .unwrap_or_else(|| "data not available yet".to_owned());
    let model = match status.effort {
        Some(effort) => format!("{} ({effort} effort)", status.model),
        None => status.model.to_owned(),
    };
    [
        format!("**Model:** {model}"),
        format!("**Directory:** {}", status.cwd),
        format!("**Approval:** {}", status.approval),
        format!("**Sandbox:** {}", status.sandbox),
        format!("**Session:** `{}`", status.session),
        String::new(),
        format!("**Token usage:** {used}"),
        format!("**Context window:** {window}"),
    ]
    .join("  \n")
}

/// `/usage`: the plan's current window and week, as `usage/read` tells them.
pub(super) fn usage_text(read: &Value) -> String {
    let usage = &read["usage"];
    if usage.is_null() {
        return "Muse has not reported your plan's usage yet. It does after the first answer."
            .to_owned();
    }
    let window = &usage["window"];
    let weekly = &usage["weekly"];
    let mut lines = Vec::new();
    if let Some(tier) = usage["tier"].as_str() {
        lines.push(format!("**Plan:** {tier}"));
    }
    if let Some(percent) = window["usedPercent"].as_u64() {
        let hours = window["windowDurationMins"].as_u64().unwrap_or(0) / 60;
        lines.push(format!("**{hours}-hour window:** {percent}% used"));
    }
    if let Some(percent) = weekly["usedPercent"].as_u64() {
        lines.push(format!("**This week:** {percent}% used"));
    }
    lines.join("  \n")
}
