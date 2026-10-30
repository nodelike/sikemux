//! How a subagent or a background task stands, in the words the chat uses,
//! and how each agent's own words for it map onto them.

use sikemux_fold::UpdateKind;
use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How work an agent reported on ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Failed,
    Stopped,
}

/// What an agent's own word for a subagent's, task's or turn's status says
/// about how it ended. Claude Code and Codex use these; a status that is not
/// an end says nothing.
pub(crate) fn outcome(status: &str) -> Option<Outcome> {
    match status {
        "completed" => Some(Outcome::Completed),
        "failed" | "errored" | "error" | "shutdown" | "notFound" => Some(Outcome::Failed),
        "killed" | "cancelled" | "canceled" | "stopped" | "interrupted" => Some(Outcome::Stopped),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Subagent {
    Running,
    Completed,
    Failed,
    Cancelled,
    /// Its agent went away before it ended.
    Disconnected,
}

impl Subagent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Disconnected => "disconnected",
        }
    }

    pub fn parse(state: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Completed,
            Self::Failed,
            Self::Cancelled,
            Self::Disconnected,
        ]
        .into_iter()
        .find(|known| known.as_str() == state)
    }

    pub fn ended(self) -> bool {
        self != Self::Running
    }

    /// The update that sets the card of the subagent in `session` to this.
    pub fn update(self, session: &str) -> Value {
        sikemux_fold::update::subagent_state(session, self.as_str())
    }
}

impl From<Outcome> for Subagent {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Completed => Self::Completed,
            Outcome::Failed => Self::Failed,
            Outcome::Stopped => Self::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Task {
    Running,
    Paused,
    Completed,
    Failed,
    Stopped,
}

impl Task {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(state: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Paused,
            Self::Completed,
            Self::Failed,
            Self::Stopped,
        ]
        .into_iter()
        .find(|known| known.as_str() == state)
    }

    /// A task from an agent's own word for its status.
    pub fn from_status(status: &str) -> Option<Self> {
        match status {
            "pending" | "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            _ => outcome(status).map(Self::from),
        }
    }

    pub fn ended(self) -> bool {
        !matches!(self, Self::Running | Self::Paused)
    }

    /// The update that sets background task `task_id` to this.
    pub fn update(self, task_id: &str) -> Value {
        json!({ "sessionUpdate": UpdateKind::AsyncTaskStateUpdate, "asyncTaskId": task_id, "state": self.as_str() })
    }
}

impl From<Outcome> for Task {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Completed => Self::Completed,
            Outcome::Failed => Self::Failed,
            Outcome::Stopped => Self::Stopped,
        }
    }
}

/// What a subagent's card says when its agent named it nothing.
pub(crate) const UNNAMED_TASK: &str = "Delegated task";

/// The update that puts a subagent's card in the chat.
pub(crate) fn spawned(session: &str, name: &str, task: &str) -> Value {
    json!({
        "sessionUpdate": UpdateKind::SubagentSpawned,
        "subagentSessionId": session,
        "name": name,
        "task": task,
        "capabilities": {},
    })
}

/// A subagent's name when its agent gave none, from the end of its id.
pub(crate) fn unnamed(id: &str) -> String {
    let start = id.char_indices().rev().nth(7).map_or(0, |(index, _)| index);
    format!("Agent {}", &id[start..])
}

/// The first of `candidates` with something in it, trimmed.
pub(crate) fn first_text(candidates: &[Option<&str>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .map(|candidate| candidate.trim())
        .find(|candidate| !candidate.is_empty())
        .map(str::to_owned)
}

/// What a subagent said before its card was put in the chat, kept until it
/// is. Past this many, the oldest go: the newest settle what the card shows.
#[derive(Serialize, Deserialize)]
pub(crate) struct Held<T> {
    items: VecDeque<T>,
}

const MAX_HELD: usize = 256;

impl<T> Default for Held<T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }
}

impl<T> Held<T> {
    pub fn push(&mut self, item: T) {
        if self.items.len() == MAX_HELD {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }
}

impl<T> IntoIterator for Held<T> {
    type Item = T;
    type IntoIter = std::collections::vec_deque::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}
