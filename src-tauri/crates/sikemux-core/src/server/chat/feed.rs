//! What a chat agent says, on its way to the clients that show it. Streamed
//! updates leave in one batch per frame. Everything since the session started
//! or loaded is kept, so a client that attaches later rebuilds the chat from
//! the same events the others watched arrive.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{
    encode_control, fits, ChatAttachment, ChatEvent, ChatEventKind, ChatMark, ChatStart, Event,
    RequestId, Response, ServerMessage,
};

use super::super::access::Peer;
use super::super::connection::{ClientConn, ClientId};

/// One frame's worth of streamed updates travels as a single event. Each
/// notification on its own costs a script eval in the webview, and an adapter
/// sends one per token.
const FLUSH: Duration = Duration::from_millis(16);
pub(crate) const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_REPLAY_EVENTS: usize = 20_000;
/// Roughly what an event costs beyond its payload once it is on the wire.
const EVENT_OVERHEAD: usize = 48;
/// The events kept as sent, for a client that reconnects to hear only what
/// it missed. Past this it rebuilds the chat from the replay.
const MAX_RECENT_BYTES: usize = 1024 * 1024;
const MAX_RECENT_EVENTS: usize = 4096;

struct Entry {
    event: ChatEvent,
    bytes: usize,
}

/// The ordered events a late client replays. Text streamed into one message
/// is kept as one event, which the chat would have joined up anyway.
pub(crate) struct Replay {
    entries: VecDeque<Entry>,
    bytes: usize,
    max_bytes: usize,
    max_events: usize,
    trimmed: bool,
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

impl Replay {
    pub(crate) fn new(max_bytes: usize, max_events: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_bytes,
            max_events,
            trimmed: false,
        }
    }

    fn join_text(&mut self, payload: &Value) -> bool {
        let Some(later) = chunk(payload) else {
            return false;
        };
        let Some(last) = self
            .entries
            .back_mut()
            .filter(|entry| entry.event.kind == ChatEventKind::SessionUpdate)
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
        true
    }

    pub(crate) fn push(&mut self, kind: ChatEventKind, payload: &Value) {
        if !is_kept(kind) {
            return;
        }
        if kind == ChatEventKind::SessionUpdate && self.join_text(payload) {
            self.trim();
            return;
        }
        let bytes = serde_json::to_vec(payload).map_or(0, |bytes| bytes.len()) + EVENT_OVERHEAD;
        self.entries.push_back(Entry {
            event: ChatEvent {
                kind,
                payload: payload.clone(),
            },
            bytes,
        });
        self.bytes += bytes;
        self.trim();
    }

    fn trim(&mut self) {
        while self.bytes > self.max_bytes || self.entries.len() > self.max_events {
            let Some(dropped) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= dropped.bytes;
            self.trimmed = true;
        }
    }

    /// An answered request is not asked again when the chat is replayed.
    pub(crate) fn forget_permission(&mut self, request_id: &str) {
        let mut freed = 0;
        self.entries.retain(|entry| {
            let answered = entry.event.kind == ChatEventKind::PermissionRequest
                && entry.event.payload.get("requestId").and_then(Value::as_str) == Some(request_id);
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
    flush_scheduled: bool,
    start: Option<ChatStart>,
    permission_mode: String,
    title: Option<String>,
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
    pub(crate) fn new(agent_id: String, permission_mode: String) -> Arc<Self> {
        Arc::new(Self {
            agent_id,
            id: uuid::Uuid::new_v4().to_string(),
            runtime: tokio::runtime::Handle::current(),
            inner: Mutex::new(Inner {
                subscribers: HashMap::new(),
                seq: 0,
                recent: Recent::default(),
                replay: Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS),
                pending: Vec::new(),
                flush_scheduled: false,
                start: None,
                permission_mode,
                title: None,
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
    pub(crate) fn emit(self: &Arc<Self>, kind: ChatEventKind, payload: Value) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.replay.push(kind, &payload);
        if kind == ChatEventKind::SessionUpdate {
            if let Some(title) = session_title(&payload) {
                inner.title = Some(title.to_owned());
            }
            inner.pending.push(payload);
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
    pub(crate) fn prompted(&self, from: ClientId, text: &str, paths: &[String]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let payload = json!({ "text": text, "paths": paths });
        inner.replay.push(ChatEventKind::Prompt, &payload);
        self.flush_locked(&mut inner);
        self.broadcast_except(&mut inner, ChatEventKind::Prompt, payload, Some(from));
    }

    pub(crate) fn subscribe(&self, client: &Arc<ClientConn>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.subscribers.insert(client.id, client.clone());
        }
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
        let missed = since
            .filter(|since| since.feed == self.id && since.seq <= inner.seq)
            .and_then(|since| inner.recent.since(since.seq, &client.peer));
        let attachment = match (inner.start.clone(), missed) {
            _ if inner.closed => ChatAttachment::Missing,
            (Some(_), Some(events)) => ChatAttachment::Resumed {
                events,
                mark: self.mark(&inner),
            },
            (Some(_), None) if inner.replay.is_trimmed() => ChatAttachment::Restart,
            (Some(start), None) => ChatAttachment::Live {
                start: Box::new(start),
                permission_mode: inner.permission_mode.clone(),
                running: standing.running,
                turned: standing.turned,
                replay: inner.replay.events(),
                mark: self.mark(&inner),
            },
            (None, _) => ChatAttachment::Missing,
        };
        let live = matches!(
            attachment,
            ChatAttachment::Live { .. } | ChatAttachment::Resumed { .. }
        );
        client.respond(request_id, Ok(Response::ChatAttached { attachment }));
        if live {
            inner.subscribers.insert(client.id, client.clone());
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

    /// What the agent last called this session, as the Mac's rail shows it.
    pub(crate) fn title(&self) -> Option<String> {
        self.inner.lock().ok()?.title.clone()
    }

    pub(crate) fn start(&self) -> Option<ChatStart> {
        self.inner.lock().ok()?.start.clone()
    }

    pub(crate) fn forget_permission(&self, request_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.replay.forget_permission(request_id);
        }
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
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS);
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
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS);
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
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS);
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
        let mut replay = Replay::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS);
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
    fn a_replay_over_its_bounds_drops_the_oldest_and_says_so() {
        let mut replay = Replay::new(MAX_REPLAY_BYTES, 3);
        for index in 0..5 {
            replay.push(ChatEventKind::Error, &json!({ "message": index }));
        }
        let events = replay.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].payload["message"], 2);
        assert!(replay.is_trimmed());

        let mut replay = Replay::new(400, MAX_REPLAY_EVENTS);
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
}
