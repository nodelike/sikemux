//! The app's window as a client of the core. Tool calls that need the window,
//! such as reading its layout or driving its browser, are sent to the window
//! that registered last, which a window does again when the person brings it
//! forward, and its answer is handed back to the caller. When that window
//! closes, the one before it takes the calls that follow. An agent's browser
//! and simulator live in the window that first answered it, so its calls to
//! them keep going there while that window is open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::sync::oneshot;

use crate::protocol::{encode_control, CallId, Event, ServerMessage, WindowAnswer, WindowCall};

use super::connection::{ClientConn, ClientId};

/// Calls the window has not answered yet, beyond which a new one is refused.
const MAX_PENDING_CALLS: usize = 64;
/// Agents remembered by the window that holds their browser.
const MAX_PINS: usize = 1_024;
const WINDOW_GONE: &str = "Sikemux's window closed before it answered";
/// How long an agent asked for while no window was open is still shown once
/// one opens, which covers the app starting.
const FOCUS_HELD: Duration = Duration::from_secs(60);

pub(crate) fn not_open(purpose: &str) -> String {
    format!("Sikemux's window is not open — open Sikemux to {purpose}")
}

struct Pending {
    client: ClientId,
    waits: bool,
    answer: Option<oneshot::Sender<Result<Value, String>>>,
    closed: Option<oneshot::Sender<bool>>,
}

#[derive(Default)]
struct Inner {
    /// Every window open, the one that registered last at the end.
    clients: Vec<Arc<ClientConn>>,
    /// The window each agent's browser and simulator live in, by agent id.
    pins: HashMap<String, ClientId>,
    calls: HashMap<CallId, Pending>,
    focus: Option<(String, Instant)>,
}

pub(crate) struct Window {
    inner: Mutex<Inner>,
    next_call: AtomicU64,
}

/// A call sent to the window. `closed` resolves once a waiting CLI `open`
/// has had every tab it opened closed: true when they were, false when the
/// window went away first.
pub(crate) struct Call {
    pub id: CallId,
    pub answer: oneshot::Receiver<Result<Value, String>>,
    pub closed: oneshot::Receiver<bool>,
}

impl Window {
    pub(crate) fn new(first_call: CallId) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            next_call: AtomicU64::new(first_call),
        }
    }

    pub(crate) fn call_mark(&self) -> CallId {
        self.next_call.load(Ordering::Acquire)
    }

    /// Carries on from the call ids an earlier core handed out.
    pub(crate) fn continue_calls(&self, mark: CallId) {
        self.next_call.fetch_max(mark, Ordering::AcqRel);
    }

    /// The connection the window in front speaks through, while one is open.
    pub(crate) fn client_id(&self) -> Option<ClientId> {
        self.inner.locked().clients.last().map(|client| client.id)
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.inner.locked().clients.is_empty()
    }

    fn fail(calls: impl IntoIterator<Item = Pending>) {
        for call in calls {
            if let Some(answer) = call.answer {
                let _ = answer.send(Err(WINDOW_GONE.into()));
            }
            if let Some(closed) = call.closed {
                let _ = closed.send(false);
            }
        }
    }

    /// Sends the window an event no other client hears. False when no window
    /// is open.
    pub(crate) fn tell(&self, event: Event) -> bool {
        let Some(client) = self.inner.locked().clients.last().cloned() else {
            return false;
        };
        let Ok(frame) = encode_control(&ServerMessage::Event { event }) else {
            return false;
        };
        client.send(frame.into())
    }

    /// Shows `agent_id` in the window, or in the next window to open. False
    /// when no window is open now.
    pub(crate) fn focus(&self, agent_id: String) -> bool {
        let shown = self.tell(Event::FocusAgent {
            agent_id: agent_id.clone(),
        });
        if !shown {
            self.inner.locked().focus = Some((agent_id, Instant::now()));
        }
        shown
    }

    /// Puts the window `client` speaks for in front, so the calls that follow
    /// go to it. Calls already sent to another window stay with that one.
    pub(crate) fn register(&self, client: Arc<ClientConn>) {
        let focus = {
            let mut inner = self.inner.locked();
            if inner
                .clients
                .last()
                .is_some_and(|last| last.id == client.id)
            {
                return;
            }
            inner.clients.retain(|known| known.id != client.id);
            inner.clients.push(client);
            inner
                .focus
                .take()
                .filter(|(_, asked)| asked.elapsed() < FOCUS_HELD)
        };
        if let Some((agent_id, _)) = focus {
            self.tell(Event::FocusAgent { agent_id });
        }
    }

    pub(crate) fn unregister(&self, client: ClientId) {
        let orphaned = {
            let mut inner = self.inner.locked();
            inner.clients.retain(|known| known.id != client);
            inner.pins.retain(|_, pinned| *pinned != client);
            let gone: Vec<CallId> = inner
                .calls
                .iter()
                .filter(|(_, call)| call.client == client)
                .map(|(id, _)| *id)
                .collect();
            gone.into_iter()
                .filter_map(|id| inner.calls.remove(&id))
                .collect::<Vec<_>>()
        };
        Self::fail(orphaned);
    }

    /// Sends the call, or says why it cannot: `purpose` finishes the sentence
    /// "open Sikemux to …". A call that `waits` stays open after its answer
    /// until the window says its tabs closed.
    /// `pinned` names the agent whose browser or simulator the call drives,
    /// which goes to the window holding them.
    pub(crate) fn call(
        &self,
        call: WindowCall,
        purpose: &str,
        waits: bool,
        pinned: Option<&str>,
    ) -> Result<Call, String> {
        let mut inner = self.inner.locked();
        let held = pinned
            .and_then(|agent| inner.pins.get(agent))
            .and_then(|id| inner.clients.iter().find(|client| client.id == *id));
        let client = held
            .or_else(|| inner.clients.last())
            .cloned()
            .ok_or_else(|| not_open(purpose))?;
        if let Some(agent) = pinned {
            if inner.pins.len() >= MAX_PINS && !inner.pins.contains_key(agent) {
                inner.pins.clear();
            }
            inner.pins.insert(agent.to_owned(), client.id);
        }
        if inner.calls.len() >= MAX_PENDING_CALLS {
            return Err("Sikemux's window has too many tool calls waiting; retry shortly".into());
        }
        let id = self.next_call.fetch_add(1, Ordering::Relaxed);
        let (answer, answer_rx) = oneshot::channel();
        let (closed, closed_rx) = oneshot::channel();
        inner.calls.insert(
            id,
            Pending {
                client: client.id,
                waits,
                answer: Some(answer),
                closed: Some(closed),
            },
        );
        drop(inner);
        client.send_message(&ServerMessage::WindowCall { call_id: id, call });
        Ok(Call {
            id,
            answer: answer_rx,
            closed: closed_rx,
        })
    }

    fn settle(&self, client: ClientId, id: CallId, finish: impl FnOnce(&mut Pending)) {
        let mut inner = self.inner.locked();
        let Some(call) = inner
            .calls
            .get_mut(&id)
            .filter(|call| call.client == client)
        else {
            return;
        };
        finish(call);
        if call.answer.is_none() && call.closed.is_none() {
            inner.calls.remove(&id);
        }
    }

    pub(crate) fn answer(&self, client: ClientId, id: CallId, answer: WindowAnswer) {
        self.settle(client, id, |call| {
            if let Some(sender) = call.answer.take() {
                let _ = sender.send(answer.into());
            }
            if !call.waits {
                call.closed = None;
            }
        });
    }

    pub(crate) fn open_closed(&self, client: ClientId, id: CallId) {
        self.settle(client, id, |call| {
            call.answer = None;
            if let Some(sender) = call.closed.take() {
                let _ = sender.send(true);
            }
        });
    }

    /// The caller stopped waiting.
    pub(crate) fn forget(&self, id: CallId) {
        self.inner.locked().calls.remove(&id);
    }
}
