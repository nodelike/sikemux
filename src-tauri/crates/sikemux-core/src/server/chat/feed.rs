//! What a chat agent says, on its way to the clients that show it. Streamed
//! updates leave in one batch per frame. Everything since the session started
//! or loaded is kept, so a client that attaches later rebuilds the chat from
//! the same events the others watched arrive.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::protocol::{
    encode_control, fits, ChatAttachment, ChatEvent, ChatEventKind, ChatMark, ChatStart, Event,
    RequestId, Response, ServerMessage,
};

use super::super::access::Peer;
use super::super::connection::{ClientConn, ClientId};
use super::history::{History, HistoryState, Page, Recovered, MAX_HISTORY_BYTES};
use super::markdown::{annotate_replay, Answers};
use super::queue::{batch_len, Queued, MAX_QUEUED};
use super::states::{Subagent, Task};

/// One frame's worth of streamed updates travels as a single event. Each
/// notification on its own costs a script eval in the webview, and an adapter
/// sends one per token.
const FLUSH: Duration = Duration::from_millis(16);
/// How long the newest event waits for more text to join it before it is
/// written to the history, so a crash loses at most this much of the chat.
const SETTLE: Duration = Duration::from_secs(1);
/// Streamed updates go out early once a batch holds this much.
const BATCH_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_REPLAY_EVENTS: usize = 20_000;
/// Roughly what an event costs beyond its payload once it is on the wire.
const EVENT_OVERHEAD: usize = 48;
/// The events kept as sent, for a client that reconnects to hear only what
/// it missed. Past this it rebuilds the chat from the replay.
const MAX_RECENT_BYTES: usize = 1024 * 1024;
const MAX_RECENT_EVENTS: usize = 4096;

/// A client opening a chat is sent about a screen or two of its end, so it
/// draws at once over a slow network, and pages back through the rest. A turn
/// longer than that, such as one full of screenshots, is cut where a page of
/// its history can start.
const TAIL_TURNS: usize = 8;
const TAIL_BYTES: usize = 256 * 1024;
const MAX_PAGE_TURNS: usize = 100;
const PAGE_BYTES: u64 = 1024 * 1024;
/// A turn longer than a page is cut into pieces about this long.
const PIECE_BYTES: usize = 64 * 1024;
/// Updates that set how the chat stands rather than say something in it. A
/// phone shown only the chat's end still needs the last of each.
const STANDING_UPDATES: [&str; 6] = [
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "usage_update",
    "session_info_update",
    "plan",
];

/// What a kept event still stands for, until a later one replaces or ends it.
#[derive(PartialEq, Eq, Serialize, Deserialize)]
enum Holds {
    Update(String),
    TaskSpawned(String),
    TaskLatest(String),
    Subagent(String),
}

#[derive(Serialize, Deserialize)]
struct Entry {
    event: ChatEvent,
    bytes: usize,
    index: u64,
    starts_turn: bool,
    /// A page of history can start here.
    point: bool,
    on_disk: bool,
}

/// The newest of a chat's events, and where the history before them ends.
pub(crate) struct Tail {
    pub events: Vec<ChatEvent>,
    pub older_before: Option<u64>,
}

/// The ordered events a late client replays. Text streamed into one message
/// is kept as one event, which the chat would have joined up anyway. Each is
/// written to the history on disk once nothing more joins it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Replay {
    entries: VecDeque<Entry>,
    bytes: usize,
    max_bytes: usize,
    max_events: usize,
    trimmed: bool,
    /// Handed over on its own, since the file stays where it is.
    #[serde(skip)]
    history: Option<History>,
    next_index: u64,
    in_turn: bool,
    /// The attempt an agent is retrying a failed request on, 0 when unsaid.
    reconnecting: Option<u32>,
    last_from_user: bool,
    since_point: usize,
    /// What a phone shown only the chat's end still needs from before it:
    /// the last of each standing update, and the background tasks and
    /// subagents still going.
    standing: Vec<(Holds, u64, ChatEvent)>,
    work: Work,
    /// Every subagent's start, so a slice of the chat that speaks of one
    /// started before it can carry the start too.
    spawns: Vec<Spawn>,
}

#[derive(Serialize, Deserialize)]
struct Spawn {
    subagent: String,
    /// The session that started it.
    parent: String,
    index: u64,
    event: ChatEvent,
}

/// One thing said or done in a chat, as another agent reads it.
pub(crate) enum Said {
    /// What the agent was sent: by the person, or another agent's message.
    Asked(String),
    /// Text the agent wrote.
    Message(String),
    Tool {
        id: String,
        title: Option<String>,
        failed: bool,
    },
    TurnStarted,
    TurnEnded {
        cancelled: bool,
    },
    Failed(String),
}

impl Said {
    fn of(event: &ChatEvent) -> Option<Self> {
        let text = |pointer: &str| {
            event
                .payload
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match event.kind {
            ChatEventKind::Prompt => text("/text").map(Said::Asked),
            ChatEventKind::TurnStarted => Some(Said::TurnStarted),
            ChatEventKind::TurnCompleted => Some(Said::TurnEnded {
                cancelled: text("/stopReason").as_deref() == Some("cancelled"),
            }),
            ChatEventKind::Error => Some(Said::Failed(text("/message").unwrap_or_default())),
            ChatEventKind::SessionUpdate => match session_update_kind(&event.payload)? {
                "agent_message_chunk" => text("/update/content/text").map(Said::Message),
                "user_message_chunk" => text("/update/content/text").map(Said::Asked),
                "tool_call" | "tool_call_update" => Some(Said::Tool {
                    id: text("/update/toolCallId")?,
                    title: text("/update/title"),
                    failed: text("/update/status").as_deref() == Some("failed"),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Said::Asked(text) | Said::Message(text) | Said::Failed(text) => text.len(),
            Said::Tool { id, title, .. } => id.len() + title.as_ref().map_or(0, String::len),
            Said::TurnStarted | Said::TurnEnded { .. } => 0,
        }
    }
}

/// What a chat said from one point on, without its thinking, its subagents'
/// work or what its tools returned.
pub(crate) struct Excerpt {
    /// Names this run of the chat; its indexes start over in the next.
    pub feed: String,
    /// Each with its index, and whether a turn starts there.
    pub said: Vec<(u64, bool, Said)>,
    /// Some of what came after the point asked for is no longer kept.
    pub skipped: bool,
    /// Where reading on starts.
    pub next: u64,
}

/// A streamed piece of a message that can be joined to the piece before it.
struct Chunk<'a> {
    session_id: &'a Value,
    kind: &'a str,
    message_id: Option<&'a Value>,
    text: &'a str,
}

fn only_keys(object: &serde_json::Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

/// What waits behind the running turn, as a session update apps that know
/// nothing of it pass over.
fn queue_update(inner: &Inner) -> Value {
    json!({
        "sessionId": inner.start.as_ref().map(|start| start.session_id.as_str()),
        "update": { "sessionUpdate": "queued_messages", "messages": inner.queued },
    })
}

/// The most of a chat's title that is kept.
const MAX_TITLE_CHARS: usize = 256;

/// A title as the chat shows it: one line of bounded length, or none when
/// blank.
pub(crate) fn title(text: &str) -> Option<String> {
    let title: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    (!title.is_empty()).then_some(title)
}

pub(crate) fn session_title(payload: &Value) -> Option<String> {
    let update = payload.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "session_info_update" {
        return None;
    }
    title(update.get("title")?.as_str()?)
}

/// A subagent or background task the agent has going.
#[derive(Serialize, Deserialize)]
struct Going {
    id: String,
    /// The session whose transcript shows it.
    session: String,
    /// Started inside a subagent rather than by the chat's own agent.
    nested: bool,
    /// Running rather than paused.
    running: bool,
}

/// The subagents and background tasks the agent has going, which end with
/// it when its process goes.
#[derive(Default, Serialize, Deserialize)]
struct Work {
    subagents: Vec<Going>,
    tasks: Vec<Going>,
    /// Every subagent's session, whose own updates are not the chat's.
    subagent_sessions: HashSet<String>,
}

impl Work {
    fn in_subagent(&self, payload: &Value) -> bool {
        payload
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| self.subagent_sessions.contains(session))
    }

    /// Notes what `payload` starts or ends.
    fn note(&mut self, payload: &Value) {
        let Some(update) = payload.get("update") else {
            return;
        };
        let session = payload
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let nested = self.in_subagent(payload);
        let text = |key: &str| update.get(key).and_then(Value::as_str);
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("subagent_spawned") => {
                let Some(id) = text("subagentSessionId") else {
                    return;
                };
                self.subagent_sessions.insert(id.to_owned());
                if !self.subagents.iter().any(|going| going.id == id) {
                    self.subagents.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some("subagent_state_update") => {
                let Some(id) = text("subagentSessionId") else {
                    return;
                };
                if subagent_ended(update) {
                    self.subagents.retain(|going| going.id != id);
                } else if !self.subagents.iter().any(|going| going.id == id) {
                    self.subagents.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some("async_task_spawned") => {
                let Some(id) = text("asyncTaskId") else {
                    return;
                };
                if !self.tasks.iter().any(|going| going.id == id) {
                    self.tasks.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some("async_task_progress" | "async_task_state_update") => {
                let Some(id) = text("asyncTaskId") else {
                    return;
                };
                match text("state").and_then(Task::parse) {
                    Some(state) if state.ended() => self.tasks.retain(|going| going.id != id),
                    Some(state) => {
                        if let Some(going) = self.tasks.iter_mut().find(|going| going.id == id) {
                            going.running = state == Task::Running;
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn running_subagents(&self) -> usize {
        self.subagents.iter().filter(|going| !going.nested).count()
    }

    fn running_tasks(&self) -> usize {
        self.tasks
            .iter()
            .filter(|going| !going.nested && going.running)
            .count()
    }

    /// The updates that end everything still going, the newest first.
    fn endings(&self) -> Vec<Value> {
        let tasks = self.tasks.iter().rev().map(|going| {
            json!({ "sessionId": going.session, "update": Task::Stopped.update(&going.id) })
        });
        let subagents = self.subagents.iter().rev().map(|going| {
            json!({ "sessionId": going.session, "update": Subagent::Disconnected.update(&going.id) })
        });
        tasks.chain(subagents).collect()
    }
}

fn subagent_ended(update: &Value) -> bool {
    update
        .get("state")
        .and_then(Value::as_str)
        .and_then(Subagent::parse)
        .is_some_and(Subagent::ended)
}

fn task_ended(update: &Value) -> bool {
    update
        .get("state")
        .and_then(Value::as_str)
        .and_then(Task::parse)
        .is_some_and(Task::ended)
}

fn chunk(payload: &Value) -> Option<Chunk<'_>> {
    let notification = payload.as_object()?;
    if !only_keys(notification, &["sessionId", "update"]) {
        return None;
    }
    let update = notification.get("update")?.as_object()?;
    if !only_keys(update, &["sessionUpdate", "content", "messageId"]) {
        return None;
    }
    let kind = update.get("sessionUpdate")?.as_str()?;
    if kind != "agent_message_chunk" && kind != "agent_thought_chunk" {
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

fn is_kept(kind: ChatEventKind) -> bool {
    !matches!(kind, ChatEventKind::Status | ChatEventKind::Ready)
}

fn is_turn_edge(kind: ChatEventKind) -> bool {
    matches!(
        kind,
        ChatEventKind::TurnStarted | ChatEventKind::TurnCompleted | ChatEventKind::Error
    )
}

/// When a turn's edges happened, so a client rebuilding the chat later can
/// tell how long each turn took.
fn stamp(payload: &mut Value) {
    if let Some(fields) = payload.as_object_mut() {
        fields
            .entry("at")
            .or_insert_with(|| json!(sikemux_process::clock::epoch_ms()));
    }
}

fn asks(event: &ChatEvent, request_id: &str) -> bool {
    event.kind == ChatEventKind::PermissionRequest
        && event.payload.get("requestId").and_then(Value::as_str) == Some(request_id)
}

/// Whether `event` is where the person's message `message_id` starts.
fn names_message(event: &ChatEvent, message_id: &str) -> bool {
    match event.kind {
        ChatEventKind::Prompt => event.payload.get("messageId"),
        ChatEventKind::SessionUpdate
            if session_update_kind(&event.payload) == Some("user_message_chunk") =>
        {
            event.payload.pointer("/update/messageId")
        }
        _ => None,
    }
    .and_then(Value::as_str)
        == Some(message_id)
}

fn session_update_kind(payload: &Value) -> Option<&str> {
    payload.pointer("/update/sessionUpdate")?.as_str()
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
            reconnecting: None,
            last_from_user: false,
            since_point: 0,
            standing: Vec::new(),
            work: Work::default(),
            spawns: Vec::new(),
        }
    }

    /// The replay of a chat an earlier core left, read back from its history
    /// file `name` in `dir`: the newest events it has room for, and what still
    /// stands from all of them.
    fn recovered(dir: &std::path::Path, name: &str) -> Self {
        let mut replay = Self::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let history = History::recover(dir, name, MAX_HISTORY_BYTES, |recovered| {
            replay.restore(recovered);
        });
        replay.history = Some(history);
        replay
    }

    fn restore(&mut self, recovered: Recovered) {
        let Recovered {
            index,
            starts_turn,
            point,
            bytes,
            event,
        } = recovered;
        let kind = event.kind;
        self.note_standing(kind, index, &event);
        self.note_spawn(kind, index, &event);
        if kind == ChatEventKind::SessionUpdate {
            self.work.note(&event.payload);
        }
        match kind {
            ChatEventKind::TurnStarted => self.in_turn = true,
            ChatEventKind::TurnCompleted | ChatEventKind::Error => self.in_turn = false,
            _ => {}
        }
        self.last_from_user = kind == ChatEventKind::Prompt
            || session_update_kind(&event.payload) == Some("user_message_chunk");
        self.entries.push_back(Entry {
            event,
            bytes,
            index,
            starts_turn,
            point,
            on_disk: true,
        });
        self.bytes += bytes;
        self.next_index = index + 1;
        self.trim();
    }

    /// Writes the newest event to the history, which nothing joins after.
    fn settle(&mut self) {
        if let Some(last) = self.entries.back_mut() {
            Self::write(&mut self.history, last);
        }
    }

    fn write(history: &mut Option<History>, entry: &mut Entry) {
        if entry.on_disk {
            return;
        }
        entry.on_disk = true;
        if let Some(history) = history.as_mut() {
            history.append(entry.index, &entry.event, entry.point, entry.starts_turn);
        }
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
                session_update_kind(payload) == Some("user_message_chunk")
                    && !self.in_turn
                    && !self.last_from_user
            }
            _ => false,
        }
    }

    fn join_text(&mut self, payload: &Value) -> bool {
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
        last.bytes += later.text.len();
        self.bytes += later.text.len();
        self.since_point += later.text.len();
        true
    }

    /// Keeps `payload`, answering with about how many bytes it took.
    pub(crate) fn push(&mut self, kind: ChatEventKind, payload: &Value) -> usize {
        if !is_kept(kind) {
            return 0;
        }
        if kind == ChatEventKind::SessionUpdate && self.join_text(payload) {
            self.trim();
            return chunk(payload).map_or(0, |chunk| chunk.text.len());
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
        self.note_spawn(kind, index, &event);
        if kind == ChatEventKind::SessionUpdate {
            self.work.note(payload);
            self.note_retry(payload);
        }
        match kind {
            ChatEventKind::TurnStarted => {
                self.in_turn = true;
                self.reconnecting = None;
            }
            ChatEventKind::TurnCompleted | ChatEventKind::Error => {
                self.in_turn = false;
                self.reconnecting = None;
            }
            _ => {}
        }
        self.last_from_user = kind == ChatEventKind::Prompt
            || session_update_kind(payload) == Some("user_message_chunk");
        self.entries.push_back(Entry {
            event,
            bytes,
            index,
            starts_turn,
            point,
            on_disk: false,
        });
        self.bytes += bytes;
        self.trim();
        bytes
    }

    fn note_spawn(&mut self, kind: ChatEventKind, index: u64, event: &ChatEvent) {
        if kind != ChatEventKind::SessionUpdate
            || session_update_kind(&event.payload) != Some("subagent_spawned")
        {
            return;
        }
        let text = |pointer: &str| event.payload.pointer(pointer).and_then(Value::as_str);
        let (Some(subagent), Some(parent)) =
            (text("/update/subagentSessionId"), text("/sessionId"))
        else {
            return;
        };
        if self.spawns.iter().any(|spawn| spawn.subagent == subagent) {
            return;
        }
        self.spawns.push(Spawn {
            subagent: subagent.to_owned(),
            parent: parent.to_owned(),
            index,
            event: event.clone(),
        });
    }

    /// The starts of the subagents `events` speak of, and of the subagents
    /// that started those, from before `first`. Without them a client would
    /// take a subagent's work for the chat's own and never see it end.
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
        chosen.sort_by_key(|spawn| spawn.index);
        chosen
            .into_iter()
            .map(|spawn| (spawn.index, &spawn.event))
            .collect()
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
        let Some(update_kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
            return;
        };
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_owned);
        if self.work.in_subagent(&event.payload) {
            return;
        }
        match update_kind {
            standing if STANDING_UPDATES.contains(&standing) => {
                self.hold(Holds::Update(standing.to_owned()), index, event);
            }
            "subagent_spawned" => {
                if let Some(id) = text("subagentSessionId") {
                    self.hold(Holds::Subagent(id), index, event);
                }
            }
            "subagent_state_update" if subagent_ended(update) => {
                if let Some(id) = text("subagentSessionId") {
                    self.standing
                        .retain(|(held, ..)| *held != Holds::Subagent(id.clone()));
                }
            }
            "async_task_spawned" => {
                if let Some(id) = text("asyncTaskId") {
                    self.hold(Holds::TaskSpawned(id), index, event);
                }
            }
            "async_task_progress" | "async_task_state_update" => {
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

    /// What the agent wrote last in its latest turn: its text after its last
    /// tool call, or before it when it wrote nothing after.
    fn last_answer(&self) -> Option<String> {
        let mut pieces: Vec<&str> = Vec::new();
        for entry in self.entries.iter().rev() {
            match entry.event.kind {
                ChatEventKind::TurnStarted | ChatEventKind::Prompt => break,
                ChatEventKind::SessionUpdate => {}
                _ => continue,
            }
            let payload = &entry.event.payload;
            if self.work.in_subagent(payload) {
                continue;
            }
            match session_update_kind(payload) {
                Some("user_message_chunk") => break,
                Some("tool_call" | "tool_call_update") if !pieces.is_empty() => break,
                Some("agent_message_chunk") => {
                    if let Some(text) = payload
                        .pointer("/update/content/text")
                        .and_then(Value::as_str)
                    {
                        pieces.push(text);
                    }
                }
                _ => {}
            }
        }
        let answer: String = pieces.into_iter().rev().collect();
        let answer = answer.trim();
        (!answer.is_empty()).then(|| answer.to_owned())
    }

    /// What the chat said from index `from` on, or over its last `turns`
    /// turns, stopping at a turn once past `max_bytes`. A message still
    /// streaming in is left for the next read.
    fn excerpt(
        &self,
        from: Option<u64>,
        turns: usize,
        max_bytes: usize,
    ) -> (Vec<(u64, bool, Said)>, bool, u64) {
        let start = match from {
            Some(from) => self.entries.partition_point(|entry| entry.index < from),
            None => self
                .entries
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, entry)| entry.starts_turn)
                .take(turns)
                .last()
                .map_or(0, |(position, _)| position),
        };
        let skipped =
            from.is_some_and(|from| self.entries.front().is_some_and(|first| first.index > from));
        let mut said = Vec::new();
        let mut bytes = 0;
        for (position, entry) in self.entries.iter().enumerate().skip(start) {
            if bytes >= max_bytes && entry.starts_turn && !said.is_empty() {
                return (said, skipped, entry.index);
            }
            if self.work.in_subagent(&entry.event.payload) {
                continue;
            }
            let Some(item) = Said::of(&entry.event) else {
                continue;
            };
            let streaming = position + 1 == self.entries.len() && self.in_turn;
            if streaming && matches!(item, Said::Message(_)) {
                return (said, skipped, entry.index);
            }
            bytes += item.bytes();
            said.push((entry.index, entry.starts_turn, item));
        }
        (said, skipped, self.next_index)
    }

    /// The last turns from index `floor` on, as many as fit, and whatever set
    /// how the chat stands before them. When not even the last turn fits, it is
    /// cut where a page of its history can start.
    pub(crate) fn tail(&self, floor: u64) -> Tail {
        let lowest = self.entries.partition_point(|entry| entry.index < floor);
        let mut chosen = None;
        let mut piece = None;
        let mut turns = 0;
        let mut bytes = 0;
        for (position, entry) in self.entries.iter().enumerate().skip(lowest).rev() {
            bytes += entry.bytes;
            if bytes > TAIL_BYTES && (chosen.is_some() || piece.is_some()) {
                break;
            }
            if entry.point {
                piece = Some(position);
            }
            if entry.starts_turn {
                chosen = Some(position);
                turns += 1;
                if turns >= TAIL_TURNS {
                    break;
                }
            }
        }
        let cut = chosen.or(piece);
        let start = cut.unwrap_or(lowest);
        let Some(first) = self.entries.get(start).map(|entry| entry.index) else {
            return Tail {
                events: Vec::new(),
                older_before: None,
            };
        };
        let oldest = self
            .history
            .as_ref()
            .and_then(History::first)
            .unwrap_or(first);
        let mut standing: Vec<(u64, &ChatEvent)> = self
            .standing
            .iter()
            .filter(|(_, index, _)| (floor..first).contains(index))
            .map(|(_, index, event)| (*index, event))
            .collect();
        let named = self.entries.range(start..).map(|entry| &entry.event);
        for (index, event) in self.spawns_before(named, first) {
            if !standing.iter().any(|(held, _)| *held == index) {
                standing.push((index, event));
            }
        }
        standing.sort_by_key(|(index, _)| *index);
        Tail {
            events: standing
                .into_iter()
                .map(|(_, event)| event.clone())
                .chain(self.entries.range(start..).map(|entry| entry.event.clone()))
                .collect(),
            older_before: (cut.is_some() && first > oldest).then_some(first),
        }
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
        self.in_turn = false;
        self.reconnecting = None;
        self.last_from_user = false;
        self.since_point = 0;
    }

    /// An agent retrying says so as a warning; its next output means it got through.
    /// A service that answered with an error is not a lost connection.
    fn note_retry(&mut self, payload: &Value) {
        match session_update_kind(payload) {
            Some("session_info_update") if self.in_turn => {
                let notice = payload.pointer("/update/_meta/jetbrains/air/sessionFailure");
                if let Some(notice) = notice.filter(|notice| notice["severity"] == "warning") {
                    let attempt = notice["attempt"].as_u64().unwrap_or(0);
                    self.reconnecting = (notice["category"] == "connection")
                        .then(|| u32::try_from(attempt).unwrap_or(u32::MAX));
                }
            }
            Some(
                "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update",
            ) => {
                self.reconnecting = None;
            }
            _ => {}
        }
    }

    pub(crate) fn page(&self, before: u64, turns: usize) -> Result<Page, String> {
        let Some(history) = self.history.as_ref() else {
            return Ok(Page::default());
        };
        let mut page = history
            .page(before, turns.clamp(1, MAX_PAGE_TURNS), PAGE_BYTES)
            .map_err(|error| error.to_string())?;
        let spawns: Vec<ChatEvent> = self
            .spawns_before(page.events.iter(), page.first)
            .into_iter()
            .map(|(_, event)| event.clone())
            .collect();
        page.events.splice(0..0, spawns);
        Ok(page)
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

    fn turn_start(&self) -> Option<u64> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.starts_turn)
            .map(|entry| entry.index)
    }

    pub(crate) fn is_trimmed(&self) -> bool {
        self.trimmed
    }

    pub(crate) fn events(&self) -> Vec<ChatEvent> {
        self.events_from(0)
    }

    fn events_from(&self, floor: u64) -> Vec<ChatEvent> {
        let lowest = self.entries.partition_point(|entry| entry.index < floor);
        self.entries
            .range(lowest..)
            .map(|entry| entry.event.clone())
            .collect()
    }
}

/// One event as clients heard it.
#[derive(Serialize, Deserialize)]
struct Sent {
    seq: u64,
    event: ChatEvent,
    bytes: usize,
    /// Who sent it, when it is a prompt the sender was not told of.
    sender: Option<Peer>,
}

/// The newest events as sent, numbered, so a client that held everything
/// up to some number can be told just the rest.
#[derive(Default, Serialize, Deserialize)]
struct Recent {
    sent: VecDeque<Sent>,
    bytes: usize,
    /// Every event after this number is still kept.
    kept_after: u64,
}

impl Recent {
    fn push(&mut self, sent: Sent) {
        self.bytes += sent.bytes;
        self.sent.push_back(sent);
        while self.bytes > MAX_RECENT_BYTES || self.sent.len() > MAX_RECENT_EVENTS {
            let Some(dropped) = self.sent.pop_front() else {
                break;
            };
            self.bytes -= dropped.bytes;
            self.kept_after = dropped.seq;
        }
    }

    /// A client catching up is not asked a request someone already answered.
    fn forget_permission(&mut self, request_id: &str) {
        let mut freed = 0;
        self.sent.retain(|sent| {
            let answered = asks(&sent.event, request_id);
            if answered {
                freed += sent.bytes;
            }
            !answered
        });
        self.bytes -= freed;
    }

    fn since(&self, seq: u64, peer: &Peer) -> Option<Vec<ChatEvent>> {
        if seq < self.kept_after {
            return None;
        }
        Some(
            self.sent
                .iter()
                .filter(|sent| sent.seq > seq && sent.sender.as_ref() != Some(peer))
                .map(|sent| sent.event.clone())
                .collect(),
        )
    }
}

/// What a client attaching now is told about the chat besides its replay.
pub(crate) struct Standing {
    pub running: bool,
    pub turned: bool,
}

struct Inner {
    subscribers: HashMap<ClientId, Arc<ClientConn>>,
    seq: u64,
    recent: Recent,
    replay: Replay,
    pending: Vec<Value>,
    pending_bytes: usize,
    flush_scheduled: bool,
    settle_scheduled: bool,
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
    /// The session ended, so nobody new is let in to wait for events that
    /// will not come.
    closed: bool,
    /// Events from this index on are kept but not sent yet.
    holding: Option<u64>,
    /// What the person sent while the agent was at work, oldest first.
    queued: Vec<Queued>,
    /// The answers being written, as their clients' trees of them stand.
    answers: Answers,
}

/// A feed as plain data, for a core that takes the chat over. Clients that
/// held a mark of it catch up from where they were.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeedState {
    id: String,
    seq: u64,
    recent: Recent,
    replay: Replay,
    history: Option<HistoryState>,
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
}

pub(crate) struct Feed {
    agent_id: String,
    /// Names this run of the chat's agent in the marks clients hold.
    id: String,
    runtime: tokio::runtime::Handle,
    inner: Mutex<Inner>,
}

impl Feed {
    /// With `history`, everything the chat says is also kept in a file there,
    /// for phones to page back through.
    pub(crate) fn new(
        agent_id: String,
        permission_mode: String,
        history: Option<&std::path::Path>,
    ) -> Arc<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        let history = history.map(|dir| History::open(dir, &id, MAX_HISTORY_BYTES));
        Arc::new(Self {
            agent_id,
            id,
            runtime: tokio::runtime::Handle::current(),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                seq: 0,
                recent: Recent::default(),
                replay: Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, history),
                pending: Vec::new(),
                pending_bytes: 0,
                flush_scheduled: false,
                settle_scheduled: false,
                start: None,
                permission_mode,
                title: None,
                closed: false,
                holding: None,
                queued: Vec::new(),
                answers: Answers::default(),
            }),
        })
    }

    /// A feed for a chat an earlier core left when it stopped, whose history
    /// file `from` in `dir` is taken over under this feed's name. Nothing in
    /// it is read until [`Feed::recover`].
    pub(crate) fn recovering(
        agent_id: String,
        permission_mode: String,
        dir: Option<&std::path::Path>,
        from: &str,
    ) -> Arc<Self> {
        let feed = Self::new(agent_id, permission_mode, None);
        if let Some(dir) = dir {
            let path = |name: &str| dir.join(format!("{name}.jsonl"));
            if let Err(error) = std::fs::rename(path(from), path(&feed.id)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "sikemux core: a chat's history was not kept through the restart: {error}"
                    );
                }
            }
        }
        feed
    }

    /// Reads back the history [`Feed::recovering`] took over, and goes on
    /// writing it. Reads the disk, so it blocks.
    pub(crate) fn recover(&self, dir: &std::path::Path) {
        let replay = Replay::recovered(dir, &self.id);
        let title = replay
            .standing
            .iter()
            .rev()
            .find_map(|(_, _, event)| session_title(&event.payload));
        let answers =
            Answers::read_back(replay.events_from(replay.turn_start().unwrap_or_default()));
        let mut inner = self.inner.locked();
        inner.replay = replay;
        inner.title = title;
        inner.answers = answers;
    }

    /// Takes up a feed an earlier core let go of.
    pub(crate) fn restored(agent_id: String, state: FeedState) -> Arc<Self> {
        let mut replay = state.replay;
        replay.history = state.history.map(History::reopen);
        let answers =
            Answers::read_back(replay.events_from(replay.turn_start().unwrap_or_default()));
        Arc::new(Self {
            agent_id,
            id: state.id,
            runtime: tokio::runtime::Handle::current(),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                seq: state.seq,
                recent: state.recent,
                replay,
                pending: Vec::new(),
                pending_bytes: 0,
                flush_scheduled: false,
                settle_scheduled: false,
                start: state.start,
                permission_mode: state.permission_mode,
                title: state.title,
                closed: false,
                holding: None,
                queued: Vec::new(),
                answers,
            }),
        })
    }

    /// Lets go of the feed for a core that takes the chat over, once what is
    /// waiting to go out has gone, with the clients that were hearing it.
    /// Nothing said here after that is kept.
    pub(crate) fn release(&self) -> Option<(FeedState, Vec<Arc<ClientConn>>)> {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.closed = true;
        let subscribers = inner
            .subscribers
            .drain()
            .map(|(_, client)| client)
            .collect();
        let history = inner.replay.history.take().map(History::release);
        let replay = std::mem::replace(
            &mut inner.replay,
            Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None),
        );
        let state = FeedState {
            id: self.id.clone(),
            seq: inner.seq,
            recent: std::mem::take(&mut inner.recent),
            replay,
            history,
            start: inner.start.clone(),
            permission_mode: inner.permission_mode.clone(),
            title: inner.title.clone(),
        };
        Some((state, subscribers))
    }

    /// The name of the file the feed keeps its history in.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    fn broadcast(&self, inner: &mut Inner, kind: ChatEventKind, payload: Value) {
        self.broadcast_except(inner, kind, payload, None);
    }

    fn mark(&self, inner: &Inner) -> ChatMark {
        ChatMark {
            feed: self.id.clone(),
            seq: inner.seq,
        }
    }

    fn broadcast_except(
        &self,
        inner: &mut Inner,
        kind: ChatEventKind,
        payload: Value,
        except: Option<ClientId>,
    ) {
        if matches!(
            kind,
            ChatEventKind::Prompt
                | ChatEventKind::TurnStarted
                | ChatEventKind::TurnCompleted
                | ChatEventKind::Error
        ) {
            inner.answers.close_all();
        }
        inner.seq += 1;
        let seq = inner.seq;
        let event = ChatEvent { kind, payload };
        let Ok(frame) = encode_control(&ServerMessage::Event {
            event: Event::Chat {
                agent_id: self.agent_id.clone(),
                seq,
                event: event.clone(),
            },
        }) else {
            return;
        };
        if !fits(&frame) {
            eprintln!(
                "sikemux core: a chat event of {} bytes is too large to send",
                frame.len()
            );
            return;
        }
        let sender = except
            .and_then(|id| inner.subscribers.get(&id))
            .map(|client| client.peer.clone());
        inner.recent.push(Sent {
            seq,
            event,
            bytes: frame.len(),
            sender,
        });
        let frame: Arc<[u8]> = frame.into();
        inner
            .subscribers
            .retain(|id, client| Some(*id) == except || client.send(frame.clone()));
    }

    fn flush_locked(&self, inner: &mut Inner) {
        if inner.pending.is_empty() {
            return;
        }
        let mut updates = std::mem::take(&mut inner.pending);
        inner.pending_bytes = 0;
        inner.answers.annotate(&mut updates);
        self.broadcast(
            inner,
            ChatEventKind::SessionUpdate,
            json!({ "updates": updates }),
        );
    }

    /// Writes the newest event to the history once it has had a while to
    /// take in more.
    fn settle_later(self: &Arc<Self>, inner: &mut Inner) {
        if inner.settle_scheduled || !inner.replay.keeps_history() {
            return;
        }
        inner.settle_scheduled = true;
        let feed = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(SETTLE).await;
            let mut inner = feed.inner.locked();
            inner.settle_scheduled = false;
            inner.replay.settle();
        });
    }

    fn flush(&self) {
        let mut inner = self.inner.locked();
        inner.flush_scheduled = false;
        self.flush_locked(&mut inner);
    }

    /// Anything that is not a streamed update reads as a reply to what came
    /// before it, so the batch behind it goes out first and the order the
    /// agent sent them in survives.
    pub(crate) fn emit(self: &Arc<Self>, kind: ChatEventKind, mut payload: Value) {
        let mut inner = self.inner.locked();
        if is_turn_edge(kind) {
            stamp(&mut payload);
        }
        let bytes = inner.replay.push(kind, &payload);
        self.settle_later(&mut inner);
        if let Some(title) = session_title(&payload) {
            inner.title = Some(title);
        }
        if inner.holding.is_some() && is_kept(kind) {
            return;
        }
        if kind == ChatEventKind::SessionUpdate {
            inner.pending.push(payload);
            inner.pending_bytes += bytes;
            // A loaded chat replays its history all at once, and in one batch
            // it would outgrow what a client can be sent.
            if inner.pending_bytes >= BATCH_BYTES {
                self.flush_locked(&mut inner);
                return;
            }
            if !inner.flush_scheduled {
                inner.flush_scheduled = true;
                let feed = self.clone();
                self.runtime.spawn(async move {
                    tokio::time::sleep(FLUSH).await;
                    feed.flush();
                });
            }
            return;
        }
        self.flush_locked(&mut inner);
        self.broadcast(&mut inner, kind, payload);
    }

    /// The client that sent a prompt already shows it; everyone else, and every
    /// later replay, learns it here.
    pub(crate) fn prompted(
        self: &Arc<Self>,
        from: ClientId,
        message_id: Option<&str>,
        text: &str,
        paths: &[String],
    ) {
        let mut inner = self.inner.locked();
        let mut payload = json!({ "text": text, "paths": paths });
        if let Some(message_id) = message_id {
            payload["messageId"] = json!(message_id);
        }
        stamp(&mut payload);
        inner.replay.push(ChatEventKind::Prompt, &payload);
        self.settle_later(&mut inner);
        self.flush_locked(&mut inner);
        self.broadcast_except(&mut inner, ChatEventKind::Prompt, payload, Some(from));
    }

    /// The person took the chat back to before their message `message_id`.
    /// It goes from the replay, and every client but the one that asked is
    /// told to drop it and everything after it.
    pub(crate) fn rewound(&self, from: ClientId, session_id: &str, message_id: &str) {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.replay.rewind(message_id);
        self.broadcast_except(
            &mut inner,
            ChatEventKind::SessionUpdate,
            json!({
                "sessionId": session_id,
                "update": { "sessionUpdate": "message_rewound", "messageId": message_id },
            }),
            Some(from),
        );
    }

    /// Keeps `message` until the running turn ends, unless as many already wait.
    pub(crate) fn enqueue(&self, message: Queued) -> bool {
        let mut inner = self.inner.locked();
        if inner.queued.len() >= MAX_QUEUED {
            return false;
        }
        inner.queued.push(message);
        self.queue_changed(&mut inner);
        true
    }

    /// Takes the waiting messages `ids`, or with none the next that go out together.
    pub(crate) fn take_queued(&self, ids: Option<&[String]>) -> Vec<Queued> {
        let mut inner = self.inner.locked();
        let taken: Vec<Queued> = match ids {
            None => {
                let count = batch_len(&inner.queued);
                inner.queued.drain(..count).collect()
            }
            Some(ids) => {
                let (taken, kept) = std::mem::take(&mut inner.queued)
                    .into_iter()
                    .partition(|message| ids.contains(&message.id));
                inner.queued = kept;
                taken
            }
        };
        if !taken.is_empty() {
            self.queue_changed(&mut inner);
        }
        taken
    }

    pub(crate) fn clear_queue(&self) {
        let mut inner = self.inner.locked();
        if !inner.queued.is_empty() {
            inner.queued.clear();
            self.queue_changed(&mut inner);
        }
    }

    fn queue_changed(&self, inner: &mut Inner) {
        self.flush_locked(inner);
        let update = queue_update(inner);
        self.broadcast(inner, ChatEventKind::SessionUpdate, update);
    }

    pub(crate) fn subscribe(&self, client: &Arc<ClientConn>) {
        let mut inner = self.inner.locked();
        inner.subscribers.insert(client.id, client.clone());
    }

    pub(crate) fn has_subscriber(&self, client: ClientId) -> bool {
        self.inner.locked().subscribers.contains_key(&client)
    }

    pub(crate) fn unsubscribe(&self, client: ClientId) {
        let mut inner = self.inner.locked();
        inner.subscribers.remove(&client);
    }

    /// Answers the client with everything said so far, or everything since
    /// `since`, and adds it to the listeners in the same step, so it hears
    /// every later event exactly once.
    pub(crate) fn attach(
        &self,
        client: &Arc<ClientConn>,
        request_id: RequestId,
        standing: Standing,
        since: Option<ChatMark>,
    ) {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        let attachment = self.attachment(&inner, &client.peer, standing, since);
        let live = matches!(
            attachment,
            ChatAttachment::Live { .. } | ChatAttachment::Resumed { .. }
        );
        client.respond(request_id, Ok(Response::ChatAttached { attachment }));
        if live {
            inner.subscribers.insert(client.id, client.clone());
        }
    }

    /// A phone, or the host's own app once the chat outgrew the replay, is sent
    /// the chat's last turns and pages back through the rest. A chat that
    /// outgrew the replay with nowhere to keep the rest starts with a note
    /// that its earlier turns are gone.
    fn attachment(
        &self,
        inner: &Inner,
        peer: &Peer,
        standing: Standing,
        since: Option<ChatMark>,
    ) -> ChatAttachment {
        let missed = since
            .filter(|since| since.feed == self.id && since.seq <= inner.seq)
            .and_then(|since| inner.recent.since(since.seq, peer));
        let trimmed = inner.replay.is_trimmed();
        let lost = trimmed && !inner.replay.keeps_history();
        let paged = lost || (inner.replay.keeps_history() && (!peer.is_local() || trimmed));
        match (inner.start.clone(), missed) {
            _ if inner.closed => ChatAttachment::Missing,
            (Some(_), Some(events)) => ChatAttachment::Resumed {
                events,
                mark: self.mark(inner),
            },
            (Some(start), None) => {
                let (mut replay, older_before) = if paged {
                    let tail = inner.replay.tail(0);
                    (tail.events, tail.older_before)
                } else {
                    (inner.replay.events(), None)
                };
                if !inner.queued.is_empty() {
                    replay.push(ChatEvent {
                        kind: ChatEventKind::SessionUpdate,
                        payload: queue_update(inner),
                    });
                }
                annotate_replay(&mut replay);
                if lost {
                    replay.insert(
                        0,
                        ChatEvent {
                            kind: ChatEventKind::SessionUpdate,
                            payload: json!({
                                "sessionId": start.session_id,
                                "update": { "sessionUpdate": "earlier_turns_lost" },
                            }),
                        },
                    );
                }
                ChatAttachment::Live {
                    start: Box::new(start),
                    permission_mode: inner.permission_mode.clone(),
                    running: standing.running,
                    turned: standing.turned,
                    replay,
                    mark: self.mark(inner),
                    older_before,
                }
            }
            (None, _) => ChatAttachment::Missing,
        }
    }

    pub(crate) fn close(&self) {
        let mut inner = self.inner.locked();
        inner.closed = true;
    }

    /// Keeps what the agent says from the clients until [`Feed::let_out`]. A
    /// resumed session replays its whole saved history while it starts, more
    /// than a client could be sent at once.
    pub(crate) fn hold(&self) {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.holding = Some(inner.replay.next_index);
    }

    /// Sends the clients the last turns of what was held back, then where
    /// the turns before them can be paged from. Without a history on disk to
    /// page through, they are sent all the replay still holds.
    pub(crate) fn let_out(&self) {
        let mut inner = self.inner.locked();
        let Some(floor) = inner.holding.take() else {
            return;
        };
        let tail = if inner.replay.keeps_history() {
            inner.replay.tail(floor)
        } else {
            Tail {
                events: inner.replay.events_from(floor),
                older_before: None,
            }
        };
        for event in tail.events {
            if event.kind != ChatEventKind::SessionUpdate {
                self.flush_locked(&mut inner);
                self.broadcast(&mut inner, event.kind, event.payload);
                continue;
            }
            inner.pending_bytes +=
                serde_json::to_vec(&event.payload).map_or(0, |bytes| bytes.len());
            inner.pending.push(event.payload);
            if inner.pending_bytes >= BATCH_BYTES {
                self.flush_locked(&mut inner);
            }
        }
        if let Some(before) = tail.older_before {
            inner.pending.push(json!({
                "sessionId": "",
                "update": { "sessionUpdate": "earlier_turns", "feed": self.id, "before": before },
            }));
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn set_start(&self, start: ChatStart) {
        let mut inner = self.inner.locked();
        inner.start = Some(start);
    }

    pub(crate) fn set_setup(&self, setup: &Value) {
        let mut inner = self.inner.locked();
        if let Some(start) = inner.start.as_mut() {
            start.setup = setup.clone();
        }
    }

    pub(crate) fn set_permission_mode(&self, mode: &str) {
        let mut inner = self.inner.locked();
        inner.permission_mode = mode.to_owned();
    }

    pub(crate) fn permission_mode(&self) -> String {
        self.inner.locked().permission_mode.clone()
    }

    /// What the agent last called this session, as the host's rail shows it.
    pub(crate) fn title(&self) -> Option<String> {
        self.inner.locked().title.clone()
    }

    /// What the chat said from index `from` on, or over its last `turns`.
    pub(crate) fn excerpt(&self, from: Option<u64>, turns: usize, max_bytes: usize) -> Excerpt {
        let (said, skipped, next) = self.inner.locked().replay.excerpt(from, turns, max_bytes);
        Excerpt {
            feed: self.id.clone(),
            said,
            skipped,
            next,
        }
    }

    /// Where the turn in progress, or the last one, starts in the history.
    pub(crate) fn turn_start(&self) -> Option<u64> {
        self.inner.locked().replay.turn_start()
    }

    /// What the agent wrote last in its latest turn.
    pub(crate) fn last_answer(&self) -> Option<String> {
        self.inner.locked().replay.last_answer()
    }

    /// Whether the last turn it holds has not ended.
    pub(crate) fn in_turn(&self) -> bool {
        self.inner.locked().replay.in_turn
    }

    pub(crate) fn reconnecting(&self) -> Option<u32> {
        self.inner.locked().replay.reconnecting
    }

    pub(crate) fn running_subagents(&self) -> u32 {
        self.inner.locked().replay.work.running_subagents() as u32
    }

    pub(crate) fn running_tasks(&self) -> u32 {
        self.inner.locked().replay.work.running_tasks() as u32
    }

    /// The agent's process went away, and every subagent and background task
    /// it had going went with it. The chat is told each one ended.
    pub(crate) fn agent_gone(&self) {
        let mut inner = self.inner.locked();
        for payload in inner.replay.work.endings() {
            let bytes = inner.replay.push(ChatEventKind::SessionUpdate, &payload);
            inner.pending.push(payload);
            inner.pending_bytes += bytes;
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn start(&self) -> Option<ChatStart> {
        self.inner.locked().start.clone()
    }

    pub(crate) fn forget_permission(&self, request_id: &str) {
        let mut inner = self.inner.locked();
        inner.replay.forget_permission(request_id);
        inner.recent.forget_permission(request_id);
    }

    /// The turns before `before` in this run of the chat, which a client
    /// shown only the chat's end scrolls back to.
    pub(crate) fn history(&self, feed: &str, before: u64, turns: usize) -> Result<Page, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        let inner = self.inner.locked();
        let mut page = inner.replay.page(before, turns)?;
        annotate_replay(&mut page.events);
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(kind: &str, message: Option<&str>, text: &str) -> Value {
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
        let mut recent = Recent::default();
        for (seq, request_id) in [(1, "a"), (2, "b")] {
            recent.push(Sent {
                seq,
                event: ChatEvent {
                    kind: ChatEventKind::PermissionRequest,
                    payload: json!({ "requestId": request_id }),
                },
                bytes: 10,
                sender: None,
            });
        }
        recent.forget_permission("a");
        let missed = recent.since(0, &Peer::Local).expect("still kept");
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].payload["requestId"], "b");
        assert_eq!(recent.bytes, 10);
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

    fn turn(replay: &mut Replay, n: usize) {
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

    fn asked(events: &[ChatEvent]) -> Vec<String> {
        events
            .iter()
            .filter(|event| event.kind == ChatEventKind::Prompt)
            .map(|event| {
                event.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    fn asks(range: std::ops::Range<usize>) -> Vec<String> {
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

        let tail = replay.tail(0);
        assert_eq!(tail.events[0].payload, commands());
        assert_eq!(tail.events[1].kind, ChatEventKind::Prompt);
        assert_eq!(asked(&tail.events), asks(78..80));
        assert_eq!(tail.older_before, Some(1 + 78 * 5));

        let mut earlier = Vec::new();
        let mut before = tail.older_before;
        while let Some(cursor) = before {
            let page = replay.page(cursor, 30).unwrap();
            assert!(asked(&page.events).len() <= 30);
            assert!(!page.events.is_empty());
            earlier.splice(0..0, page.events);
            before = page.older_before;
        }
        assert_eq!(earlier[0].payload, commands());
        assert_eq!(asked(&earlier), asks(0..78));
        assert!(earlier
            .iter()
            .any(|event| event.payload["update"]["content"]["text"] == "working"));
        assert_eq!(earlier.len(), 1 + 78 * 5);
    }

    fn update(session: &str, update: Value) -> Value {
        json!({ "sessionId": session, "update": update })
    }

    fn standing_kinds(tail: &Tail) -> Vec<(String, String)> {
        tail.events
            .iter()
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
                crate::acp::account::retrying("t:retry", "connection", attempt, "x"),
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

        let busy = crate::acp::account::retrying("t:retry", "service", 4, "Anthropic answered 529");
        replay.push(ChatEventKind::SessionUpdate, &update("s", busy));
        assert_eq!(replay.reconnecting, None);

        replay.push(ChatEventKind::SessionUpdate, &retry(3));
        replay.push(ChatEventKind::TurnCompleted, &json!({}));
        assert_eq!(replay.reconnecting, None);
    }

    #[test]
    fn a_phone_shown_the_end_still_hears_the_plan_and_the_work_still_going() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
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
        for n in 0..TAIL_TURNS {
            turn(&mut replay, n);
        }
        assert_eq!(
            standing_kinds(&replay.tail(0)),
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

    #[test]
    fn a_tail_carries_the_start_of_every_subagent_it_speaks_of() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let spawned = |session: &str, id: &str| {
            update(
                session,
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": id }),
            )
        };
        let ended = |id: &str| {
            update(
                "s",
                json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": id, "state": "completed" }),
            )
        };
        replay.push(ChatEventKind::SessionUpdate, &spawned("s", "corpus"));
        replay.push(ChatEventKind::SessionUpdate, &spawned("corpus", "fixtures"));
        replay.push(ChatEventKind::SessionUpdate, &spawned("s", "forgotten"));
        replay.push(ChatEventKind::SessionUpdate, &ended("forgotten"));
        for n in 0..TAIL_TURNS {
            turn(&mut replay, n);
        }
        replay.push(
            ChatEventKind::SessionUpdate,
            &update(
                "fixtures",
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Recorded" } }),
            ),
        );
        replay.push(ChatEventKind::SessionUpdate, &ended("corpus"));

        let starts: Vec<String> = replay
            .tail(0)
            .events
            .iter()
            .filter(|event| session_update_kind(&event.payload) == Some("subagent_spawned"))
            .map(|event| {
                event.payload["update"]["subagentSessionId"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(starts, ["corpus", "fixtures"]);
    }

    #[tokio::test]
    async fn subagents_outlive_their_turn_but_not_the_agent() {
        let feed = Feed::new("agent".into(), "default".into(), None);
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

    #[tokio::test]
    async fn a_long_history_loaded_at_the_start_reaches_clients_as_its_last_turns() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::server::Core::new(Default::default(), None).unwrap();
        let (client, mut frames) = ClientConn::inside(&core, Peer::Local);
        let feed = Feed::new("agent".into(), "default".into(), Some(dir.path()));
        feed.subscribe(&client);

        feed.hold();
        for n in 0..60 {
            let (asked, answer) = (format!("ask {n}"), format!("m{n}"));
            feed.emit(
                ChatEventKind::SessionUpdate,
                text("user_message_chunk", None, &asked),
            );
            feed.emit(
                ChatEventKind::SessionUpdate,
                text("agent_message_chunk", Some(&answer), "done"),
            );
        }
        let mut heard = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            heard.push(frame);
        }
        assert!(heard.is_empty(), "nothing goes out while the history loads");

        feed.let_out();
        let mut updates = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            let frame = crate::protocol::read_frame_sync(&mut &frame[..])
                .unwrap()
                .unwrap();
            let Ok(ServerMessage::Event {
                event: Event::Chat { event, .. },
            }) = serde_json::from_slice(&frame.payload)
            else {
                continue;
            };
            updates.extend(
                event.payload["updates"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        let asked: Vec<String> = updates
            .iter()
            .filter(|update| update["update"]["sessionUpdate"] == "user_message_chunk")
            .filter_map(|update| update["update"]["content"]["text"].as_str())
            .map(str::to_owned)
            .collect();
        let first = 60 - TAIL_TURNS;
        assert_eq!(asked, asks(first..60));
        assert_eq!(
            updates.last().unwrap()["update"],
            json!({ "sessionUpdate": "earlier_turns", "feed": feed.id, "before": first * 2 })
        );
    }

    #[tokio::test]
    async fn the_last_turns_go_to_a_phone_and_to_the_app_once_the_replay_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let feed = Feed::new("agent".into(), "default".into(), Some(dir.path()));
        feed.set_start(ChatStart {
            session_id: "s".into(),
            capabilities: json!({}),
            setup: json!({}),
        });
        let standing = || Standing {
            running: false,
            turned: true,
        };
        let phone = Peer::Device { id: "phone".into() };
        let mut inner = feed.inner.lock().unwrap();
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }

        let ChatAttachment::Live {
            replay,
            older_before,
            ..
        } = feed.attachment(&inner, &phone, standing(), None)
        else {
            panic!("a phone takes up a long chat");
        };
        let first = 60 - TAIL_TURNS;
        assert_eq!(asked(&replay), asks(first..60));
        assert_eq!(older_before, Some(first as u64 * 5));
        drop(inner);
        let page = feed.history(&feed.id, first as u64 * 5, 100).unwrap();
        assert_eq!(asked(&page.events), asks(0..first));
        assert_eq!(page.older_before, None);
        assert!(feed
            .history("an earlier run", first as u64 * 5, 100)
            .is_err());
        let mut inner = feed.inner.lock().unwrap();

        let history = History::open(dir.path(), "trimmed", MAX_HISTORY_BYTES);
        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, Some(history));
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let ChatAttachment::Live { older_before, .. } =
            feed.attachment(&inner, &Peer::Local, standing(), None)
        else {
            panic!("the app takes up a chat longer than the replay without restarting it");
        };
        assert_eq!(older_before, Some(58 * 5));
        let ChatAttachment::Live { older_before, .. } =
            feed.attachment(&inner, &phone, standing(), None)
        else {
            panic!("a phone takes up a chat longer than the replay");
        };
        assert_eq!(older_before, Some(58 * 5));

        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, None);
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        for peer in [&Peer::Local, &phone] {
            let ChatAttachment::Live {
                replay,
                older_before,
                ..
            } = feed.attachment(&inner, peer, standing(), None)
            else {
                panic!("a chat longer than the replay is taken up without its history");
            };
            assert_eq!(
                session_update_kind(&replay[0].payload),
                Some("earlier_turns_lost")
            );
            assert_eq!(asked(&replay), asks(58..60));
            assert_eq!(older_before, None);
        }
    }
}
