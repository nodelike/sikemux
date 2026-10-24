//! What a chat agent says, on its way to the clients that show it. Streamed
//! updates leave in one batch per frame. Everything since the session started
//! or loaded is kept, so a client that attaches later rebuilds the chat from
//! the same events the others watched arrive.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::protocol::{
    encode_control, fits, ChatAttachment, ChatContext, ChatMark, ChatStart, Event, RequestId,
    Response, ServerMessage,
};
use sikemux_process::clock::epoch_ms;
use sikemux_wire::chat::{ChatChange, ChatPage, QueuedMessage, SentContext, ToolDetail};

use super::super::connection::{ClientConn, ClientId};
use super::history::{History, HistoryState, Page, Reader, Recovered, MAX_HISTORY_BYTES};
use super::queue::{batch_len, Queued, MAX_QUEUED};
use super::states::{Subagent, Task};
use sikemux_fold::{ChatEvent, ChatEventKind, Place};
use sikemux_fold::{Fold, PageOf, PAGE_BYTES as ROW_BYTES};

/// One frame's worth of streamed updates travels as a single event. Each
/// notification on its own costs a script eval in the webview, and an adapter
/// sends one per token.
const FLUSH: Duration = Duration::from_millis(16);
/// How long the newest event waits for more text to join it before it is
/// written to the history, so a crash loses at most this much of the chat.
const SETTLE: Duration = Duration::from_secs(1);
pub(crate) const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_REPLAY_EVENTS: usize = 20_000;
/// Roughly what an event costs beyond its payload once it is on the wire.
const EVENT_OVERHEAD: usize = 48;
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

/// The rows a find reads: those the fold holds while the place it reads from is among them,
/// else pages of the chat's history.
struct FindRows<'a> {
    feed: &'a Feed,
    client: ClientId,
    number: u64,
}

impl super::find::Rows for FindRows<'_> {
    fn batch(
        &self,
        wanted: PageOf,
        budget: usize,
    ) -> Result<(Vec<sikemux_wire::chat::ChatMessage>, bool), String> {
        let held = {
            let mut inner = self.feed.inner.locked();
            let fold = self.feed.fold(&mut inner);
            let inside = fold.floor().is_some_and(|floor| match wanted {
                PageOf::Before(before) => before > floor,
                PageOf::After(after) => after >= floor,
                PageOf::From(from) => from >= floor,
            });
            inside.then(|| fold.held(wanted, budget))
        };
        if let Some((rows, all)) = held {
            let last = all && !matches!(wanted, PageOf::Before(_));
            return Ok((rows, last));
        }
        let page = self.feed.page_of(wanted, MAX_PAGE_TURNS, budget, None)?;
        let last = match wanted {
            PageOf::Before(_) => page.older_before.is_none(),
            PageOf::After(_) | PageOf::From(_) => page.newer_after.is_none(),
        };
        let mut rows = page.rows;
        if matches!(wanted, PageOf::Before(_)) {
            rows.reverse();
        }
        Ok((rows, last))
    }

    fn superseded(&self) -> bool {
        self.feed.finding.locked().get(&self.client) != Some(&self.number)
    }
}

/// Which rows of a chat a client asks for, by event index.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Anchor {
    Before(u64),
    After(u64),
    /// The row holding the event, and about half a page either side of it.
    Around(u64),
}

/// The events a page folds, chosen under the chat's lock. The part on disk is read once the
/// lock is let go; the newest, not yet on disk, are copied with the choice.
struct Planned {
    reader: Option<Reader>,
    lo: u64,
    hi: u64,
    held: Vec<(u64, ChatEvent)>,
    /// The chat holds more on the side the page reads away from.
    beyond: bool,
}

impl Planned {
    /// Where a client goes on from past these events, when the chat goes on past them.
    fn edge(&self, side: PageOf) -> Option<u64> {
        let edge = match side {
            PageOf::Before(_) => self.lo,
            PageOf::After(_) | PageOf::From(_) => self.hi.saturating_sub(1),
        };
        self.beyond.then_some(edge)
    }
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
    /// Every subagent's start and latest state, so a slice of the chat that
    /// speaks of one started before it can carry them too.
    spawns: Vec<Spawn>,
}

#[derive(Serialize, Deserialize)]
struct Spawn {
    subagent: String,
    /// The session that started it.
    parent: String,
    index: u64,
    event: ChatEvent,
    state: Option<(u64, ChatEvent)>,
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
    if !only_keys(notification, &["sessionId", "update", "at", "until"]) {
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

/// When the event happened, so the chat folded again later times its turns
/// and answers as they ran.
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
        self.note_subagent(kind, index, &event);
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

    /// Writes every event not yet on disk to the history and lets go of them in memory, for a
    /// chat that sleeps; answers how many bytes the history keeps.
    fn rest(&mut self) -> u64 {
        for entry in &mut self.entries {
            Self::write(&mut self.history, entry);
        }
        if self
            .history
            .as_ref()
            .is_some_and(|history| history.first().is_some())
        {
            self.entries.clear();
            self.bytes = 0;
        }
        self.history.as_ref().map_or(0, History::size)
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
        if let Some(at) = payload.get("at") {
            last.event.payload["until"] = at.clone();
        }
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
        if kind == ChatEventKind::SessionUpdate && self.join_text(payload) {
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
        match session_update_kind(&event.payload) {
            Some("subagent_spawned") => {
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
            Some("subagent_state_update") => {
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

    /// Every event the replay holds, with what still stands from before
    /// them and the starts of the subagents they speak of, for folding the
    /// chat again.
    pub(crate) fn fold_events(&self) -> Vec<(u64, ChatEvent)> {
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
            .map(|(index, event)| (index, event.clone()))
            .chain(
                self.entries
                    .iter()
                    .map(|entry| (entry.index, entry.event.clone())),
            )
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

    /// The events of the turns before `before`, from the history on disk, or
    /// from the replay when the chat keeps none.
    #[cfg(test)]
    pub(crate) fn page(&self, before: u64, turns: usize) -> Result<Page, String> {
        let planned = self.plan(PageOf::Before(before), turns.clamp(1, MAX_PAGE_TURNS))?;
        let mut events = match &planned.reader {
            Some(reader) => reader
                .range(planned.lo, planned.hi)
                .map_err(|error| error.to_string())?,
            None => Vec::new(),
        };
        events.extend(planned.held);
        Ok(self.with_spawns(Page {
            events,
            older_before: planned.beyond.then_some(planned.lo),
            first: planned.lo,
        }))
    }

    /// The events of the turns `side` asks for, as many as `turns` while their events stay
    /// under the page's size: whole turns, or from the nearest point inside a turn too big for
    /// a page of its own.
    fn plan(&self, side: PageOf, turns: usize) -> Result<Planned, String> {
        let reader = self
            .history
            .as_ref()
            .filter(|history| history.first().is_some())
            .map(History::reader)
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        let (first, on_disk) = reader.as_ref().map_or_else(
            || {
                let first = self
                    .entries
                    .front()
                    .map_or(self.next_index, |entry| entry.index);
                (first, first)
            },
            Reader::span,
        );
        let mut points: Vec<(u64, u64, bool)> = reader.iter().flat_map(Reader::points).collect();
        let mut end = reader.as_ref().map_or(0, Reader::bytes);
        for entry in self.entries.iter().filter(|entry| entry.index >= on_disk) {
            if entry.point || entry.starts_turn {
                points.push((entry.index, end, entry.starts_turn));
            }
            end += entry.bytes as u64;
        }
        let next = self.next_index;
        let budget = PAGE_BYTES;
        let forward = |from: u64| {
            let start = points
                .iter()
                .rev()
                .find(|(index, _, starts)| *index <= from && *starts)
                .or_else(|| points.iter().rev().find(|(index, _, _)| *index <= from))
                .map_or((first, 0), |&(index, at, _)| (index, at));
            let mut hi = next;
            let mut counted = 1;
            for &(index, at, starts) in points.iter().filter(|(index, _, _)| *index > start.0) {
                if at - start.1 > budget || (starts && counted >= turns) {
                    hi = index;
                    break;
                }
                counted += usize::from(starts);
            }
            (start.0, hi, hi < next)
        };
        let (lo, hi, beyond) = match side {
            PageOf::Before(before) => {
                let before = before.min(next);
                let after = |turn: bool| {
                    points
                        .iter()
                        .find(|(index, _, starts)| *index >= before && (*starts || !turn))
                        .map(|&(index, at, _)| (index, at))
                };
                let (hi, to) = match (after(true), after(false)) {
                    (Some(turn), Some(point)) if turn.1 - point.1 <= budget => turn,
                    (_, Some(point)) => point,
                    _ => (next, end),
                };
                let earlier: Vec<&(u64, u64, bool)> = points
                    .iter()
                    .rev()
                    .filter(|(index, _, _)| *index < before)
                    .collect();
                let (mut turn, mut piece, mut counted) = (None, None, 0);
                for &&(index, at, starts) in &earlier {
                    if to - at > budget {
                        break;
                    }
                    piece = Some(index);
                    if starts {
                        turn = Some(index);
                        counted += 1;
                        if counted >= turns {
                            break;
                        }
                    }
                }
                let lo = turn
                    .or(piece)
                    .or_else(|| earlier.first().map(|point| point.0))
                    .unwrap_or(first);
                (lo, hi, lo > first)
            }
            PageOf::After(after) => forward(after.saturating_add(1)),
            PageOf::From(from) => forward(from),
        };
        let held = self
            .entries
            .iter()
            .filter(|entry| entry.index >= on_disk && (lo..hi).contains(&entry.index))
            .filter(|entry| entry.event.kind != ChatEventKind::PermissionRequest)
            .map(|entry| (entry.index, entry.event.clone()))
            .collect();
        Ok(Planned {
            reader,
            lo,
            hi,
            held,
            beyond,
        })
    }

    /// `page` with the starts of the subagents it speaks of that began before it.
    fn with_spawns(&self, mut page: Page) -> Page {
        let spawns: Vec<(u64, ChatEvent)> = self
            .spawns_before(page.events.iter().map(|(_, event)| event), page.first)
            .into_iter()
            .map(|(index, event)| (index, event.clone()))
            .collect();
        page.events.splice(0..0, spawns);
        page
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

/// What a client attaching now is told about the chat besides its rows.
pub(crate) struct Standing {
    pub running: bool,
    pub turned: bool,
}

/// A client hearing the chat, and the last change it was sent when it fell
/// behind: a slow client is sent only how the chat stands once it catches up.
struct Subscriber {
    client: Arc<ClientConn>,
    behind: Option<u64>,
}

/// Past this much waiting to go out to a client, it is not sent each change
/// but caught up once it has read what it has.
const SLOW_BYTES: usize = 512 * 1024;
/// How often a client that fell behind is looked at again.
const CATCH_UP: Duration = Duration::from_millis(50);

struct Inner {
    subscribers: HashMap<ClientId, Subscriber>,
    seq: u64,
    replay: Replay,
    /// The chat folded into rows, while a client watches it or a turn runs.
    fold: Option<Fold>,
    /// The agent always offers an effort picker.
    offers_effort: bool,
    flush_scheduled: bool,
    settle_scheduled: bool,
    catch_up_scheduled: bool,
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
    /// The session ended, so nobody new is let in to wait for events that
    /// will not come.
    closed: bool,
    /// Events are kept and folded but not sent while a session loads.
    holding: bool,
    /// What the person sent while the agent was at work, oldest first.
    queued: Vec<Queued>,
    /// The folder the chat's agent works in, and where its edits sit in their files,
    /// for each fold made for it.
    work: Option<(PathBuf, Place)>,
    /// A new chat whose agent waits for the person: clients take it up before it starts.
    waits_for_person: bool,
}

/// A feed as plain data, for a core that takes the chat over. Clients that
/// held a mark of it catch up from where they were.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeedState {
    id: String,
    seq: u64,
    replay: Replay,
    history: Option<HistoryState>,
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
}

pub(crate) struct Feed {
    me: std::sync::Weak<Feed>,
    agent_id: String,
    /// Names this run of the chat's agent in the marks clients hold.
    id: String,
    runtime: tokio::runtime::Handle,
    inner: Mutex<Inner>,
    /// Each client's latest find in this chat, by number, so a newer one stops the one before.
    finding: Mutex<HashMap<ClientId, u64>>,
}

fn queued_view(queued: &[Queued]) -> Vec<QueuedMessage> {
    queued
        .iter()
        .map(|message| QueuedMessage {
            id: message.id.clone(),
            text: message.text.clone(),
            paths: message.paths.clone(),
            context: message
                .context
                .iter()
                .map(|item| SentContext {
                    uri: item.uri.clone(),
                    title: item.title.clone(),
                })
                .collect(),
        })
        .collect()
}

/// A fold for a chat, showing an effort picker from the start when its agent
/// always offers one.
fn new_fold(permission_mode: &str, offers_effort: bool, work: Option<&(PathBuf, Place)>) -> Fold {
    let mut fold = Fold::new(permission_mode);
    if offers_effort {
        fold.offer_effort();
    }
    if let Some((folder, place)) = work {
        fold.work_in(folder.clone(), place.clone());
    }
    fold
}

/// The largest file an edit is looked for in; a bigger one leaves the edit's lines unnumbered.
const MAX_PLACED_BYTES: u64 = 2 * 1024 * 1024;

/// The line `text` starts on in the file at `path`, when it is there exactly once.
fn line_of(path: &std::path::Path, text: &str) -> Option<u64> {
    if text.is_empty() || std::fs::metadata(path).ok()?.len() > MAX_PLACED_BYTES {
        return None;
    }
    let held = std::fs::read_to_string(path).ok()?;
    let at = held.find(text)?;
    if held[at + text.len()..].contains(text) {
        return None;
    }
    Some(held[..at].matches('\n').count() as u64 + 1)
}

impl Feed {
    /// With `history`, everything the chat says is also kept in a file there,
    /// for clients to page back through.
    pub(crate) fn new(
        agent_id: String,
        permission_mode: String,
        history: Option<&std::path::Path>,
    ) -> Arc<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        let history = history.map(|dir| History::open(dir, &id, MAX_HISTORY_BYTES));
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            agent_id,
            id,
            runtime: tokio::runtime::Handle::current(),
            finding: Mutex::default(),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                seq: 0,
                replay: Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, history),
                fold: Some(Fold::new(&permission_mode)),
                offers_effort: false,
                flush_scheduled: false,
                settle_scheduled: false,
                catch_up_scheduled: false,
                start: None,
                permission_mode,
                title: None,
                closed: false,
                holding: false,
                queued: Vec::new(),
                work: None,
                waits_for_person: false,
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
        let mut inner = self.inner.locked();
        inner.replay = replay;
        inner.title = title;
        inner.fold = None;
    }

    /// Takes up a feed an earlier core let go of.
    pub(crate) fn restored(agent_id: String, state: FeedState) -> Arc<Self> {
        let mut replay = state.replay;
        replay.history = state.history.map(History::reopen);
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            agent_id,
            id: state.id,
            runtime: tokio::runtime::Handle::current(),
            finding: Mutex::default(),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                seq: state.seq,
                replay,
                fold: None,
                offers_effort: false,
                flush_scheduled: false,
                settle_scheduled: false,
                catch_up_scheduled: false,
                start: state.start,
                permission_mode: state.permission_mode,
                title: state.title,
                closed: false,
                holding: false,
                queued: Vec::new(),
                work: None,
                waits_for_person: false,
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
            .map(|(_, subscriber)| subscriber.client)
            .collect();
        let history = inner.replay.history.take().map(History::release);
        let replay = std::mem::replace(
            &mut inner.replay,
            Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None),
        );
        let state = FeedState {
            id: self.id.clone(),
            seq: inner.seq,
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

    fn mark(&self, inner: &Inner) -> ChatMark {
        ChatMark {
            feed: self.id.clone(),
            seq: inner.seq,
        }
    }

    fn frame(&self, seq: u64, changes: &[ChatChange]) -> Option<Arc<[u8]>> {
        let frame = encode_control(&ServerMessage::Event {
            event: Event::Chat {
                agent_id: self.agent_id.clone(),
                seq,
                changes: serde_json::to_value(changes).ok()?,
            },
        })
        .ok()?;
        if !fits(&frame) {
            eprintln!(
                "sikemux core: a chat change of {} bytes is too large to send",
                frame.len()
            );
            return None;
        }
        Some(frame.into())
    }

    /// The fold, built again from the chat's events when it was let go.
    fn fold<'a>(&self, inner: &'a mut Inner) -> &'a mut Fold {
        if inner.fold.is_none() {
            let mut fold = new_fold(
                &inner.permission_mode,
                inner.offers_effort,
                inner.work.as_ref(),
            );
            if let Some(start) = &inner.start {
                fold.apply(
                    inner.replay.next_index,
                    ChatEventKind::Ready,
                    &json!(start),
                    0,
                );
            }
            for (index, event) in inner.replay.fold_events() {
                let at = event
                    .payload
                    .get("at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                fold.apply(index, event.kind, &event.payload, at);
            }
            fold.set_queued(queued_view(&inner.queued));
            if inner.replay.is_trimmed() && !inner.replay.keeps_history() {
                fold.set_earlier_lost();
            }
            fold.rebuilt(inner.seq);
            inner.fold = Some(fold);
        }
        inner.fold.as_mut().expect("just built")
    }

    /// Whether the chat's events hold turns before the fold's first row.
    fn holds_older(inner: &Inner) -> bool {
        let Some(floor) = inner.fold.as_ref().and_then(Fold::floor) else {
            return false;
        };
        let oldest = inner
            .replay
            .history
            .as_ref()
            .and_then(History::first)
            .or_else(|| inner.replay.entries.front().map(|entry| entry.index));
        oldest.is_some_and(|oldest| oldest < floor)
    }

    /// Sends every client what changed since the last flush. A client behind
    /// on reading is left to catch up once it has.
    fn flush_locked(&self, inner: &mut Inner) {
        let Some(fold) = inner.fold.as_mut() else {
            return;
        };
        if !fold.is_dirty() {
            return;
        }
        let seq = inner.seq + 1;
        let changes = fold.flush(seq);
        if inner.holding || changes.is_empty() {
            return;
        }
        inner.seq = seq;
        let frame = self.frame(seq, &changes);
        let mut behind = false;
        inner.subscribers.retain(|_, subscriber| {
            if subscriber.behind.is_none() && subscriber.client.backlog() <= SLOW_BYTES {
                return frame
                    .as_ref()
                    .is_none_or(|frame| subscriber.client.send(frame.clone()));
            }
            subscriber.behind.get_or_insert(seq - 1);
            behind = true;
            !subscriber.client.is_closed()
        });
        if behind {
            self.catch_up_later(inner);
        }
    }

    fn catch_up_later(&self, inner: &mut Inner) {
        if inner.catch_up_scheduled {
            return;
        }
        inner.catch_up_scheduled = true;
        let feed = self.me.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(CATCH_UP).await;
            if let Some(feed) = feed.upgrade() {
                feed.catch_up();
            }
        });
    }

    /// Sends each client that fell behind how the chat stands, once it has
    /// read what was waiting for it.
    fn catch_up(&self) {
        let mut inner = self.inner.locked();
        inner.catch_up_scheduled = false;
        self.flush_locked(&mut inner);
        let seq = inner.seq;
        let mut still = false;
        let ready: Vec<(ClientId, u64)> = inner
            .subscribers
            .iter()
            .filter_map(|(id, subscriber)| {
                let since = subscriber.behind?;
                if subscriber.client.backlog() > SLOW_BYTES / 2 {
                    still = true;
                    return None;
                }
                Some((*id, since))
            })
            .collect();
        for (id, since) in ready {
            let changes = {
                let older = Self::holds_older(&inner);
                let fold = self.fold(&mut inner);
                fold.catch_up(since).unwrap_or_else(|| {
                    vec![ChatChange::View {
                        view: Box::new(fold.view(older)),
                    }]
                })
            };
            let frame = self.frame(seq, &changes);
            if let Some(subscriber) = inner.subscribers.get_mut(&id) {
                subscriber.behind = None;
                if let Some(frame) = frame {
                    subscriber.client.send(frame);
                }
            }
        }
        if still {
            self.catch_up_later(&mut inner);
        }
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
        self.let_go(&mut inner);
    }

    fn flush_later(self: &Arc<Self>, inner: &mut Inner) {
        if inner.flush_scheduled {
            return;
        }
        inner.flush_scheduled = true;
        let feed = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(FLUSH).await;
            feed.flush();
        });
    }

    /// Lets go of the fold of a chat nobody watches and no turn runs in. It is
    /// built again from the chat's events when a client comes.
    fn let_go(&self, inner: &mut Inner) {
        if inner.subscribers.is_empty()
            && !inner.replay.in_turn
            && !inner.holding
            && inner.fold.as_ref().is_some_and(|fold| !fold.is_dirty())
        {
            inner.fold = None;
        }
    }

    fn fold_in(&self, inner: &mut Inner, index: u64, kind: ChatEventKind, payload: &Value) {
        let at = payload
            .get("at")
            .and_then(Value::as_u64)
            .unwrap_or_else(epoch_ms);
        if let Some(fold) = inner.fold.as_mut() {
            fold.apply(index, kind, payload, at);
        }
    }

    /// Streamed updates go out once a frame; anything else goes out at once,
    /// after what streamed before it.
    pub(crate) fn emit(self: &Arc<Self>, kind: ChatEventKind, mut payload: Value) {
        let mut inner = self.inner.locked();
        if is_kept(kind) {
            stamp(&mut payload);
        }
        let index = inner.replay.push(kind, &payload);
        self.settle_later(&mut inner);
        if let Some(title) = session_title(&payload) {
            inner.title = Some(title);
        }
        self.fold_in(&mut inner, index, kind, &payload);
        if kind == ChatEventKind::SessionUpdate {
            self.flush_later(&mut inner);
            return;
        }
        self.flush_locked(&mut inner);
        self.let_go(&mut inner);
    }

    /// What a person sent the agent, as every client and every later reader
    /// of the chat sees it.
    pub(crate) fn prompted(
        self: &Arc<Self>,
        message_id: Option<&str>,
        text: &str,
        paths: &[String],
        context: &[ChatContext],
    ) {
        let mut payload = json!({ "text": text, "paths": paths });
        if let Some(message_id) = message_id {
            payload["messageId"] = json!(message_id);
        }
        if !context.is_empty() {
            payload["context"] = json!(context
                .iter()
                .map(|item| json!({ "uri": item.uri, "title": item.title }))
                .collect::<Vec<_>>());
        }
        self.emit(ChatEventKind::Prompt, payload);
    }

    /// The person took the chat back to before their message `message_id`:
    /// it goes, and everything after it.
    pub(crate) fn rewound(&self, message_id: &str) {
        let mut inner = self.inner.locked();
        inner.replay.rewind(message_id);
        if let Some(fold) = inner.fold.as_mut() {
            fold.rewind(message_id);
        }
        self.flush_locked(&mut inner);
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
        let queued = queued_view(&inner.queued);
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_queued(queued);
        }
        self.flush_locked(inner);
    }

    pub(crate) fn subscribe(&self, client: &Arc<ClientConn>) {
        let mut inner = self.inner.locked();
        self.fold(&mut inner);
        inner.subscribers.insert(
            client.id,
            Subscriber {
                client: client.clone(),
                behind: None,
            },
        );
    }

    pub(crate) fn has_subscriber(&self, client: ClientId) -> bool {
        self.inner.locked().subscribers.contains_key(&client)
    }

    pub(crate) fn unsubscribe(&self, client: ClientId) {
        let mut inner = self.inner.locked();
        inner.subscribers.remove(&client);
        self.let_go(&mut inner);
    }

    /// Answers the client with the chat as it stands, or what changed since
    /// `since`, and adds it to the listeners in the same step, so it hears
    /// every later change exactly once.
    pub(crate) fn attach(
        &self,
        client: &Arc<ClientConn>,
        request_id: RequestId,
        standing: Standing,
        since: Option<ChatMark>,
    ) {
        let mut inner = self.inner.locked();
        if inner.closed || (inner.start.is_none() && !inner.waits_for_person) {
            client.respond(
                request_id,
                Ok(Response::ChatAttached {
                    attachment: ChatAttachment::Missing,
                }),
            );
            return;
        }
        let fold = self.fold(&mut inner);
        fold.set_running(standing.running);
        fold.set_turned(standing.turned);
        self.flush_locked(&mut inner);
        let resumed = since
            .filter(|since| since.feed == self.id && since.seq <= inner.seq)
            .and_then(|since| self.fold(&mut inner).catch_up(since.seq));
        let mark = self.mark(&inner);
        let attachment = match resumed {
            Some(changes) => ChatAttachment::Resumed {
                changes: serde_json::to_value(changes).unwrap_or_default(),
                mark,
            },
            None => {
                let older = Self::holds_older(&inner);
                let view = self.fold(&mut inner).view(older);
                ChatAttachment::Live {
                    view: serde_json::to_value(view).unwrap_or_default(),
                    mark,
                }
            }
        };
        client.respond(request_id, Ok(Response::ChatAttached { attachment }));
        inner.subscribers.insert(
            client.id,
            Subscriber {
                client: client.clone(),
                behind: None,
            },
        );
    }

    pub(crate) fn close(&self) {
        let mut inner = self.inner.locked();
        inner.closed = true;
    }

    /// Lets go of everything the chat holds in memory once it sleeps, writing out what is not
    /// yet on disk, so clients can still attach, page and find through its history, and
    /// hear the chat when it wakes in this feed. Answers how many bytes of history it keeps.
    pub(crate) fn rest(&self) -> u64 {
        let mut inner = self.inner.locked();
        inner.closed = false;
        inner.fold = None;
        inner.subscribers.clear();
        inner.replay.rest()
    }

    /// Keeps what the agent says from the clients until [`Feed::let_out`]. A
    /// resumed session replays its whole saved history while it starts, more
    /// than a client could be sent a change at a time.
    pub(crate) fn hold(&self) {
        let mut inner = self.inner.locked();
        self.flush_locked(&mut inner);
        inner.holding = true;
    }

    /// Sends every client the chat as it stands once the history is in.
    pub(crate) fn let_out(&self) {
        let mut inner = self.inner.locked();
        if !inner.holding {
            return;
        }
        self.flush_locked(&mut inner);
        inner.holding = false;
        if inner.replay.is_trimmed() && !inner.replay.keeps_history() {
            self.fold(&mut inner).set_earlier_lost();
        }
        let seq = inner.seq + 1;
        self.fold(&mut inner).flush(seq);
        inner.seq = seq;
        let older = Self::holds_older(&inner);
        let view = self.fold(&mut inner).view(older);
        let frame = self.frame(
            seq,
            &[ChatChange::View {
                view: Box::new(view),
            }],
        );
        if let Some(frame) = frame {
            inner
                .subscribers
                .retain(|_, subscriber| subscriber.client.send(frame.clone()));
        }
    }

    /// Shows an effort picker from the start, as agents that offer effort on
    /// every model do.
    /// Makes the paths the chat's agent names whole against `cwd`, and numbers the lines
    /// of its edits from where they sit in their files.
    pub(crate) fn work_in(&self, cwd: PathBuf) {
        let folder = cwd.clone();
        let place: Place =
            Arc::new(move |path: &str, text: &str| line_of(&folder.join(path), text));
        let mut inner = self.inner.locked();
        if let Some(fold) = inner.fold.as_mut() {
            fold.work_in(cwd.clone(), place.clone());
        }
        inner.work = Some((cwd, place));
    }

    pub(crate) fn offer_effort(&self) {
        let mut inner = self.inner.locked();
        inner.offers_effort = true;
        if let Some(fold) = inner.fold.as_mut() {
            fold.offer_effort();
        }
    }

    /// Lets clients take up a new chat whose agent has not started, as it waits for the person.
    pub(crate) fn wait_for_person(&self) {
        self.inner.locked().waits_for_person = true;
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
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_setup(setup);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn set_permission_mode(&self, mode: &str) {
        let mut inner = self.inner.locked();
        inner.permission_mode = mode.to_owned();
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_permission_mode(mode);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn set_changing_mode(&self, changing: bool) {
        let mut inner = self.inner.locked();
        if let Some(fold) = inner.fold.as_mut() {
            fold.set_changing_mode(changing);
        }
        self.flush_locked(&mut inner);
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
        for mut payload in inner.replay.work.endings() {
            stamp(&mut payload);
            let index = inner.replay.push(ChatEventKind::SessionUpdate, &payload);
            self.fold_in(&mut inner, index, ChatEventKind::SessionUpdate, &payload);
        }
        self.flush_locked(&mut inner);
    }

    pub(crate) fn start(&self) -> Option<ChatStart> {
        self.inner.locked().start.clone()
    }

    pub(crate) fn forget_permission(&self, request_id: &str) {
        let mut inner = self.inner.locked();
        inner.replay.forget_permission(request_id);
        if let Some(fold) = inner.fold.as_mut() {
            fold.forget_permission(request_id);
        }
        self.flush_locked(&mut inner);
    }

    /// The rows before `before` in this run of the chat, which a client
    /// shown only the chat's end scrolls back to.
    pub(crate) fn history(
        &self,
        feed: &str,
        before: u64,
        turns: usize,
    ) -> Result<ChatPage, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        self.page(feed, Anchor::Before(before), turns, None)
    }

    /// The rows `anchor` asks for in this run of the chat, `turns` of them at most: those
    /// before or after an event, or around the row holding one.
    /// With `session`, the rows are that subagent's rather than the chat's own.
    pub(crate) fn page(
        &self,
        feed: &str,
        anchor: Anchor,
        turns: usize,
        session: Option<&str>,
    ) -> Result<ChatPage, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        let turns = turns.clamp(1, MAX_PAGE_TURNS);
        match anchor {
            Anchor::Before(before) => {
                self.page_of(PageOf::Before(before), turns, ROW_BYTES, session)
            }
            Anchor::After(after) => self.page_of(PageOf::After(after), turns, ROW_BYTES, session),
            Anchor::Around(event) => {
                let half = (turns / 2).max(1);
                let before = PageOf::Before(event.saturating_add(1));
                let mut page = self.page_of(before, half, ROW_BYTES / 2, session)?;
                let newer = self.page_of(PageOf::After(event), half, ROW_BYTES / 2, session)?;
                let held: HashSet<String> = page.rows.iter().map(|row| row.id.clone()).collect();
                page.rows
                    .extend(newer.rows.into_iter().filter(|row| !held.contains(&row.id)));
                page.subagents.extend(newer.subagents);
                page.newer_after = newer.newer_after;
                Ok(page)
            }
        }
    }

    /// One step of `client`'s find through this run of the chat.
    pub(crate) fn find(
        &self,
        client: ClientId,
        params: &super::find::FindParams,
    ) -> Result<super::find::FindAnswer, String> {
        if params.feed != self.id {
            return Err("the chat started again; open it again to find in it".into());
        }
        let number = {
            let mut finding = self.finding.locked();
            let number = finding.get(&client).map_or(0, |number| number + 1);
            finding.insert(client, number);
            number
        };
        let rows = FindRows {
            feed: self,
            client,
            number,
        };
        let answer = super::find::step(&rows, params);
        let mut finding = self.finding.locked();
        if finding.get(&client) == Some(&number) {
            finding.remove(&client);
        }
        answer
    }

    fn page_of(
        &self,
        side: PageOf,
        turns: usize,
        budget: usize,
        session: Option<&str>,
    ) -> Result<ChatPage, String> {
        for _ in 0..2 {
            let planned = self.inner.locked().replay.plan(side, turns)?;
            let mut events = match &planned.reader {
                Some(reader) => reader
                    .range(planned.lo, planned.hi)
                    .map_err(|error| error.to_string())?,
                None => Vec::new(),
            };
            if planned
                .reader
                .as_ref()
                .is_some_and(|reader| !reader.still_current())
            {
                continue;
            }
            let edge = planned.edge(side);
            events.extend(planned.held);
            let mut inner = self.inner.locked();
            let page = inner.replay.with_spawns(Page {
                events,
                older_before: None,
                first: planned.lo,
            });
            return Ok(self
                .fold(&mut inner)
                .page_of(&page.events, side, budget, edge, session));
        }
        Err("that part of the chat changed; ask again".into())
    }

    /// A tool call's change and output, line by line, for a client that opened its row.
    pub(crate) fn tool_detail(&self, tool_call_id: &str) -> Option<ToolDetail> {
        let mut inner = self.inner.locked();
        if let Some(detail) = self.fold(&mut inner).tool_detail(tool_call_id) {
            return Some(detail);
        }
        let inner = &*inner;
        inner.replay.tool_detail(tool_call_id, inner.work.as_ref())
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
        let mut fold = Fold::new("default");
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
        fold.flush(1);
        fold.forget_permission("a");
        fold.flush(2);
        let missed = fold.catch_up(0).expect("still holds what it missed");
        let ChatChange::Head { set } = &missed[0] else {
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

        let held = replay.fold_events();
        assert_eq!(held[0].1.payload, commands());
        assert_eq!(asked(held.iter().map(|(_, event)| event)), asks(78..80));

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

    fn update(session: &str, update: Value) -> Value {
        json!({ "sessionId": session, "update": update })
    }

    fn standing_kinds(events: &[(u64, ChatEvent)]) -> Vec<(String, String)> {
        events
            .iter()
            .map(|(_, event)| event)
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

    #[test]
    fn a_chat_folded_from_its_end_carries_the_start_of_every_subagent_it_speaks_of() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 50 * 5 + 2, None);
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
        for n in 0..50 {
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
            .fold_events()
            .iter()
            .map(|(_, event)| event)
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
        let (client, mut frames) = ClientConn::inside(&core, crate::server::access::Peer::Local);
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
        let mut views = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            let frame = crate::protocol::read_frame_sync(&mut &frame[..])
                .unwrap()
                .unwrap();
            let Ok(ServerMessage::Event {
                event: Event::Chat { changes, .. },
            }) = serde_json::from_slice(&frame.payload)
            else {
                continue;
            };
            let changes: Vec<ChatChange> = serde_json::from_value(changes).unwrap();
            views.extend(changes.into_iter().filter_map(|change| match change {
                ChatChange::View { view } => Some(view),
                _ => None,
            }));
        }
        let [view] = &views[..] else {
            panic!(
                "the history goes out as one view, not {} of them",
                views.len()
            );
        };
        let first = 60 - sikemux_fold::TAIL_TURNS;
        assert_eq!(prompts(&view.rows), asks(first..60));
        assert_eq!(view.older_before, Some(first as u64 * 2));
    }

    fn prompts(rows: &[sikemux_wire::chat::ChatMessage]) -> Vec<String> {
        rows.iter()
            .filter(|row| row.role == sikemux_wire::chat::ChatRole::User)
            .flat_map(|row| row.parts.iter())
            .filter_map(|part| match part {
                sikemux_wire::chat::ChatPart::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_long_chat_opens_at_its_last_turns_and_pages_back() {
        let dir = tempfile::tempdir().unwrap();
        let feed = Feed::new("agent".into(), "default".into(), Some(dir.path()));
        feed.set_start(ChatStart {
            session_id: "s".into(),
            capabilities: json!({}),
            setup: json!({}),
        });
        let view = |inner: &mut Inner| {
            inner.fold = None;
            feed.fold(inner);
            let older = Feed::holds_older(inner);
            feed.fold(inner).view(older)
        };
        let mut inner = feed.inner.lock().unwrap();
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let opened = view(&mut inner);
        let first = 60 - sikemux_fold::TAIL_TURNS;
        assert_eq!(prompts(&opened.rows), asks(first..60));
        assert_eq!(opened.older_before, Some(first as u64 * 5));
        drop(inner);
        let page = feed.history(&feed.id, first as u64 * 5, 100).unwrap();
        assert_eq!(prompts(&page.rows), asks(0..first));
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
        let opened = view(&mut inner);
        assert_eq!(prompts(&opened.rows), asks(58..60));
        assert_eq!(opened.older_before, Some(58 * 5));
        assert!(!opened.head.earlier_lost);

        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, None);
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        let opened = view(&mut inner);
        assert!(opened.head.earlier_lost);
        assert_eq!(prompts(&opened.rows), asks(58..60));
    }
}
