//! The events a chat keeps for the clients that attach later: text streamed into one
//! message joined up, written to the history on disk, paged back through and rewound.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::{ChatEvent, ChatEventKind, Place, UpdateKind};
use sikemux_wire::chat::ToolDetail;

use super::super::history::History;
use super::work::{subagent_ended, task_ended, Work};

mod disk;
mod pages;
mod said;

pub(crate) use pages::Anchor;
pub(crate) use said::{session_title, title, Excerpt, Said};

/// Roughly what an event costs beyond its payload once it is on the wire.
const EVENT_OVERHEAD: usize = 48;
/// A turn longer than a page is cut into pieces about this long.
const PIECE_BYTES: usize = 64 * 1024;

/// What a kept event still stands for, until a later one replaces or ends it.
#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Holds {
    Update(UpdateKind),
    TaskSpawned(String),
    TaskLatest(String),
    Subagent(String),
}

#[derive(Serialize, Deserialize)]
pub(super) struct Entry {
    pub(super) event: ChatEvent,
    pub(super) bytes: usize,
    pub(super) index: u64,
    /// The number of the last change it made; 0 when it came without one.
    #[serde(default)]
    pub(super) seq: u64,
    pub(super) starts_turn: bool,
    /// A page of history can start here.
    point: bool,
    on_disk: bool,
}

/// The ordered events a late client replays. Text streamed into one message
/// is kept as one event, which the chat would have joined up anyway. Each is
/// written to the history on disk once nothing more joins it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Replay {
    pub(super) entries: VecDeque<Entry>,
    pub(super) bytes: usize,
    max_bytes: usize,
    max_events: usize,
    trimmed: bool,
    /// Handed over on its own, since the file stays where it is.
    #[serde(skip)]
    pub(super) history: Option<History>,
    pub(super) next_index: u64,
    pub(super) in_turn: bool,
    /// A prompt the agent was sent has had no turn end since.
    #[serde(default)]
    pub(super) awaiting: bool,
    /// The attempt an agent is retrying a failed request on, 0 when unsaid.
    pub(super) reconnecting: Option<u32>,
    last_from_user: bool,
    since_point: usize,
    /// What a phone shown only the chat's end still needs from before it:
    /// the last of each standing update, and the background tasks and
    /// subagents still going.
    pub(super) standing: Vec<(Holds, u64, ChatEvent)>,
    pub(super) work: Work,
    /// Every subagent's start and latest state, so a slice of the chat that
    /// speaks of one started before it can carry them too.
    spawns: Vec<Spawn>,
    /// The number of the chat's latest delivery to its clients. A change made now takes
    /// the one after it, the delivery it goes out in.
    #[serde(skip)]
    pub(super) seq: u64,
    /// A change took the next number and no delivery carried it yet.
    #[serde(skip)]
    pub(super) unsent: bool,
    /// What an event that came without a number counts as: the change at which this core
    /// took the chat over.
    #[serde(skip)]
    pub(super) unnumbered: u64,
}

#[derive(Serialize, Deserialize)]
struct Spawn {
    subagent: String,
    /// The session that started it.
    parent: String,
    pub(super) index: u64,
    pub(super) event: ChatEvent,
    state: Option<(u64, ChatEvent)>,
}

/// A streamed piece of a message that can be joined to the piece before it.
struct Chunk<'a> {
    session_id: &'a Value,
    kind: UpdateKind,
    message_id: Option<&'a Value>,
    text: &'a str,
}

fn only_keys(object: &serde_json::Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn chunk(payload: &Value) -> Option<Chunk<'_>> {
    let notification = payload.as_object()?;
    if !only_keys(notification, &["sessionId", "update", "at", "until"]) {
        return None;
    }
    let update = notification.get("update")?.as_object()?;
    if !only_keys(update, &["sessionUpdate", "content", "messageId"]) {
        return None;
    }
    let kind = UpdateKind::parse(update.get("sessionUpdate")?.as_str()?)?;
    if kind != UpdateKind::AgentMessageChunk && kind != UpdateKind::AgentThoughtChunk {
        return None;
    }
    let content = update.get("content")?.as_object()?;
    if !only_keys(content, &["type", "text"]) || content.get("type")? != "text" {
        return None;
    }
    Some(Chunk {
        session_id: notification.get("sessionId")?,
        kind,
        message_id: update.get("messageId"),
        text: content.get("text")?.as_str()?,
    })
}

fn joins(earlier: &Chunk<'_>, later: &Chunk<'_>) -> bool {
    earlier.session_id == later.session_id
        && earlier.kind == later.kind
        && earlier.message_id == later.message_id
}

pub(super) fn is_kept(kind: ChatEventKind) -> bool {
    !matches!(kind, ChatEventKind::Status | ChatEventKind::Ready)
}

/// When the event happened, so the chat folded again later times its turns
/// and answers as they ran.
pub(super) fn stamp(payload: &mut Value) {
    if let Some(fields) = payload.as_object_mut() {
        fields
            .entry("at")
            .or_insert_with(|| json!(sikemux_process::clock::epoch_ms()));
    }
}

pub(super) fn asks(event: &ChatEvent, request_id: &str) -> bool {
    event.kind == ChatEventKind::PermissionRequest
        && event.payload.get("requestId").and_then(Value::as_str) == Some(request_id)
}

/// Whether `event` is where the person's message `message_id` starts.
fn names_message(event: &ChatEvent, message_id: &str) -> bool {
    match event.kind {
        ChatEventKind::Prompt => event.payload.get("messageId"),
        ChatEventKind::SessionUpdate
            if UpdateKind::within(&event.payload) == Some(UpdateKind::UserMessageChunk) =>
        {
            event.payload.pointer("/update/messageId")
        }
        _ => None,
    }
    .and_then(Value::as_str)
        == Some(message_id)
}

impl Replay {
    pub(crate) fn new(max_bytes: usize, max_events: usize, history: Option<History>) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_bytes,
            max_events,
            trimmed: false,
            history,
            next_index: 0,
            in_turn: false,
            awaiting: false,
            reconnecting: None,
            last_from_user: false,
            since_point: 0,
            standing: Vec::new(),
            work: Work::default(),
            spawns: Vec::new(),
            seq: 0,
            unsent: false,
            unnumbered: 0,
        }
    }

    /// The number a change made now goes out under.
    fn numbered(&mut self) -> u64 {
        self.unsent = true;
        self.seq + 1
    }

    /// The change numbered next went out to the clients.
    pub(super) fn delivered(&mut self) -> u64 {
        self.seq += 1;
        self.unsent = false;
        self.seq
    }

    /// Whether `payload` begins a turn, which a prompt does when the turn
    /// starts right after it. A loaded session has no turns, only what the
    /// person said and what the agent answered.
    fn starts_turn(&mut self, kind: ChatEventKind, payload: &Value) -> bool {
        match kind {
            ChatEventKind::TurnStarted => {
                let prompt = self.entries.back_mut().filter(|entry| {
                    entry.event.kind == ChatEventKind::Prompt
                        && entry.index + 1 == self.next_index
                        && !entry.on_disk
                });
                match prompt {
                    Some(prompt) => {
                        prompt.starts_turn = true;
                        prompt.point = true;
                        false
                    }
                    None => true,
                }
            }
            ChatEventKind::SessionUpdate => {
                UpdateKind::within(payload) == Some(UpdateKind::UserMessageChunk)
                    && !self.in_turn
                    && !self.last_from_user
            }
            _ => false,
        }
    }

    fn join_text(&mut self, payload: &Value, seq: u64) -> bool {
        let Some(later) = chunk(payload) else {
            return false;
        };
        let Some(last) = self
            .entries
            .back_mut()
            .filter(|entry| entry.event.kind == ChatEventKind::SessionUpdate && !entry.on_disk)
        else {
            return false;
        };
        if !chunk(&last.event.payload).is_some_and(|earlier| joins(&earlier, &later)) {
            return false;
        }
        let Some(text) = last
            .event
            .payload
            .pointer_mut("/update/content/text")
            .and_then(|text| match text {
                Value::String(text) => Some(text),
                _ => None,
            })
        else {
            return false;
        };
        text.push_str(later.text);
        if let Some(at) = payload.get("at") {
            last.event.payload["until"] = at.clone();
        }
        last.seq = seq;
        last.bytes += later.text.len();
        self.bytes += later.text.len();
        self.since_point += later.text.len();
        true
    }

    /// Keeps `payload`, answering with the index it is kept at: an event not
    /// kept answers with the next index, and text joined to the event before
    /// it with that event's.
    pub(crate) fn push(&mut self, kind: ChatEventKind, payload: &Value) -> u64 {
        if !is_kept(kind) {
            return self.next_index;
        }
        let seq = self.numbered();
        if kind == ChatEventKind::SessionUpdate && self.join_text(payload, seq) {
            let index = self
                .entries
                .back()
                .map_or(self.next_index, |entry| entry.index);
            self.trim();
            return index;
        }
        let starts_turn = self.starts_turn(kind, payload);
        if let Some(previous) = self.entries.back_mut() {
            Self::write(&mut self.history, previous);
        }
        let bytes = serde_json::to_vec(payload).map_or(0, |bytes| bytes.len()) + EVENT_OVERHEAD;
        let point = starts_turn
            || (kind != ChatEventKind::PermissionRequest && self.since_point >= PIECE_BYTES);
        if point {
            self.since_point = 0;
        }
        self.since_point += bytes;
        let index = self.next_index;
        self.next_index += 1;
        let event = ChatEvent {
            kind,
            payload: payload.clone(),
        };
        self.note_standing(kind, index, &event);
        self.note_subagent(kind, index, &event);
        if kind == ChatEventKind::SessionUpdate {
            self.work.note(payload);
            self.note_retry(payload);
        }
        let ends_turn = matches!(kind, ChatEventKind::TurnCompleted | ChatEventKind::Error);
        match kind {
            ChatEventKind::TurnStarted => {
                self.in_turn = true;
                self.reconnecting = None;
            }
            ChatEventKind::Prompt => self.awaiting = true,
            _ if ends_turn => {
                self.in_turn = false;
                self.awaiting = false;
                self.reconnecting = None;
            }
            _ => {}
        }
        self.last_from_user = kind == ChatEventKind::Prompt
            || UpdateKind::within(payload) == Some(UpdateKind::UserMessageChunk);
        self.entries.push_back(Entry {
            event,
            bytes,
            index,
            seq,
            starts_turn,
            point,
            on_disk: false,
        });
        self.bytes += bytes;
        // A turn's end goes to disk at once, so a core that dies right after does not take
        // the turn for cut short and send its prompt again.
        if ends_turn {
            self.settle();
        }
        self.trim();
        index
    }

    fn note_subagent(&mut self, kind: ChatEventKind, index: u64, event: &ChatEvent) {
        if kind != ChatEventKind::SessionUpdate {
            return;
        }
        let text = |pointer: &str| event.payload.pointer(pointer).and_then(Value::as_str);
        let Some(subagent) = text("/update/subagentSessionId") else {
            return;
        };
        let known = self
            .spawns
            .iter_mut()
            .find(|spawn| spawn.subagent == subagent);
        match UpdateKind::within(&event.payload) {
            Some(UpdateKind::SubagentSpawned) => {
                let (None, Some(parent)) = (known, text("/sessionId")) else {
                    return;
                };
                self.spawns.push(Spawn {
                    subagent: subagent.to_owned(),
                    parent: parent.to_owned(),
                    index,
                    event: event.clone(),
                    state: None,
                });
            }
            Some(UpdateKind::SubagentStateUpdate) => {
                if let Some(spawn) = known {
                    spawn.state = Some((index, event.clone()));
                }
            }
            _ => {}
        }
    }

    /// The starts of the subagents `events` speak of, and of the subagents
    /// that started those, from before `first`, each with its latest state
    /// from before `first`. Without them a client would take a subagent's
    /// work for the chat's own, or draw one that already ended as running.
    fn spawns_before<'a>(
        &self,
        events: impl Iterator<Item = &'a ChatEvent>,
        first: u64,
    ) -> Vec<(u64, &ChatEvent)> {
        let mut named: HashSet<&str> = HashSet::new();
        for event in events.filter(|event| event.kind == ChatEventKind::SessionUpdate) {
            let payload = &event.payload;
            named.extend(payload.get("sessionId").and_then(Value::as_str));
            named.extend(
                payload
                    .pointer("/update/subagentSessionId")
                    .and_then(Value::as_str),
            );
        }
        let mut named: Vec<&str> = named.into_iter().collect();
        let mut chosen: Vec<&Spawn> = Vec::new();
        while let Some(subagent) = named.pop() {
            let Some(spawn) = self.spawns.iter().find(|spawn| spawn.subagent == subagent) else {
                continue;
            };
            if spawn.index >= first || chosen.iter().any(|kept| kept.subagent == spawn.subagent) {
                continue;
            }
            chosen.push(spawn);
            named.push(&spawn.parent);
        }
        let mut found: Vec<(u64, &ChatEvent)> = chosen
            .into_iter()
            .flat_map(|spawn| {
                let state = spawn
                    .state
                    .as_ref()
                    .filter(|(index, _)| *index < first)
                    .map(|(index, event)| (*index, event));
                std::iter::once((spawn.index, &spawn.event)).chain(state)
            })
            .collect();
        found.sort_by_key(|(index, _)| *index);
        found
    }

    fn hold(&mut self, holds: Holds, index: u64, event: &ChatEvent) {
        self.standing.retain(|(held, ..)| *held != holds);
        self.standing.push((holds, index, event.clone()));
    }

    fn note_standing(&mut self, kind: ChatEventKind, index: u64, event: &ChatEvent) {
        if kind != ChatEventKind::SessionUpdate {
            return;
        }
        let Some(update) = event.payload.get("update") else {
            return;
        };
        let Some(update_kind) = UpdateKind::of(update) else {
            return;
        };
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_owned);
        if self.work.in_subagent(&event.payload) {
            return;
        }
        match update_kind {
            standing if standing.standing() => {
                self.hold(Holds::Update(standing), index, event);
            }
            UpdateKind::SubagentSpawned => {
                if let Some(id) = text("subagentSessionId") {
                    self.hold(Holds::Subagent(id), index, event);
                }
            }
            UpdateKind::SubagentStateUpdate if subagent_ended(update) => {
                if let Some(id) = text("subagentSessionId") {
                    self.standing
                        .retain(|(held, ..)| *held != Holds::Subagent(id.clone()));
                }
            }
            UpdateKind::AsyncTaskSpawned => {
                if let Some(id) = text("asyncTaskId") {
                    self.hold(Holds::TaskSpawned(id), index, event);
                }
            }
            UpdateKind::AsyncTaskProgress | UpdateKind::AsyncTaskStateUpdate => {
                let Some(id) = text("asyncTaskId") else {
                    return;
                };
                if task_ended(update) {
                    self.standing.retain(|(held, ..)| {
                        *held != Holds::TaskSpawned(id.clone())
                            && *held != Holds::TaskLatest(id.clone())
                    });
                } else if self
                    .standing
                    .iter()
                    .any(|(held, ..)| *held == Holds::TaskSpawned(id.clone()))
                {
                    self.hold(Holds::TaskLatest(id), index, event);
                }
            }
            _ => {}
        }
    }

    fn trim(&mut self) {
        while self.bytes > self.max_bytes || self.entries.len() > self.max_events {
            let Some(mut dropped) = self.entries.pop_front() else {
                break;
            };
            Self::write(&mut self.history, &mut dropped);
            self.bytes -= dropped.bytes;
            self.trimmed = true;
        }
    }

    pub(crate) fn keeps_history(&self) -> bool {
        self.history
            .as_ref()
            .is_some_and(|history| history.first().is_some())
    }

    /// Every event the replay holds, with what still stands from before
    /// them and the starts of the subagents they speak of, for folding the
    /// chat again: each by its index and the number of the last change it made.
    pub(crate) fn fold_events(&self) -> Vec<(u64, u64, ChatEvent)> {
        let Some(first) = self.entries.front().map(|entry| entry.index) else {
            return Vec::new();
        };
        let mut before: Vec<(u64, &ChatEvent)> = self
            .standing
            .iter()
            .filter(|(_, index, _)| *index < first)
            .map(|(_, index, event)| (*index, event))
            .collect();
        for (index, event) in
            self.spawns_before(self.entries.iter().map(|entry| &entry.event), first)
        {
            if !before.iter().any(|(held, _)| *held == index) {
                before.push((index, event));
            }
        }
        before.sort_by_key(|(index, _)| *index);
        before
            .into_iter()
            .map(|(index, event)| (index, self.unnumbered, event.clone()))
            .chain(self.entries.iter().map(|entry| {
                let seq = match entry.seq {
                    0 => self.unnumbered,
                    seq => seq,
                };
                (entry.index, seq, entry.event.clone())
            }))
            .collect()
    }

    /// Everything said of the tool call `id` in the events held, settled.
    fn tool_call(&self, id: &str) -> Option<serde_json::Map<String, Value>> {
        let mut merged: Option<serde_json::Map<String, Value>> = None;
        for entry in &self.entries {
            if entry.event.kind != ChatEventKind::SessionUpdate {
                continue;
            }
            let Some(update) = entry.event.payload.get("update").and_then(Value::as_object) else {
                continue;
            };
            if update.get("toolCallId").and_then(Value::as_str) != Some(id) {
                continue;
            }
            let merged = merged.get_or_insert_with(serde_json::Map::new);
            for (key, value) in update {
                if !value.is_null() {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
        merged
    }

    /// Forgets the person's message `message_id` and everything after it,
    /// looking for it on disk when it is older than the replay. When neither
    /// holds it, it is gone only if the replay dropped events it never wrote
    /// down, and then everything kept came after it.
    pub(crate) fn rewind(&mut self, message_id: &str) {
        let names = |event: &ChatEvent| names_message(event, message_id);
        let index = match self.entries.iter().find(|entry| names(&entry.event)) {
            Some(entry) => entry.index,
            None => match self
                .history
                .as_ref()
                .and_then(|history| history.find(names))
            {
                Some(index) => index,
                None if self.trimmed && !self.keeps_history() => self
                    .entries
                    .front()
                    .map_or(self.next_index, |entry| entry.index),
                None => return,
            },
        };
        let position = self.entries.partition_point(|entry| entry.index < index);
        let dropped: usize = self
            .entries
            .drain(position..)
            .map(|entry| entry.bytes)
            .sum();
        self.bytes -= dropped;
        if let Some(history) = self.history.as_mut() {
            history.truncate(index);
        }
        self.standing.retain(|(_, held, _)| *held < index);
        self.spawns.retain(|spawn| spawn.index < index);
        for spawn in &mut self.spawns {
            if spawn.state.as_ref().is_some_and(|(at, _)| *at >= index) {
                spawn.state = None;
            }
        }
        self.in_turn = false;
        self.reconnecting = None;
        self.last_from_user = false;
        self.since_point = 0;
    }

    /// An agent retrying says so as a warning; its next output means it got through.
    /// A service that answered with an error is not a lost connection.
    fn note_retry(&mut self, payload: &Value) {
        match UpdateKind::within(payload) {
            Some(UpdateKind::SessionInfoUpdate) if self.in_turn => {
                let notice = payload.pointer("/update/_meta/jetbrains/air/sessionFailure");
                if let Some(notice) = notice.filter(|notice| notice["severity"] == "warning") {
                    let attempt = notice["attempt"].as_u64().unwrap_or(0);
                    self.reconnecting = (notice["category"] == "connection")
                        .then(|| u32::try_from(attempt).unwrap_or(u32::MAX));
                }
            }
            Some(
                UpdateKind::AgentMessageChunk
                | UpdateKind::AgentThoughtChunk
                | UpdateKind::ToolCall
                | UpdateKind::ToolCallUpdate,
            ) => {
                self.reconnecting = None;
            }
            _ => {}
        }
    }

    /// An answered request is not asked again when the chat is replayed.
    pub(crate) fn forget_permission(&mut self, request_id: &str) {
        let mut freed = 0;
        self.entries.retain(|entry| {
            let answered = asks(&entry.event, request_id);
            if answered {
                freed += entry.bytes;
            }
            !answered
        });
        self.bytes -= freed;
    }

    pub(super) fn turn_start(&self) -> Option<u64> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.starts_turn)
            .map(|entry| entry.index)
    }

    pub(crate) fn is_trimmed(&self) -> bool {
        self.trimmed
    }

    #[cfg(test)]
    pub(crate) fn events(&self) -> Vec<ChatEvent> {
        self.entries
            .iter()
            .map(|entry| entry.event.clone())
            .collect()
    }

    /// The change a tool call made and what it printed, read again from the
    /// events held, for a call the fold no longer holds.
    pub(crate) fn tool_detail(
        &self,
        id: &str,
        work: Option<&(PathBuf, Place)>,
    ) -> Option<ToolDetail> {
        let work = work.map(|(folder, place)| (folder.as_path(), place));
        self.tool_call(id)
            .map(|raw| sikemux_fold::detail_of(&raw, work))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::super::history::MAX_HISTORY_BYTES;
    use super::super::{Feed, MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS};
    use super::*;
    use crate::protocol::PermissionMode;
    use sikemux_fold::Fold;
    use sikemux_wire::chat::ChatChange;

    pub(in super::super) fn text(kind: &str, message: Option<&str>, text: &str) -> Value {
        let mut update = json!({
            "sessionUpdate": kind,
            "content": { "type": "text", "text": text },
        });
        if let Some(message) = message {
            update["messageId"] = json!(message);
        }
        json!({ "sessionId": "s", "update": update })
    }

    fn kinds(replay: &Replay) -> Vec<ChatEventKind> {
        replay.events().iter().map(|event| event.kind).collect()
    }

    #[test]
    fn a_rewind_forgets_the_message_and_everything_after_it() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let prompt = |id: &str| json!({ "text": id, "paths": [], "messageId": id });
        for id in ["first", "second"] {
            replay.push(ChatEventKind::Prompt, &prompt(id));
            replay.push(ChatEventKind::TurnStarted, &json!({}));
            replay.push(
                ChatEventKind::SessionUpdate,
                &text("agent_message_chunk", Some(id), "answer"),
            );
            replay.push(ChatEventKind::TurnCompleted, &json!({}));
        }
        replay.rewind("second");
        assert_eq!(
            kinds(&replay),
            [
                ChatEventKind::Prompt,
                ChatEventKind::TurnStarted,
                ChatEventKind::SessionUpdate,
                ChatEventKind::TurnCompleted,
            ]
        );
        assert_eq!(replay.events()[0].payload["messageId"], "first");
    }

    #[test]
    fn a_rewind_finds_a_message_a_loaded_session_replayed() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", Some("u1"), "hello"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", Some("a1"), "hi"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", Some("u2"), "again"),
        );
        replay.rewind("u2");
        assert_eq!(replay.events().len(), 2);
        replay.rewind("missing");
        assert_eq!(replay.events().len(), 2);
    }

    #[test]
    fn a_rewind_past_the_replay_keeps_the_history_before_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 8, Some(history));
        let prompted = |replay: &Replay| {
            let page = replay.page(u64::MAX, 100).unwrap();
            let mut ids: Vec<String> = page
                .events
                .iter()
                .map(|(_, event)| event)
                .chain(replay.entries.iter().map(|entry| &entry.event))
                .filter_map(|event| event.payload["messageId"].as_str().map(str::to_owned))
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        for n in 0..6 {
            let id = format!("p{n}");
            replay.push(
                ChatEventKind::Prompt,
                &json!({ "text": id, "paths": [], "messageId": id }),
            );
            replay.push(ChatEventKind::TurnStarted, &json!({}));
            replay.push(ChatEventKind::TurnCompleted, &json!({}));
        }
        replay.rewind("missing");
        assert_eq!(prompted(&replay), ["p0", "p1", "p2", "p3", "p4", "p5"]);
        replay.rewind("p2");
        assert_eq!(prompted(&replay), ["p0", "p1"]);
    }

    #[test]
    fn subagents_count_from_spawn_until_they_stop() {
        let update =
            |session: &str, update: Value| json!({ "sessionId": session, "update": update });
        let spawned =
            |id: &str| json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": id });
        let state = |id: &str, state: &str| json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": id, "state": state });
        let mut work = Work::default();
        work.note(&update("s", spawned("a")));
        work.note(&update("s", spawned("b")));
        work.note(&update("a", spawned("nested")));
        work.note(&update("s", state("a", "running")));
        assert_eq!(work.running_subagents(), 2);
        work.note(&update("s", state("a", "completed")));
        assert_eq!(work.running_subagents(), 1);
    }

    #[test]
    fn the_title_is_the_last_one_the_agent_named() {
        let info = |title: &str| {
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "session_info_update", "title": title },
            })
        };
        assert_eq!(
            session_title(&info("Fix the flaky test")).as_deref(),
            Some("Fix the flaky test")
        );
        assert_eq!(session_title(&info("  ")), None);
        assert_eq!(
            session_title(&text("agent_message_chunk", None, "hi")),
            None
        );
    }

    #[test]
    fn streamed_text_of_one_message_is_kept_as_one_event() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        for piece in ["Hel", "lo", " there"] {
            replay.push(
                ChatEventKind::SessionUpdate,
                &text("agent_message_chunk", Some("m1"), piece),
            );
        }
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_thought_chunk", Some("m1"), "hmm"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", Some("m2"), "next"),
        );
        let events = replay.events();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0].payload,
            text("agent_message_chunk", Some("m1"), "Hello there")
        );
        assert_eq!(events[2].payload["update"]["content"]["text"], "next");
    }

    #[test]
    fn only_plain_text_pieces_are_joined() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let mut tagged = text("agent_message_chunk", None, "a");
        tagged["update"]["_meta"] = json!({ "x": 1 });
        replay.push(ChatEventKind::SessionUpdate, &tagged);
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", None, "b"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", None, "c"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", None, "d"),
        );
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", None, "e"),
        );
        assert_eq!(replay.events().len(), 6);
    }

    #[test]
    fn status_and_ready_are_not_replayed() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        replay.push(ChatEventKind::Status, &json!({ "state": "starting" }));
        replay.push(ChatEventKind::Ready, &json!({}));
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(ChatEventKind::Error, &json!({ "message": "no" }));
        replay.push(
            ChatEventKind::TurnCompleted,
            &json!({ "stopReason": "end_turn" }),
        );
        assert_eq!(
            kinds(&replay),
            [
                ChatEventKind::TurnStarted,
                ChatEventKind::Error,
                ChatEventKind::TurnCompleted
            ]
        );
    }

    #[test]
    fn an_answered_permission_request_leaves_the_replay() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        replay.push(
            ChatEventKind::PermissionRequest,
            &json!({ "requestId": "a" }),
        );
        replay.push(
            ChatEventKind::PermissionRequest,
            &json!({ "requestId": "b" }),
        );
        let before = replay.bytes;
        replay.forget_permission("a");
        let events = replay.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["requestId"], "b");
        assert!(replay.bytes < before);
        assert!(!replay.is_trimmed());
    }

    #[test]
    fn a_client_catching_up_is_not_asked_an_answered_permission_request() {
        let mut fold = Fold::new(PermissionMode::WorkspaceWrite);
        for request_id in ["a", "b"] {
            fold.apply(
                0,
                ChatEventKind::PermissionRequest,
                &json!({
                    "requestId": request_id,
                    "sessionId": "s",
                    "toolCall": { "toolCallId": request_id, "title": "Touch a file" },
                    "options": [],
                }),
                0,
            );
        }
        fold.flush();
        fold.forget_permission("a");
        fold.flush();
        let missed = fold.catch_up(0).expect("still holds what it missed");
        let ChatChange::Head { set, .. } = &missed[0] else {
            panic!("a client catching up hears the head first");
        };
        let asked: Vec<&str> = set["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|ask| ask["requestId"].as_str())
            .collect();
        assert_eq!(asked, ["b"]);
    }

    #[test]
    fn a_replay_over_its_bounds_drops_the_oldest_and_says_so() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 3, None);
        for index in 0..5 {
            replay.push(ChatEventKind::Error, &json!({ "message": index }));
        }
        let events = replay.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].payload["message"], 2);
        assert!(replay.is_trimmed());

        let mut replay = Replay::new(400, MAX_REPLAY_EVENTS, None);
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", None, "x"),
        );
        assert!(!replay.is_trimmed());
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", None, &"y".repeat(500)),
        );
        assert!(replay.is_trimmed());
        assert!(replay.bytes <= 400);
    }

    pub(in super::super) fn turn(replay: &mut Replay, n: usize) {
        replay.push(
            ChatEventKind::Prompt,
            &json!({ "text": format!("ask {n}"), "paths": [] }),
        );
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        for piece in ["work", "ing"] {
            replay.push(
                ChatEventKind::SessionUpdate,
                &text("agent_message_chunk", Some(&format!("m{n}")), piece),
            );
        }
        replay.push(
            ChatEventKind::SessionUpdate,
            &json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "tool_call", "toolCallId": format!("t{n}"), "title": "ls" },
            }),
        );
        replay.push(
            ChatEventKind::TurnCompleted,
            &json!({ "stopReason": "end_turn" }),
        );
    }

    fn commands() -> Value {
        json!({
            "sessionId": "s",
            "update": { "sessionUpdate": "available_commands_update", "availableCommands": [] },
        })
    }

    fn asked<'a>(events: impl IntoIterator<Item = &'a ChatEvent>) -> Vec<String> {
        events
            .into_iter()
            .filter(|event| event.kind == ChatEventKind::Prompt)
            .map(|event| {
                event.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    pub(in super::super) fn asks(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|n| format!("ask {n}")).collect()
    }

    fn turn_starts(replay: &Replay) -> Vec<u64> {
        replay
            .entries
            .iter()
            .filter(|entry| entry.starts_turn)
            .map(|entry| entry.index)
            .collect()
    }

    #[test]
    fn a_turn_starts_where_the_person_speaks() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", Some("u1"), "loaded"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", Some("u2"), "same message, new block"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("agent_message_chunk", None, "answer"),
        );
        turn(&mut replay, 0);
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(
            ChatEventKind::SessionUpdate,
            &text("user_message_chunk", None, "<task-notification/>"),
        );
        replay.push(
            ChatEventKind::Prompt,
            &json!({ "text": "steer", "paths": [] }),
        );
        replay.push(
            ChatEventKind::TurnCompleted,
            &json!({ "stopReason": "end_turn" }),
        );
        assert_eq!(turn_starts(&replay), [0, 3, 8]);
    }

    #[test]
    fn a_trimmed_replay_still_pages_back_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 12, Some(history));
        replay.push(ChatEventKind::SessionUpdate, &commands());
        for n in 0..80 {
            turn(&mut replay, n);
        }
        assert!(replay.is_trimmed());

        let held = replay.fold_events();
        assert_eq!(held[0].2.payload, commands());
        assert_eq!(asked(held.iter().map(|(_, _, event)| event)), asks(78..80));

        let mut earlier: Vec<(u64, ChatEvent)> = Vec::new();
        let mut before = Some(1 + 78 * 5);
        while let Some(cursor) = before {
            let page = replay.page(cursor, 30).unwrap();
            assert!(asked(page.events.iter().map(|(_, event)| event)).len() <= 30);
            assert!(!page.events.is_empty());
            earlier.splice(0..0, page.events);
            before = page.older_before;
        }
        let earlier: Vec<ChatEvent> = earlier.into_iter().map(|(_, event)| event).collect();
        assert_eq!(earlier[0].payload, commands());
        assert_eq!(asked(&earlier), asks(0..78));
        assert!(earlier
            .iter()
            .any(|event| event.payload["update"]["content"]["text"] == "working"));
        assert_eq!(earlier.len(), 1 + 78 * 5);
    }

    pub(in super::super) fn update(session: &str, update: Value) -> Value {
        json!({ "sessionId": session, "update": update })
    }

    fn standing_kinds(events: &[(u64, u64, ChatEvent)]) -> Vec<(String, String)> {
        events
            .iter()
            .map(|(_, _, event)| event)
            .filter(|event| event.kind == ChatEventKind::SessionUpdate)
            .map(|event| {
                let update = &event.payload["update"];
                let id = ["asyncTaskId", "subagentSessionId"]
                    .iter()
                    .find_map(|key| update[*key].as_str())
                    .or_else(|| update["entries"][0]["content"].as_str())
                    .unwrap_or_default();
                (
                    update["sessionUpdate"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    id.to_owned(),
                )
            })
            .filter(|(kind, _)| kind != "agent_message_chunk" && kind != "tool_call")
            .collect()
    }

    #[test]
    fn a_retrying_agent_reads_as_reconnecting_until_it_answers_or_the_turn_ends() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let retry = |attempt: u64| {
            update(
                "s",
                crate::server::chat::failure::retrying("t:retry", "connection", attempt, "x"),
            )
        };
        replay.push(ChatEventKind::SessionUpdate, &retry(1));
        assert_eq!(replay.reconnecting, None);

        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(ChatEventKind::SessionUpdate, &retry(2));
        assert_eq!(replay.reconnecting, Some(2));
        let chunk = json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Back" } });
        replay.push(ChatEventKind::SessionUpdate, &update("s", chunk));
        assert_eq!(replay.reconnecting, None);

        let busy = crate::server::chat::failure::retrying(
            "t:retry",
            "service",
            4,
            "Anthropic answered 529",
        );
        replay.push(ChatEventKind::SessionUpdate, &update("s", busy));
        assert_eq!(replay.reconnecting, None);

        replay.push(ChatEventKind::SessionUpdate, &retry(3));
        replay.push(ChatEventKind::TurnCompleted, &json!({}));
        assert_eq!(replay.reconnecting, None);
    }

    #[test]
    fn a_chat_folded_from_its_end_still_hears_the_plan_and_the_work_still_going() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 50 * 5, None);
        let plan = |step: &str| {
            update(
                "s",
                json!({ "sessionUpdate": "plan", "entries": [{ "content": step, "status": "pending" }] }),
            )
        };
        let task = |kind: &str, id: &str, state: &str| {
            update(
                "s",
                json!({ "sessionUpdate": kind, "asyncTaskId": id, "state": state }),
            )
        };
        replay.push(ChatEventKind::SessionUpdate, &plan("first"));
        replay.push(ChatEventKind::SessionUpdate, &plan("second"));
        replay.push(
            ChatEventKind::SessionUpdate,
            &task("async_task_spawned", "shell", "running"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &task("async_task_spawned", "done", "running"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &task("async_task_progress", "shell", "running"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &task("async_task_state_update", "done", "completed"),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &update(
                "s",
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "helper" }),
            ),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &update(
                "helper",
                json!({ "sessionUpdate": "plan", "entries": [{ "content": "the helper's own" }] }),
            ),
        );
        replay.push(
            ChatEventKind::SessionUpdate,
            &update(
                "helper",
                json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "nested" }),
            ),
        );
        for n in 0..50 {
            turn(&mut replay, n);
        }
        assert_eq!(
            standing_kinds(&replay.fold_events()),
            [
                ("plan".to_owned(), "second".to_owned()),
                ("async_task_spawned".to_owned(), "shell".to_owned()),
                ("async_task_progress".to_owned(), "shell".to_owned()),
                ("subagent_spawned".to_owned(), "helper".to_owned()),
            ]
        );
    }

    #[test]
    fn a_running_subagent_stands_until_it_stops() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let spawned = |id: &str| {
            update(
                "s",
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": id }),
            )
        };
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(ChatEventKind::SessionUpdate, &spawned("a"));
        replay.push(ChatEventKind::SessionUpdate, &spawned("b"));
        replay.push(
            ChatEventKind::SessionUpdate,
            &update("s", json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "a", "state": "completed" })),
        );
        let held = |replay: &Replay| {
            replay
                .standing
                .iter()
                .filter_map(|(held, ..)| match held {
                    Holds::Subagent(id) => Some(id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(held(&replay), ["b"]);
        replay.push(
            ChatEventKind::TurnCompleted,
            &json!({ "stopReason": "end_turn" }),
        );
        assert_eq!(held(&replay), ["b"]);
        replay.push(ChatEventKind::TurnStarted, &json!({}));
        replay.push(
            ChatEventKind::TurnCompleted,
            &json!({ "stopReason": "cancelled" }),
        );
        assert_eq!(held(&replay), ["b"]);
    }

    #[tokio::test]
    async fn subagents_outlive_their_turn_but_not_the_agent() {
        let feed = Feed::new("agent".into(), PermissionMode::WorkspaceWrite, None);
        let spawned = json!({ "sessionId": "s", "update": { "sessionUpdate": "subagent_spawned", "subagentSessionId": "a" } });
        feed.emit(ChatEventKind::TurnStarted, json!({}));
        feed.emit(ChatEventKind::SessionUpdate, spawned.clone());
        feed.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "end_turn" }),
        );
        feed.emit(
            ChatEventKind::SessionUpdate,
            json!({ "sessionId": "s", "update": { "sessionUpdate": "async_task_spawned", "asyncTaskId": "shell" } }),
        );
        assert_eq!((feed.running_subagents(), feed.running_tasks()), (1, 1));

        feed.agent_gone();
        assert_eq!((feed.running_subagents(), feed.running_tasks()), (0, 0));
        let inner = feed.inner.lock().unwrap();
        assert!(inner.replay.standing.is_empty());
        let ended: Vec<Value> = inner.replay.events()[4..]
            .iter()
            .map(|event| event.payload["update"]["state"].clone())
            .collect();
        assert_eq!(ended, [json!("stopped"), json!("disconnected")]);
        drop(inner);

        feed.emit(ChatEventKind::SessionUpdate, spawned);
        feed.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "cancelled" }),
        );
        assert_eq!(feed.running_subagents(), 1);
    }
}
