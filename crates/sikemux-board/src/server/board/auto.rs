//! Auto: for each card nobody holds and nothing holds up, whoever added it,
//! the board starts an agent of its own in a git worktree of its own, a
//! few at once, and ships its work when its turn ends.

pub use super::AutoPull;
use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::{
    BoardReviewerParams, BoardSettings, BoardSettingsParams, CardPull, CardRun, Landing,
    ReviewerChoice, RunHold, RunStage,
};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::contract::{self, Contract};
use super::elsewhere::{self, RunElsewhere};
use super::guard::{self, Burn};
use super::lease::Stalled;
use super::record::RunRecord;
use super::recover::{self, Recovery, TookOn};
use super::review::Review;
use super::{blind, finish, goal, queue, risk, Board, Card, CardStatus, Owner, Team};
use crate::server::chat::Ending;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::worktree::{self, AgentWorktree};
use crate::server::{chat, Core};
use sikemux_wire::protocol::{ChatLaunch, Event};

pub const MAX_SLOTS: u32 = crate::server::names::MAX_SPAWNED_PER_PROJECT as u32;
/// How many unfinished runs may stand above a card one of Auto's agents added
/// before that card rests until one of them finishes.
const MAX_RUNS_ABOVE: usize = 3;
/// How many unfinished cards one run may add for Auto to take.
pub const MAX_CARDS_PER_RUN: usize = 16;
pub const PICKED_IN: &str = "in the board's Auto settings";
const MAX_LAUNCHER_CHARS: usize = 256;
const MAX_MODEL_CHARS: usize = 200;

fn known_provider(
    provider: Option<String>,
    knows: &impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    match provider {
        Some(id) if !knows(&id) => Err(format!("Sikemux does not know a provider called {id}")),
        provider => Ok(provider),
    }
}

pub trait BoardReviewerParamsExt: Sized {
    fn choice(self, knows: &impl Fn(&str) -> bool) -> Result<ReviewerChoice, String>;
}

impl BoardReviewerParamsExt for BoardReviewerParams {
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

pub trait BoardSettingsExt: Sized {
    /// These settings with what `params` changes, naming only providers Sikemux `knows`.
    fn changed(
        &self,
        params: BoardSettingsParams,
        knows: impl Fn(&str) -> bool,
    ) -> Result<Self, String>;
}

impl BoardSettingsExt for BoardSettings {
    fn changed(
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
        if let Some(landing) = params.landing {
            settings.landing = landing;
        }
        if let Some(landed) = params.landed {
            settings.landed = landed;
        }
        if let Some(spend_ceiling) = params.spend_ceiling {
            if spend_ceiling == Some(0) {
                return Err("spendCeiling must be at least 1 dollar".into());
            }
            settings.spend_ceiling = spend_ceiling;
        }
        if let Some(hosts) = params.hosts {
            settings.hosts = elsewhere::checked_hosts(hosts)?;
        }
        Ok(settings)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRun {
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

    pub fn view(&self) -> CardRun {
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

/// How many of the board's cards have an agent of Auto's at work that is
/// not waiting on the person.
pub fn running(core: &Core, board: &Board) -> usize {
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
                    && !(core.board_above().questions_blocks_on_person)(core, &run.agent)
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
    fn takes(&self, card: &Card) -> bool {
        card.status == CardStatus::Open
            && card.owner.is_none()
            && card.auto.is_none()
            && card.ticket_done_unlanded().is_none()
            && stalled_allows(card)
            && !card.depends_on.iter().any(|id| self.blocking.contains(id))
            && !self.parents.contains(&card.id)
            && self.runs_above(card) <= MAX_RUNS_ABOVE
    }
}

/// Whether the team of the agent that added a card may still give it to one
/// of its own: one of them is in a turn, or something will bring one back.
fn team_keeps(core: &Core, maker: &str) -> bool {
    let lead = core.names().team_of(maker);
    let now = core.agent_now(&lead);
    now.open() && (now.maybe_mid_turn() || now.at_work() || now.waits_on_something())
}

/// The card of the Auto run an agent adding a card works for: its own, or its lead's.
pub fn run_adding(core: &Core, board: &Board, agent: &str) -> Option<u64> {
    let lead = core.names().team_of(agent);
    (board.cards.iter())
        .find(|card| card.kept_run().is_some_and(|run| run.agent == lead))
        .map(|card| card.id)
}

/// A slice whose owner lost the lease is Auto's, unless it stays with the person.
fn stalled_allows(card: &Card) -> bool {
    (card.slice.as_ref())
        .and_then(|slice| slice.stalled.as_ref())
        .is_none_or(|stalled| !stalled.stays)
}

/// Whether some board has Auto on with work it could still do, which keeps
/// the core running with nothing else open.
pub fn has_work(boards: &super::Boards) -> bool {
    let state = boards.state.locked();
    state.boards.values().any(|board| {
        board.settings.auto && {
            let reservoir = Reservoir::of(board);
            let starts_more = !guard::ceiling_reached(board);
            (board.cards.iter())
                .any(|card| (starts_more && reservoir.takes(card)) || card.run().is_some())
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
        .filter(|card| reservoir.takes(card) && !stepping.contains(&card.id))
        .map(|card| (Reverse(card.priority), card.id, card.maker_id()))
        .collect();
    if takeable.is_empty() {
        return Vec::new();
    }
    takeable.sort_unstable();
    let room = core.names().room_for_started(project, core.open_agents());
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
pub fn start_free(core: &Arc<Core>) {
    let launchers = core.workspaces().has_launchers();
    let starts: Vec<Start> = {
        let mut state = core.boards().state.locked();
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
                let no_contract = settings
                    .lands_itself()
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
                        (place.is_none() && settings.lands_itself() && no_contract.is_none())
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
            core.boards().save(&mut state, &project);
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
            core.boards().end_job(&project, id);
            if let Err(error) = started {
                let gone = core.chats().get(&agent).is_none();
                if gone {
                    core.names().forget(&agent);
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
pub struct Seat<'a> {
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
    let (mut launch, launcher) = core.workspaces().board_launch(
        settings.launcher.as_deref(),
        &tree.cwd,
        settings.provider.as_deref(),
        settings.model.clone(),
        PICKED_IN,
    )?;
    let (named, _) = core.names().give(
        Starting {
            agent_id: agent,
            project: project.to_owned(),
            provider: Some(&launch.provider),
            resume_id: None,
        },
        core.open_agents(),
    );
    core.names().count_as_started(agent);
    launch.agent_id = agent.to_owned();
    Ok(Launched {
        launch,
        launcher,
        name: named.name,
    })
}

/// Puts `agent` on the run `current` holds. False when the run has moved on, as nothing is
/// started then.
pub fn seat(
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
    let mut kept = AgentWorktree {
        repo: tree.repo,
        path: tree.path,
        branch: tree.branch,
        base: Some(tree.base),
        start_sha: tree.start_sha,
        name: String::new(),
        first_port: None,
    };
    crate::server::ports::checkout::claim(core, &mut kept);
    let _ = core
        .workspaces()
        .edit_agent(agent, |agent| agent.worktree = Some(kept));
    core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
    core.changed(What::View);
    chat::until_started(core, chat)
        .await
        .map_err(|error| error.to_string())?;
    (core.board_above().messages_hand_task)(
        core,
        agent,
        (core.board_above().messages_auto_task)(core, agent, text),
    )
    .await
}

/// The worktree of a run, as Auto started it.
pub fn tree_of(run: &AutoRun, project: &str) -> worktree::Tree {
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
    let kept = core.workspaces().agent(&stalled.agent)?.worktree?;
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
        let state = core.boards().state.locked();
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
        let said = recover::last_said(core, &stalled.agent);
        let (owner, cwd, start_sha, from) = (
            core.clone(),
            tree.cwd.clone(),
            tree.start_sha.clone(),
            stalled.clone(),
        );
        let note = blocking(move || {
            Ok(match reuses {
                true => recover::takeover_note(&cwd, &start_sha, &from, said),
                false => recover::left_elsewhere(&owner, &from, said),
            })
        });
        text.push_str(&note.await.unwrap_or_default());
        note_takeover(core, project, id, agent, &card.label(), stalled, reuses);
    }
    if !reuses {
        if let Some(failed) =
            worktree::agent::set_up(core, Path::new(project), Some(agent), &tree.cwd).await
        {
            text.push_str(&format!(
                "\n\nThe project's worktree setup did not finish: {failed}"
            ));
        }
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
        asked: Some(format!(
            "Who carries on {label}, now @{} has stalled?",
            stalled.name
        )),
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
pub async fn replace_agent(
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
pub fn turn_ended(core: &Arc<Core>, agent_id: &str, ending: &Ending) {
    guard::turn_ended(core, agent_id);
    if blind::turn_ended(core, agent_id, ending) {
        return;
    }
    let found = {
        let state = core.boards().state.locked();
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
        .names()
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
    if core.names().reports_to(agent_id).is_some() {
        return;
    }
    let lead = core.names().team_of(agent_id);
    if lead == agent_id {
        return;
    }
    let found = {
        let state = core.boards().state.locked();
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
pub fn own_run(card: &Card, agent: &str) -> bool {
    card.kept_run().is_some_and(|run| run.agent == agent)
}
