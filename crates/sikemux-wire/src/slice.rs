//! A slice of a goal as the core judges it: one promise, the gates it passes
//! on its way to done, and the evidence the core collected itself on the way.
//! A slice is a board card under its goal's card; what kind of work it is is
//! never kept, only what is attached to it.

use serde::{Deserialize, Serialize};

use crate::memory::Learned;

/// A step a slice passes on its way to done. A slice's ladder lists them in
/// the order it passes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Gate {
    /// Its owner says it is done, and the project's check passed on its latest commit.
    Built,
    /// The person looked at its evidence and it looks right.
    LooksRight,
    /// Its commits are on the project's branch.
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
    /// The slice's commits to be on the project's branch.
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
    /// The change from `base` to `head`, which are the ends of a run of the
    /// slice's own commits with nobody else's between them.
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

/// A slice as clients draw it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceView {
    /// One sentence saying what it changes.
    pub promise: String,
    pub state: SliceState,
    /// The gates it passes on its way to done, in order.
    pub gates: Vec<GateView>,
    /// What the core collected itself to show its work, oldest first.
    pub evidence: Vec<Evidence>,
    pub notes: Vec<Note>,
}

/// Where a slice stands as a whole, so clients mark it without working it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SliceState {
    /// Every gate of its ladder passed.
    Done,
    /// The latest check toward the gate it waits at failed.
    Failed,
    /// The gate it waits at is the person's, and nothing it needs is missing.
    NeedsYou,
    /// It waits on another card, or nobody holds it.
    Waiting,
    Moving,
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

/// What a goal decided, kept on its card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// The same for as long as the goal keeps it, never given to a decision
    /// or rule of the goal again.
    pub id: u64,
    pub text: String,
    pub why: Option<String>,
    pub learned: Learned,
    /// What it said before it was last corrected.
    pub was: Option<String>,
    /// Who retired it and when. A full goal drops its oldest retired
    /// decisions first.
    pub retired: Option<Learned>,
}

/// A rule only a goal's own agents follow, kept on its card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalRule {
    /// The same for as long as the goal keeps it, never given to a decision
    /// or rule of the goal again.
    pub id: u64,
    pub text: String,
    pub learned: Learned,
    /// What it said before it was last corrected.
    pub was: Option<String>,
    /// Who retired it and when. A full goal drops its oldest retired rules
    /// first.
    pub retired: Option<Learned>,
}
