//! What this host answered the calls linked hosts sent it that do work, by the id each call
//! carries, so a call sent again after its answer was lost is answered the same and not done
//! twice. Kept in `host-receipts.json`, long enough for a host away over a weekend to ask again.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::super::kept::Kept;
use super::link::Answer;

const FILE: &str = "host-receipts.json";
const KEEP_MS: u64 = 30 * 24 * 60 * 60_000;
const MAX_RECEIPTS: usize = 4_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    host: String,
    call: String,
    answer: Answer,
    at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Record {
    receipts: Vec<Receipt>,
}

#[derive(Default)]
struct State {
    record: Record,
    /// Calls being done now, by host and id.
    running: HashSet<(String, String)>,
}

pub(crate) struct Receipts {
    state: Mutex<State>,
    kept: Kept,
}

/// Where a call stands on this host.
pub(crate) enum Stands {
    /// Done, and answered this.
    Done(Answer),
    /// Being done now.
    Running,
    /// Never heard of.
    Unknown,
}

impl Stands {
    /// How a host asking what became of a call is told.
    pub(crate) fn told(self) -> Value {
        match self {
            Stands::Done(answer) => json!({ "state": "done", "answer": answer }),
            Stands::Running => json!({ "state": "running" }),
            Stands::Unknown => json!({ "state": "unknown" }),
        }
    }

    pub(crate) fn heard(value: &Value) -> Option<Self> {
        match value["state"].as_str()? {
            "done" => serde_json::from_value(value["answer"].clone())
                .ok()
                .map(Stands::Done),
            "running" => Some(Stands::Running),
            "unknown" => Some(Stands::Unknown),
            _ => None,
        }
    }
}

/// A call this host started doing; it is no longer running once dropped.
pub(crate) struct Doing<'a> {
    receipts: &'a Receipts,
    key: (String, String),
}

impl Doing<'_> {
    /// The call ended with `answer`, which a call sent again gets.
    pub(crate) fn done(self, answer: &Answer) {
        let mut state = self.receipts.state.locked();
        let now = epoch_ms();
        let receipts = &mut state.record.receipts;
        receipts.retain(|receipt| now.saturating_sub(receipt.at) < KEEP_MS);
        receipts.push(Receipt {
            host: self.key.0.clone(),
            call: self.key.1.clone(),
            answer: answer.clone(),
            at: now,
        });
        let over = receipts.len().saturating_sub(MAX_RECEIPTS);
        receipts.drain(..over);
        self.receipts.kept.save(&state.record);
    }
}

impl Drop for Doing<'_> {
    fn drop(&mut self) {
        self.receipts.state.locked().running.remove(&self.key);
    }
}

fn stands_in(state: &State, host: &str, call: &str) -> Stands {
    if let Some(receipt) =
        (state.record.receipts.iter()).find(|receipt| receipt.host == host && receipt.call == call)
    {
        return Stands::Done(receipt.answer.clone());
    }
    match state.running.contains(&(host.to_owned(), call.to_owned())) {
        true => Stands::Running,
        false => Stands::Unknown,
    }
}

impl Receipts {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            state: Mutex::new(State {
                record: kept.load(),
                running: HashSet::new(),
            }),
            kept,
        }
    }

    pub(crate) fn stands(&self, host: &str, call: &str) -> Stands {
        stands_in(&self.state.locked(), host, call)
    }

    /// Starts doing the call `call` from `host`, unless it was done or is being done.
    pub(crate) fn begin(&self, host: &str, call: &str) -> Result<Doing<'_>, Stands> {
        let mut state = self.state.locked();
        let stands = stands_in(&state, host, call);
        let Stands::Unknown = stands else {
            return Err(stands);
        };
        let key = (host.to_owned(), call.to_owned());
        state.running.insert(key.clone());
        Ok(Doing {
            receipts: self,
            key,
        })
    }

    /// Lets go of what `host` called, once it is no longer linked.
    pub(crate) fn forget(&self, host: &str) {
        let mut state = self.state.locked();
        let before = state.record.receipts.len();
        state.record.receipts.retain(|receipt| receipt.host != host);
        if state.record.receipts.len() != before {
            self.kept.save(&state.record);
        }
    }
}
