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

#[derive(Clone)]
pub(crate) struct Control {
    writer: Writer,
    waiting: Arc<Mutex<HashMap<String, Answer>>>,
}

impl Control {
    pub fn new(writer: Writer) -> Self {
        Self {
            writer,
            waiting: Arc::new(Mutex::new(HashMap::new())),
        }
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
            waiting.insert(id.clone(), answer);
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

    /// Hands a `control_response` to the request waiting on it.
    pub fn answered(&self, message: &Value) {
        let Some(response) = message.get("response") else {
            return;
        };
        let Some(id) = response.get("request_id").and_then(Value::as_str) else {
            return;
        };
        let Some(answer) = self
            .waiting
            .lock()
            .ok()
            .and_then(|mut waiting| waiting.remove(id))
        else {
            return;
        };
        let result = match response.get("subtype").and_then(Value::as_str) {
            Some("error") => Err(response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Claude refused the request")
                .to_owned()),
            _ => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
        };
        let _ = answer.send(result);
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
            for (_, answer) in waiting.drain() {
                let _ = answer.send(Err("Claude stopped before it answered".into()));
            }
        }
    }
}
