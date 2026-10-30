//! A chat as a client holds it: the view the core sends on attach, with each
//! change after it applied, the way the app and the phone apply them.

#![allow(dead_code)]

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sikemux_core::client::ClientEvent;
use sikemux_core::protocol::{ChatAttachment, Event};
use sikemux_wire::chat::{
    ChatChange, ChatConnection, ChatHead, ChatMessage, ChatPart, ChatRole, ChatView, StopReason,
};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Clone, Debug, Default)]
pub struct Folded {
    pub view: ChatView,
    /// `Event::Chat` frames applied.
    pub frames: usize,
    pub turns_started: usize,
    /// The stop reason each ended turn left in the head, in order.
    pub ends: Vec<Option<StopReason>>,
    pub errors: usize,
    /// Every head change, in order, for what comes and goes between frames.
    pub heads: Vec<Value>,
}

impl Folded {
    pub fn of_view(view: &Value) -> Self {
        Self {
            view: serde_json::from_value(view.clone()).expect("a chat view"),
            ..Self::default()
        }
    }

    /// The chat a client is sent on attaching, or what it missed applied to `self`.
    pub fn attached(&mut self, attachment: &ChatAttachment) {
        match attachment {
            ChatAttachment::Live { view, .. } => {
                self.view = serde_json::from_value(view.clone()).expect("a chat view")
            }
            ChatAttachment::Resumed { changes, .. } => self.apply(changes),
            ChatAttachment::Missing => panic!("the chat is missing"),
        }
    }

    pub fn apply(&mut self, changes: &Value) {
        let changes: Vec<ChatChange> =
            serde_json::from_value(changes.clone()).expect("a list of chat changes");
        for change in changes {
            self.change(change);
        }
    }

    fn change(&mut self, change: ChatChange) {
        match change {
            ChatChange::Head { set, .. } => {
                let was = self.view.head.running;
                self.view.head = merged(&self.view.head, &set);
                if set.get("error").is_some_and(|error| !error.is_null()) {
                    self.errors += 1;
                }
                self.heads.push(set);
                self.turned(was);
            }
            ChatChange::Row { row, within } => {
                let rows = self.rows_mut(within);
                match rows.iter_mut().find(|held| held.id == row.id) {
                    Some(held) => *held = row,
                    None => rows.push(row),
                }
            }
            ChatChange::Part { row, part, within } => {
                let parts = &mut self.row_mut(within, &row).parts;
                match parts.iter_mut().find(|held| held.id() == part.id()) {
                    Some(held) => *held = part,
                    None => parts.push(part),
                }
            }
            ChatChange::Text {
                row,
                part,
                text,
                within,
                ..
            } => {
                let held = self
                    .row_mut(within, &row)
                    .parts
                    .iter_mut()
                    .find(|held| held.id() == part)
                    .unwrap_or_else(|| panic!("text for part {part} of {row}, which is not held"));
                match held {
                    ChatPart::Text { text: held, .. } | ChatPart::Thought { text: held, .. } => {
                        held.push_str(&text)
                    }
                    other => panic!("text for a part that holds none: {other:?}"),
                }
            }
            ChatChange::Set { row, set, within } => {
                let held = self.row_mut(within, &row);
                *held = merged(held, &set);
            }
            ChatChange::Keep { rows } => self.view.rows.retain(|row| rows.contains(&row.id)),
            ChatChange::Drop { from } => {
                if let Some(at) = self.view.rows.iter().position(|row| row.id == from) {
                    self.view.rows.truncate(at);
                }
            }
            ChatChange::View { view } => {
                let was = self.view.head.running;
                self.view = *view;
                self.turned(was);
            }
        }
    }

    fn turned(&mut self, was: bool) {
        match (was, self.view.head.running) {
            (false, true) => self.turns_started += 1,
            (true, false) => self.ends.push(self.view.head.stop_reason),
            _ => {}
        }
    }

    fn rows_mut(&mut self, within: Option<String>) -> &mut Vec<ChatMessage> {
        match within {
            None => &mut self.view.rows,
            Some(session) => self.view.subagents.entry(session).or_default(),
        }
    }

    fn row_mut(&mut self, within: Option<String>, id: &str) -> &mut ChatMessage {
        self.rows_mut(within)
            .iter_mut()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("a change to row {id}, which is not held"))
    }

    pub fn head(&self) -> &ChatHead {
        &self.view.head
    }

    pub fn ready(&self) -> bool {
        self.view.head.connection == ChatConnection::Ready
    }

    pub fn turns_ended(&self) -> usize {
        self.ends.len()
    }

    /// Everything the agent said, in order.
    pub fn text(&self) -> String {
        said(&self.view.rows)
    }

    /// What was sent the agent, a row at a time.
    pub fn prompts(&self) -> Vec<String> {
        prompts(&self.view.rows)
    }

    pub fn parts(&self) -> impl Iterator<Item = &ChatPart> {
        self.view.rows.iter().flat_map(|row| &row.parts)
    }

    pub fn notices(&self) -> Vec<String> {
        self.parts()
            .filter_map(|part| match part {
                ChatPart::AgentNotice { notice, .. } => Some(notice.title.clone()),
                _ => None,
            })
            .collect()
    }

    /// The rows as a client draws them, without the markdown trees only a
    /// whole row carries.
    pub fn drawn(&self) -> Vec<ChatMessage> {
        self.view
            .rows
            .iter()
            .cloned()
            .map(|mut row| {
                for part in &mut row.parts {
                    if let ChatPart::Text { md, .. } | ChatPart::Thought { md, .. } = part {
                        *md = None;
                    }
                }
                row
            })
            .collect()
    }
}

fn merged<T: Serialize + DeserializeOwned>(held: &T, set: &Value) -> T {
    let mut whole = serde_json::to_value(held).expect("serializable");
    for (field, value) in set.as_object().expect("a set of fields") {
        whole[field] = value.clone();
    }
    serde_json::from_value(whole).expect("the fields fit")
}

fn text_of(row: &ChatMessage) -> String {
    row.parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub fn said(rows: &[ChatMessage]) -> String {
    rows.iter()
        .filter(|row| row.role == ChatRole::Assistant)
        .map(text_of)
        .collect()
}

pub fn prompts(rows: &[ChatMessage]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.role == ChatRole::User)
        .map(text_of)
        .collect()
}

/// One chat as one client hears it.
pub struct Chat {
    events: UnboundedReceiver<ClientEvent>,
    agent: String,
    pub folded: Folded,
    pub wait: Duration,
    /// Frames numbered up to this were already in the view an attach answered with.
    floor: Option<u64>,
    started: usize,
    ended: usize,
    said: usize,
}

impl std::ops::Deref for Chat {
    type Target = Folded;

    fn deref(&self) -> &Folded {
        &self.folded
    }
}

impl Chat {
    pub fn new(events: UnboundedReceiver<ClientEvent>, agent: &str) -> Self {
        Self {
            events,
            agent: agent.to_owned(),
            folded: Folded::default(),
            wait: Duration::from_secs(30),
            floor: None,
            started: 0,
            ended: 0,
            said: 0,
        }
    }

    pub fn attached(&mut self, attachment: &ChatAttachment) {
        self.folded.attached(attachment);
        if let ChatAttachment::Live { mark, .. } | ChatAttachment::Resumed { mark, .. } = attachment
        {
            self.floor = Some(mark.seq);
        }
    }

    /// Takes in the next event, and answers the number of the frame it
    /// applied, when the event was one of this chat's.
    async fn pump(&mut self) -> Option<u64> {
        let Ok(event) = tokio::time::timeout(self.wait, self.events.recv()).await else {
            panic!(
                "timed out waiting for the chat, which holds {:#?}",
                self.folded.view
            );
        };
        let ClientEvent::Event(Event::Chat {
            agent_id,
            seq,
            changes,
        }) = event.expect("the core disconnected")
        else {
            return None;
        };
        if agent_id != self.agent {
            return None;
        }
        if let Some(floor) = self.floor {
            if seq <= floor {
                return None;
            }
            self.floor = None;
        }
        self.folded.frames += 1;
        self.folded.apply(&changes);
        Some(seq)
    }

    pub async fn next_frame(&mut self) -> u64 {
        loop {
            if let Some(seq) = self.pump().await {
                return seq;
            }
        }
    }

    pub async fn until(&mut self, mut found: impl FnMut(&Folded) -> bool) -> &Folded {
        while !found(&self.folded) {
            self.pump().await;
        }
        &self.folded
    }

    pub async fn until_ready(&mut self) {
        self.until(Folded::ready).await;
    }

    /// Waits for the next turn to start after the last one waited for.
    pub async fn turn_started(&mut self) {
        let next = self.started + 1;
        self.until(|chat| chat.turns_started >= next).await;
        self.started = next;
    }

    /// Waits for the next turn to end after the last one waited for, and
    /// answers why it stopped.
    pub async fn turn_ended(&mut self) -> Option<StopReason> {
        let next = self.ended + 1;
        self.until(|chat| chat.turns_ended() >= next).await;
        self.ended = next;
        self.folded.ends[next - 1]
    }

    pub async fn until_turns_ended(&mut self, turns: usize) {
        self.until(|chat| chat.turns_ended() >= turns).await;
        self.ended = self.ended.max(turns);
    }

    /// Waits until the agent says `needle`, and answers what it said since
    /// the last time this was asked.
    pub async fn until_said(&mut self, needle: &str) -> String {
        let from = self.said;
        let text = self
            .until(|chat| {
                chat.text()
                    .get(from..)
                    .is_some_and(|said| said.contains(needle))
            })
            .await
            .text();
        self.said = text.len();
        text[from..].to_owned()
    }
}
