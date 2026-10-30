//! Where an agent stands for the person, and how the workspace shows it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::SessionId;

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

impl From<crate::pty::agent_detection::AgentDetectionState> for AgentState {
    fn from(state: crate::pty::agent_detection::AgentDetectionState) -> Self {
        use crate::pty::agent_detection::AgentDetectionState;
        match state {
            AgentDetectionState::Unknown => AgentState::Unknown,
            AgentDetectionState::Idle => AgentState::Idle,
            AgentDetectionState::Working => AgentState::Working,
            AgentDetectionState::Blocked => AgentState::Blocked,
        }
    }
}

/// The checkout an agent was given, as its workspace record keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorktree {
    /// The main checkout, which git commands about the worktree run against.
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The branch the project was on when the worktree was cut, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub start_sha: String,
}

/// The character an agent wears: a pack and shape when the person picked one,
/// and a slot in the pack's palette, which the core gives out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBuddy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

/// An agent in the workspace, kept whether or not it runs now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgent {
    pub id: String,
    /// The CLI it runs, such as `claude`.
    pub provider: String,
    /// What the core calls it, such as Otter, open or asleep. The core fills
    /// it in; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its project, by id.
    pub project: String,
    pub cwd: PathBuf,
    pub title: String,
    /// The person named it, so the agent's own titles no longer replace it.
    #[serde(default)]
    pub renamed: bool,
    /// The agent that started it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// The provider's session, which resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_id: Option<String>,
    pub permission_mode: String,
    /// The person's account it runs on, by profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model provider it runs on in place of its maker's account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Never put to sleep while idle.
    #[serde(default)]
    pub keep_alive: bool,
    /// Stopped, and woken again on its saved conversation when opened.
    #[serde(default)]
    pub asleep: bool,
    /// The git worktree it works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktree>,
    /// The character drawn for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buddy: Option<AgentBuddy>,
    /// The face it wears in place of its CLI's logo, from `buddy` and the
    /// person's face style; none while it shows the logo. The core fills it
    /// in; clients only read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<AgentFace>,
    /// The terminal session running it, for an agent in a terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_id: Option<SessionId>,
    /// It runs in its CLI in a terminal rather than as a chat, whether or not that terminal
    /// runs now. One whose terminal ended starts it again once a client shows it.
    #[serde(default)]
    pub terminal: bool,
    /// Unix milliseconds.
    #[serde(default)]
    pub created_at: u64,
}

/// The face an agent wears, drawn by each client from the same art.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFace {
    pub pack: String,
    pub shape: String,
    /// Which of the pack's colours, in the order its palette lists them.
    pub color: u32,
    /// The part of the shape's 24-unit square its drawing fills, as an SVG
    /// viewBox.
    #[serde(rename = "box")]
    pub view_box: String,
}

/// How agents' faces look, as the person picked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceStyle {
    /// Every agent wears a face; otherwise only one with a name does.
    pub on: bool,
    /// A pack's id, or `all` for a pack per agent.
    pub pack: String,
    /// A shape's id, or `mixed` for a shape per agent.
    pub shape: String,
    /// Faces move with what their agents are doing.
    pub motion: bool,
}

impl Default for FaceStyle {
    fn default() -> Self {
        Self {
            on: false,
            pack: "all".into(),
            shape: "mixed".into(),
            motion: true,
        }
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
