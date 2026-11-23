//! Antigravity's tool calls under the names the chat reads, and the errors it ends turns on.
//! It titles a tool server's tool `Run mcp_<server>_<tool>`, calls a command `CommandLine`
//! and its label `toolSummary`, and heads what a command printed with when it ran.

use serde_json::{json, Map, Value};
use sikemux_fold::update::mcp_title;
use sikemux_fold::UpdateKind;

use super::mcp_calls::split;
use crate::acp::failure::{Failure, FailureKind};

const PRINTED: &str = "Command output:\n";

pub(super) fn as_common_call(update: &mut Value, servers: &[String]) {
    if !UpdateKind::of(update).is_some_and(UpdateKind::is_tool) {
        return;
    }
    let summary = update["rawInput"]
        .as_object_mut()
        .and_then(|input| input.remove("toolSummary"));
    let named = update["name"]
        .as_str()
        .and_then(|name| name.strip_prefix("mcp_"))
        .and_then(|name| split(name, servers));
    if let Some((server, tool)) = named {
        update["title"] = json!(mcp_title(&server, &tool));
        return;
    }
    if update["kind"] != "execute" {
        return;
    }
    if let Some(input) = update["rawInput"].as_object_mut() {
        rename(input, "CommandLine", "command");
        if let Some(summary) = summary {
            input.insert("description".to_owned(), summary);
        }
    }
    if let Some(output) = update.pointer_mut("/rawOutput/output") {
        trim_head(output);
    }
    if let Some(blocks) = update["content"].as_array_mut() {
        for block in blocks {
            if let Some(text) = block.pointer_mut("/content/text") {
                trim_head(text);
            }
        }
    }
}

fn rename(input: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = input.remove(from) {
        input.insert(to.to_owned(), value);
    }
}

/// What a command printed, without the times and exit line Antigravity writes above it.
fn trim_head(text: &mut Value) {
    let Some(said) = text.as_str() else {
        return;
    };
    let printed = said
        .split_once(PRINTED)
        .map_or(said, |(_, printed)| printed);
    *text = json!(printed.replace("\r\n", "\n"));
}

/// The failure an error step names, such as `RESOURCE_EXHAUSTED (code 429): …` or
/// `Error 502, Message: …, Status: …`, titled by its message and kept whole in the details.
pub(super) fn turn_failure(line: String) -> Failure {
    let limited = [
        "(code 429)",
        "Error 429,",
        "RESOURCE_EXHAUSTED",
        "QUOTA_EXHAUSTED",
    ]
    .iter()
    .any(|sign| line.contains(sign));
    let message = line
        .split_once("): ")
        .or_else(|| line.split_once(", Message: "))
        .map(|(_, message)| message.split(", Status: ").next().unwrap_or(message))
        .map(|message| message.trim_start_matches("* ").trim())
        .filter(|message| !message.is_empty())
        .unwrap_or(&line)
        .to_owned();
    Failure {
        kind: if limited {
            FailureKind::Limit
        } else {
            FailureKind::Other
        },
        details: (message != line).then_some(line),
        ..Failure::other(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sikemux_tool_call_reads_as_the_tool() {
        let mut update = json!({
            "sessionUpdate": "tool_call", "toolCallId": "call_1",
            "name": "mcp_sikemux-tools_agent_list", "args": { "toolSummary": "List Sikemux agents" },
            "status": "in_progress", "kind": "other", "title": "Run mcp_sikemux-tools_agent_list",
            "rawInput": { "toolSummary": "List Sikemux agents" }
        });
        as_common_call(&mut update, &["sikemux-tools".to_owned()]);
        assert_eq!(update["title"], "mcp__sikemux-tools__agent_list");
        assert_eq!(update["rawInput"], json!({}));
    }

    #[test]
    fn a_command_reads_as_one() {
        let printed = "Created At: 2026-11-22T06:30:27+05:30\nCompleted At: 2026-11-22T06:30:28+05:30\nCommand exited with code 0.\n\nCommand output:\n./notes.txt:1:two\r\n";
        let mut update = json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "call_2", "name": "run_command",
            "status": "completed", "kind": "execute", "title": "grep -rn \"two\" .",
            "rawInput": { "CommandLine": "grep -rn \"two\" .", "WaitMsBeforeAsync": 5000, "toolSummary": "Search folder for two" },
            "rawOutput": { "output": printed, "exit_code": 0 },
            "content": [{ "type": "content", "content": { "type": "text", "text": printed } }]
        });
        as_common_call(&mut update, &[]);
        assert_eq!(update["rawInput"]["command"], "grep -rn \"two\" .");
        assert_eq!(update["rawInput"]["description"], "Search folder for two");
        assert_eq!(update["rawOutput"]["output"], "./notes.txt:1:two\n");
        assert_eq!(
            update["content"][0]["content"]["text"],
            "./notes.txt:1:two\n"
        );
    }
}
