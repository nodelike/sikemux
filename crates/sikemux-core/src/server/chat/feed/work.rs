//! The subagents and background tasks a chat's agent has going.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::UpdateKind;

use super::super::states::{Subagent, Task};

/// A subagent or background task the agent has going.
#[derive(Serialize, Deserialize)]
struct Going {
    id: String,
    /// The session whose transcript shows it.
    session: String,
    /// Started inside a subagent rather than by the chat's own agent.
    nested: bool,
    /// Running rather than paused.
    running: bool,
}

/// The subagents and background tasks the agent has going, which end with
/// it when its process goes.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Work {
    subagents: Vec<Going>,
    tasks: Vec<Going>,
    /// Every subagent's session, whose own updates are not the chat's.
    subagent_sessions: HashSet<String>,
}

impl Work {
    pub(super) fn in_subagent(&self, payload: &Value) -> bool {
        payload
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| self.subagent_sessions.contains(session))
    }

    /// Notes what `payload` starts or ends.
    pub(super) fn note(&mut self, payload: &Value) {
        let Some(update) = payload.get("update") else {
            return;
        };
        let session = payload
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let nested = self.in_subagent(payload);
        let text = |key: &str| update.get(key).and_then(Value::as_str);
        match UpdateKind::of(update) {
            Some(UpdateKind::SubagentSpawned) => {
                let Some(id) = text("subagentSessionId") else {
                    return;
                };
                self.subagent_sessions.insert(id.to_owned());
                if !self.subagents.iter().any(|going| going.id == id) {
                    self.subagents.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some(UpdateKind::SubagentStateUpdate) => {
                let Some(id) = text("subagentSessionId") else {
                    return;
                };
                if subagent_ended(update) {
                    self.subagents.retain(|going| going.id != id);
                } else if !self.subagents.iter().any(|going| going.id == id) {
                    self.subagents.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some(UpdateKind::AsyncTaskSpawned) => {
                let Some(id) = text("asyncTaskId") else {
                    return;
                };
                if !self.tasks.iter().any(|going| going.id == id) {
                    self.tasks.push(Going {
                        id: id.to_owned(),
                        session: session.to_owned(),
                        nested,
                        running: true,
                    });
                }
            }
            Some(UpdateKind::AsyncTaskProgress | UpdateKind::AsyncTaskStateUpdate) => {
                let Some(id) = text("asyncTaskId") else {
                    return;
                };
                match text("state").and_then(Task::parse) {
                    Some(state) if state.ended() => self.tasks.retain(|going| going.id != id),
                    Some(state) => {
                        if let Some(going) = self.tasks.iter_mut().find(|going| going.id == id) {
                            going.running = state == Task::Running;
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    pub(super) fn running_subagents(&self) -> usize {
        self.subagents.iter().filter(|going| !going.nested).count()
    }

    pub(super) fn running_tasks(&self) -> usize {
        self.tasks
            .iter()
            .filter(|going| !going.nested && going.running)
            .count()
    }

    /// The updates that end everything still going, the newest first.
    pub(super) fn endings(&self) -> Vec<Value> {
        let tasks = self.tasks.iter().rev().map(|going| {
            json!({ "sessionId": going.session, "update": Task::Stopped.update(&going.id) })
        });
        tasks
            .chain(self.subagent_endings(Subagent::Disconnected))
            .collect()
    }

    /// The updates that end every subagent still going as `state`, the newest first.
    pub(super) fn subagent_endings(&self, state: Subagent) -> impl Iterator<Item = Value> + '_ {
        self.subagents.iter().rev().map(
            move |going| json!({ "sessionId": going.session, "update": state.update(&going.id) }),
        )
    }
}

pub(super) fn subagent_ended(update: &Value) -> bool {
    update
        .get("state")
        .and_then(Value::as_str)
        .and_then(Subagent::parse)
        .is_some_and(Subagent::ended)
}

pub(super) fn task_ended(update: &Value) -> bool {
    update
        .get("state")
        .and_then(Value::as_str)
        .and_then(Task::parse)
        .is_some_and(Task::ended)
}
