//! The app's window as a client of the core. Tool calls that need the window,
//! such as reading its layout or opening a file in it, are sent to whichever
//! connection registered last, and its answer is handed back to the caller.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::oneshot;

use crate::protocol::{CallId, ServerMessage, WindowAnswer, WindowCall};

use super::connection::{ClientConn, ClientId};

/// Calls the window has not answered yet, beyond which a new one is refused.
const MAX_PENDING_CALLS: usize = 64;
const WINDOW_GONE: &str = "Sikemux's window closed before it answered";

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
    client: Option<Arc<ClientConn>>,
    calls: HashMap<CallId, Pending>,
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

    pub(crate) fn is_open(&self) -> bool {
        self.inner.lock().is_ok_and(|inner| inner.client.is_some())
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

    pub(crate) fn register(&self, client: Arc<ClientConn>) {
        let orphaned = match self.inner.lock() {
            Ok(mut inner) => {
                let previous = inner.client.replace(client.clone());
                if previous.is_some_and(|previous| previous.id == client.id) {
                    return;
                }
                let stale: Vec<CallId> = inner
                    .calls
                    .iter()
                    .filter(|(_, call)| call.client != client.id)
                    .map(|(id, _)| *id)
                    .collect();
                stale
                    .into_iter()
                    .filter_map(|id| inner.calls.remove(&id))
                    .collect::<Vec<_>>()
            }
            Err(_) => return,
        };
        Self::fail(orphaned);
    }

    pub(crate) fn unregister(&self, client: ClientId) {
        let orphaned = match self.inner.lock() {
            Ok(mut inner) => {
                if inner
                    .client
                    .as_ref()
                    .is_some_and(|current| current.id == client)
                {
                    inner.client = None;
                }
                let gone: Vec<CallId> = inner
                    .calls
                    .iter()
                    .filter(|(_, call)| call.client == client)
                    .map(|(id, _)| *id)
                    .collect();
                gone.into_iter()
                    .filter_map(|id| inner.calls.remove(&id))
                    .collect::<Vec<_>>()
            }
            Err(_) => return,
        };
        Self::fail(orphaned);
    }

    /// Sends the call, or says why it cannot: `purpose` finishes the sentence
    /// "open Sikemux to …". A call that `waits` stays open after its answer
    /// until the window says its tabs closed.
    pub(crate) fn call(
        &self,
        call: WindowCall,
        purpose: &str,
        waits: bool,
    ) -> Result<Call, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "window lock poisoned".to_string())?;
        let client = inner.client.clone().ok_or_else(|| not_open(purpose))?;
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
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
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
        if let Ok(mut inner) = self.inner.lock() {
            inner.calls.remove(&id);
        }
    }
}
