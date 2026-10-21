//! What an agent writes, read as markdown once here and sent with the text, so
//! no client parses it again. Each chunk of an answer carries the change to
//! the tree of the answer so far, as `_meta.sikemux.md`: `n`, the answer's
//! length in UTF-16 units once the chunk is added, and `o`, the patch.

use std::collections::HashMap;

use serde_json::{json, Value};
use sikemux_markdown::{Doc, Options};

use crate::protocol::{ChatEvent, ChatEventKind};

const ANSWER: Options = Options {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: false,
};

/// Updates that leave the answer being written open: none of them puts
/// anything in the transcript between its pieces.
const LEAVES_OPEN: [&str; 10] = [
    "usage_update",
    "current_mode_update",
    "config_option_update",
    "available_commands_update",
    "session_info_update",
    "plan",
    "task_notice",
    "async_task_spawned",
    "async_task_progress",
    "async_task_state_update",
];

/// The most tree one batch of replayed events carries. Past it the rest of the
/// answers go without one, and their clients draw them as plain text.
const MAX_TREE_BYTES: usize = 4 * 1024 * 1024;

/// An answer's place in a session: which kind of text, and which message.
#[derive(PartialEq, Eq)]
struct Key {
    kind: String,
    message: Option<String>,
}

/// A piece of an answer: the session it is written in, which answer, and the text.
fn piece(notification: &Value) -> Option<(&str, Key, &str)> {
    let session = notification.get("sessionId")?.as_str()?;
    let update = notification.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    if kind != "agent_message_chunk" && kind != "agent_thought_chunk" {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type")?.as_str()? != "text" {
        return None;
    }
    let key = Key {
        kind: kind.to_owned(),
        message: update
            .get("messageId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    Some((session, key, content.get("text")?.as_str()?))
}

fn leaves_open(notification: &Value) -> bool {
    notification
        .pointer("/update/sessionUpdate")
        .and_then(Value::as_str)
        .is_some_and(|kind| LEAVES_OPEN.contains(&kind))
}

fn mark(notification: &mut Value, tree: Value) {
    let Some(update) = notification
        .get_mut("update")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let meta = update.entry("_meta").or_insert_with(|| json!({}));
    if let Some(meta) = meta.as_object_mut() {
        let ours = meta.entry("sikemux").or_insert_with(|| json!({}));
        if let Some(ours) = ours.as_object_mut() {
            ours.insert("md".to_owned(), tree);
        }
    }
}

struct Answer {
    key: Key,
    doc: Doc,
}

/// The answers being written, one per session, as their clients were last told.
#[derive(Default)]
pub(crate) struct Answers {
    open: HashMap<String, Answer>,
}

impl Answers {
    /// Reads each notification in order, giving the last piece of each answer
    /// in the run what changed in its tree.
    pub(crate) fn annotate(&mut self, notifications: &mut [Value]) {
        let mut last: HashMap<String, usize> = HashMap::new();
        for at in 0..notifications.len() {
            let Some(notification) = notifications.get(at) else {
                break;
            };
            let Some((session, key, text)) = piece(notification) else {
                if !leaves_open(notification) {
                    if let Some(session) = notification.get("sessionId").and_then(Value::as_str) {
                        let session = session.to_owned();
                        self.tell(&session, &mut last, notifications);
                        self.open.remove(&session);
                    }
                }
                continue;
            };
            let (session, text) = (session.to_owned(), text.to_owned());
            if self
                .open
                .get(&session)
                .is_some_and(|answer| answer.key != key)
            {
                self.tell(&session, &mut last, notifications);
                self.open.remove(&session);
            }
            self.open
                .entry(session.clone())
                .or_insert_with(|| Answer {
                    key,
                    doc: Doc::new(ANSWER),
                })
                .doc
                .push(&text);
            last.insert(session, at);
        }
        let sessions: Vec<String> = last.keys().cloned().collect();
        for session in sessions {
            self.tell(&session, &mut last, notifications);
        }
    }

    /// Puts what changed in `session`'s answer on its last piece in the run.
    fn tell(
        &mut self,
        session: &str,
        last: &mut HashMap<String, usize>,
        notifications: &mut [Value],
    ) {
        let (Some(at), Some(answer)) = (last.remove(session), self.open.get_mut(session)) else {
            return;
        };
        let units = answer.doc.units();
        let patch = answer.doc.patch();
        if let (false, Some(notification)) = (patch.is_empty(), notifications.get_mut(at)) {
            mark(notification, json!({ "n": units, "o": patch }));
        }
    }

    /// Something else was said in the chat, which ends every answer.
    pub(crate) fn close_all(&mut self) {
        self.open.clear();
    }

    /// The answers of a chat whose clients already hold `events`, for a core
    /// that takes the chat up partway through an answer.
    pub(crate) fn read_back(mut events: Vec<ChatEvent>) -> Self {
        let mut answers = Self::default();
        for event in &mut events {
            if event.kind != ChatEventKind::SessionUpdate {
                answers.close_all();
                continue;
            }
            for notification in notifications(event) {
                answers.annotate(std::slice::from_mut(notification));
            }
        }
        answers
    }
}

fn notifications(event: &mut ChatEvent) -> Vec<&mut Value> {
    if event.kind != ChatEventKind::SessionUpdate {
        return Vec::new();
    }
    if !event.payload.get("updates").is_some_and(Value::is_array) {
        return vec![&mut event.payload];
    }
    event
        .payload
        .get_mut("updates")
        .and_then(Value::as_array_mut)
        .map(|updates| updates.iter_mut().collect())
        .unwrap_or_default()
}

/// Gives every answer in `events`, a slice of the chat sent at once, its whole
/// tree, within a bound on how much tree that adds.
pub(crate) fn annotate_replay(events: &mut [ChatEvent]) {
    let mut answers = Answers::default();
    let mut bytes = 0;
    for event in events.iter_mut() {
        if event.kind != ChatEventKind::SessionUpdate {
            answers.close_all();
            continue;
        }
        for notification in notifications(event) {
            if bytes > MAX_TREE_BYTES {
                return;
            }
            answers.annotate(std::slice::from_mut(notification));
            bytes += notification
                .pointer("/update/_meta/sikemux/md")
                .map_or(0, |tree| tree.to_string().len());
        }
    }
}
