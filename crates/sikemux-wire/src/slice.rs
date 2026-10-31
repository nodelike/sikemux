//! A slice of a goal as the core judges it: one claim, the gates it passes
//! on its way to done, and the evidence the core collected itself on the way.
//! A slice is a board card under its goal's card; what kind of work it is is
//! never kept, only what is attached to it.

use serde::{Deserialize, Serialize};

/// A step a slice passes on its way to done, in the order a ladder lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Gate {
    /// Its owner says it is done, and the project's check passed on its latest commit.
    Built,
    /// The person looked at its evidence and it looks right.
    LooksRight,
    /// Its commits are on the goal's branch.
    Landed,
}

/// Who may pass a gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Passer {
    /// The agent that owns the slice, with what the core checks first.
    Owner,
    Person,
    /// The core, once what the gate needs is so.
    Core,
}

/// Where a gate of a slice's ladder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateState {
    Passed,
    /// The first gate not passed: what the slice is at now.
    Waiting,
    /// After the one waiting.
    Ahead,
}

/// A gate of a slice's ladder as clients draw it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub gate: Gate,
    pub passer: Passer,
    pub state: GateState,
    /// The agent that passed it, or that may pass it as the owner; none for
    /// the person and the core.
    pub agent: Option<String>,
    pub agent_name: Option<String>,
    /// When it was passed.
    pub at: Option<u64>,
    /// What is not so yet that it needs before it can pass.
    pub needs: Vec<Need>,
}

/// What a gate needs before it can pass, besides the gates before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Need {
    /// The project's check to pass on the slice's latest commit.
    PassingCheck,
    /// The slice's commits to be on the goal's branch.
    OnBranch,
}

/// Whether evidence held up when the core checked it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Passed,
    Failed,
}

/// Something the core collected itself to show a slice's work, toward the
/// gate the slice was at then. With a verdict it is a check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// The same for as long as the slice keeps it.
    pub id: u64,
    pub gate: Gate,
    pub at: u64,
    /// The agent whose work it shows, which notes pinned to it go to.
    pub agent: Option<String>,
    pub verdict: Option<Verdict>,
    pub proof: Proof,
}

/// The five kinds of evidence, each drawn one way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Proof {
    /// A screenshot, or a recording when it has a length.
    Picture {
        path: String,
        seconds: Option<f64>,
        /// The chat run and turn it was made in.
        feed: Option<String>,
        turn: Option<u64>,
    },
    /// A change between two commits.
    Diff {
        base: String,
        head: String,
        /// The files it changes, the first few.
        files: Vec<String>,
        added: u64,
        removed: u64,
    },
    /// Measures before and after, such as seconds or cost.
    Numbers { measures: Vec<Measure> },
    /// A command and what it printed, such as the project's check.
    Run {
        command: String,
        /// The commit it ran on.
        sha: Option<String>,
        /// The end of what it printed.
        output: String,
    },
    /// Logs and events in order.
    Timeline { moments: Vec<Moment> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measure {
    pub label: String,
    pub before: Option<String>,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Moment {
    pub at: u64,
    pub text: String,
}

/// What someone wrote about a slice, pinned to a piece of its evidence when
/// it was written on one, for the agent whose work that is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub text: String,
    /// The agent that wrote it; none for the person.
    pub by: Option<String>,
    pub evidence: Option<u64>,
    pub at: u64,
}

/// A gate one of a goal's slices waits at, as the goal lists what comes next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextGate {
    /// The slice's card.
    pub card: u64,
    pub gate: Gate,
    pub passer: Passer,
}
