//! JSON-RPC over an agent's standard input and output: requests this side
//! sends and waits on, and the agent's own requests and notifications.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::pending::{unanswered, Answered, Pending, Purpose};
use super::process::Writer;

/// How long a request may wait for its answer before the agent is taken to
/// be stuck.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(120);

/// How the requests stand, for a connection to the same agent that goes on
/// after this one: the id the next takes, those sent with a purpose that are
/// still unanswered, and whether every message names its JSON-RPC version.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RpcState {
    pub next_id: i64,
    pub waiting: Vec<(i64, Purpose)>,
    pub versioned: bool,
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
        purpose: Purpose,
        result: Answered,
    },
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
    pending: Arc<Pending<i64>>,
    /// Whether every message names its JSON-RPC version. Codex leaves it out;
    /// Muse refuses a message without it.
    versioned: bool,
}

/// A request that has been sent, and the answer still to come.
pub(crate) struct Sent {
    pending: Arc<Pending<i64>>,
    id: i64,
    method: String,
    answered: oneshot::Receiver<Answered>,
}

impl Sent {
    /// The agent's answer, or its error object. With a `wait`, a request left
    /// unanswered that long fails.
    pub async fn answer(self, wait: Option<Duration>) -> Answered {
        self.pending
            .answer(&self.id, self.answered, wait, &self.method)
            .await
    }
}

impl Rpc {
    pub fn new(writer: Writer, versioned: bool) -> Self {
        Self::restored(
            writer,
            RpcState {
                next_id: 1,
                waiting: Vec::new(),
                versioned,
            },
        )
    }

    /// Goes on from `state`, which an earlier connection to the same agent
    /// left: answers to its requests still find what they were for.
    pub fn restored(writer: Writer, state: RpcState) -> Self {
        Self {
            writer,
            next_id: Arc::new(AtomicI64::new(state.next_id)),
            pending: Arc::new(Pending::restored(state.waiting)),
            versioned: state.versioned,
        }
    }

    /// Where the requests stand, for a connection that goes on after this one.
    pub fn state(&self) -> RpcState {
        RpcState {
            next_id: self.next_id.load(Ordering::Acquire),
            waiting: self.pending.state(),
            versioned: self.versioned,
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

    /// Every page of `method`'s answer, each asked for with the `params` the
    /// cursor the page before it named gives, until one names none new.
    pub async fn pages(
        &self,
        method: &str,
        params: impl Fn(&Value) -> Value,
    ) -> Result<Vec<Value>, String> {
        let mut pages = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let page = self.request(method, params(&cursor)).await?;
            let next = page["nextCursor"]
                .as_str()
                .filter(|next| cursor != *next)
                .map(Value::from);
            pages.push(page);
            match next {
                Some(next) => cursor = next,
                None => return Ok(pages),
            }
        }
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
        purpose: Option<Purpose>,
    ) -> Result<Sent, Value> {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        let answered = self.pending.waited(id, purpose);
        self.write_request(id, method, params)?;
        Ok(Sent {
            pending: self.pending.clone(),
            id,
            method: method.to_owned(),
            answered,
        })
    }

    /// Sends a request whose answer comes back only as an
    /// [`Incoming::Answer`] with `purpose`.
    pub fn send_for(&self, method: &str, params: Value, purpose: Purpose) -> Result<(), String> {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        self.pending.noted(id, purpose);
        self.write_request(id, method, params)
            .map_err(|error| error_text(&error))
    }

    fn write_request(&self, id: i64, method: &str, params: Value) -> Result<(), Value> {
        self.send(json!({ "id": id, "method": method, "params": params }))
            .map_err(|error| {
                self.pending.forget(&id);
                unanswered(error)
            })
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
                let result = match message.remove("error") {
                    Some(error) => Err(error),
                    None => Ok(message.remove("result").unwrap_or(Value::Null)),
                };
                let (purpose, result) = self.pending.settle(&id.as_i64()?, result)?;
                Some(Incoming::Answer { purpose, result })
            }
            (None, None) => None,
        }
    }

    /// Fails every request still waiting, once the agent is gone.
    pub fn close(&self) {
        self.pending.close();
    }
}
