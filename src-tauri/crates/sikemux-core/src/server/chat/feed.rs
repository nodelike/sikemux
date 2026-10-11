//! What a chat agent says, on its way to the clients that show it. Streamed
//! updates leave in one batch per frame. Everything since the session started
//! or loaded is kept, so a client that attaches later rebuilds the chat from
//! the same events the others watched arrive.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{
    encode_control, fits, ChatAttachment, ChatEvent, ChatEventKind, ChatMark, ChatStart, Event,
    RequestId, Response, ServerMessage,
};

use super::super::access::Peer;
use super::super::connection::{ClientConn, ClientId};
use super::history::{History, Page, MAX_HISTORY_BYTES};

/// One frame's worth of streamed updates travels as a single event. Each
/// notification on its own costs a script eval in the webview, and an adapter
/// sends one per token.
const FLUSH: Duration = Duration::from_millis(16);
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

/// A phone opening a chat is sent this much of its end, and pages back
/// through the rest.
const TAIL_TURNS: usize = 50;
const TAIL_BYTES: usize = 1024 * 1024;
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
#[derive(PartialEq, Eq)]
enum Holds {
    Update(String),
    TaskSpawned(String),
    TaskLatest(String),
    Subagent(String),
}

struct Entry {
    event: ChatEvent,
    bytes: usize,
    index: u64,
    starts_turn: bool,
    /// A page of history can start here.
    point: bool,
    on_disk: bool,
}

/// The newest of a chat's events, for a phone, and where the history before
/// them ends.
pub(crate) struct Tail {
    pub events: Vec<ChatEvent>,
    pub older_before: Option<u64>,
}

/// The ordered events a late client replays. Text streamed into one message
/// is kept as one event, which the chat would have joined up anyway. Each is
/// written to the history on disk once nothing more joins it.
pub(crate) struct Replay {
    entries: VecDeque<Entry>,
    bytes: usize,
    max_bytes: usize,
    max_events: usize,
    trimmed: bool,
    history: Option<History>,
    next_index: u64,
    in_turn: bool,
    last_from_user: bool,
    since_point: usize,
    /// What a phone shown only the chat's end still needs from before it:
    /// the last of each standing update, and the background tasks and
    /// subagents still going.
    standing: Vec<(Holds, u64, ChatEvent)>,
    /// Every subagent's session, whose own updates never stand for the chat.
    subagent_sessions: HashSet<String>,
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

fn session_title(payload: &Value) -> Option<&str> {
    let update = payload.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "session_info_update" {
        return None;
    }
    update
        .get("title")?
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
}

/// Subagents outlive the turn that started them, but not one that was cancelled.
/// The agent says so itself; this covers one that does not, as clients do.
fn cancels_subagents(kind: ChatEventKind, payload: &Value) -> bool {
    kind == ChatEventKind::TurnCompleted
        && payload.get("stopReason").and_then(Value::as_str) == Some("cancelled")
}

/// Keeps `running` to the subagents the parent session has going. A subagent's
/// own updates arrive under its session id and never change the count.
fn track_subagent(running: &mut HashSet<String>, payload: &Value) {
    let in_subagent = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .is_some_and(|session| running.contains(session));
    let Some(update) = payload.get("update") else {
        return;
    };
    let Some(id) = update.get("subagentSessionId").and_then(Value::as_str) else {
        return;
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("subagent_spawned") if !in_subagent => {
            running.insert(id.to_owned());
        }
        Some("subagent_state_update")
            if update.get("state").and_then(Value::as_str) != Some("running") =>
        {
            running.remove(id);
        }
        _ => {}
    }
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
            .or_insert_with(|| json!(crate::server::remote::unix_ms()));
    }
}

fn asks(event: &ChatEvent, request_id: &str) -> bool {
    event.kind == ChatEventKind::PermissionRequest
        && event.payload.get("requestId").and_then(Value::as_str) == Some(request_id)
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
            last_from_user: false,
            since_point: 0,
            standing: Vec::new(),
            subagent_sessions: HashSet::new(),
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
        match kind {
            ChatEventKind::TurnStarted => self.in_turn = true,
            ChatEventKind::TurnCompleted | ChatEventKind::Error => self.in_turn = false,
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

    fn hold(&mut self, holds: Holds, index: u64, event: &ChatEvent) {
        self.standing.retain(|(held, ..)| *held != holds);
        self.standing.push((holds, index, event.clone()));
    }

    fn forget_subagents(&mut self) {
        self.standing
            .retain(|(held, ..)| !matches!(held, Holds::Subagent(_)));
    }

    fn note_standing(&mut self, kind: ChatEventKind, index: u64, event: &ChatEvent) {
        if cancels_subagents(kind, &event.payload) {
            self.forget_subagents();
            return;
        }
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
        let in_subagent = event
            .payload
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| self.subagent_sessions.contains(session));
        if update_kind == "subagent_spawned" {
            if let Some(id) = text("subagentSessionId") {
                self.subagent_sessions.insert(id);
            }
        }
        if in_subagent {
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
            "subagent_state_update" if text("state").as_deref() != Some("running") => {
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
                let ended = matches!(
                    text("state").as_deref(),
                    Some("completed" | "failed" | "stopped")
                );
                if ended {
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

    /// The last turns, as many as fit, and whatever set how the chat stands
    /// before them. A chat that has gone a whole replay without a turn
    /// starting is cut where a page of its history can start.
    pub(crate) fn tail(&self) -> Tail {
        let mut chosen = None;
        let mut piece = None;
        let mut turns = 0;
        let mut bytes = 0;
        for (position, entry) in self.entries.iter().enumerate().rev() {
            bytes += entry.bytes;
            if bytes > TAIL_BYTES && chosen.is_some() {
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
        let start = cut.unwrap_or(0);
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
            .filter(|(_, index, _)| *index < first)
            .map(|(_, index, event)| (*index, event))
            .collect();
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

    /// Forgets the person's message `message_id` and everything after it.
    /// A message too old to still be kept here takes the whole replay with it.
    pub(crate) fn rewind(&mut self, message_id: &str) {
        let names = |entry: &Entry| {
            match entry.event.kind {
                ChatEventKind::Prompt => entry.event.payload.get("messageId"),
                ChatEventKind::SessionUpdate
                    if session_update_kind(&entry.event.payload) == Some("user_message_chunk") =>
                {
                    entry.event.payload.pointer("/update/messageId")
                }
                _ => None,
            }
            .and_then(Value::as_str)
                == Some(message_id)
        };
        let found = self.entries.iter().position(names);
        let position = found.unwrap_or(0);
        let index = match found {
            Some(position) => self.entries[position].index,
            None => self
                .history
                .as_ref()
                .and_then(History::first)
                .or_else(|| self.entries.front().map(|entry| entry.index))
                .unwrap_or(self.next_index),
        };
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
        self.in_turn = false;
        self.last_from_user = false;
        self.since_point = 0;
    }

    pub(crate) fn page(&self, before: u64, turns: usize) -> Result<Page, String> {
        let Some(history) = self.history.as_ref() else {
            return Ok(Page::default());
        };
        history
            .page(before, turns.clamp(1, MAX_PAGE_TURNS), PAGE_BYTES)
            .map_err(|error| error.to_string())
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

    pub(crate) fn is_trimmed(&self) -> bool {
        self.trimmed
    }

    pub(crate) fn events(&self) -> Vec<ChatEvent> {
        self.entries
            .iter()
            .map(|entry| entry.event.clone())
            .collect()
    }
}

/// One event as clients heard it.
struct Sent {
    seq: u64,
    event: ChatEvent,
    bytes: usize,
    /// Who sent it, when it is a prompt the sender was not told of.
    sender: Option<Peer>,
}

/// The newest events as sent, numbered, so a client that held everything
/// up to some number can be told just the rest.
#[derive(Default)]
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
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
    subagents: HashSet<String>,
    /// The session ended, so nobody new is let in to wait for events that
    /// will not come.
    closed: bool,
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
                start: None,
                permission_mode,
                title: None,
                subagents: HashSet::new(),
                closed: false,
            }),
        })
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
        let updates = std::mem::take(&mut inner.pending);
        inner.pending_bytes = 0;
        self.broadcast(
            inner,
            ChatEventKind::SessionUpdate,
            json!({ "updates": updates }),
        );
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.flush_scheduled = false;
            self.flush_locked(&mut inner);
        }
    }

    /// Anything that is not a streamed update reads as a reply to what came
    /// before it, so the batch behind it goes out first and the order the
    /// agent sent them in survives.
    pub(crate) fn emit(self: &Arc<Self>, kind: ChatEventKind, mut payload: Value) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if is_turn_edge(kind) {
            stamp(&mut payload);
        }
        let bytes = inner.replay.push(kind, &payload);
        if kind == ChatEventKind::SessionUpdate {
            if let Some(title) = session_title(&payload) {
                inner.title = Some(title.to_owned());
            }
            track_subagent(&mut inner.subagents, &payload);
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
        // An agent starting again knows nothing of the old one's subagents.
        if kind == ChatEventKind::Status {
            inner.replay.forget_subagents();
        }
        if kind == ChatEventKind::Status || cancels_subagents(kind, &payload) {
            inner.subagents.clear();
        }
        self.flush_locked(&mut inner);
        self.broadcast(&mut inner, kind, payload);
    }

    /// The client that sent a prompt already shows it; everyone else, and every
    /// later replay, learns it here.
    pub(crate) fn prompted(
        &self,
        from: ClientId,
        message_id: Option<&str>,
        text: &str,
        paths: &[String],
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let mut payload = json!({ "text": text, "paths": paths });
        if let Some(message_id) = message_id {
            payload["messageId"] = json!(message_id);
        }
        stamp(&mut payload);
        inner.replay.push(ChatEventKind::Prompt, &payload);
        self.flush_locked(&mut inner);
        self.broadcast_except(&mut inner, ChatEventKind::Prompt, payload, Some(from));
    }

    /// The person took the chat back to before their message `message_id`.
    /// It goes from the replay, and every client but the one that asked is
    /// told to drop it and everything after it.
    pub(crate) fn rewound(&self, from: ClientId, session_id: &str, message_id: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
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

    pub(crate) fn subscribe(&self, client: &Arc<ClientConn>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.subscribers.insert(client.id, client.clone());
        }
    }

    pub(crate) fn has_subscriber(&self, client: ClientId) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| inner.subscribers.contains_key(&client))
    }

    pub(crate) fn unsubscribe(&self, client: ClientId) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.subscribers.remove(&client);
        }
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
        let Ok(mut inner) = self.inner.lock() else {
            client.respond(request_id, Err("chat feed lock poisoned".into()));
            return;
        };
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

    /// A phone is sent the chat's last turns and pages back through the rest.
    /// The host's own app reloads a chat longer than the replay from the
    /// provider instead.
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
        let phone = !peer.is_local() && inner.replay.keeps_history();
        match (inner.start.clone(), missed) {
            _ if inner.closed => ChatAttachment::Missing,
            (Some(_), Some(events)) => ChatAttachment::Resumed {
                events,
                mark: self.mark(inner),
            },
            (Some(_), None) if inner.replay.is_trimmed() && !phone => ChatAttachment::Restart,
            (Some(start), None) => {
                let (replay, older_before) = if phone {
                    let tail = inner.replay.tail();
                    (tail.events, tail.older_before)
                } else {
                    (inner.replay.events(), None)
                };
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
        if let Ok(mut inner) = self.inner.lock() {
            inner.closed = true;
        }
    }

    pub(crate) fn set_start(&self, start: ChatStart) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.start = Some(start);
        }
    }

    pub(crate) fn set_setup(&self, setup: &Value) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(start) = inner.start.as_mut() {
                start.setup = setup.clone();
            }
        }
    }

    pub(crate) fn set_permission_mode(&self, mode: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.permission_mode = mode.to_owned();
        }
    }

    pub(crate) fn permission_mode(&self) -> String {
        self.inner
            .lock()
            .map(|inner| inner.permission_mode.clone())
            .unwrap_or_default()
    }

    /// What the agent last called this session, as the host's rail shows it.
    pub(crate) fn title(&self) -> Option<String> {
        self.inner.lock().ok()?.title.clone()
    }

    pub(crate) fn running_subagents(&self) -> u32 {
        self.inner
            .lock()
            .map(|inner| inner.subagents.len() as u32)
            .unwrap_or_default()
    }

    pub(crate) fn start(&self) -> Option<ChatStart> {
        self.inner.lock().ok()?.start.clone()
    }

    pub(crate) fn forget_permission(&self, request_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.replay.forget_permission(request_id);
            inner.recent.forget_permission(request_id);
        }
    }

    /// The turns before `before` in this run of the chat, which a phone
    /// shown only the chat's end scrolls back to.
    pub(crate) fn history(&self, feed: &str, before: u64, turns: usize) -> Result<Page, String> {
        if feed != self.id {
            return Err("the chat started again; open it again to see its history".into());
        }
        let inner = self.inner.lock().map_err(|_| "chat feed lock poisoned")?;
        inner.replay.page(before, turns)
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
        assert!(replay.events().is_empty());
    }

    #[test]
    fn subagents_count_from_spawn_until_they_stop() {
        let update =
            |session: &str, update: Value| json!({ "sessionId": session, "update": update });
        let spawned =
            |id: &str| json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": id });
        let state = |id: &str, state: &str| json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": id, "state": state });
        let mut running = HashSet::new();
        track_subagent(&mut running, &update("s", spawned("a")));
        track_subagent(&mut running, &update("s", spawned("b")));
        track_subagent(&mut running, &update("a", spawned("nested")));
        track_subagent(&mut running, &update("s", state("a", "running")));
        assert_eq!(running.len(), 2);
        track_subagent(&mut running, &update("s", state("a", "completed")));
        assert_eq!(running, HashSet::from(["b".to_owned()]));
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
            session_title(&info("Fix the flaky test")),
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

        let tail = replay.tail();
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
            standing_kinds(&replay.tail()),
            [
                ("plan".to_owned(), "second".to_owned()),
                ("async_task_spawned".to_owned(), "shell".to_owned()),
                ("async_task_progress".to_owned(), "shell".to_owned()),
                ("subagent_spawned".to_owned(), "helper".to_owned()),
            ]
        );
    }

    #[test]
    fn a_running_subagent_stands_until_it_stops_or_its_turn_is_cancelled() {
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
        assert!(held(&replay).is_empty());
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
        assert_eq!(feed.running_subagents(), 1);

        feed.emit(ChatEventKind::Status, json!({ "state": "starting" }));
        assert_eq!(feed.running_subagents(), 0);
        assert!(feed.inner.lock().unwrap().replay.standing.is_empty());

        feed.emit(ChatEventKind::SessionUpdate, spawned);
        feed.emit(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": "cancelled" }),
        );
        assert_eq!(feed.running_subagents(), 0);
    }

    #[tokio::test]
    async fn a_phone_is_sent_the_last_turns_and_the_host_app_everything() {
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
        assert_eq!(asked(&replay), asks(10..60));
        assert_eq!(older_before, Some(10 * 5));
        drop(inner);
        let page = feed.history(&feed.id, 10 * 5, 100).unwrap();
        assert_eq!(asked(&page.events), asks(0..10));
        assert_eq!(page.older_before, None);
        assert!(feed.history("an earlier run", 10 * 5, 100).is_err());
        let mut inner = feed.inner.lock().unwrap();

        let ChatAttachment::Live {
            replay,
            older_before,
            ..
        } = feed.attachment(&inner, &Peer::Local, standing(), None)
        else {
            panic!("the app takes up a chat the replay still holds");
        };
        assert_eq!(asked(&replay), asks(0..60));
        assert_eq!(older_before, None);

        let history = History::open(dir.path(), "trimmed", MAX_HISTORY_BYTES);
        inner.replay = Replay::new(MAX_REPLAY_BYTES, 12, Some(history));
        for n in 0..60 {
            turn(&mut inner.replay, n);
        }
        assert!(matches!(
            feed.attachment(&inner, &Peer::Local, standing(), None),
            ChatAttachment::Restart
        ));
        let ChatAttachment::Live { older_before, .. } =
            feed.attachment(&inner, &phone, standing(), None)
        else {
            panic!("a phone takes up a chat longer than the replay");
        };
        assert_eq!(older_before, Some(58 * 5));
    }
}
