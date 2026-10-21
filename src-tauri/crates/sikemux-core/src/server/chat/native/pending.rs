//! Requests sent to an agent that are still unanswered, and what each one
//! was sent for.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::oneshot;

/// The code JSON-RPC gives an error inside the side that reports it.
const INTERNAL_ERROR: i64 = -32603;

/// An answer, or the error object the agent turned the request down with.
pub(crate) type Answered = Result<Value, Value>;

/// What a request was sent for, when its answer is read once it comes rather
/// than waited on, such as by a core that took the agent over since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Purpose {
    /// The prompt of this turn.
    Prompt(u64),
    /// A message sent mid-turn, and what the person said in it.
    Steer {
        text: String,
        paths: Vec<String>,
    },
    StopTask(String),
    /// Stopping the turn with this id.
    Interrupt(String),
    /// Following a subagent's conversation.
    Subscribe(String),
    /// Reading a finished subagent's conversation, or else what it answered.
    ReadSubagent {
        child: String,
        result: Option<String>,
    },
    /// The patch an edit stored.
    Patch {
        session: String,
        call: String,
    },
    /// A name for the chat.
    Title,
}

/// An error object for a request that got no answer from the agent.
pub(crate) fn unanswered(message: impl Into<String>) -> Value {
    json!({ "code": INTERNAL_ERROR, "message": message.into() })
}

struct Waiting {
    answer: Option<oneshot::Sender<Answered>>,
    purpose: Option<Purpose>,
}

/// The requests still waiting on their answers, by id. One with a purpose is
/// answered through [`Pending::settle`] once nothing here waits on it.
pub(crate) struct Pending<K> {
    waiting: Mutex<HashMap<K, Waiting>>,
}

impl<K: Eq + Hash + Ord + Clone> Pending<K> {
    /// Goes on from requests an earlier connection to the same agent sent
    /// with a purpose, whose answers have not come yet.
    pub fn restored(waiting: Vec<(K, Purpose)>) -> Self {
        let waiting = waiting
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
            waiting: Mutex::new(waiting),
        }
    }

    /// The requests sent with a purpose that are still unanswered.
    pub fn state(&self) -> Vec<(K, Purpose)> {
        let mut kept: Vec<(K, Purpose)> = self
            .waiting()
            .iter()
            .filter_map(|(id, waiting)| Some((id.clone(), waiting.purpose.clone()?)))
            .collect();
        kept.sort_by(|a, b| a.0.cmp(&b.0));
        kept
    }

    fn waiting(&self) -> MutexGuard<'_, HashMap<K, Waiting>> {
        self.waiting.locked()
    }

    /// Notes request `id`, whose answer comes to the receiver, or once nothing
    /// waits on it is read for `purpose`.
    pub fn waited(&self, id: K, purpose: Option<Purpose>) -> oneshot::Receiver<Answered> {
        let (answer, answered) = oneshot::channel();
        let answer = Some(answer);
        self.waiting().insert(id, Waiting { answer, purpose });
        answered
    }

    /// Notes request `id`, whose answer is read for `purpose`.
    pub fn noted(&self, id: K, purpose: Purpose) {
        let purpose = Some(purpose);
        self.waiting().insert(
            id,
            Waiting {
                answer: None,
                purpose,
            },
        );
    }

    pub fn forget(&self, id: &K) {
        self.waiting().remove(id);
    }

    /// The answer to request `id`. With a `wait`, one left unanswered that
    /// long fails, and its purpose, if it has one, still has it read.
    pub async fn answer(
        &self,
        id: &K,
        answered: oneshot::Receiver<Answered>,
        wait: Option<Duration>,
        what: &str,
    ) -> Answered {
        let answered = match wait {
            Some(wait) => match tokio::time::timeout(wait, answered).await {
                Ok(answered) => answered,
                Err(_) => {
                    self.abandon(id);
                    return Err(unanswered(format!("The agent did not answer {what}")));
                }
            },
            None => answered.await,
        };
        answered.unwrap_or_else(|_| Err(unanswered("The agent stopped before it answered")))
    }

    /// Nothing waits on request `id` any more; one with a purpose still has
    /// its answer read for it.
    pub fn abandon(&self, id: &K) {
        let mut waiting = self.waiting();
        match waiting.get_mut(id) {
            Some(entry) if entry.purpose.is_some() => entry.answer = None,
            _ => {
                waiting.remove(id);
            }
        }
    }

    /// Hands `result` to whatever waits on request `id`, or back with the
    /// purpose it was sent for when nothing does.
    pub fn settle(&self, id: &K, result: Answered) -> Option<(Purpose, Answered)> {
        let waiting = self.waiting().remove(id)?;
        let result = match waiting.answer {
            Some(answer) => answer.send(result).err()?,
            None => result,
        };
        Some((waiting.purpose?, result))
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
