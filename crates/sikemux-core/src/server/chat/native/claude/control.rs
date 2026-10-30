//! Claude Code's control channel: requests either side sends inside the
//! stream of messages, each answered by a `control_response` naming it.

use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use super::super::pending::{Answered, Pending, Purpose};
use super::super::process::Writer;
use super::super::rpc::{error_text, ANSWER_WAIT};
use super::messages::ControlResponse;

#[derive(Clone)]
pub(crate) struct Control {
    writer: Writer,
    pending: Arc<Pending<String>>,
}

fn request_id() -> String {
    Uuid::new_v4().simple().to_string()
}

impl Control {
    pub fn new(writer: Writer) -> Self {
        Self::restored(writer, Vec::new())
    }

    /// Goes on from requests an earlier connection to the same Claude sent
    /// with a purpose, whose answers have not come yet.
    pub fn restored(writer: Writer, waiting: Vec<(String, Purpose)>) -> Self {
        Self {
            writer,
            pending: Arc::new(Pending::restored(waiting)),
        }
    }

    /// The requests sent with a purpose that are still unanswered.
    pub fn state(&self) -> Vec<(String, Purpose)> {
        self.pending.state()
    }

    fn write(&self, id: &str, request: Value) -> Result<(), String> {
        self.writer
            .send(&json!({
                "type": "control_request",
                "request_id": id,
                "request": request,
            }))
            .inspect_err(|_| self.pending.forget(&id.to_owned()))
    }

    /// Sends `request` without waiting: its answer comes back from
    /// [`Control::answered`] with `purpose`.
    pub fn send_for(&self, request: Value, purpose: Purpose) -> Result<(), String> {
        let id = request_id();
        self.pending.noted(id.clone(), purpose);
        self.write(&id, request)
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// Sends `request`, whose `subtype` names it, and waits for its answer.
    pub async fn request(&self, request: Value) -> Result<Value, String> {
        let id = request_id();
        let subtype = request["subtype"].as_str().unwrap_or("request").to_owned();
        let answered = self.pending.waited(id.clone(), None);
        self.write(&id, request)?;
        self.pending
            .answer(&id, answered, Some(ANSWER_WAIT), &subtype)
            .await
            .map_err(|error| error_text(&error))
    }

    /// Hands a `control_response` to the request waiting on it, or answers
    /// with the purpose of a request sent without waiting and its result.
    pub fn answered(&self, response: ControlResponse) -> Option<(Purpose, Answered)> {
        let result = match response.subtype.as_deref() {
            Some("error") => Err(json!({
                "message": response
                    .error
                    .as_deref()
                    .unwrap_or("Claude refused the request"),
            })),
            _ => Ok(response.response.unwrap_or(Value::Null)),
        };
        self.pending.settle(&response.request_id, result)
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
        self.pending.close();
    }
}
