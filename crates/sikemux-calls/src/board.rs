use crate::params::given;
use crate::places::Passes;
use crate::places::Place;
use serde::{Deserialize, Serialize};
use sikemux_view::chat::TriggerWhen;
use sikemux_view::inbox::InboxRecording;
use sikemux_view::slice::Assertion;
use sikemux_view::slice::Collect;
use sikemux_view::slice::Decision;
use sikemux_view::slice::Gate;
use sikemux_view::slice::GoalEta;
use sikemux_view::slice::GoalReport;
use sikemux_view::slice::GoalRule;
use sikemux_view::slice::NamedFile;
use sikemux_view::slice::NextGate;
use sikemux_view::slice::ReportEvery;
use sikemux_view::slice::SliceState;
use sikemux_view::slice::SliceView;
use sikemux_view::slice::Spot;
use sikemux_view::turn::Landed;
use sikemux_wire::protocol::PermissionMode;
use std::collections::BTreeMap;

/// The chat a goal's button acts on.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGoalParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalReportsParams {
    pub project: String,
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Open,
    Progress,
    Done,
}

impl CardStatus {
    pub fn words(self) -> &'static str {
        match self {
            CardStatus::Open => "open",
            CardStatus::Progress => "in progress",
            CardStatus::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_lowercase().as_str() {
            "open" => Ok(CardStatus::Open),
            "progress" | "in progress" | "in_progress" => Ok(CardStatus::Progress),
            "done" => Ok(CardStatus::Done),
            _ => Err("status must be open, progress or done".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tracker {
    Jira,
    Linear,
}

/// The Jira or Linear ticket the person put a card on the board for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardTicket {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What the ticket is in its tracker, such as Epic, Story or Bug.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub provider: Tracker,
    /// The status last put on the ticket in its tracker, so each change goes
    /// there once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced: Option<CardStatus>,
    /// Why the last change did not reach the ticket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_note: Option<String>,
    /// Its tracker closed it while its slice had not landed, where the person
    /// lands slices, so the person decides what that means.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done_unlanded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCard {
    pub id: u64,
    pub title: String,
    pub details: String,
    pub acceptance: String,
    pub contract: Option<CardContract>,
    pub status: CardStatus,
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub owner_cli: Option<String>,
    /// The model provider paying for the owner's turns, when not its own account.
    pub paid_by: Option<String>,
    pub team: Option<String>,
    pub team_name: Option<String>,
    pub parent: Option<u64>,
    pub depends_on: Vec<u64>,
    pub waits_on: Vec<u64>,
    pub created_by: Option<String>,
    pub created_by_name: String,
    pub result: Option<String>,
    pub ticket: Option<CardTicket>,
    /// The ticket of the nearest card above it that has one, which it is filed under.
    pub ticket_above: Option<String>,
    pub column: BoardColumn,
    /// The question of its owner's it waits on the person to answer, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub auto: Option<CardRun>,
    /// Browser recordings, for a card that is not a slice; a slice's are in its evidence.
    pub recordings: Vec<InboxRecording>,
    /// For a slice of a goal: its promise, gates, evidence and notes.
    pub slice: Option<SliceView>,
    /// For a goal, its slices' cards in board order: those right under it,
    /// and its own card when it is a slice itself.
    #[serde(default)]
    pub goal_slices: Vec<u64>,
    /// It sits at the top of the board with slices, or with an agent working it or a card under
    /// it, as a chat started from Home does.
    #[serde(default)]
    pub is_goal: bool,
    /// A short name its summary gave a goal whose title is only its prompt cut short.
    #[serde(default)]
    pub display_title: Option<String>,
    /// For a goal, the gates its slices wait at, in the order they will be
    /// reached: a slice that waits on another comes after it.
    pub next: Vec<NextGate>,
    /// For a goal, how its slices stand together; none for a card with no
    /// slices under it.
    #[serde(default)]
    pub goal_state: Option<SliceState>,
    /// Every slice left in the goal lost its owner, so it isn't moving: the person's inbox says so.
    #[serde(default)]
    pub goal_stalled: bool,
    /// For a goal, when its work should be through and whether that slipped.
    #[serde(default)]
    pub eta: Option<GoalEta>,
    /// For a goal, how often its report is written; none writes none.
    #[serde(default)]
    pub report_every: Option<ReportEvery>,
    /// For a goal, its latest report; `goalReports` lists those before it.
    #[serde(default)]
    pub report: Option<GoalReport>,
    /// For a goal, what it decided, oldest first, retired ones too.
    pub decisions: Vec<Decision>,
    /// For a goal, rules only its own agents follow, oldest first, retired
    /// ones too.
    pub rules: Vec<GoalRule>,
    /// Its ticket or trigger arriving again while it was open, oldest first.
    pub arrivals: Vec<Arrival>,
    /// For a goal, what must hold once its slices are done, oldest first,
    /// retired ones too.
    pub assertions: Vec<Assertion>,
    pub at: u64,
    pub updated_at: u64,
    /// When it was put aside in Later, off the board until someone takes it up again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aside_at: Option<u64>,
}

/// Where a board shows a card: `needs` holds the cards waiting on the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoardColumn {
    Open,
    Progress,
    Needs,
    Done,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBoard {
    pub project: String,
    pub cards: Vec<BoardCard>,
    pub settings: BoardSettings,
    /// Places an agent proposed, waiting for the person's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places_proposal: Option<PlacesProposal>,
    /// How many of its cards have an agent of Auto's at work.
    pub running: usize,
    pub auto_note: Option<String>,
    /// What could not be done of what the person asked, a line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    /// The triggers that add cards to it when their time comes, soonest first.
    pub triggers: Vec<sikemux_view::chat::ChatTrigger>,
    /// When its drift audits run and what the latest found; none when it never had any.
    #[serde(default)]
    pub audit: Option<sikemux_view::audit::AuditView>,
}

/// The project a board call is about, by its absolute path.
#[derive(Serialize, Deserialize)]
pub struct BoardParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardAddParams {
    pub project: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    /// One sentence saying what it changes, which makes it a slice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promise: Option<String>,
    /// The goal's assertions it fulfils, as a slice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asserts: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<BoardTicketParams>,
    /// The agent in this project that holds it at once, such as the one
    /// started to do it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// The Jira or Linear ticket a card is put on the board for.
#[derive(Serialize, Deserialize)]
pub struct BoardTicketParams {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What the ticket is in its tracker, such as Epic, Story or Bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Jira when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Tracker>,
}

/// A ticket the person puts on a project's board, with the tickets under it and the keys
/// of the tickets it waits on and holds up.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardAddTicketParams {
    pub project: String,
    pub title: String,
    #[serde(default)]
    pub details: String,
    pub ticket: BoardTicketParams,
    #[serde(default)]
    pub subtasks: Vec<BoardTicketCard>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub blocks: Vec<String>,
}

/// A ticket under the one put on the board, made a card under its card.
#[derive(Serialize, Deserialize)]
pub struct BoardTicketCard {
    pub title: String,
    #[serde(default)]
    pub details: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<BoardTicketParams>,
}

/// What the person changes of a card; what it leaves out stays as it is.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardUpdateParams {
    pub project: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    /// One sentence saying what it changes, which makes it a slice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promise: Option<String>,
    /// The goal's assertions it fulfils, as a slice; the person alone takes
    /// one off a slice that has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asserts: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<CardStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The cards it waits on, none to wait on nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<u64>>,
    /// The card it goes under, 0 for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct BoardCardParams {
    pub project: String,
    pub id: u64,
}

/// The folder environment a released client removes, from a project under it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetInheritedParams {
    pub project: String,
    pub folder: String,
    pub name: String,
}

/// Work that arrived again for a card that already held it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrival {
    /// The ticket's, trigger's or CI failure's title.
    pub title: String,
    /// What came with it, such as a webhook's request or what CI printed, cut
    /// to what a card keeps.
    pub details: String,
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEveryParams {
    pub project: String,
    /// An `every` of an hour or more, or `daily`; none stops them.
    #[serde(default)]
    pub every: Option<TriggerWhen>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditNowParams {
    pub project: String,
}

pub const DEFAULT_SLOTS: u32 = 2;

/// Who passes a slice's Landed gate in a project, and so whether anything
/// lands before the person says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Landing {
    /// Sikemux, once the slice's commits are on the branch; Auto merges its
    /// pull request itself once its checks pass and its reviewer approves.
    #[default]
    Core,
    /// The person: nothing counts as landed until they pass it, and Auto
    /// leaves its pull request open for them to merge.
    Person,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewerChoice {
    /// The app's launcher it starts with, which must run another CLI than
    /// the card's builder.
    pub launcher: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// How a project's board runs on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardSettings {
    pub auto: bool,
    /// How many of Auto's agents work at once.
    pub slots: u32,
    /// The app's launcher Auto's agents start with; none for the first that works.
    pub launcher: Option<String>,
    /// The connected provider its agents run on in place of their own accounts.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub landing: Landing,
    /// What counts as landed, as the person set it; none reads it from the project's history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<Landed>,
    /// Who reviews each change before it merges; none for any signed-in
    /// agent on another CLI than its builder's.
    pub reviewer: Option<ReviewerChoice>,
    /// The project's places, as its place file keeps them, in the order its slices reach
    /// those that give them a step.
    #[serde(skip)]
    pub places: Vec<Place>,
    #[serde(skip)]
    pub places_confirmed: Option<u64>,
    /// Whole dollars of Auto's spending in the project past which it starts no new cards.
    pub spend_ceiling: Option<u32>,
    /// The linked hosts cards go to past this host's places, by key, the first with room first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

impl BoardSettings {
    pub fn place(&self, name: &str) -> Option<&Place> {
        self.places.iter().find(|known| known.shown.name == name)
    }

    pub fn passes(&self, name: &str) -> Option<Passes> {
        let confirmed = self.places_confirmed.is_some();
        self.place(name).map(|place| place.passes(confirmed))
    }

    /// What it means for the place called `name`, in the core's words.
    pub fn passes_words(&self, name: &str) -> String {
        match self.passes(name) {
            Some(Passes::Nobody) => format!("{name} goes freely"),
            Some(Passes::Check) => format!("{name} goes once the project's check passes"),
            Some(Passes::Person) => format!("{name} waits for the person's yes"),
            None => format!("{name} is no longer one of the project's, so it waits for a person"),
        }
    }

    /// Who passes the place called `name`, under its step on a ladder.
    pub fn step_words(&self, name: &str) -> &'static str {
        match self.passes(name) {
            Some(Passes::Nobody) => "Goes freely",
            Some(Passes::Check) => "Once the check passes",
            Some(Passes::Person) | None => "The person",
        }
    }

    /// What a card's run record compares runs by.
    pub fn switches(&self) -> BTreeMap<String, bool> {
        BTreeMap::from([("coreLands".to_owned(), self.lands_itself())])
    }

    /// Whether Auto merges the project's changes itself, and so agrees a contract first, keeps
    /// each card's commits on the remote and queues its merges. A person who lands the changes
    /// merges them as they please, with nothing of Auto's pushed.
    pub fn lands_itself(&self) -> bool {
        self.landing == Landing::Core
    }

    /// Where a deploy's health is read after it, for the place called `name`.
    pub fn health(&self, name: &str) -> Option<String> {
        self.place(name)?.health.clone()
    }
}

impl Default for BoardSettings {
    fn default() -> Self {
        Self {
            auto: false,
            slots: DEFAULT_SLOTS,
            launcher: None,
            provider: None,
            model: None,
            landing: Landing::Core,
            landed: None,
            reviewer: None,
            places: Vec::new(),
            places_confirmed: None,
            spend_ceiling: None,
            hosts: Vec::new(),
        }
    }
}

/// What the person changes of how a project's board runs on its own; what it leaves out
/// stays as it is, and a null clears it.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSettingsParams {
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<u32>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub launcher: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub reviewer: Option<Option<BoardReviewerParams>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<Landing>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub landed: Option<Option<Landed>>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub spend_ceiling: Option<Option<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct BoardReviewerParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The step of Auto's own work a card's run is at. Its slice and `held` say
/// what waits on the person and whether the work landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStage {
    /// Its worktree and agent are being made.
    Starting,
    Working,
    /// An agent on another CLI reviews what its builder proposes to build.
    Contracting,
    /// Its work is being committed, pushed and put in a pull request.
    Shipping,
    /// Its pull request's checks are running.
    Checking,
    /// Its checks passed, and an agent on another CLI reviews the change
    /// blind before it merges.
    Reviewing,
    /// Its agent was asked to fix what failed.
    Fixing,
    Merging,
    /// Its pull request is open, and Auto watches it for the person to merge
    /// it or comment on it.
    Watching,
}

/// What the person is needed for once Auto gives a card back to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunHold {
    /// Something failed that needs mending.
    Fix,
    /// Something only the person can do, such as starting its agent again.
    Unblock,
}

impl RunStage {
    /// Its agent is at work, so it takes one of Auto's places.
    pub fn busy(self) -> bool {
        matches!(
            self,
            RunStage::Starting | RunStage::Working | RunStage::Contracting | RunStage::Fixing
        )
    }

    /// A turn of its agent ending sends its work on.
    pub fn ships_on_turn_end(self) -> bool {
        matches!(
            self,
            RunStage::Working | RunStage::Fixing | RunStage::Watching
        )
    }
}

/// A card's run as the app and devices read it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRun {
    pub agent: String,
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    pub base: String,
    pub start_sha: String,
    pub stage: RunStage,
    /// Auto gave the card back to the person, who is needed for this.
    pub held: Option<RunHold>,
    /// Auto gave it back, or its pull request waits for them.
    pub waits_on_person: bool,
    pub note: Option<String>,
    pub pull: Option<CardPull>,
}

/// A run's pull request as the app and devices read it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardPull {
    pub number: u64,
    pub url: String,
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractTerm {
    pub behaviour: String,
    pub verify: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContractState {
    /// Its builder has not proposed one yet.
    Awaited,
    /// Its builder ended a turn without one and was reminded.
    Reminded,
    /// A reviewer has the latest proposal.
    Reviewing,
    /// A reviewer objected and its builder is proposing again.
    Revising,
    Agreed,
    /// The latest proposal stands without a reviewer's agreement, for the reason on record.
    Stands,
    /// Its builder never proposed one.
    Skipped,
}

/// A card's contract as the app and devices read it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardContract {
    pub terms: Vec<ContractTerm>,
    pub state: ContractState,
    pub rounds: u32,
    pub why: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideParams {
    pub project: String,
    pub card: u64,
    /// The decision to correct or retire; none to record a new one.
    pub decision: Option<u64>,
    pub text: Option<String>,
    pub why: Option<String>,
    #[serde(default)]
    pub retire: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverruleParams {
    pub project: String,
    /// The goal whose log keeps the decision; none for its project's own log.
    pub goal: Option<u64>,
    pub decision: u64,
    /// What the person wants instead, when they said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instead: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOverruleParams {
    pub project: String,
    pub goal: Option<u64>,
    pub decision: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerCommandParams {
    pub project: String,
    pub card: u64,
    pub assertion: u64,
    /// The command the person saw, which an answer holds for only while the assertion names it.
    pub command: String,
    /// The changed files the person saw, which it holds for only while these are what wait.
    #[serde(default)]
    pub files: Vec<NamedFile>,
    pub accept: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertParams {
    pub project: String,
    pub card: u64,
    /// The assertion to correct or retire; none to write a new one.
    pub assertion: Option<u64>,
    pub text: Option<String>,
    pub evidence: Option<Collect>,
    #[serde(default)]
    pub retire: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardHandOffParams {
    pub project: String,
    /// What the person typed, which the agent is sent and the slice promises.
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The app's launcher the agent starts with; none for the first that works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The connected provider it runs on in place of its own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Whether it asks before it acts; none for what its launcher does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Where the person hands a slice whose owner stalled.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandOnParams {
    pub project: String,
    pub id: u64,
    /// The agent that takes it; none starts a new one in a checkout of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The app's launcher a new one starts with; none for the first that works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
}

/// The slice that tracks the work, and the agent doing it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandedOff {
    pub card: u64,
    pub agent: String,
    pub board: ProjectBoard,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEveryParams {
    pub project: String,
    pub id: u64,
    /// None stops its reports.
    #[serde(default)]
    pub every: Option<ReportEvery>,
}

#[derive(Serialize, Deserialize)]
pub struct BoardPassParams {
    pub project: String,
    pub card: u64,
    pub gate: Gate,
    /// The project's environment `gate` stands for; a step that is one passes only by this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// A change the person asks for on a slice, on a piece of its evidence when given, at `spot`
/// on it. With `reopens`, they say that change, ticked off, is not done, and `text` may say why.
#[derive(Serialize, Deserialize)]
pub struct BoardNoteParams {
    pub project: String,
    pub card: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spot: Option<Spot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopens: Option<u64>,
}

/// Places an agent proposed, for the person to accept or turn down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacesProposal {
    pub places: Vec<Place>,
    pub why: String,
    /// The agent that proposed them, by name.
    pub by: String,
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardProposalParams {
    pub project: String,
    pub accept: bool,
}
