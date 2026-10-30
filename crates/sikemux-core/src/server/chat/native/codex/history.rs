//! A resumed thread's history told to the chat as the updates it would have
//! seen live, the way Codex's adapter replays it on `session/load`.

use sikemux_fold::UpdateKind;
use std::collections::HashMap;

use serde_json::{json, Value};

use super::super::super::states::Subagent;
use super::items;
use super::notifications::{self, HistoryTurn, ItemHead, SubagentItem};
use super::subagents::{Addressed, Route, Subagents};

pub(super) use super::super::super::feed::title;

/// The request a thread's history is read with, for saying what could not be read.
const LISTED: &str = "thread/turns/list";

/// Each of `turns` with what it says of itself, leaving out one Sikemux cannot read.
fn turn_heads(turns: &[Value]) -> impl Iterator<Item = (HistoryTurn<'_>, &Value)> {
    turns
        .iter()
        .filter_map(|turn| Some((notifications::readable(LISTED, turn).ok()?, turn)))
}

/// Each item of `turn` with what it says of itself, leaving out one Sikemux cannot read.
fn items_of(turn: &Value) -> impl Iterator<Item = (ItemHead<'_>, &Value)> {
    turn.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| Some((notifications::item_head(LISTED, item)?, item)))
}

fn user_messages(turns: &[Value]) -> impl Iterator<Item = (HistoryTurn<'_>, &Value)> {
    turn_heads(turns).flat_map(|(turn, whole)| {
        items_of(whole)
            .filter(|(head, _)| head.kind == "userMessage")
            .map(move |(_, item)| (turn, item))
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
            Some((item["clientId"].as_str()?.to_owned(), turn.id.to_owned()))
        })
        .collect()
}

/// The subagent threads a thread's history started.
pub(super) fn subagent_threads(turns: &[Value]) -> Vec<String> {
    let mut threads = Vec::new();
    for turn in turns {
        for (_, item) in items_of(turn) {
            let Some(SubagentItem::Activity(activity)) = notifications::subagent_item(LISTED, item)
            else {
                continue;
            };
            let thread = activity.agent_thread_id;
            if activity.kind == "started" && !threads.iter().any(|known| known == thread) {
                threads.push(thread.to_owned());
            }
        }
    }
    threads
}

/// The updates replaying `turns` of the thread `root`. A user message is told
/// under the id of the turn it began, which is what taking the chat back to
/// before it names. Subagents go through the same [`Subagents`] the live chat
/// uses, so a card reloads as it ran: each time one is opened, the next turn
/// of its own thread in `children` is told into it, and that turn's status
/// ends it. One still open at the end lost its agent.
pub(super) fn replay(
    root: &str,
    turns: &[Value],
    children: &HashMap<String, Vec<Value>>,
    read: &impl Fn(&str) -> Option<String>,
    out: &mut Vec<Addressed>,
) {
    let mut replaying = Replaying {
        subagents: Subagents::new(root.to_owned()),
        children,
        read,
        told: HashMap::new(),
        out,
    };
    replaying.thread(root, turns);
    let mut lost = Vec::new();
    replaying
        .subagents
        .finish_outstanding(Subagent::Disconnected, &mut lost);
    replaying.out.extend(lost);
}

struct Replaying<'a, R> {
    subagents: Subagents,
    children: &'a HashMap<String, Vec<Value>>,
    read: &'a R,
    /// How many of each subagent thread's turns have been told.
    told: HashMap<String, usize>,
    out: &'a mut Vec<Addressed>,
}

impl<R: Fn(&str) -> Option<String>> Replaying<'_, R> {
    fn thread(&mut self, thread: &str, turns: &[Value]) {
        let session = self.subagents.session_of(thread);
        for (turn, whole) in turn_heads(turns) {
            let mut first_message = true;
            let milliseconds = |seconds: Option<u64>| seconds.map(|at| at * 1000);
            let (started, completed) = (
                milliseconds(turn.started_at),
                milliseconds(turn.completed_at),
            );
            for (head, item) in items_of(whole) {
                let from = self.out.len();
                let mut opened = Vec::new();
                let said = head.kind == "userMessage";
                if said {
                    let id = if first_message { turn.id } else { head.id };
                    first_message = false;
                    for block in items::user_blocks(item) {
                        self.out.push((
                            session.clone(),
                            json!({ "sessionUpdate": UpdateKind::UserMessageChunk, "messageId": id, "content": block }),
                        ));
                    }
                } else {
                    opened = self.item(thread, item);
                }
                let at = if said { started } else { completed };
                if let Some(at) = at {
                    for (target, update) in &mut self.out[from..] {
                        if *target == session {
                            super::super::stamp_replayed(update, Some(at), false);
                        }
                    }
                }
                for child in opened {
                    self.next_turn(&child);
                }
            }
        }
    }

    /// Tells one finished item of `thread`, answering with the subagents it
    /// opened.
    fn item(&mut self, thread: &str, item: &Value) -> Vec<String> {
        let params = json!({ "threadId": thread, "item": item });
        let mut said = Vec::new();
        if let Route::Session(target) = self.subagents.route(
            "item/completed",
            Some(thread),
            &params,
            &mut said,
            &mut Vec::new(),
        ) {
            self.out.extend(
                items::replayed(item, self.read)
                    .into_iter()
                    .map(|update| (target.clone(), update)),
            );
        }
        let opened = said
            .iter()
            .filter(|(_, update)| {
                UpdateKind::of(update) == Some(UpdateKind::SubagentSpawned)
                    || update["state"] == Subagent::Running.as_str()
            })
            .filter_map(|(_, update)| update["subagentSessionId"].as_str().map(str::to_owned))
            .collect();
        self.out.extend(said);
        if let Some(SubagentItem::Activity(activity)) = notifications::subagent_item(LISTED, item) {
            self.settle(activity.agent_thread_id, activity.kind);
        }
        opened
    }

    fn next_turn(&mut self, child: &str) {
        let told = self.told.entry(child.to_owned()).or_default();
        let Some(turn) = self.children.get(child).and_then(|turns| turns.get(*told)) else {
            return;
        };
        *told += 1;
        self.thread(child, std::slice::from_ref(turn));
        let ended = notifications::readable::<HistoryTurn>(LISTED, turn)
            .ok()
            .and_then(|turn| turn.status);
        if let Some(status) = ended {
            self.settle(child, status);
        }
    }

    /// Ends subagent `child` as `status` says, when it says it ended.
    fn settle(&mut self, child: &str, status: &str) {
        let params = json!({ "threadId": child, "turn": { "status": status } });
        let mut said = Vec::new();
        self.subagents.route(
            "turn/completed",
            Some(child),
            &params,
            &mut said,
            &mut Vec::new(),
        );
        self.out.extend(said);
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
        replay("thread", &turns(), &HashMap::new(), &no_files, &mut out);
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
            { "type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent", "status": "completed",
              "senderThreadId": "root", "receiverThreadIds": ["kid"], "prompt": "Check the tests",
              "agentsStates": { "kid": { "status": "pendingInit" } } },
            { "type": "subAgentActivity", "id": "a1", "kind": "started",
              "agentThreadId": "kid", "agentPath": "/root/helper" },
            { "type": "collabAgentToolCall", "id": "c2", "tool": "wait", "status": "completed",
              "senderThreadId": "root", "receiverThreadIds": ["kid"],
              "agentsStates": { "kid": { "status": "completed" } } },
            { "type": "subAgentActivity", "id": "a2", "kind": "completed",
              "agentThreadId": "kid", "agentPath": "/root/helper" },
            { "type": "subAgentActivity", "id": "a3", "kind": "started",
              "agentThreadId": "lost", "agentPath": "/root/lost" },
        ] })];
        let children = HashMap::from([(
            "kid".to_owned(),
            vec![json!({ "id": "kid-turn", "status": "failed", "items": [
                { "type": "agentMessage", "id": "msg_k", "text": "child says", "phase": null },
            ] })],
        )]);
        assert_eq!(subagent_threads(&root), ["kid", "lost"]);
        let mut out = Vec::new();
        replay("root", &root, &children, &no_files, &mut out);
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
                ("root".to_owned(), "tool_call".to_owned()),
                ("root".to_owned(), "subagent_spawned".to_owned()),
                ("root".to_owned(), "subagent_state_update".to_owned()),
            ]
        );
        assert_eq!(out[1].1["name"], "Helper");
        assert_eq!(out[1].1["task"], "Check the tests");
        assert_eq!(out[3].1["state"], "failed");
        assert_eq!(out[6].1, Subagent::Disconnected.update("lost"));
    }
}
