//! Auto: for each card nobody holds and nothing holds up, whoever added it,
//! the board starts an agent of its own in a git worktree of its own, a
//! few at once, and ships its work when its turn ends.

use crate::server::changed::What;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::contract::{self, Contract};
use super::elsewhere::{self, RunElsewhere};
use super::environments::{Environment, Passes};
use super::guard::{self, Burn};
use super::lease::Stalled;
use super::record::RunRecord;
use super::recover::{self, Recovery, TookOn};
use super::review::Review;
use super::{blind, finish, goal, queue, risk, Board, Card, CardStatus, Owner, Team};
use crate::server::calls::given;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::spawn::Ending;
use crate::server::worktree::{self, AgentWorktree};
use crate::server::{chat, messages, Core};
use sikemux_wire::protocol::{ChatLaunch, Event};

pub(super) const MAX_SLOTS: u32 = crate::server::names::MAX_SPAWNED_PER_PROJECT as u32;
/// How many unfinished runs may stand above a card one of Auto's agents added
/// before that card rests until one of them finishes.
const MAX_RUNS_ABOVE: usize = 3;
/// How many unfinished cards one run may add for Auto to take.
pub(super) const MAX_CARDS_PER_RUN: usize = 16;
const DEFAULT_SLOTS: u32 = 2;
pub(super) const PICKED_IN: &str = "in the board's Auto settings";
const MAX_LAUNCHER_CHARS: usize = 256;
const MAX_MODEL_CHARS: usize = 200;

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
    /// Auto also takes cards agents added, once their team is through with them.
    pub agent_cards: bool,
    /// Who reviews each change before it merges; none for any signed-in
    /// agent on another CLI than its builder's.
    pub reviewer: Option<ReviewerChoice>,
    /// The reviewer builds and uses the change, rather than only reading it.
    pub reviewer_runs: bool,
    /// Each builder proposes a contract, reviewed blind, before it builds.
    pub contract: bool,
    /// Where the project deploys, in the order its slices reach them. None
    /// gives slices the usual ladder and holds every deploy for the person.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<Environment>,
    /// A card's agent that fails, dies or hangs is brought back without the person, and a
    /// stalled slice Auto can run is taken on.
    pub recover: bool,
    /// The project's `fastCheck`, when it names one, stands in for its check on most commits.
    pub fast_check: bool,
    /// A failed check is traced to the commit that broke it, and one agent mends it.
    pub culprit_finding: bool,
    /// A card that spends without progress, or keeps failing the same way, is held.
    pub progress_guard: bool,
    /// Whole dollars of Auto's spending in the project past which it starts no new cards.
    pub spend_ceiling: Option<u32>,
    /// A change merges only after its check passed on top of the latest base.
    pub merge_queue: bool,
    /// The project's `reservoir`, when it names one, keeps cards on the board.
    pub reservoir: bool,
    /// A small change away from anything sensitive gets a read review and no contract round.
    pub scaled_review: bool,
    /// Each card's commits are kept on the remote, so a lost worktree can be made again.
    pub branch_backup: bool,
    /// The linked hosts cards go to past this host's places, by key, the first with room first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

impl BoardSettings {
    fn environment(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|known| known.name == name)
    }

    pub(super) fn passes(&self, name: &str) -> Option<Passes> {
        self.environment(name).map(|environment| environment.passes)
    }

    /// What it means for the environment called `name`, in the core's words.
    pub(super) fn passes_words(&self, name: &str) -> String {
        match self.environment(name) {
            Some(environment) => environment.passes_words(),
            None => format!("{name} is no longer one of the project's, so it waits for a person"),
        }
    }

    /// Who passes the environment called `name`, under its step on a ladder.
    pub(super) fn step_words(&self, name: &str) -> &'static str {
        self.environment(name)
            .map_or("A person", Environment::step_words)
    }

    /// Each behaviour a project can turn off, by name, for a card's run record.
    pub(super) fn switches(&self) -> BTreeMap<String, bool> {
        [
            ("coreLands", self.landing == Landing::Core),
            ("agentCards", self.agent_cards),
            ("reviewerRuns", self.reviewer_runs),
            ("contract", self.contract),
            ("autoRecovery", self.recover),
            ("fastCheck", self.fast_check),
            ("culpritFinding", self.culprit_finding),
            ("progressGuard", self.progress_guard),
            ("mergeQueue", self.merge_queue),
            ("reservoir", self.reservoir),
            ("scaledReview", self.scaled_review),
            ("branchBackup", self.branch_backup),
        ]
        .map(|(name, on)| (name.to_owned(), on))
        .into()
    }

    /// Whether a card's builder agrees a contract first. A change the person
    /// lands themselves has no reviewer to grade it against one.
    fn wants_contract(&self) -> bool {
        self.contract && self.landing == Landing::Core
    }

    /// Whether Auto's merges of this project wait their turn. A person who
    /// lands the changes merges them as they please.
    /// Whether Auto keeps a card's commits on the remote. It pushes nothing of its own to a
    /// project the person lands themselves.
    pub(super) fn backs_up(&self) -> bool {
        self.branch_backup && self.landing == Landing::Core
    }

    pub(super) fn queues(&self) -> bool {
        self.merge_queue && self.landing == Landing::Core
    }

    /// Where a deploy's health is read after it, for the environment called `name`.
    pub(crate) fn health(&self, name: &str) -> Option<String> {
        self.environment(name)?.health.clone()
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
            agent_cards: true,
            reviewer: None,
            reviewer_runs: true,
            contract: true,
            environments: Vec::new(),
            recover: true,
            fast_check: true,
            culprit_finding: true,
            progress_guard: true,
            spend_ceiling: None,
            merge_queue: true,
            reservoir: true,
            scaled_review: false,
            branch_backup: true,
            hosts: Vec::new(),
        }
    }
}

fn known_provider(
    provider: Option<String>,
    knows: &impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    match provider {
        Some(id) if !knows(&id) => Err(format!("Sikemux does not know a provider called {id}")),
        provider => Ok(provider),
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
    pub reviewer_runs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<Landing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_cards: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<Environment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recover: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_check: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub culprit_finding: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_guard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaled_review: Option<bool>,
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub spend_ceiling: Option<Option<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_queue: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservoir: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_backup: Option<bool>,
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

impl BoardReviewerParams {
    fn choice(self, knows: &impl Fn(&str) -> bool) -> Result<ReviewerChoice, String> {
        let launcher = text("launcher", self.launcher, MAX_LAUNCHER_CHARS)?
            .ok_or("reviewer.launcher must name the agent that reviews")?;
        Ok(ReviewerChoice {
            launcher,
            provider: known_provider(text("provider", self.provider, MAX_LAUNCHER_CHARS)?, knows)?,
            model: text("model", self.model, MAX_MODEL_CHARS)?,
        })
    }
}

/// The person's `text` for `key`, none when blank.
fn text(key: &str, text: Option<String>, max: usize) -> Result<Option<String>, String> {
    match text {
        Some(text) if text.trim().is_empty() => Ok(None),
        Some(text) if text.len() > max => Err(format!("{key} must be text of at most {max} bytes")),
        text => Ok(text.map(|text| text.trim().to_owned())),
    }
}

impl BoardSettings {
    /// These settings with what `params` changes, naming only providers Sikemux `knows`.
    pub(super) fn changed(
        &self,
        params: BoardSettingsParams,
        knows: impl Fn(&str) -> bool,
    ) -> Result<Self, String> {
        let mut settings = self.clone();
        if let Some(auto) = params.auto {
            settings.auto = auto;
        }
        if let Some(slots) = params.slots {
            if !(1..=MAX_SLOTS).contains(&slots) {
                return Err(format!("slots must be from 1 to {MAX_SLOTS}"));
            }
            settings.slots = slots;
        }
        if let Some(launcher) = params.launcher {
            settings.launcher = text("launcher", launcher, MAX_LAUNCHER_CHARS)?;
        }
        if let Some(provider) = params.provider {
            settings.provider =
                known_provider(text("provider", provider, MAX_LAUNCHER_CHARS)?, &knows)?;
        }
        if let Some(model) = params.model {
            settings.model = text("model", model, MAX_MODEL_CHARS)?;
        }
        if let Some(reviewer) = params.reviewer {
            settings.reviewer = reviewer
                .map(|reviewer| reviewer.choice(&knows))
                .transpose()?;
        }
        if let Some(reviewer_runs) = params.reviewer_runs {
            settings.reviewer_runs = reviewer_runs;
        }
        if let Some(contract) = params.contract {
            settings.contract = contract;
        }
        if let Some(landing) = params.landing {
            settings.landing = landing;
        }
        if let Some(agent_cards) = params.agent_cards {
            settings.agent_cards = agent_cards;
        }
        if let Some(environments) = params.environments {
            settings.environments = super::environments::checked(environments)?;
        }
        if let Some(recover) = params.recover {
            settings.recover = recover;
        }
        if let Some(fast_check) = params.fast_check {
            settings.fast_check = fast_check;
        }
        if let Some(culprit_finding) = params.culprit_finding {
            settings.culprit_finding = culprit_finding;
        }
        if let Some(progress_guard) = params.progress_guard {
            settings.progress_guard = progress_guard;
        }
        if let Some(scaled_review) = params.scaled_review {
            settings.scaled_review = scaled_review;
        }
        if let Some(spend_ceiling) = params.spend_ceiling {
            if spend_ceiling == Some(0) {
                return Err("spendCeiling must be at least 1 dollar".into());
            }
            settings.spend_ceiling = spend_ceiling;
        }
        if let Some(merge_queue) = params.merge_queue {
            settings.merge_queue = merge_queue;
        }
        if let Some(branch_backup) = params.branch_backup {
            settings.branch_backup = branch_backup;
        }
        if let Some(reservoir) = params.reservoir {
            settings.reservoir = reservoir;
        }
        if let Some(hosts) = params.hosts {
            settings.hosts = elsewhere::checked_hosts(hosts)?;
        }
        Ok(settings)
    }
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
    pub(super) fn busy(self) -> bool {
        matches!(
            self,
            RunStage::Starting | RunStage::Working | RunStage::Contracting | RunStage::Fixing
        )
    }

    /// A turn of its agent ending sends its work on.
    fn ships_on_turn_end(self) -> bool {
        matches!(
            self,
            RunStage::Working | RunStage::Fixing | RunStage::Watching
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoPull {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub url: String,
    pub account: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AutoRun {
    pub agent: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub worktree: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub start_sha: String,
    pub stage: RunStage,
    /// Auto gave the card back to the person at `stage`, for the reason in `note`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<RunHold>,
    /// What is happening, or why the card is back with the person.
    pub note: Option<String>,
    pub pull: Option<AutoPull>,
    /// The commit whose checks are awaited, and since when.
    pub head: Option<String>,
    pub checks_since: Option<u64>,
    /// The branch was put on the latest base once already.
    #[serde(default)]
    pub rebased: bool,
    /// The agent was asked once already to fix what failed.
    #[serde(default)]
    pub fixed: bool,
    #[serde(default)]
    pub review: Review,
    /// The reviewer at work on the change now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<blind::Reviewer>,
    /// What the reviewer said, waiting to be acted on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<blind::Verdict>,
    /// The commit a reviewer approved. A push of any other leaves the change
    /// unapproved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<String>,
    /// The commit last kept on the remote for the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backed_up: Option<String>,
    #[serde(default)]
    pub recovery: Recovery,
    #[serde(default)]
    pub burn: Burn,
    /// Its place in the merge queue, once it has joined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<queue::Place>,
    /// The code host's checks passed it, rather than the project's command.
    #[serde(default)]
    pub checked_on_host: bool,
    /// Its builder works on a linked host; the worktree above stays here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere: Option<RunElsewhere>,
}

impl AutoRun {
    fn starting(agent: String, elsewhere: Option<RunElsewhere>) -> Self {
        Self {
            agent,
            repo: String::new(),
            worktree: String::new(),
            branch: String::new(),
            base: String::new(),
            start_sha: String::new(),
            stage: RunStage::Starting,
            held: None,
            note: Some("Making its worktree".to_owned()),
            pull: None,
            head: None,
            checks_since: None,
            rebased: false,
            fixed: false,
            review: Review::default(),
            reviewer: None,
            verdict: None,
            approved: None,
            backed_up: None,
            recovery: Recovery::default(),
            burn: Burn::default(),
            queue: None,
            checked_on_host: false,
            elsewhere,
        }
    }

    pub(super) fn view(&self) -> CardRun {
        CardRun {
            agent: self.agent.clone(),
            repo: self.repo.clone(),
            worktree: self.worktree.clone(),
            branch: self.branch.clone(),
            base: self.base.clone(),
            start_sha: self.start_sha.clone(),
            stage: self.stage,
            held: self.held,
            waits_on_person: self.held.is_some() || self.stage == RunStage::Watching,
            note: self.note.clone(),
            pull: self.pull.as_ref().map(|pull| CardPull {
                number: pull.number,
                url: pull.url.clone(),
                host: pull.host.clone(),
            }),
        }
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

/// How many of the board's cards have an agent of Auto's at work that is
/// not waiting on the person.
pub(super) fn running(core: &Core, board: &Board) -> usize {
    running_where(core, board, |_| true)
}

fn running_where(core: &Core, board: &Board, counts: impl Fn(&AutoRun) -> bool) -> usize {
    board
        .cards
        .iter()
        .filter(|card| {
            card.run().is_some_and(|run| {
                run.held.is_none()
                    && run.stage.busy()
                    && counts(run)
                    && !core.harness.questions.blocks_on_person(&run.agent)
            })
        })
        .count()
}

/// What choosing Auto's next cards needs of the whole board, gathered once so
/// a long reservoir costs one pass instead of one per card.
struct Reservoir {
    /// Cards others wait on until they are done or built.
    blocking: HashSet<u64>,
    /// Cards with something unfinished under them.
    parents: HashSet<u64>,
    /// Each unfinished card an Auto run added, and that run's card.
    added_by_run: HashMap<u64, u64>,
}

impl Reservoir {
    fn of(board: &Board) -> Self {
        let undone = || (board.cards.iter()).filter(|card| card.status != CardStatus::Done);
        Self {
            blocking: undone()
                .filter(|card| !card.slice.as_ref().is_some_and(|slice| slice.built_in))
                .map(|card| card.id)
                .collect(),
            parents: undone().filter_map(|card| card.parent).collect(),
            added_by_run: undone()
                .filter_map(|card| Some((card.id, card.from_run?)))
                .collect(),
        }
    }

    /// How many unfinished runs stand above the card, up to the first finished one.
    fn runs_above(&self, card: &Card) -> usize {
        std::iter::successors(card.from_run, |run| self.added_by_run.get(run).copied())
            .take_while(|run| self.blocking.contains(run))
            .take(MAX_RUNS_ABOVE + 1)
            .count()
    }

    /// Whether Auto may start an agent on the card, as far as the board can say.
    fn takes(&self, settings: &BoardSettings, card: &Card) -> bool {
        card.status == CardStatus::Open
            && card.owner.is_none()
            && card.auto.is_none()
            && (card.created_by.is_none() || settings.agent_cards)
            && card.ticket_done_unlanded().is_none()
            && stalled_allows(settings, card)
            && !card.depends_on.iter().any(|id| self.blocking.contains(id))
            && !self.parents.contains(&card.id)
            && self.runs_above(card) <= MAX_RUNS_ABOVE
    }
}

/// Whether the team of the agent that added a card may still give it to one
/// of its own: one of them is in a turn, or something will bring one back.
fn team_keeps(core: &Core, maker: &str) -> bool {
    let lead = core.names.team_of(maker);
    let now = core.agent_now(&lead);
    now.open() && (now.maybe_mid_turn() || now.at_work() || now.waits_on_something())
}

/// The card of the Auto run an agent adding a card works for: its own, or its lead's.
pub(super) fn run_adding(core: &Core, board: &Board, agent: &str) -> Option<u64> {
    let lead = core.names.team_of(agent);
    (board.cards.iter())
        .find(|card| card.kept_run().is_some_and(|run| run.agent == lead))
        .map(|card| card.id)
}

/// A slice whose owner lost the lease is Auto's only with recovery on, and not when it stays
/// with the person.
fn stalled_allows(settings: &BoardSettings, card: &Card) -> bool {
    (card.slice.as_ref())
        .and_then(|slice| slice.stalled.as_ref())
        .is_none_or(|stalled| settings.recover && !stalled.stays)
}

/// Whether some board has Auto on with work it could still do, which keeps
/// the core running with nothing else open.
pub(crate) fn has_work(boards: &super::Boards) -> bool {
    let state = boards.state.locked();
    state.boards.values().any(|board| {
        board.settings.auto && {
            let reservoir = Reservoir::of(board);
            let starts_more = !guard::ceiling_reached(board);
            (board.cards.iter()).any(|card| {
                (starts_more && reservoir.takes(&board.settings, card)) || card.run().is_some()
            })
        }
    })
}

/// The cards Auto starts next on the board, at most `free`, the highest priority first, then the
/// lowest numbers.
fn picks(
    core: &Core,
    project: &str,
    board: &Board,
    stepping: &HashSet<u64>,
    free: usize,
) -> Vec<u64> {
    if free == 0 {
        return Vec::new();
    }
    let reservoir = Reservoir::of(board);
    let mut takeable: Vec<(Reverse<i32>, u64, Option<&str>)> = (board.cards.iter())
        .filter(|card| reservoir.takes(&board.settings, card) && !stepping.contains(&card.id))
        .map(|card| (Reverse(card.priority), card.id, card.maker_id()))
        .collect();
    if takeable.is_empty() {
        return Vec::new();
    }
    takeable.sort_unstable();
    let room = core.names.room_for_started(project, core.open_agents());
    let mut kept_by_team: HashMap<&str, bool> = HashMap::new();
    takeable
        .into_iter()
        .filter(|(_, _, maker)| {
            maker.is_none_or(|maker| {
                !*kept_by_team
                    .entry(maker)
                    .or_insert_with(|| team_keeps(core, maker))
            })
        })
        .map(|(_, id, _)| id)
        .take(free.min(room))
        .collect()
}

/// A card Auto just took, and where its agent is to start: here, or the place kept on a host.
struct Start {
    project: String,
    id: u64,
    agent: String,
    stalled: Option<Stalled>,
    place: Option<RunElsewhere>,
}

/// Starts an agent on each card Auto may take, while it has places free.
pub(super) fn start_free(core: &Arc<Core>) {
    let launchers = core.workspaces.has_launchers();
    let starts: Vec<Start> = {
        let mut state = core.boards.state.locked();
        let projects: Vec<String> = state
            .boards
            .iter()
            .filter(|(_, board)| board.settings.auto)
            .map(|(project, _)| project.clone())
            .collect();
        let mut starts = Vec::new();
        let mut state_jobs = Vec::new();
        for project in projects {
            let Some(board) = state.boards.get_mut(&project) else {
                continue;
            };
            let note = (!launchers)
                .then(|| {
                    "Auto starts once Sikemux has opened and said which agents it can start"
                        .to_owned()
                })
                .or_else(|| guard::ceiling_note(board));
            if board.auto_note != note {
                board.auto_note = note;
                state.dirty.insert(project.clone());
            }
            let stepping: HashSet<u64> = state
                .work
                .iter()
                .filter(|((other, _), work)| *other == project && work.job)
                .map(|((_, id), _)| *id)
                .collect();
            let Some(board) = state.boards.get_mut(&project) else {
                continue;
            };
            if !launchers || guard::ceiling_reached(board) {
                continue;
            }
            let here = running_where(core, board, |run| run.elsewhere.is_none());
            let free = (board.settings.slots as usize).saturating_sub(here);
            let mut places = elsewhere::free_places(core, board).into_iter();
            let picks = picks(core, &project, board, &stepping, free + places.len());
            if picks.is_empty() {
                continue;
            }
            let settings = board.settings.clone();
            for (index, id) in picks.into_iter().enumerate() {
                let agent = crate::server::names::new_agent_id();
                let no_contract = (settings.scaled_review && settings.wants_contract())
                    .then(|| {
                        board
                            .card(id)
                            .ok()
                            .and_then(|card| risk::no_contract(board, card))
                    })
                    .flatten();
                if let Ok(card) = board.card_mut(id) {
                    let place = (index >= free)
                        .then(|| places.next())
                        .flatten()
                        .map(RunElsewhere::on);
                    let stalled = (card.slice.as_ref()).and_then(|slice| slice.stalled.clone());
                    if place.is_some() && stalled.is_some() {
                        continue;
                    }
                    card.hold(Owner::starting(&agent));
                    card.team = None;
                    card.auto = Some(AutoRun::starting(agent.clone(), place.clone()));
                    card.record = Some(RunRecord::begun(&settings, card.reservoir.is_some()));
                    card.contract =
                        (place.is_none() && settings.wants_contract() && no_contract.is_none())
                            .then(Contract::awaited);
                    if let Some(record) = card.record.as_mut() {
                        record.contract_outcome = no_contract;
                    }
                    super::slice::auto_slice(card, &settings);
                    starts.push(Start {
                        project: project.clone(),
                        id,
                        agent,
                        stalled,
                        place,
                    });
                    state_jobs.push((project.clone(), id));
                }
            }
            core.boards.save(&mut state, &project);
        }
        for (project, id) in state_jobs {
            state.work(&project, id).job = true;
        }
        starts
    };
    for start_card in starts {
        let Start {
            project,
            id,
            agent,
            stalled,
            place,
        } = start_card;
        let core = core.clone();
        tokio::spawn(async move {
            let started = start(&core, &project, id, &agent, stalled, place).await;
            core.boards.end_job(&project, id);
            if let Err(error) = started {
                let gone = core.chats.get(&agent).is_none();
                if gone {
                    core.names.forget(&agent);
                }
                finish::hold(
                    &core,
                    &project,
                    id,
                    &agent,
                    RunHold::Fix,
                    format!("Auto could not start it: {error}"),
                );
            }
        });
    }
}

fn task(
    card: &Card,
    tree: &worktree::Tree,
    settings: &BoardSettings,
    ticket: Option<String>,
    on_another_host: bool,
) -> String {
    let mut text = format!(
        "Sikemux's Auto started you to do card #{} of this project's board on your own, in a git worktree nobody else works in.\n\nCard #{}: {}",
        card.id, card.id, card.title
    );
    if !card.details.is_empty() {
        text.push_str(&format!("\n\n{}", card.details));
    }
    text.push_str(card.printed_by_command());
    if !card.acceptance.is_empty() {
        text.push_str(&format!(
            "\n\nAcceptance criteria, which a reviewer checks the change against:\n{}",
            card.acceptance
        ));
    }
    if let Some(ticket) = ticket {
        text.push_str(&format!("\n\nTicket: {ticket}"));
    }
    let landing = match settings.landing {
        Landing::Core => format!(
            "waits for its checks to pass, has an agent on another CLI review it, and merges it into {}",
            tree.base
        ),
        Landing::Person => "leaves it for the person to review and merge".to_owned(),
    };
    let contract_step = if card.contract.is_some() {
        format!("{}\n", contract::builder_step())
    } else {
        String::new()
    };
    if on_another_host {
        text.push_str(&format!(
            "\n\nYou are in a checkout of your own on branch {branch}, cut from {base}, on another host than the board that holds this card.\n\n1. Plan first. Read what you need, then ask every question you have at once with ask, before you change anything. If you cannot go on without an answer, ask with blocked true and end your turn: the answer comes back to you. Otherwise say what you assume and go on.\n2. Do the work, and run the tests and checks that cover it.\n3. Commit it on {branch}, and end with a short summary: your final answer becomes the card's result.\n\nDo not push, open a pull request or merge, and leave the board's tools alone, as the card is not on this host's board. When your turn ends, Sikemux brings your commits back, pushes the branch, opens the pull request and {landing}. So do not end your turn before the work is done, unless you are waiting on an answer. Only this checkout's commits come back, so if you start other agents to help, take their work in here before you end your turn.",
            branch = tree.branch,
            base = tree.base,
        ));
        return text;
    }
    text.push_str(&format!(
        "\n\nYou are in {} on branch {}, cut from {}.\n\n1. Plan first. Read what you need, then ask every question you have at once with ask, before you change anything. If you cannot go on without an answer, ask with blocked true and end your turn: the answer comes back to you. Otherwise say what you assume and go on.\n{contract_step}2. Do the work, and run the tests and checks that cover it.\n3. Commit it on {}, and put a short summary in the card's result with board_update.\n\nDo not push, open a pull request or merge. When your turn ends, Sikemux commits anything left, pushes the branch, opens the pull request and {landing}. So do not end your turn before the work is done, unless you are waiting on an answer. If you start other agents to help, Sikemux ships only once none of them is still working.",
        tree.cwd.display(),
        tree.branch,
        tree.base,
        tree.branch,
    ));
    text
}

/// What Auto needs to put an agent on a card's worktree.
struct Launched {
    launch: ChatLaunch,
    launcher: String,
    name: String,
}

impl Launched {
    fn seat(&self) -> Seat<'_> {
        Seat {
            name: &self.name,
            provider: self.launch.provider.clone(),
            elsewhere: None,
        }
    }
}

/// Who takes a run's seat, and where it works when that is not here.
pub(super) struct Seat<'a> {
    pub name: &'a str,
    pub provider: String,
    pub elsewhere: Option<RunElsewhere>,
}

fn launch_in(
    core: &Arc<Core>,
    project: &str,
    agent: &str,
    settings: &BoardSettings,
    tree: &worktree::Tree,
) -> Result<Launched, String> {
    let (mut launch, launcher) = core.workspaces.board_launch(
        settings.launcher.as_deref(),
        &tree.cwd,
        settings.provider.as_deref(),
        settings.model.clone(),
        PICKED_IN,
    )?;
    let (named, _) = core.names.give(
        Starting {
            agent_id: agent,
            project: project.to_owned(),
            provider: Some(&launch.provider),
            resume_id: None,
        },
        core.open_agents(),
    );
    core.names.count_as_started(agent);
    launch.agent_id = agent.to_owned();
    Ok(Launched {
        launch,
        launcher,
        name: named.name,
    })
}

/// Puts `agent` on the run `current` holds. False when the run has moved on, as nothing is
/// started then.
pub(super) fn seat(
    core: &Core,
    project: &str,
    id: u64,
    current: &str,
    agent: &str,
    seat: Seat<'_>,
    tree: &worktree::Tree,
) -> bool {
    let Seat {
        name,
        provider,
        elsewhere,
    } = seat;
    finish::change_run(core, project, id, current, |card, run| {
        card.run_by(
            Owner {
                id: agent.to_owned(),
                name: Some(name.to_owned()),
                cli: Some(provider),
            },
            Team {
                lead: agent.to_owned(),
                name: name.to_owned(),
            },
        );
        run.agent = agent.to_owned();
        run.elsewhere = elsewhere;
        run.repo = tree.repo.to_string_lossy().into_owned();
        run.worktree = tree.path.to_string_lossy().into_owned();
        run.branch = tree.branch.clone();
        run.base = tree.base.clone();
        run.start_sha = tree.start_sha.clone();
        if run.stage == RunStage::Starting {
            run.stage = RunStage::Working;
        }
        run.recovery.due = None;
        if !std::mem::take(&mut run.recovery.fresh) {
            run.note = None;
        }
    })
}

/// Starts the seated agent's chat in `tree` and hands it `text`.
async fn set_going(
    core: &Arc<Core>,
    agent: &str,
    launched: Launched,
    tree: worktree::Tree,
    text: String,
) -> Result<(), String> {
    let chat = chat::begin(
        core,
        launched.launch,
        None,
        Some(launched.launcher),
        false,
        true,
    )
    .map_err(|error| error.to_string())?;
    let _ = core.workspaces.edit_agent(agent, |agent| {
        agent.worktree = Some(AgentWorktree {
            repo: tree.repo,
            path: tree.path,
            branch: tree.branch,
            base: Some(tree.base),
            start_sha: tree.start_sha,
        })
    });
    core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
    core.changed(What::View);
    chat::until_started(core, chat)
        .await
        .map_err(|error| error.to_string())?;
    messages::hand_task(core, agent, messages::auto_task(core, agent, text)).await
}

/// The worktree of a run, as Auto started it.
pub(super) fn tree_of(run: &AutoRun, project: &str) -> worktree::Tree {
    let (repo, path) = (PathBuf::from(&run.repo), PathBuf::from(&run.worktree));
    worktree::Tree {
        cwd: path.join(worktree::inside(Path::new(project), &repo)),
        repo,
        path,
        branch: run.branch.clone(),
        base: run.base.clone(),
        start_sha: run.start_sha.clone(),
    }
}

/// The worktree a stalled owner left, when nothing runs in it any more.
fn left_behind(core: &Core, project: &str, stalled: &Stalled) -> Option<worktree::Tree> {
    if core.running_agents().contains(&stalled.agent) {
        return None;
    }
    let kept = core.workspaces.agent(&stalled.agent)?.worktree?;
    let base = kept.base?;
    kept.path.is_dir().then(|| worktree::Tree {
        cwd: kept
            .path
            .join(worktree::inside(Path::new(project), &kept.repo)),
        base,
        repo: kept.repo,
        path: kept.path,
        branch: kept.branch,
        start_sha: kept.start_sha,
    })
}

fn ticket_line(card: &Card) -> Option<String> {
    card.ticket.as_ref().map(|ticket| match &ticket.url {
        Some(url) => format!("{} {url}", ticket.key),
        None => ticket.key.clone(),
    })
}

async fn start(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    agent: &str,
    stalled: Option<Stalled>,
    place: Option<RunElsewhere>,
) -> Result<(), String> {
    let (card, settings) = {
        let state = core.boards.state.locked();
        let board = state.boards.get(project).ok_or("the board is gone")?;
        (board.card(id)?.clone(), board.settings.clone())
    };
    let ticket = ticket_line(&card);
    let branch = worktree::branch_for(&match &card.ticket {
        Some(ticket) => format!("{} {}", ticket.key, card.title),
        None => format!("card {} {}", card.id, card.title),
    });
    if let Some(place) = place {
        let tree = {
            let project = project.to_owned();
            blocking(move || {
                worktree::cut_for_card(Path::new(&project), &branch).map_err(Into::into)
            })
            .await
            .map_err(|error| error.to_string())?
        };
        let text = task(&card, &tree, &settings, ticket, true);
        return elsewhere::start(core, project, id, agent, place, tree, text).await;
    }
    let reused = stalled
        .as_ref()
        .and_then(|stalled| left_behind(core, project, stalled));
    let reuses = reused.is_some();
    let prepared = {
        let (core, project, agent, settings) = (
            core.clone(),
            project.to_owned(),
            agent.to_owned(),
            settings.clone(),
        );
        blocking(move || {
            let tree = match reused {
                Some(tree) => tree,
                None => worktree::cut_for_card(Path::new(&project), &branch)?,
            };
            match launch_in(&core, &project, &agent, &settings, &tree) {
                Ok(launched) => Ok((tree, launched)),
                Err(error) => {
                    if !reuses {
                        worktree::remove_with_remote(&tree.repo, &tree.path, &tree.branch, true);
                    }
                    Err(error.into())
                }
            }
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (tree, launched) = prepared;
    let mut text = task(&card, &tree, &settings, ticket, false);
    if !seat(core, project, id, agent, agent, launched.seat(), &tree) {
        if !reuses {
            blocking(move || {
                worktree::remove_with_remote(&tree.repo, &tree.path, &tree.branch, true);
                Ok(())
            })
            .await
            .ok();
        }
        return Ok(());
    }
    if let Some(stalled) = stalled {
        let (cwd, start_sha, from) = (tree.cwd.clone(), tree.start_sha.clone(), stalled.clone());
        text.push_str(
            &blocking(move || Ok(recover::takeover_note(&cwd, &start_sha, &from)))
                .await
                .unwrap_or_default(),
        );
        note_takeover(core, project, id, agent, &card.label(), stalled, reuses);
    }
    set_going(core, agent, launched, tree, text).await
}

/// Tells the person Auto took a stalled slice on, where they can overrule it.
fn note_takeover(
    core: &Core,
    project: &str,
    id: u64,
    agent: &str,
    label: &str,
    stalled: Stalled,
    reused: bool,
) {
    let settled = goal::Settled {
        project: project.to_owned(),
        by: agent.to_owned(),
        holder: agent.to_owned(),
        asked: format!("Who carries on {label}, now @{} has stalled?", stalled.name),
        text: "Auto took it on, as its owner lost the lease".to_owned(),
        why: Some(stalled.why.words()),
    };
    let Some((goal, decision)) = goal::keep_settled(core, settled) else {
        return;
    };
    finish::change_run(core, project, id, agent, |_, run| {
        run.recovery.took_on = Some(TookOn {
            from: stalled,
            goal,
            decision,
            reused,
        });
    });
}

/// A fresh agent takes over the run in its own worktree, told what `note` says.
pub(super) async fn replace_agent(
    core: &Arc<Core>,
    step: &finish::Step,
    tree: worktree::Tree,
    note: String,
) -> Result<(), String> {
    let agent = crate::server::names::new_agent_id();
    let launched = {
        let (core, project, agent, settings, place) = (
            core.clone(),
            step.project.clone(),
            agent.clone(),
            step.settings.clone(),
            tree_of(&step.run, &step.project),
        );
        blocking(move || launch_in(&core, &project, &agent, &settings, &place).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    if !seat(
        core,
        &step.project,
        step.id,
        step.agent(),
        &agent,
        launched.seat(),
        &tree,
    ) {
        return Ok(());
    }
    let mut text = task(
        &step.card,
        &tree,
        &step.settings,
        ticket_line(&step.card),
        false,
    );
    text.push_str(&note);
    if let Err(error) = set_going(core, &agent, launched, tree, text).await {
        finish::hold(
            core,
            &step.project,
            step.id,
            &agent,
            RunHold::Unblock,
            format!("A fresh agent could not start: {error}"),
        );
    }
    Ok(())
}

/// The turn of one of Auto's agents ended: its work ships, or its card goes
/// back to the person when the turn did not finish.
pub(crate) fn turn_ended(core: &Arc<Core>, agent_id: &str, ending: &Ending) {
    guard::turn_ended(core, agent_id);
    if blind::turn_ended(core, agent_id, ending) {
        return;
    }
    let found = {
        let state = core.boards.state.locked();
        state.boards.iter().find_map(|(project, board)| {
            board.cards.iter().find_map(|card| {
                let run = card.kept_run()?;
                (run.agent == agent_id && run.stage != RunStage::Starting).then(|| {
                    (
                        project.clone(),
                        card.id,
                        run.stage,
                        run.held.is_some(),
                        recover::bringing_back(run),
                        run.recovery.fresh,
                    )
                })
            })
        })
    };
    let Some((project, id, stage, held_back, bringing_back, replacing)) = found else {
        worker_ended(core, agent_id);
        return;
    };
    let name = core
        .names
        .name(agent_id)
        .unwrap_or_else(|| "Its agent".to_owned());
    match ending {
        Ending::Finished if replacing => {}
        Ending::Finished => {
            recover::finished_turn(core, &project, id, agent_id);
            let now = core.agent_now(agent_id);
            if now.waits_on_something()
                || !(held_back || stage.ships_on_turn_end())
                || now.workers_busy(agent_id)
            {
                return;
            }
            if !held_back
                && stage == RunStage::Working
                && contract::keeps_building(core, &project, id, agent_id)
            {
                return;
            }
            finish::ship(core, project, id);
        }
        _ if held_back || !stage.busy() || bringing_back => {}
        ending => recover::ended(core, &project, id, agent_id, &name, stage, ending),
    }
}

/// A turn ended of an agent one of Auto's agents started. One that reports
/// to nobody wakes nobody, so when its lead's run waits only on its workers,
/// the run ships now.
fn worker_ended(core: &Arc<Core>, agent_id: &str) {
    if core.names.reports_to(agent_id).is_some() {
        return;
    }
    let lead = core.names.team_of(agent_id);
    if lead == agent_id {
        return;
    }
    let found = {
        let state = core.boards.state.locked();
        state.boards.iter().find_map(|(project, board)| {
            board.cards.iter().find_map(|card| {
                let run = card.run()?;
                (run.agent == lead
                    && !recover::bringing_back(run)
                    && matches!(run.stage, RunStage::Working | RunStage::Fixing))
                .then(|| (project.clone(), card.id))
            })
        })
    };
    let Some((project, id)) = found else {
        return;
    };
    let now = core.agent_now(&lead);
    if now.mid_turn() || now.waits_on_something() || now.workers_busy(agent_id) {
        return;
    }
    finish::ship(core, project, id);
}

/// Whether the card is one of `agent`'s own runs, whose work ships when its
/// turn ends rather than the card closing before it lands.
pub(super) fn own_run(card: &Card, agent: &str) -> bool {
    card.kept_run().is_some_and(|run| run.agent == agent)
}
