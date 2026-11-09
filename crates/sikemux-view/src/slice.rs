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
    /// The person looked at what a goal's work shows, once for the whole goal, and it looks right.
    /// No slice's ladder has it; a goal's card waits at it.
    LooksRight,
    /// Its commits are on the project's branch.
    Landed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Passer {
    /// The agent that owns the slice, with what the core checks first.
    Owner,
    Person,
    /// The core, once what the gate needs is so.
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateState {
    Passed,
    /// The first gate not passed: what the slice is at now.
    Waiting,
    /// After the one waiting.
    Ahead,
}

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
    /// The project's environment this step is, by name. A client that knows no
    /// environments reads such a step's `gate` as Landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Who an environment's step waits on, in the core's few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
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
    /// Every change the person asked for on it to be ticked off by the agent it went to.
    Changes,
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
    /// The project's environment it was collected toward, as `GateView` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
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

/// A change the person asked for on a slice, pinned to a piece of its evidence when it was
/// written on one, for the agent whose work that is, which ticks it off with proof.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// The same for as long as the slice keeps it.
    pub id: u64,
    pub text: String,
    /// The agent that wrote it; none for the person.
    pub by: Option<String>,
    pub evidence: Option<u64>,
    /// Where on that evidence it is pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spot: Option<Spot>,
    pub at: u64,
    /// The agent it went to, which ticks it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticked: Option<Tick>,
}

/// Where a change is pinned on a piece of evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Spot {
    /// A point on a picture, as fractions of its width and height from its top left; on a
    /// recording, in the frame `seconds` in.
    Point {
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<f64>,
    },
    /// A moment of a recording, `seconds` in.
    Moment { seconds: f64 },
    /// A line of a file in a diff, as the new side numbers it.
    Line { file: String, line: u32 },
}

/// How the agent a change went to ticked it off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tick {
    pub by: String,
    pub at: u64,
    /// What it did and what shows it.
    pub proof: String,
    /// The piece of the slice's evidence that shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<u64>,
}

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
    /// The piece of it the gate waiting leads with, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leading: Option<u64>,
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
    /// The address of the pull request its work is reviewed in, once the core found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
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
    /// Another card it waits on, neither done nor built on the project's branch yet.
    Card { card: u64 },
    /// The person, to pass `gate`, or the environment it stands for.
    Person {
        gate: Gate,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<String>,
    },
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SliceState {
    /// Every gate of its ladder passed.
    Done,
    /// The latest check toward the gate it waits at failed.
    Failed,
    /// The gate it waits at is the person's, and nothing it needs is missing.
    NeedsYou,
    /// It waits on another card, or nobody holds it.
    #[default]
    Waiting,
    Moving,
}

/// A gate one of a goal's slices waits at, as the goal lists what comes next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextGate {
    pub card: u64,
    pub gate: Gate,
    pub passer: Passer,
    #[serde(default)]
    pub wait: Option<Wait>,
    /// The project's environment this step is, as `GateView` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// How often a goal's report is written: every so many minutes, or whenever the goal moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReportEvery {
    Minutes { minutes: u32 },
    Change,
}

/// How a goal stood when the core wrote it down, in one fixed shape: what waits on the person,
/// what is done, what moves, what waits on something else, what comes next and when it should be
/// through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalReport {
    pub at: u64,
    /// When the report before it was written; none for the first.
    #[serde(default)]
    pub since: Option<u64>,
    pub needs_you: Vec<ReportLine>,
    /// Done since the report before it, or all done for the first.
    pub done: Vec<ReportLine>,
    pub moving: Vec<ReportLine>,
    pub waiting: Vec<ReportLine>,
    pub next: Vec<NextGate>,
    #[serde(default)]
    pub eta: Option<GoalEta>,
    /// When the report before it said the goal would be through.
    #[serde(default)]
    pub eta_before: Option<u64>,
}

/// One slice in a goal's report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportLine {
    pub card: u64,
    pub promise: String,
    #[serde(default)]
    pub owner_name: Option<String>,
    pub state: SliceState,
    /// The gate it waits at; none once it is done.
    #[serde(default)]
    pub gate: Option<Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default)]
    pub wait: Option<Wait>,
    /// When it got where it is: since its wait began, or when it was done.
    #[serde(default)]
    pub at: Option<u64>,
    /// It was not here in the report before.
    #[serde(default)]
    pub new: bool,
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
    /// What was being decided, when it settled a question an agent asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
    /// The agent that goes by it, which hears when the person overrules it; none for the
    /// person's own decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
}

impl Decision {
    /// What was decided in a line: the question it settled, when there was one, and the outcome.
    pub fn says(&self) -> String {
        match &self.asked {
            Some(asked) => format!("{asked} → {}", self.text),
            None => self.text.clone(),
        }
    }
}

/// One of a goal's assertions a slice fulfils.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asserted {
    pub assertion: u64,
    /// Its evidence held up on the slice's latest commit.
    pub met: bool,
    /// Its command waits for the person to approve it, and does not run until they do.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub awaits_approval: bool,
    /// Its command does not run on the slice's latest commit, which holds a file it names as the
    /// person turned it down.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub turned_down: bool,
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
    /// The command the person wrote or last approved for it. A command in `evidence` counts only
    /// while it is exactly this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<String>,
    /// The repository files its command names, as the person accepted them.
    #[serde(default, skip_serializing_if = "CommandFiles::is_empty")]
    pub files: CommandFiles,
}

impl Assertion {
    /// The command it names that the person has not approved.
    pub fn awaits_approval(&self) -> Option<&str> {
        match self.command() {
            Some(command) if self.approved.as_deref() != Some(command) => Some(command),
            _ => None,
        }
    }

    pub fn command(&self) -> Option<&str> {
        match &self.evidence {
            Collect::Command { command } => Some(command),
            _ => None,
        }
    }

    /// It waits on the person: for its command, or for a file its command names that changed.
    pub fn needs_person(&self) -> bool {
        self.awaits_approval().is_some() || !self.files.changed.is_empty()
    }
}

/// A file in the repository, as a commit holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedFile {
    pub path: String,
    /// The git id of its content; none when the commit does not hold it.
    pub blob: Option<String>,
}

/// A file a command names that a slice's latest commit holds differently from what the person
/// accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub card: u64,
    pub head: String,
    pub file: NamedFile,
}

/// What the person has said of the repository files an assertion's command names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandFiles {
    /// Each content they accepted, along with the command or since.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<NamedFile>,
    /// As they stood when an agent wrote the command, accepted along with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed: Vec<NamedFile>,
    /// What waits for them to accept or turn down.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ChangedFile>,
    /// Each content they turned down, which the command does not run on and they are not asked
    /// of again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turned_down: Vec<NamedFile>,
    /// The slices whose latest commit holds a content they turned down, which wait until it
    /// changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<ChangedFile>,
}

impl CommandFiles {
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
            && self.proposed.is_empty()
            && self.changed.is_empty()
            && self.turned_down.is_empty()
            && self.refused.is_empty()
    }
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
    /// `command` exiting with success, run in a clean checkout of the slice's latest commit.
    Command { command: String },
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

/// Which goal's summary a client reads. `refresh` looks at the goal again and writes a new one
/// if it changed, as when its page opens; `again` also tries once more after a failed attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSummaryParams {
    pub project: String,
    pub card: u64,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub again: bool,
}

/// A goal's summary as the core last wrote it, and how writing the next one goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSummaryView {
    pub summary: Option<GoalSummary>,
    /// A model is writing a newer one now.
    pub writing: bool,
    /// Why the latest attempt wrote nothing; a summary from before it may still stand.
    pub failed: Option<String>,
}

/// What happened on a goal and what was decided, from what its agents really did. Every line
/// comes with the moments behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSummary {
    pub at: u64,
    /// When the latest moment it was written from happened.
    pub through: u64,
    pub happened: Vec<SummaryLine>,
    /// Decisions a model found in what was said, which nobody recorded.
    pub extracted: Vec<SummaryLine>,
    /// Sikemux ran no check of its own among these moments, so they rest on what agents said.
    pub unchecked: bool,
    /// Agents whose transcripts were not read, each with why.
    #[serde(default)]
    pub left_out: Vec<LeftOut>,
}

/// One sentence of a summary and the moments it rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryLine {
    pub text: String,
    pub sources: Vec<SummarySource>,
}

/// A moment a sentence rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySource {
    pub kind: SourceKind,
    pub at: u64,
    /// The agent it comes from; none for the person and for the core.
    pub who: Option<String>,
    pub text: String,
    pub open: Option<SourceLink>,
}

/// The kinds of moment a summary is written from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// What the person typed or answered.
    Person,
    /// What an agent said when its turn ended.
    Answer,
    /// A check an agent ran with its own tools, which only it reported on.
    Ran,
    Commit,
    /// Evidence the core collected itself.
    Evidence,
    Gate,
    /// An action that leaves this machine, held for the person.
    Held,
    Decision,
    Assertion,
    /// What the person wrote about a slice.
    Note,
    /// Where a pull request stands, as its code host says.
    PullRequest,
    /// An agent's turn that ended in an error.
    TurnFailed,
}

/// Where a moment can be opened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceLink {
    /// The agent's chat; `feed` and `index` place it when the core could. Once the agent has
    /// closed, `saved` names the session that opens it again.
    Chat {
        agent: String,
        feed: Option<String>,
        index: Option<u64>,
        saved: Option<SavedChat>,
    },
    Commit {
        sha: String,
    },
    Evidence {
        card: u64,
        evidence: u64,
    },
    Card {
        card: u64,
    },
    Url {
        url: String,
    },
}

/// A chat's session as its CLI saved it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedChat {
    pub provider: String,
    pub session: String,
}

/// An agent of a goal whose transcript no model was given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftOut {
    pub name: String,
    pub why: LeftOutWhy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LeftOutWhy {
    /// It runs on another vendor's agent than the one asked, and its words stay with its own.
    OtherVendor,
    /// No session file of it could be read.
    NoSession,
}
