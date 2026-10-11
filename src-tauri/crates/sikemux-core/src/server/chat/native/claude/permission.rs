//! What the person is asked when Claude Code wants to use a tool, and how
//! their answer goes back to it.

use std::path::Path;

use serde_json::{json, Value};

use super::tools;

const ALLOW: &str = "allow-once";
const ALLOW_ALWAYS: &str = "allow-always";
const REJECT: &str = "reject";
const EXIT_PLAN: &str = "exit-plan";
const KEEP_PLANNING: &str = "keep-planning";

fn option(id: &str, name: &str, kind: &str) -> Value {
    json!({ "optionId": id, "name": name, "kind": kind })
}

fn suggestions(request: &Value) -> Option<&Value> {
    let suppressed = request
        .get("suppress_always_allow_rule")
        .and_then(Value::as_bool)
        == Some(true)
        || request
            .get("matched_ask_rule")
            .is_some_and(|rule| !rule.is_null());
    request.get("permission_suggestions").filter(|suggestions| {
        !suppressed && suggestions.as_array().is_some_and(|list| !list.is_empty())
    })
}

/// The commands a suggestion would stop asking about, such as `git status`.
fn suggested_commands(suggestions: &Value) -> Vec<String> {
    suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|suggestion| suggestion["type"] == "addRules")
        .flat_map(|suggestion| suggestion["rules"].as_array().cloned().unwrap_or_default())
        .filter_map(|rule| {
            rule.get("ruleContent")
                .and_then(Value::as_str)
                .map(|content| content.trim_end_matches(":*").to_owned())
        })
        .collect()
}

fn always_label(name: &str, request: &Value, suggestions: &Value) -> String {
    let display = request
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or(name);
    if matches!(name, "Bash" | "PowerShell") {
        let commands = suggested_commands(suggestions);
        let joined = commands
            .iter()
            .map(|command| format!("`{command}`"))
            .collect::<Vec<_>>()
            .join(", ");
        if !commands.is_empty() && joined.len() <= 50 {
            return format!("Yes, and don't ask again for {joined} commands");
        }
        if !commands.is_empty() {
            return "Yes, and don't ask again for similar commands".to_owned();
        }
    }
    let session_wide = suggestions
        .as_array()
        .into_iter()
        .flatten()
        .any(|suggestion| suggestion["destination"] == "session");
    if session_wide {
        return "Yes, during this session".to_owned();
    }
    format!("Yes, and don't ask again for {display} commands")
}

/// The ACP permission request for Claude Code's `can_use_tool` request, in
/// the session the tool call belongs to. `mode` is the permission mode a plan
/// that is approved moves the session to.
pub(crate) fn request(
    session: &str,
    tool_id: &str,
    name: &str,
    input: &Value,
    cli: &Value,
    cwd: &Path,
    mode: &str,
) -> Value {
    let info = tools::tool_info(name, input, cwd);
    let mut locations = info.locations.clone();
    if let Some(blocked) = cli.get("blocked_path").and_then(Value::as_str) {
        if !locations.iter().any(|location| location["path"] == blocked) {
            locations.push(json!({ "path": blocked }));
        }
    }
    let mut options = match name {
        "ExitPlanMode" => vec![
            option(
                &format!("{EXIT_PLAN}:{mode}"),
                "Yes, start implementing",
                "allow_once",
            ),
            option(KEEP_PLANNING, "No, keep planning", "reject_once"),
        ],
        "EnterPlanMode" => vec![
            option(ALLOW, "Yes, enter plan mode", "allow_once"),
            option(REJECT, "No, start implementing now", "reject_once"),
        ],
        _ => {
            let mut options = vec![option(ALLOW, "Yes", "allow_once")];
            if let Some(suggestions) = suggestions(cli) {
                options.push(option(
                    ALLOW_ALWAYS,
                    &always_label(name, cli, suggestions),
                    "allow_always",
                ));
            }
            options.push(option(REJECT, "No", "reject_once"));
            options
        }
    };
    if cli.get("default_to_no").and_then(Value::as_bool) == Some(true) {
        options.sort_by_key(|option| {
            !option["kind"]
                .as_str()
                .unwrap_or_default()
                .starts_with("reject")
        });
    }
    let mut tool_call = json!({
        "toolCallId": tool_id,
        "title": info.title,
        "kind": info.kind,
        "status": "pending",
        "rawInput": input,
        "content": info.content,
    });
    if !locations.is_empty() {
        tool_call["locations"] = json!(locations);
    }
    json!({ "sessionId": session, "toolCall": tool_call, "options": options })
}

/// Claude Code's answer for the option the person chose, or for no answer.
pub(crate) fn answer(tool_id: &str, input: &Value, cli: &Value, chosen: Option<&str>) -> Value {
    let allow = || json!({ "behavior": "allow", "updatedInput": input, "toolUseID": tool_id });
    match chosen {
        Some(ALLOW) => allow(),
        Some(ALLOW_ALWAYS) => {
            let mut answer = allow();
            if let Some(suggestions) = suggestions(cli) {
                answer["updatedPermissions"] = suggestions.clone();
            }
            answer
        }
        Some(chosen) if chosen.starts_with(EXIT_PLAN) => {
            let mut answer = allow();
            if let Some(mode) = chosen.strip_prefix(&format!("{EXIT_PLAN}:")) {
                answer["updatedPermissions"] =
                    json!([{ "type": "setMode", "mode": mode, "destination": "session" }]);
            }
            answer
        }
        Some(KEEP_PLANNING) => json!({
            "behavior": "deny",
            "message": "User chose to keep planning",
            "interrupt": true,
            "toolUseID": tool_id,
        }),
        Some(_) => json!({
            "behavior": "deny",
            "message": "User refused permission to run tool",
            "toolUseID": tool_id,
        }),
        None => json!({
            "behavior": "deny",
            "message": "The request was cancelled",
            "interrupt": true,
            "toolUseID": tool_id,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(suggestions: Value) -> Value {
        json!({
            "tool_name": "Bash",
            "input": { "command": "git status" },
            "tool_use_id": "t1",
            "permission_suggestions": suggestions,
        })
    }

    #[test]
    fn a_suggested_rule_becomes_an_always_option() {
        let cli = bash(
            json!([{ "type": "addRules", "rules": [{ "toolName": "Bash", "ruleContent": "git status:*" }], "behavior": "allow", "destination": "localSettings" }]),
        );
        let asked = request(
            "s",
            "t1",
            "Bash",
            &cli["input"],
            &cli,
            Path::new("/repo"),
            "bypassPermissions",
        );
        let ids: Vec<&str> = asked["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["optionId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [ALLOW, ALLOW_ALWAYS, REJECT]);
        assert_eq!(
            asked["options"][1]["name"],
            "Yes, and don't ask again for `git status` commands"
        );
        let answered = answer("t1", &cli["input"], &cli, Some(ALLOW_ALWAYS));
        assert_eq!(answered["behavior"], "allow");
        assert_eq!(answered["updatedPermissions"][0]["type"], "addRules");
    }

    #[test]
    fn without_suggestions_there_is_no_always_option() {
        let cli = bash(json!([]));
        let asked = request(
            "s",
            "t1",
            "Bash",
            &cli["input"],
            &cli,
            Path::new("/repo"),
            "acceptEdits",
        );
        assert_eq!(asked["options"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn approving_a_plan_keeps_the_chats_permissions() {
        let cli = json!({ "tool_name": "ExitPlanMode", "input": { "plan": "do it" } });
        let asked = request(
            "s",
            "t1",
            "ExitPlanMode",
            &cli["input"],
            &cli,
            Path::new("/repo"),
            "bypassPermissions",
        );
        let id = asked["options"][0]["optionId"].as_str().unwrap().to_owned();
        let answered = answer("t1", &cli["input"], &cli, Some(&id));
        assert_eq!(
            answered["updatedPermissions"][0]["mode"],
            "bypassPermissions"
        );
        let kept = answer("t1", &cli["input"], &cli, Some(KEEP_PLANNING));
        assert_eq!(kept["interrupt"], true);
    }

    #[test]
    fn a_cancelled_request_stops_the_tool() {
        let cli = bash(json!([]));
        let answered = answer("t1", &cli["input"], &cli, None);
        assert_eq!(answered["behavior"], "deny");
        assert_eq!(answered["interrupt"], true);
    }
}
