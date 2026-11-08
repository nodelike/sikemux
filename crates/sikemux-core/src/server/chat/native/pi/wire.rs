//! Pi's RPC mode: one JSON command per line in, one response or event per line
//! out. It is not JSON-RPC: a command carries its name in `type`, a response
//! repeats the command's string `id`, and events have none.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::super::pending::{unanswered, Answered, Pending, Purpose};
use super::super::process::Writer;
use super::super::rpc::{error_text, RpcState, Sent, ANSWER_WAIT};

pub(super) enum Incoming {
    Event(Value),
    /// The answer to a command sent with a purpose, which nothing waited on.
    Answer {
        purpose: Purpose,
        result: Answered,
    },
}

#[derive(Clone)]
pub(super) struct Wire {
    writer: Writer,
    next_id: Arc<AtomicI64>,
    pending: Arc<Pending<i64>>,
}

impl Wire {
    pub fn new(writer: Writer) -> Self {
        Self::restored(
            writer,
            RpcState {
                next_id: 1,
                waiting: Vec::new(),
                versioned: false,
            },
        )
    }

    pub fn restored(writer: Writer, state: RpcState) -> Self {
        Self {
            writer,
            next_id: Arc::new(AtomicI64::new(state.next_id)),
            pending: Arc::new(Pending::restored(state.waiting)),
        }
    }

    pub fn state(&self) -> RpcState {
        RpcState {
            next_id: self.next_id.load(Ordering::Acquire),
            waiting: self.pending.state(),
            versioned: false,
        }
    }

    fn numbered(&self, mut command: Value) -> (i64, Value) {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        command["id"] = json!(id.to_string());
        (id, command)
    }

    /// Writes a record Pi gives no response to.
    pub fn tell(&self, record: &Value) {
        if let Err(error) = self.writer.send(record) {
            eprintln!("sikemux core: Pi was not told: {error}");
        }
    }

    pub fn start(&self, command: Value, purpose: Option<Purpose>) -> Result<Sent, String> {
        let name = command["type"].as_str().unwrap_or("command").to_owned();
        let (id, command) = self.numbered(command);
        let answered = self.pending.waited(id, purpose);
        self.writer.send(&command).inspect_err(|_| {
            self.pending.forget(&id);
        })?;
        Ok(Sent::new(self.pending.clone(), id, &name, answered))
    }

    pub async fn request(&self, command: Value) -> Result<Value, String> {
        self.start(command, None)?
            .answer(Some(ANSWER_WAIT))
            .await
            .map_err(|error| error_text(&error))
    }

    /// Sends a command whose answer comes back as an [`Incoming::Answer`].
    pub fn send_for(&self, command: Value, purpose: Purpose) {
        let (id, command) = self.numbered(command);
        self.pending.noted(id, purpose);
        if let Err(error) = self.writer.send(&command) {
            self.pending.forget(&id);
            eprintln!("sikemux core: Pi was not sent a command: {error}");
        }
    }

    /// Hands a response to what waits on it, or reads the record as an event.
    pub fn read(&self, message: Value) -> Option<Incoming> {
        if message["type"] != "response" {
            return Some(Incoming::Event(message));
        }
        let id = message["id"].as_str()?.parse::<i64>().ok()?;
        let result = if message["success"] == true {
            Ok(message.get("data").cloned().unwrap_or(Value::Null))
        } else {
            let said = message["error"]
                .as_str()
                .unwrap_or("Pi refused the command");
            Err(unanswered(said))
        };
        let (purpose, result) = self.pending.settle(&id, result)?;
        Some(Incoming::Answer { purpose, result })
    }

    pub fn close(&self) {
        self.pending.close();
    }
}
