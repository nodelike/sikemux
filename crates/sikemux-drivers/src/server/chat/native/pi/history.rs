//! A resumed session's history told to the chat as the updates it would have
//! seen live, read from Pi's session entries, which hold every branch.

use std::collections::HashMap;

use serde_json::{json, Value};
use sikemux_fold::update::{self, agent_text, compaction, notice, stamp_replayed, thought};
use sikemux_fold::UpdateKind;

use crate::accounts::live::parse_rfc3339;

use super::super::json::{joined_text, text, text_or_empty};
use super::tools;

/// The entries of the branch ending at `leaf`, oldest first.
pub fn branch<'a>(entries: &'a [Value], leaf: Option<&str>) -> Vec<&'a Value> {
    let by_id: HashMap<&str, &Value> = entries
        .iter()
        .filter_map(|entry| Some((text(entry, "id")?, entry)))
        .collect();
    let mut walked = Vec::new();
    let mut at = leaf;
    while let Some(entry) = at.and_then(|id| by_id.get(id)) {
        if walked.len() > entries.len() {
            break;
        }
        walked.push(*entry);
        at = text(entry, "parentId");
    }
    walked.reverse();
    walked
}

/// The first user message among `entries`, the new ones since a turn began.
/// The words of the first user message on a branch, to title a session Pi has no name for.
pub(super) fn first_prompt(branch: &[&Value]) -> Option<String> {
    let entry = branch
        .iter()
        .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "user")?;
    crate::server::chat::feed::title(&user_text(&entry["message"]))
}

pub(super) fn first_user_entry(entries: &[&Value]) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "user")
        .and_then(|entry| text(entry, "id"))
        .map(str::to_owned)
}

fn user_text(message: &Value) -> String {
    joined_text(message.get("content"))
}

fn assistant(id: &str, message: &Value, results: &HashMap<&str, &Value>, cwd: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (at, block) in message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let part = format!("{id}:{at}");
        match block["type"].as_str() {
            Some("text") => {
                out.extend(text(block, "text").map(|said| agent_text(said, Some(&part))))
            }
            Some("thinking") => {
                out.extend(text(block, "thinking").map(|said| thought(said, Some(&part))))
            }
            Some("toolCall") => {
                let call_id = text_or_empty(block, "id");
                let name = text_or_empty(block, "name");
                out.push(tools::call(
                    call_id,
                    name,
                    &block["arguments"],
                    cwd,
                    "in_progress",
                ));
                if let Some(result) = results.get(call_id) {
                    let failed = result["isError"] == true;
                    out.push(tools::done(call_id, name, result, failed));
                }
            }
            _ => {}
        }
    }
    if message["stopReason"] == "error" {
        if let Some(said) = text(message, "errorMessage").filter(|said| !said.contains("aborted")) {
            out.push(notice("error", said, None));
        }
    }
    out
}

fn bash_execution(id: &str, message: &Value, cwd: &str) -> Vec<Value> {
    let command = text_or_empty(message, "command");
    let failed = message["exitCode"].as_i64().is_some_and(|code| code != 0);
    vec![
        tools::call(
            id,
            "bash",
            &json!({ "command": command }),
            cwd,
            "in_progress",
        ),
        update::terminal_done(
            id,
            if failed { "failed" } else { "completed" },
            text_or_empty(message, "output"),
        ),
    ]
}

/// The updates replaying `branch`. A person's message is told under its
/// entry's id, which is what going back to before it names.
pub fn replay(branch: &[&Value], cwd: &str) -> Vec<Value> {
    let results: HashMap<&str, &Value> = branch
        .iter()
        .filter(|entry| entry["message"]["role"] == "toolResult")
        .filter_map(|entry| Some((text(&entry["message"], "toolCallId")?, &entry["message"])))
        .collect();
    let mut out = Vec::new();
    for entry in branch {
        let id = text_or_empty(entry, "id");
        let at = text(entry, "timestamp").and_then(parse_rfc3339);
        let from = out.len();
        match entry["type"].as_str() {
            Some("compaction") => out.push(compaction(id, true, "completed", None)),
            Some("message") => {
                let message = &entry["message"];
                match message["role"].as_str() {
                    Some("user") => {
                        let said = crate::sessions::for_agent::person_wrote(&user_text(message));
                        if !said.trim().is_empty() {
                            out.push(json!({
                                "sessionUpdate": UpdateKind::UserMessageChunk,
                                "messageId": id,
                                "content": { "type": "text", "text": said },
                            }));
                        }
                    }
                    Some("assistant") => out.extend(assistant(id, message, &results, cwd)),
                    Some("bashExecution") => out.extend(bash_execution(id, message, cwd)),
                    _ => {}
                }
            }
            _ => {}
        }
        for update in &mut out[from..] {
            stamp_replayed(update, at, false);
        }
    }
    out
}
