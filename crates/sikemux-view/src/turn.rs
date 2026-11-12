//! How an agent's turn stands, worked out once by the core and sent on every agent row and chat
//! head. Clients word and draw it; none works out a state of its own.

use serde::{Deserialize, Serialize};

use crate::chat::FailureKind;

/// How an agent's turn stands now, and the facts each state carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnState {
    pub state: TurnPhase,
    /// When it came to stand so, in Unix milliseconds.
    pub since: u64,
    /// While working: the step of its plan it is on, of a plan it said this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<TurnWait>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<TurnCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<TurnDone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<TurnStopped>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<TurnFailed>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnPhase {
    Working,
    /// Its turn ended on something set to wake it.
    Waiting,
    /// It waits on the person to answer or allow something.
    NeedsYou,
    Done,
    /// Its turn ended with work left open and nothing set to wake it.
    Stopped,
    Failed,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// What a waiting agent waits on, and how long that usually takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWait {
    pub kind: TurnWaitKind,
    /// What it waits on, in words a line can show: an agent's name, a process, a timer's title.
    pub what: String,
    pub started_at: u64,
    /// How long the last few waits on the same thing took, once there were three.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_ms: Option<u64>,
    /// When a timer fires, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnWaitKind {
    Ci,
    Deploy,
    Agent,
    Timer,
    Process,
    Check,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// What the agent waits on the person for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCall {
    /// The question's or the permission request's id, which answering it names.
    pub id: String,
    pub question: String,
    pub waiting_since: u64,
    /// How many things it waits on the person for, this one included.
    pub count: u32,
}

/// Where a finished turn left the work, how its checks stand, and what can be done next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDone {
    pub changes: WorkAt,
    pub checks: ChecksStand,
    /// How many tests passed, when the check said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<u32>,
    /// Once shipped: whether the place's health check passed after it went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    /// The next steps by the project's landing rule, soonest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<NextStep>,
}

/// How far the agent's changes have gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkAt {
    /// It changed nothing, or what it changed is gone.
    None,
    Uncommitted,
    Committed,
    /// On the project's branch, as the project's landing rule takes it.
    Landed,
    Staged,
    Shipped,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChecksStand {
    Pass,
    Failed,
    /// No run of the project's check is kept for where the work is now.
    NotChecked,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextStep {
    pub step: TurnStep,
    /// The one step to draw as the main button: the next that needs no call.
    pub primary: bool,
}

/// A step the person can take on an agent's work with one click, by `takeStep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStep {
    Commit,
    Push,
    DeployStaging,
    /// Raises the production call rather than shipping.
    ShipProduction,
    RollBack,
    TryIt,
    /// Tells a stopped agent to go on with what is left.
    CarryOn,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStopped {
    /// What is left open, in the agent's own words, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFailed {
    pub why: FailureKind,
    /// What the agent or its driver said went wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// When the account's usage limit resets, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub can: Vec<Remedy>,
}

/// What the person can do about a failed turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Remedy {
    OtherAccount,
    /// Send the turn again: now, or at `resetsAt` when there is one.
    Retry,
    Reconnect,
    SignIn,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// What counts as landed in a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Landed {
    Committed,
    Pushed,
    Merged,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// What landing means in a project, and how its latest changes reached its branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandedMeans {
    pub means: Landed,
    /// The person set it, rather than the core reading it from `history`.
    pub chosen: bool,
    /// None for a project with no commits on its branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<LandingHistory>,
}

/// How most of a project's latest changes reached its branch: `count` of the last `of` were
/// `means`, such as 41 of the last 50 pushed straight to main.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandingHistory {
    pub means: Landed,
    pub count: u32,
    pub of: u32,
    pub branch: String,
}
