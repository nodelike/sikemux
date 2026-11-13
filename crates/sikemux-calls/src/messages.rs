use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageKind {
    Message,
    /// How a worker's turn ended, sent to its lead.
    Report,
    Question,
    Answer,
    /// A note from Sikemux itself, such as two agents changing the same lines.
    Notice,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessagesParams {
    pub project: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A project's messages between agents, one row each, newest first, under the
/// project's own path.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageRows {
    pub project: String,
    pub rows: Vec<MessageRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageOutcome {
    /// Taken into the turn the agent was working on.
    Steered,
    /// Started a turn of the agent's.
    NewTurn,
    /// Typed into a terminal agent's prompt, not yet seen taken.
    Typed,
    /// Waits for the agent to be free.
    Waiting,
    /// Reached the agent after waiting, or a terminal agent took it.
    Delivered,
    /// The agent did not take it, so it waits again.
    Returned,
    /// The same text went to the same agent a moment before.
    Repeat,
    Refused,
    /// Dropped unread: its agent closed, or it was withdrawn.
    Dropped,
}

/// Whether a message still waits for its agent, or never reached it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageTone {
    Held,
    Failed,
}

/// One message between agents, from being sent to what last became of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub sent_at: u64,
    pub at: u64,
    /// The sending agent's id, or a [`super::Sender`]'s.
    pub from: Option<String>,
    /// The sending agent's name now, else when it was logged; none for those that are
    /// not agents.
    pub from_name: Option<String>,
    pub to: String,
    pub to_name: Option<String>,
    pub kind: Option<MessageKind>,
    pub outcome: MessageOutcome,
    /// It waited for its agent before it got where it is.
    pub waited: bool,
    pub tone: Option<MessageTone>,
}
