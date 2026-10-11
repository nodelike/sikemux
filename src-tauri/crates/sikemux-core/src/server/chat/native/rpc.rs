//! JSON-RPC over an agent's standard input and output: requests this side
//! sends and waits on, and the agent's own requests and notifications.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::process::Writer;

/// How long a request may wait for its answer before the agent is taken to
/// be stuck.
const ANSWER_WAIT: Duration = Duration::from_secs(120);

type Answer = oneshot::Sender<Result<Value, String>>;

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
}

#[derive(Clone)]
pub(crate) struct Rpc {
    writer: Writer,
    next_id: Arc<AtomicI64>,
    waiting: Arc<Mutex<HashMap<i64, Answer>>>,
}

impl Rpc {
    pub fn new(writer: Writer) -> Self {
        Self {
            writer,
            next_id: Arc::new(AtomicI64::new(1)),
            waiting: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id, answer);
        }
        if let Err(error) = self
            .writer
            .send(&json!({ "id": id, "method": method, "params": params }))
        {
            self.forget(id);
            return Err(error);
        }
        match tokio::time::timeout(ANSWER_WAIT, answered).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("The agent stopped before it answered".into()),
            Err(_) => {
                self.forget(id);
                Err(format!("The agent did not answer {method}"))
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.writer
            .send(&json!({ "method": method, "params": params }))
    }

    pub fn respond(&self, id: Value, result: Value) {
        let _ = self.writer.send(&json!({ "id": id, "result": result }));
    }

    pub fn refuse(&self, id: Value, message: &str) {
        let _ = self.writer.send(&json!({
            "id": id,
            "error": { "code": -32601, "message": message },
        }));
    }

    fn forget(&self, id: i64) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(&id);
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
                let answer = id
                    .as_i64()
                    .and_then(|id| self.waiting.lock().ok()?.remove(&id));
                if let Some(answer) = answer {
                    let result = match message.remove("error") {
                        Some(error) => Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .map_or_else(|| error.to_string(), str::to_owned)),
                        None => Ok(message.remove("result").unwrap_or(Value::Null)),
                    };
                    let _ = answer.send(result);
                }
                None
            }
            (None, None) => None,
        }
    }

    /// Fails every request still waiting, once the agent is gone.
    pub fn close(&self) {
        if let Ok(mut waiting) = self.waiting.lock() {
            for (_, answer) in waiting.drain() {
                let _ = answer.send(Err("The agent stopped before it answered".into()));
            }
        }
    }
}
