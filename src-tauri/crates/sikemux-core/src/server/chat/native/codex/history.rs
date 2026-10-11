//! A resumed thread's history told to the chat as the updates it would have
//! seen live, the way Codex's adapter replays it on `session/load`.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::items;
use super::subagents::{fallback_name, name_from_path, spawned, state_update, Addressed};

/// A title as the chat shows it: one line, or none when blank.
pub(super) fn title(text: &str) -> Option<String> {
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn user_messages(turns: &[Value]) -> impl Iterator<Item = (&Value, &Value)> {
    turns.iter().flat_map(|turn| {
        turn["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "userMessage")
            .map(move |item| (turn, item))
    })
}

/// The thread's name, or else its first words, as the chat's title.
pub(super) fn thread_title(thread: &Value, turns: &[Value]) -> Option<String> {
    if let Some(name) = thread["name"].as_str().and_then(title) {
        return Some(name);
    }
    user_messages(turns)
        .find_map(|(_, item)| {
            let said: Vec<&str> = item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|input| input["type"] == "text")
                .filter_map(|input| input["text"].as_str())
                .collect();
            title(&said.join(" "))
        })
        .or_else(|| thread["preview"].as_str().and_then(title))
}

/// The turn each of the app's own message ids began, for the messages the app
/// sent with one.
pub(super) fn message_turns(turns: &[Value]) -> HashMap<String, String> {
    user_messages(turns)
        .filter_map(|(turn, item)| {
            Some((
                item["clientId"].as_str()?.to_owned(),
                turn["id"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The subagent threads a thread's history started.
pub(super) fn subagent_threads(turns: &[Value]) -> Vec<String> {
    let mut threads = Vec::new();
    for turn in turns {
        for item in turn["items"].as_array().into_iter().flatten() {
            if item["type"] == "subAgentActivity" && item["kind"] == "started" {
                if let Some(thread) = item["agentThreadId"].as_str() {
                    if !threads.iter().any(|known| known == thread) {
                        threads.push(thread.to_owned());
                    }
                }
            }
        }
    }
    threads
}

struct Announced {
    child: String,
    generation: usize,
    session: String,
    ended: bool,
}

/// The updates replaying `turns` into session `session`. A user message is
/// told under the id of the turn it began, which is what taking the chat back
/// to before it names; a subagent's history is told into its own session
/// from `children`, its thread's turns.
pub(super) fn replay(
    session: &str,
    turns: &[Value],
    children: &HashMap<String, Vec<Value>>,
    ancestry: &mut HashSet<String>,
    read: &impl Fn(&str) -> Option<String>,
    out: &mut Vec<Addressed>,
) {
    let mut announced: Vec<Announced> = Vec::new();
    for turn in turns {
        let mut first_message = true;
        let seconds = |field: &str| turn[field].as_u64().map(|at| at * 1000);
        let (started, completed) = (seconds("startedAt"), seconds("completedAt"));
        for item in turn["items"].as_array().into_iter().flatten() {
            let from = out.len();
            match item["type"].as_str() {
                Some("subAgentActivity") => {
                    replay_activity(session, item, children, ancestry, read, &mut announced, out);
                }
                Some("collabAgentToolCall") => {}
                Some("userMessage") => {
                    let id = if first_message {
                        &turn["id"]
                    } else {
                        &item["id"]
                    };
                    first_message = false;
                    for block in items::user_blocks(item) {
                        out.push((
                            session.to_owned(),
                            json!({ "sessionUpdate": "user_message_chunk", "messageId": id, "content": block }),
                        ));
                    }
                }
                _ => out.extend(
                    items::replayed(item, read)
                        .into_iter()
                        .map(|update| (session.to_owned(), update)),
                ),
            }
            let at = if item["type"] == "userMessage" {
                started
            } else {
                completed
            };
            if let Some(at) = at {
                for (target, update) in &mut out[from..] {
                    if target == session {
                        super::super::stamp_replayed(update, at);
                    }
                }
            }
        }
    }
    for child in announced.iter().filter(|child| !child.ended) {
        out.push((
            session.to_owned(),
            state_update(&child.session, "disconnected"),
        ));
    }
}

fn replay_activity(
    session: &str,
    item: &Value,
    children: &HashMap<String, Vec<Value>>,
    ancestry: &mut HashSet<String>,
    read: &impl Fn(&str) -> Option<String>,
    announced: &mut Vec<Announced>,
    out: &mut Vec<Addressed>,
) {
    let Some(child) = item["agentThreadId"].as_str() else {
        return;
    };
    let name = name_from_path(
        item["agentPath"].as_str().unwrap_or_default(),
        fallback_name(child),
    );
    let task = format!("Delegated task for {name}");
    let previous = announced.iter().position(|known| known.child == child);
    match item["kind"].as_str() {
        Some("started") => {
            if previous.is_some_and(|at| !announced[at].ended) {
                return;
            }
            let generation = previous.map_or(1, |at| announced[at].generation + 1);
            let child_session = if generation == 1 {
                child.to_owned()
            } else {
                format!("{child}:generation:{generation}")
            };
            out.push((session.to_owned(), spawned(&child_session, &name, &task)));
            let entry = Announced {
                child: child.to_owned(),
                generation,
                session: child_session.clone(),
                ended: false,
            };
            match previous {
                Some(at) => announced[at] = entry,
                None => announced.push(entry),
            }
            if ancestry.contains(child) {
                return;
            }
            if let Some(turn) = children
                .get(child)
                .and_then(|turns| turns.get(generation - 1))
            {
                ancestry.insert(child.to_owned());
                replay(
                    &child_session,
                    std::slice::from_ref(turn),
                    children,
                    ancestry,
                    read,
                    out,
                );
                ancestry.remove(child);
            }
        }
        Some(kind @ ("completed" | "interrupted")) => {
            let Some(at) = previous else {
                out.push((session.to_owned(), spawned(child, &name, &task)));
                announced.push(Announced {
                    child: child.to_owned(),
                    generation: 1,
                    session: child.to_owned(),
                    ended: false,
                });
                return;
            };
            if announced[at].ended {
                return;
            }
            let state = if kind == "completed" {
                "completed"
            } else {
                "cancelled"
            };
            out.push((
                session.to_owned(),
                state_update(&announced[at].session, state),
            ));
            announced[at].ended = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_files(_: &str) -> Option<String> {
        None
    }

    fn turns() -> Vec<Value> {
        vec![
            json!({ "id": "turn-1", "status": "completed", "items": [
                { "type": "userMessage", "id": "u1", "clientId": "app-1",
                  "content": [{ "type": "text", "text": "Fix   the\nbug", "text_elements": [] }] },
                { "type": "reasoning", "id": "rs_1", "summary": ["Looking"], "content": [] },
                { "type": "commandExecution", "id": "exec-1", "command": "/bin/zsh -lc 'cargo test'",
                  "cwd": "/w", "status": "completed",
                  "commandActions": [{ "type": "unknown", "command": "cargo test" }],
                  "aggregatedOutput": "ok", "exitCode": 0 },
                { "type": "userMessage", "id": "u2", "clientId": null,
                  "content": [{ "type": "text", "text": "also this", "text_elements": [] }] },
                { "type": "agentMessage", "id": "msg_1", "text": "Done", "phase": "final_answer" },
            ] }),
            json!({ "id": "turn-2", "status": "failed", "items": [
                { "type": "userMessage", "id": "u3", "clientId": null,
                  "content": [{ "type": "text", "text": "again", "text_elements": [] }] },
            ] }),
        ]
    }

    #[test]
    fn history_replays_as_live_updates_with_turn_ids_on_user_messages() {
        let mut out = Vec::new();
        replay(
            "thread",
            &turns(),
            &HashMap::new(),
            &mut HashSet::new(),
            &no_files,
            &mut out,
        );
        let kinds: Vec<_> = out
            .iter()
            .map(|(session, update)| {
                assert_eq!(session, "thread");
                update["sessionUpdate"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user_message_chunk",
                "agent_thought_chunk",
                "tool_call",
                "tool_call_update",
                "user_message_chunk",
                "agent_message_chunk",
                "user_message_chunk",
            ]
        );
        assert_eq!(out[0].1["messageId"], "turn-1");
        assert_eq!(
            out[0].1["content"],
            json!({ "type": "text", "text": "Fix   the\nbug" })
        );
        assert_eq!(out[4].1["messageId"], "u2");
        assert_eq!(out[5].1["_meta"]["codex"]["phase"], "final_answer");
        assert_eq!(out[6].1["messageId"], "turn-2");
    }

    #[test]
    fn the_title_and_message_ids_come_from_the_history() {
        let turns = turns();
        assert_eq!(
            thread_title(&json!({ "name": null }), &turns).as_deref(),
            Some("Fix the bug")
        );
        assert_eq!(
            thread_title(&json!({ "name": " Named " }), &turns).as_deref(),
            Some("Named")
        );
        assert_eq!(
            thread_title(&json!({ "preview": "p" }), &[]).as_deref(),
            Some("p")
        );
        assert_eq!(
            message_turns(&turns),
            HashMap::from([("app-1".to_owned(), "turn-1".to_owned())])
        );
    }

    #[test]
    fn a_subagents_history_goes_to_its_own_session() {
        let root = vec![json!({ "id": "turn-1", "items": [
            { "type": "userMessage", "id": "u1", "content": [{ "type": "text", "text": "go" }] },
            { "type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent" },
            { "type": "subAgentActivity", "id": "a1", "kind": "started",
              "agentThreadId": "kid", "agentPath": "/root/helper" },
            { "type": "subAgentActivity", "id": "a2", "kind": "completed",
              "agentThreadId": "kid", "agentPath": "/root/helper" },
            { "type": "subAgentActivity", "id": "a3", "kind": "started",
              "agentThreadId": "lost", "agentPath": "/root/lost" },
        ] })];
        let children = HashMap::from([(
            "kid".to_owned(),
            vec![json!({ "id": "kid-turn", "items": [
                { "type": "agentMessage", "id": "msg_k", "text": "child says", "phase": null },
            ] })],
        )]);
        assert_eq!(subagent_threads(&root), ["kid", "lost"]);
        let mut out = Vec::new();
        replay(
            "root",
            &root,
            &children,
            &mut HashSet::new(),
            &no_files,
            &mut out,
        );
        let said: Vec<(String, String)> = out
            .iter()
            .map(|(session, update)| {
                (
                    session.clone(),
                    update["sessionUpdate"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                ("root".to_owned(), "user_message_chunk".to_owned()),
                ("root".to_owned(), "subagent_spawned".to_owned()),
                ("kid".to_owned(), "agent_message_chunk".to_owned()),
                ("root".to_owned(), "subagent_state_update".to_owned()),
                ("root".to_owned(), "subagent_spawned".to_owned()),
                ("root".to_owned(), "subagent_state_update".to_owned()),
            ]
        );
        assert_eq!(out[1].1["name"], "Helper");
        assert_eq!(out[3].1["state"], "completed");
        assert_eq!(out[5].1, state_update("lost", "disconnected"));
    }
}
