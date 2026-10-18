//! JSON-RPC over an agent's standard input and output: requests this side
//! sends and waits on, and the agent's own requests and notifications.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::process::Writer;

/// How long a request may wait for its answer before the agent is taken to
/// be stuck.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(120);

/// The code JSON-RPC gives an error inside the side that reports it.
const INTERNAL_ERROR: i64 = -32603;

/// An answer, or the error object the agent turned the request down with.
type Answer = oneshot::Sender<Result<Value, Value>>;

/// A request still waiting on its answer. One with a `purpose` is answered
/// through [`Incoming::Answer`] when nothing here waits on it any more, such
/// as after a core that replaced this one took the agent up.
struct Waiting {
    answer: Option<Answer>,
    purpose: Option<Value>,
}

/// The requests a backend still waits on that say what their answer is for,
/// and the id the next request takes, as plain data.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RpcState {
    pub next_id: i64,
    pub waiting: Vec<(i64, Value)>,
}

/// One message the agent sent that is not an answer to this side.
pub(crate) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// The answer to a request sent with a purpose, which nothing here waited on.
    Answer {
        purpose: Value,
        result: Result<Value, Value>,
    },
}

/// An error object for a request that got no answer from the agent.
fn unanswered(message: impl Into<String>) -> Value {
    json!({ "code": INTERNAL_ERROR, "message": message.into() })
}

/// What an error object says, in words.
pub(crate) fn error_text(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| error.to_string(), str::to_owned)
}

#[derive(Clone)]
pub(crate) struct Rpc {
    writer: Writer,
    next_id: Arc<AtomicI64>,
    waiting: Arc<Mutex<HashMap<i64, Waiting>>>,
    /// Whether every message names its JSON-RPC version. Codex leaves it out;
    /// Muse refuses a message without it.
    versioned: bool,
}

/// A request that has been sent, and the answer still to come.
pub(crate) struct Sent {
    rpc: Rpc,
    id: i64,
    method: String,
    answered: oneshot::Receiver<Result<Value, Value>>,
}

impl Sent {
    /// The agent's answer, or its error object. With a `wait`, a request left
    /// unanswered that long fails.
    pub async fn answer(self, wait: Option<Duration>) -> Result<Value, Value> {
        let answered = match wait {
            Some(wait) => match tokio::time::timeout(wait, self.answered).await {
                Ok(answered) => answered,
                Err(_) => {
                    self.rpc.abandon(self.id);
                    return Err(unanswered(format!(
                        "The agent did not answer {}",
                        self.method
                    )));
                }
            },
            None => self.answered.await,
        };
        answered.unwrap_or_else(|_| Err(unanswered("The agent stopped before it answered")))
    }
}

impl Rpc {
    pub fn new(writer: Writer) -> Self {
        Self {
            writer,
            next_id: Arc::new(AtomicI64::new(1)),
            waiting: Arc::new(Mutex::new(HashMap::new())),
            versioned: false,
        }
    }

    pub fn versioned(writer: Writer) -> Self {
        Self {
            versioned: true,
            ..Self::new(writer)
        }
    }

    /// Goes on from `state`, which an earlier connection to the same agent
    /// left: answers to its requests still find what they were for.
    pub fn restored(writer: Writer, versioned: bool, state: RpcState) -> Self {
        let waiting = state
            .waiting
            .into_iter()
            .map(|(id, purpose)| {
                let purpose = Some(purpose);
                (
                    id,
                    Waiting {
                        answer: None,
                        purpose,
                    },
                )
            })
            .collect();
        Self {
            writer,
            next_id: Arc::new(AtomicI64::new(state.next_id)),
            waiting: Arc::new(Mutex::new(waiting)),
            versioned,
        }
    }

    /// Where the requests stand, for a connection that goes on after this one.
    pub fn state(&self) -> RpcState {
        let mut waiting: Vec<(i64, Value)> = self
            .waiting()
            .iter()
            .filter_map(|(id, waiting)| Some((*id, waiting.purpose.clone()?)))
            .collect();
        waiting.sort_by_key(|(id, _)| *id);
        RpcState {
            next_id: self.next_id.load(Ordering::Acquire),
            waiting,
        }
    }

    fn send(&self, mut message: Value) -> Result<(), String> {
        if self.versioned {
            message["jsonrpc"] = json!("2.0");
        }
        self.writer.send(&message)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.start(method, params)
            .map_err(|error| error_text(&error))?
            .answer(Some(ANSWER_WAIT))
            .await
            .map_err(|error| error_text(&error))
    }

    /// Sends a request without waiting on its answer.
    pub fn start(&self, method: &str, params: Value) -> Result<Sent, Value> {
        self.start_for(method, params, None)
    }

    /// Sends a request whose answer, once nothing waits on the [`Sent`],
    /// comes back as an [`Incoming::Answer`] with `purpose`.
    pub fn start_for(
        &self,
        method: &str,
        params: Value,
        purpose: Option<Value>,
    ) -> Result<Sent, Value> {
        let (answer, answered) = oneshot::channel();
        let id = self.send_request(method, params, Some(answer), purpose)?;
        Ok(Sent {
            rpc: self.clone(),
            id,
            method: method.to_owned(),
            answered,
        })
    }

    /// Sends a request whose answer comes back only as an
    /// [`Incoming::Answer`] with `purpose`.
    pub fn send_for(&self, method: &str, params: Value, purpose: Value) -> Result<(), String> {
        self.send_request(method, params, None, Some(purpose))
            .map(drop)
            .map_err(|error| error_text(&error))
    }

    fn send_request(
        &self,
        method: &str,
        params: Value,
        answer: Option<Answer>,
        purpose: Option<Value>,
    ) -> Result<i64, Value> {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        self.waiting().insert(id, Waiting { answer, purpose });
        if let Err(error) = self.send(json!({ "id": id, "method": method, "params": params })) {
            self.forget(id);
            return Err(unanswered(error));
        }
        Ok(id)
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let mut message = json!({ "method": method });
        if !(self.versioned && params.is_null()) {
            message["params"] = params;
        }
        self.send(message)
    }

    pub fn respond(&self, id: Value, result: Value) {
        let _ = self.send(json!({ "id": id, "result": result }));
    }

    pub fn refuse(&self, id: Value, message: &str) {
        let _ = self.send(json!({
            "id": id,
            "error": { "code": -32601, "message": message },
        }));
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Waiting>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn forget(&self, id: i64) {
        self.waiting().remove(&id);
    }

    /// Nothing waits on request `id` any more; one with a purpose still has
    /// its answer read for it.
    fn abandon(&self, id: i64) {
        let mut waiting = self.waiting();
        match waiting.get_mut(&id) {
            Some(entry) if entry.purpose.is_some() => entry.answer = None,
            _ => {
                waiting.remove(&id);
            }
        }
    }

    /// Hands an answer to the request waiting on it, or reads the message as
    /// one of the agent's own.
    pub fn read(&self, message: Value) -> Option<Incoming> {
        let Value::Object(mut message) = message else {
            return None;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let params = message.remove("params").unwrap_or(Value::Null);
        match (message.remove("id"), method) {
            (Some(id), Some(method)) => Some(Incoming::Request { id, method, params }),
            (None, Some(method)) => Some(Incoming::Notification { method, params }),
            (Some(id), None) => {
                let waiting = id.as_i64().and_then(|id| self.waiting().remove(&id))?;
                let result = match message.remove("error") {
                    Some(error) => Err(error),
                    None => Ok(message.remove("result").unwrap_or(Value::Null)),
                };
                let result = match waiting.answer {
                    Some(answer) => answer.send(result).err()?,
                    None => result,
                };
                waiting
                    .purpose
                    .map(|purpose| Incoming::Answer { purpose, result })
            }
            (None, None) => None,
        }
    }

    /// Fails every request still waiting, once the agent is gone.
    pub fn close(&self) {
        for (_, waiting) in self.waiting().drain() {
            if let Some(answer) = waiting.answer {
                let _ = answer.send(Err(unanswered("The agent stopped before it answered")));
            }
        }
    }
}
