//! Claude Code's control channel: requests either side sends inside the
//! stream of messages, each answered by a `control_response` naming it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::super::process::Writer;

const ANSWER_WAIT: Duration = Duration::from_secs(60);

type Answer = oneshot::Sender<Result<Value, String>>;

/// A request still waiting on its answer: waited on here, or sent with a
/// purpose its answer is read for once it comes.
enum Waiting {
    Here(Answer),
    For(Value),
}

#[derive(Clone)]
pub(crate) struct Control {
    writer: Writer,
    waiting: Arc<Mutex<HashMap<String, Waiting>>>,
}

impl Control {
    pub fn new(writer: Writer) -> Self {
        Self::restored(writer, Vec::new())
    }

    /// Goes on from requests an earlier connection to the same Claude sent
    /// with a purpose, whose answers have not come yet.
    pub fn restored(writer: Writer, waiting: Vec<(String, Value)>) -> Self {
        let waiting = waiting
            .into_iter()
            .map(|(id, purpose)| (id, Waiting::For(purpose)))
            .collect();
        Self {
            writer,
            waiting: Arc::new(Mutex::new(waiting)),
        }
    }

    /// The requests sent with a purpose that are still unanswered.
    pub fn state(&self) -> Vec<(String, Value)> {
        let Ok(waiting) = self.waiting.lock() else {
            return Vec::new();
        };
        let mut kept: Vec<(String, Value)> = waiting
            .iter()
            .filter_map(|(id, waiting)| match waiting {
                Waiting::For(purpose) => Some((id.clone(), purpose.clone())),
                Waiting::Here(_) => None,
            })
            .collect();
        kept.sort_by(|a, b| a.0.cmp(&b.0));
        kept
    }

    /// Sends `request` without waiting: its answer comes back from
    /// [`Control::answered`] with `purpose`.
    pub fn send_for(&self, request: Value, purpose: Value) -> Result<(), String> {
        let id = Uuid::new_v4().simple().to_string();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id.clone(), Waiting::For(purpose));
        }
        let sent = self.writer.send(&json!({
            "type": "control_request",
            "request_id": id,
            "request": request,
        }));
        if sent.is_err() {
            self.forget(&id);
        }
        sent
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// Sends `request`, whose `subtype` names it, and waits for its answer.
    pub async fn request(&self, request: Value) -> Result<Value, String> {
        let id = Uuid::new_v4().simple().to_string();
        let subtype = request
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("request")
            .to_owned();
        let (answer, answered) = oneshot::channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id.clone(), Waiting::Here(answer));
        }
        let sent = self.writer.send(&json!({
            "type": "control_request",
            "request_id": id,
            "request": request,
        }));
        if let Err(error) = sent {
            self.forget(&id);
            return Err(error);
        }
        match tokio::time::timeout(ANSWER_WAIT, answered).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Claude stopped before it answered".into()),
            Err(_) => {
                self.forget(&id);
                Err(format!("Claude did not answer {subtype}"))
            }
        }
    }

    fn forget(&self, id: &str) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(id);
        }
    }

    /// Hands a `control_response` to the request waiting on it, or answers
    /// with the purpose of a request sent without waiting and its result.
    pub fn answered(&self, message: &Value) -> Option<(Value, Result<Value, String>)> {
        let response = message.get("response")?;
        let id = response.get("request_id").and_then(Value::as_str)?;
        let waiting = self
            .waiting
            .lock()
            .ok()
            .and_then(|mut waiting| waiting.remove(id))?;
        let result = match response.get("subtype").and_then(Value::as_str) {
            Some("error") => Err(response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Claude refused the request")
                .to_owned()),
            _ => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
        };
        match waiting {
            Waiting::Here(answer) => {
                let _ = answer.send(result);
                None
            }
            Waiting::For(purpose) => Some((purpose, result)),
        }
    }

    /// Answers one of Claude's own requests.
    pub fn respond(&self, request_id: &str, response: Value) {
        let _ = self.writer.send(&json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        }));
    }

    pub fn refuse(&self, request_id: &str, error: &str) {
        let _ = self.writer.send(&json!({
            "type": "control_response",
            "response": { "subtype": "error", "request_id": request_id, "error": error },
        }));
    }

    pub fn close(&self) {
        if let Ok(mut waiting) = self.waiting.lock() {
            for (_, waiting) in waiting.drain() {
                if let Waiting::Here(answer) = waiting {
                    let _ = answer.send(Err("Claude stopped before it answered".into()));
                }
            }
        }
    }
}
