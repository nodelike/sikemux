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
    /// Every assertion the slice fulfils to have evidence of its kind, which
    /// held up, collected on the slice's latest commit.
    Assertions,
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
    /// The commit the slice's work was at when it was collected.
    #[serde(default)]
    pub commit: Option<String>,
    /// The goal's assertions it was collected for.
    #[serde(default)]
    pub assertions: Vec<u64>,
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
        /// The width the page was laid out at, in CSS pixels, for a screenshot
        /// the core took beside the agent's at a phone's or a tablet's width.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        /// The page the core opened itself to take it, once the check passed
        /// on a change to the project's pages; none for the agent's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opened: Option<String>,
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
    /// The goal's assertions it fulfils, each with whether its evidence is in.
    #[serde(default)]
    pub asserts: Vec<Asserted>,
    /// Assertions it named that no longer stand on its goal, so no longer bind it.
    #[serde(default)]
    pub detached: Vec<u64>,
    /// What the gate it waits at waits on now; none when it is done, or
    /// nothing is under way.
    #[serde(default)]
    pub wait: Option<Wait>,
}

/// What a slice waits on now, since when, and roughly how long it takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wait {
    pub on: WaitOn,
    pub since: u64,
    /// Roughly how long it takes from `since`, from what the core saw it take
    /// before; none when it has no idea.
    #[serde(default)]
    pub estimate_ms: Option<u64>,
    /// It has taken longer than `estimate_ms`.
    #[serde(default)]
    pub overdue: bool,
}

impl Wait {
    /// The moment it should end by, when the core has an idea.
    pub fn eta(&self) -> Option<u64> {
        self.estimate_ms.map(|estimate| self.since + estimate)
    }
}

/// When the work left in a goal should be through, not counting the time the
/// person takes at their own gates. The core promises one only from what it
/// saw the same work take before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalEta {
    /// When it should be through now; none once the core has no estimate left.
    pub at: Option<u64>,
    /// When the core first said it would be.
    pub promised: u64,
    /// A gate only the person passes is left once it is through.
    pub then_you: bool,
    /// Why it runs noticeably later than promised; none while it does not.
    pub slip: Option<Slip>,
}

/// Why a goal runs later than the core promised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Slip {
    /// The check on a slice's latest commit failed; `rerunning` once the next
    /// commit's check is on its way.
    CheckFailed { ci: bool, rerunning: bool },
    /// The goal got another slice.
    NewSlice,
    /// A slice whose work was checked got a new commit, which the check has to
    /// pass again.
    NewCommit,
    /// A slice waits on another card, which has no end in sight.
    OnCard { card: u64 },
    /// A slice waits on one the person has to pass first.
    OnPerson,
    /// A slice has nobody working on it.
    NoOwner,
    /// Something under way is taking longer than the same work did before.
    TakingLonger,
}

impl Slip {
    /// Why, as the end of "ETA slipped: …".
    pub fn words(self) -> String {
        match self {
            Self::CheckFailed { ci, rerunning } => {
                let check = if ci { "CI" } else { "the check" };
                let then = if rerunning { ", re-running" } else { "" };
                format!("{check} failed{then}")
            }
            Self::NewSlice => "a slice was added".to_owned(),
            Self::NewCommit => "a new commit needs checking again".to_owned(),
            Self::OnCard { card } => format!("waiting on #{card}"),
            Self::OnPerson => "waiting on you".to_owned(),
            Self::NoOwner => "a slice has no owner".to_owned(),
            Self::TakingLonger => "taking longer than it has before".to_owned(),
        }
    }
}

/// The kinds of thing a slice waits on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WaitOn {
    /// The project's check on its owner's latest commit.
    Check { sha: String, run: CheckRun },
    /// Another card it waits on, not done yet.
    Card { card: u64 },
    /// The person, to pass `gate`.
    Person { gate: Gate },
    /// Its owner, in the middle of a turn.
    Agent { agent: String },
}

/// How far a check on a commit has got.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CheckRun {
    /// It waits its turn to run.
    Queued,
    /// It runs here, in the project's integration checkout.
    Local,
    /// It runs in CI, as `workflow`, at `url` when the code host gives one.
    Ci {
        workflow: String,
        url: Option<String>,
    },
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
    /// What that gate waits on now.
    #[serde(default)]
    pub wait: Option<Wait>,
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

/// One of a goal's assertions a slice fulfils.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asserted {
    pub assertion: u64,
    /// Its evidence held up on the slice's latest commit.
    pub met: bool,
}

/// What must hold once a goal's slices are done, agreed before they are cut,
/// with the kind of evidence that shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    /// The same for as long as the goal keeps it, never given to a decision,
    /// rule or assertion of the goal again.
    pub id: u64,
    pub text: String,
    pub evidence: Collect,
    pub learned: Learned,
    /// What it said before it was last corrected.
    pub was: Option<String>,
    /// Who retired it and when.
    pub retired: Option<Learned>,
}

/// The evidence an assertion needs, which the core collects itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Collect {
    /// The project's check, passing on the slice's latest commit.
    Check,
    /// A browser recording made on the slice's latest commit.
    Recording,
    /// A screenshot of a page taken on the slice's latest commit.
    Screenshot,
    /// The CI workflow `name` passing on the pushed tip of the project's
    /// branch that holds the slice's latest commit.
    Workflow { name: String },
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
