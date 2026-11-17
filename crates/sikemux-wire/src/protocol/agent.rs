//! Where an agent stands for the person, and how the workspace shows it.

use serde::{Deserialize, Serialize};

/// Where an agent stands for the person, in the order lists show the groups:
/// waiting on them first. Only a running turn is Working; subagents and
/// shells left running are counted beside the agent instead.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum AgentGroup {
    /// It waits on a permission or an answer.
    Blocked,
    Working,
    /// It finished a turn the person has not seen.
    Done,
    #[default]
    Idle,
}

/// What a list says an agent is doing: its group, unless it is not running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Blocked,
    Working,
    Done,
    #[default]
    Idle,
    /// The app stopped it while idle; it starts again when opened.
    Asleep,
    /// It ended or failed to start.
    Stopped,
    Starting,
}

impl AgentStatus {
    /// The group a list puts an agent with this status in. Only a turn under way or a wait
    /// on the person counts as busy, so an agent still starting up rests with the idle ones.
    pub fn group(self) -> AgentGroup {
        match self {
            AgentStatus::Blocked => AgentGroup::Blocked,
            AgentStatus::Working => AgentGroup::Working,
            AgentStatus::Done => AgentGroup::Done,
            AgentStatus::Idle
            | AgentStatus::Asleep
            | AgentStatus::Stopped
            | AgentStatus::Starting => AgentGroup::Idle,
        }
    }
}

/// What an agent's turn is doing now, as its chat or the screen of its terminal shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum AgentState {
    /// Nothing has said yet.
    #[default]
    Unknown,
    Idle,
    Working,
    /// It waits on the person in the middle of a turn.
    Blocked,
    /// Its terminal's program ended.
    Stopped,
}

impl AgentState {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => AgentState::Idle,
            2 => AgentState::Working,
            3 => AgentState::Blocked,
            4 => AgentState::Stopped,
            _ => AgentState::Unknown,
        }
    }

    /// It is at work on a turn, or waits on the person in the middle of one.
    pub fn busy(self) -> bool {
        matches!(self, AgentState::Working | AgentState::Blocked)
    }
}

/// The app's `agent_state_changed` payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStateEvent {
    pub agent_id: String,
    pub state: AgentState,
    pub sequence: u64,
    pub source: String,
    pub confidence: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
}
