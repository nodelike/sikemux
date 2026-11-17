//! Claude Code's control channel: requests either side sends inside the
//! stream of messages, each answered by a `control_response` naming it.

use std::sync::{Arc, Mutex, Weak};

use serde_json::{json, Value};
use uuid::Uuid;

use super::super::pending::{Answered, Pending, Purpose};
use super::super::process::Writer;
use super::super::rpc::{error_text, ANSWER_WAIT};
use super::super::session::Sink;
use super::messages::ControlResponse;
use super::reader::Shared;
use sikemux_process::sync::Locked;

/// An ask written to Claude: its id, and what its answer comes back on.
type ModeAsked = (String, tokio::sync::oneshot::Receiver<Answered>);

#[derive(Clone)]
pub struct Control {
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

    /// Writes the ask for Claude to work in permission `mode`, answering what waits for its
    /// answer.
    fn ask_mode(&self, mode: &str) -> Result<ModeAsked, String> {
        let id = request_id();
        let answered = self.pending.waited(id.clone(), None);
        let request = json!({ "subtype": "set_permission_mode", "mode": mode });
        self.write(&id, request)?;
        Ok((id, answered))
    }

    /// Whether Claude worked in the mode asked, or None when it gave no answer in time.
    async fn mode_answer(&self, asked: Result<ModeAsked, String>) -> Option<Result<(), String>> {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
        let (id, answered) = match asked {
            Ok(asked) => asked,
            Err(error) => return Some(Err(error)),
        };
        match tokio::time::timeout(WAIT, answered).await {
            Ok(Ok(answer)) => Some(answer.map(drop).map_err(|error| error_text(&error))),
            Ok(Err(_)) => Some(Err("Claude stopped before it answered".to_owned())),
            Err(_) => {
                self.pending.forget(&id);
                None
            }
        }
    }

    /// Asks Claude to work in permission `mode`: whether it did, or None when it gave no
    /// answer in time.
    pub async fn set_mode(&self, mode: &str) -> Option<Result<(), String>> {
        self.mode_answer(self.ask_mode(mode)).await
    }

    /// Puts the chat's own permission mode back after a hidden turn ran in another, asking
    /// again when Claude refuses. Claude still refusing, or not answering, is started again on
    /// its session, which it launches in that mode, unless the chat has let go of that Claude,
    /// whose `shared` state is then gone. The first ask is written before this returns, so a
    /// mode the person picks after it goes to Claude after it, and asks again for the chat's
    /// mode as it then stands.
    pub fn restore_mode(&self, mode: String, sink: Sink, shared: Weak<Mutex<Shared>>) {
        const ASKS: usize = 3;
        let control = self.clone();
        let first = self.ask_mode(&mode);
        tokio::spawn(async move {
            let mut why = String::new();
            let mut asked = Some(first);
            for _ in 0..ASKS {
                let answer = match asked.take() {
                    Some(first) => control.mode_answer(first).await,
                    None => {
                        let Some(now) = shared.upgrade().map(|shared| shared.locked().mode.clone())
                        else {
                            return;
                        };
                        control.set_mode(&now).await
                    }
                };
                match answer {
                    Some(Ok(())) => return,
                    Some(Err(error)) => why = error,
                    None => {
                        why = "it did not answer".to_owned();
                        break;
                    }
                }
            }
            if shared.strong_count() == 0 {
                return;
            }
            eprintln!("sikemux core: Claude did not go back to its permission mode {mode} after a handoff note: {why}");
            sink.broke(format!(
                "Claude would not go back to its permission mode after a handoff note: {why}"
            ));
        });
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

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
