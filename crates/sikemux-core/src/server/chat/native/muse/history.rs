//! A resumed session's history told to the chat as the updates it would have
//! seen live.

use sikemux_fold::UpdateKind;
use std::collections::HashMap;

use serde_json::{json, Value};

use crate::accounts::live::parse_rfc3339;

use super::super::super::states::{self, Subagent};
use super::super::json::{text, text_or_empty};
use super::items::{self, subagent_name, subagent_result, subagent_task};

pub(super) use super::super::super::feed::title;

/// How deep subagents of subagents are told.
const MAX_DEPTH: usize = 4;

/// An RFC 3339 time such as `2026-10-13T03:59:22.120239Z`, in Unix
/// milliseconds. A time without a zone is read as UTC.
fn unix_ms(stamp: &str) -> Option<u64> {
    parse_rfc3339(stamp).or_else(|| parse_rfc3339(&format!("{stamp}Z")))
}

/// The session's name, or else its first words, as the chat's title.
pub(super) fn session_title(session: &Value) -> Option<String> {
    ["name", "title", "firstUserPrompt"]
        .iter()
        .find_map(|key| text(session, key).and_then(title))
}

/// The ids of the turns `items` hold, in order.
pub(super) fn turns(items: &[Value]) -> Vec<String> {
    let mut turns: Vec<String> = Vec::new();
    for item in items {
        if let Some(turn) = item["turnId"].as_str() {
            if !turns.iter().any(|known| known == turn) {
                turns.push(turn.to_owned());
            }
        }
    }
    turns
}

/// The subagent sessions `items` opened.
pub(super) fn subagents(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|item| item["kind"] == "subagent")
        .filter_map(|item| item["childSessionId"].as_str().map(str::to_owned))
        .collect()
}

/// The updates replaying `items` of session `session`. A person's message is
/// told under the id of the turn it began, which is what going back to before
/// it names.
pub(super) fn replay(
    session: &str,
    items: &[Value],
    children: &HashMap<String, Vec<Value>>,
    cwd: &str,
    out: &mut Vec<(String, Value)>,
) {
    tell(session, items, children, cwd, 0, out);
}

fn tell(
    session: &str,
    items: &[Value],
    children: &HashMap<String, Vec<Value>>,
    cwd: &str,
    depth: usize,
    out: &mut Vec<(String, Value)>,
) {
    let mut opened_turns: Vec<&str> = Vec::new();
    for item in items {
        let from = out.len();
        let at = text(item, "recordedAt").and_then(unix_ms);
        match text_or_empty(item, "kind") {
            "userMessage" => {
                let turn = text_or_empty(item, "turnId");
                let id = if opened_turns.contains(&turn) {
                    text_or_empty(item, "itemId")
                } else {
                    opened_turns.push(turn);
                    turn
                };
                let said = text(item, "displayText").or_else(|| text(item, "text"));
                if let Some(said) = said {
                    out.push((
                        session.to_owned(),
                        json!({
                            "sessionUpdate": UpdateKind::UserMessageChunk,
                            "messageId": id,
                            "content": { "type": "text", "text": said },
                        }),
                    ));
                }
            }
            "subagent" => {
                let Some(child) = item["childSessionId"].as_str() else {
                    continue;
                };
                out.push((
                    session.to_owned(),
                    states::spawned(child, &subagent_name(item), &subagent_task(item)),
                ));
                match children.get(child).filter(|_| depth < MAX_DEPTH) {
                    Some(said) if said.iter().any(|item| item["kind"] != "userMessage") => {
                        tell(child, said, children, cwd, depth + 1, out)
                    }
                    _ => {
                        if let Some(result) = subagent_result(item) {
                            out.push((
                                child.to_owned(),
                                sikemux_fold::update::agent_text(&result, None),
                            ));
                        }
                    }
                }
                let state = match text_or_empty(item, "status") {
                    "completed" => Subagent::Completed,
                    "cancelled" => Subagent::Cancelled,
                    "inProgress" => Subagent::Disconnected,
                    _ => Subagent::Failed,
                };
                out.push((session.to_owned(), state.update(child)));
            }
            _ => out.extend(
                items::replayed(item, cwd)
                    .into_iter()
                    .map(|update| (session.to_owned(), update)),
            ),
        }
        for (target, update) in &mut out[from..] {
            if target == session {
                super::super::stamp_replayed(update, at, false);
            }
        }
    }
}
