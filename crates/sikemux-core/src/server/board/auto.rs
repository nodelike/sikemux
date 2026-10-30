//! Auto: for each card the person added that nobody holds and nothing holds
//! up, the board starts an agent of its own in a git worktree of its own, a
//! few at once, and ships its work when its turn ends.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::review::Review;
use super::{blind, finish, Board, Card, Owner, Status, Team};
use crate::protocol::Event;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::spawn::Ending;
use crate::server::worktree::{self, AgentWorktree};
use crate::server::{chat, messages, Core};

pub(super) const MAX_SLOTS: u32 = 8;
const DEFAULT_SLOTS: u32 = 2;
const MAX_LAUNCHER_CHARS: usize = 256;
const MAX_MODEL_CHARS: usize = 200;

/// Where a card's run ends when its checks pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Finish {
    #[default]
    Merge,
    /// At an open pull request, for the person to merge.
    Pr,
}

/// The agent that reviews a card's change before Auto merges it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewerChoice {
    /// The app's launcher it starts with, which must run another CLI than
    /// the card's builder.
    pub launcher: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// How a project's board runs on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Settings {
    pub auto: bool,
    /// How many of Auto's agents work at once.
    pub slots: u32,
    /// The app's launcher Auto's agents start with; none for the first that works.
    pub launcher: Option<String>,
    /// The connected provider its agents run on in place of their own accounts.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub finish: Finish,
    /// Who reviews each change before it merges; none for any signed-in
    /// agent on another CLI than its builder's.
    pub reviewer: Option<ReviewerChoice>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto: false,
            slots: DEFAULT_SLOTS,
            launcher: None,
            provider: None,
            model: None,
            finish: Finish::Merge,
            reviewer: None,
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

fn reviewer_choice(
    value: &Value,
    knows: &impl Fn(&str) -> bool,
) -> Result<Option<ReviewerChoice>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let launcher = optional(value, "launcher", MAX_LAUNCHER_CHARS)?
        .flatten()
        .ok_or("reviewer.launcher must name the agent that reviews")?;
    Ok(Some(ReviewerChoice {
        launcher,
        provider: known_provider(
            optional(value, "provider", MAX_LAUNCHER_CHARS)?.flatten(),
            knows,
        )?,
        model: optional(value, "model", MAX_MODEL_CHARS)?.flatten(),
    }))
}

fn optional(value: &Value, key: &str, max: usize) -> Result<Option<Option<String>>, String> {
    match value.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) if text.trim().is_empty() => Ok(Some(None)),
        Some(Value::String(text)) if text.len() <= max => Ok(Some(Some(text.trim().to_owned()))),
        Some(_) => Err(format!("{key} must be text of at most {max} bytes")),
    }
}

impl Settings {
    /// These settings with what `params` changes, naming only providers Sikemux `knows`.
    pub(super) fn changed(
        &self,
        params: &Value,
        knows: impl Fn(&str) -> bool,
    ) -> Result<Self, String> {
        let mut settings = self.clone();
        if let Some(auto) = params.get("auto") {
            settings.auto = auto.as_bool().ok_or("auto must be true or false")?;
        }
        if let Some(slots) = params.get("slots") {
            settings.slots = slots
                .as_u64()
                .filter(|slots| (1..=u64::from(MAX_SLOTS)).contains(slots))
                .ok_or(format!("slots must be from 1 to {MAX_SLOTS}"))?
                as u32;
        }
        if let Some(launcher) = optional(params, "launcher", MAX_LAUNCHER_CHARS)? {
            settings.launcher = launcher;
        }
        if let Some(provider) = optional(params, "provider", MAX_LAUNCHER_CHARS)? {
            settings.provider = known_provider(provider, &knows)?;
        }
        if let Some(model) = optional(params, "model", MAX_MODEL_CHARS)? {
            settings.model = model;
        }
        if let Some(reviewer) = params.get("reviewer") {
            settings.reviewer = reviewer_choice(reviewer, &knows)?;
        }
        match params.get("finish").and_then(Value::as_str) {
            None => {}
            Some("merge") => settings.finish = Finish::Merge,
            Some("pr") => settings.finish = Finish::Pr,
            Some(_) => return Err("finish must be merge or pr".into()),
        }
        Ok(settings)
    }
}

/// Where a card's run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Stage {
    /// Its worktree and agent are being made.
    Starting,
    Working,
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
    /// Its pull request waits for the person.
    Review,
    /// Back with the person, for the reason in its note.
    Held,
    Merged,
    /// The card was done another way.
    Closed,
}

impl Stage {
    pub(super) fn is_over(self) -> bool {
        matches!(self, Stage::Merged | Stage::Closed)
    }

    /// Its agent is at work, so it takes one of Auto's places.
    fn busy(self) -> bool {
        matches!(self, Stage::Starting | Stage::Working | Stage::Fixing)
    }

    /// A turn of its agent ending sends its work on.
    fn ships_on_turn_end(self) -> bool {
        matches!(
            self,
            Stage::Working | Stage::Fixing | Stage::Held | Stage::Review
        )
    }
}

/// A card's pull request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pull {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub url: String,
    pub account: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Run {
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
    pub stage: Stage,
    /// What is happening, or why the card is back with the person.
    pub note: Option<String>,
    pub pull: Option<Pull>,
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
}

impl Run {
    fn starting(agent: String) -> Self {
        Self {
            agent,
            repo: String::new(),
            worktree: String::new(),
            branch: String::new(),
            base: String::new(),
            start_sha: String::new(),
            stage: Stage::Starting,
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
        }
    }

    pub(super) fn view(&self) -> RunView {
        RunView {
            agent: self.agent.clone(),
            repo: self.repo.clone(),
            worktree: self.worktree.clone(),
            branch: self.branch.clone(),
            base: self.base.clone(),
            start_sha: self.start_sha.clone(),
            stage: self.stage,
            note: self.note.clone(),
            pull: self.pull.as_ref().map(|pull| PullView {
                number: pull.number,
                url: pull.url.clone(),
                host: pull.host.clone(),
            }),
        }
    }
}

/// A card's run as the app and devices read it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunView {
    agent: String,
    repo: String,
    worktree: String,
    branch: String,
    base: String,
    start_sha: String,
    stage: Stage,
    note: Option<String>,
    pull: Option<PullView>,
}

/// A run's pull request as the app and devices read it.
#[derive(Serialize)]
pub(crate) struct PullView {
    number: u64,
    url: String,
    host: String,
}

/// How many of the board's cards have an agent of Auto's at work that is
/// not waiting on the person.
pub(super) fn running(core: &Core, board: &Board) -> usize {
    board
        .cards
        .iter()
        .filter(|card| {
            card.auto.as_ref().is_some_and(|run| {
                run.stage.busy() && !core.harness.questions.blocks_on_person(&run.agent)
            })
        })
        .count()
}

/// Whether Auto may start an agent on the card.
fn takes(board: &Board, card: &Card) -> bool {
    card.status == Status::Open
        && card.owner.is_none()
        && card.auto.is_none()
        && card.created_by.is_none()
        && board.waits_on(card).is_empty()
        && !board
            .cards
            .iter()
            .any(|other| other.parent == Some(card.id) && other.status != Status::Done)
}

/// Whether some board has Auto on with work it could still do, which keeps
/// the core running with nothing else open.
pub(crate) fn has_work(core: &Core) -> bool {
    let state = core.boards.state.locked();
    state.boards.values().any(|board| {
        board.settings.auto
            && board.cards.iter().any(|card| {
                takes(board, card)
                    || card
                        .auto
                        .as_ref()
                        .is_some_and(|run| !run.stage.is_over() && run.stage != Stage::Held)
            })
    })
}

/// Starts an agent on each card Auto may take, while it has places free.
pub(super) fn start_free(core: &Arc<Core>) {
    let now = epoch_ms();
    let launchers = core.workspaces.has_launchers();
    let starts: Vec<(String, u64, String)> = {
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
            let note = (!launchers).then(|| {
                "Auto starts once Sikemux has opened and said which agents it can start".to_owned()
            });
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
            if !launchers {
                continue;
            }
            let free = (board.settings.slots as usize).saturating_sub(running(core, board));
            let mut picks: Vec<u64> = board
                .cards
                .iter()
                .filter(|card| takes(board, card) && !stepping.contains(&card.id))
                .map(|card| card.id)
                .collect();
            picks.sort_unstable();
            picks.truncate(free);
            if picks.is_empty() {
                continue;
            }
            for id in picks {
                let agent = format!("agent-{}", uuid::Uuid::new_v4().simple());
                if let Ok(card) = board.card_mut(id) {
                    card.owner = Some(Owner::starting(&agent));
                    card.team = None;
                    card.status = Status::Progress;
                    card.auto = Some(Run::starting(agent.clone()));
                    card.updated_at = now;
                    starts.push((project.clone(), id, agent));
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
    for (project, id, agent) in starts {
        let core = core.clone();
        tokio::spawn(async move {
            let started = start(&core, &project, id, &agent).await;
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
                    format!("Auto could not start it: {error}"),
                );
            }
        });
    }
}

/// What the agent is asked to do.
fn task(card: &Card, tree: &worktree::Tree, settings: &Settings, ticket: Option<String>) -> String {
    let mut text = format!(
        "Sikemux's Auto started you to do card #{} of this project's board on your own, in a git worktree nobody else works in.\n\nCard #{}: {}",
        card.id, card.id, card.title
    );
    if !card.details.is_empty() {
        text.push_str(&format!("\n\n{}", card.details));
    }
    if !card.acceptance.is_empty() {
        text.push_str(&format!(
            "\n\nAcceptance criteria, which a reviewer checks the change against:\n{}",
            card.acceptance
        ));
    }
    if let Some(ticket) = ticket {
        text.push_str(&format!("\n\nTicket: {ticket}"));
    }
    let landing = match settings.finish {
        Finish::Merge => format!(
            "waits for its checks to pass, has an agent on another CLI review it, and merges it into {}",
            tree.base
        ),
        Finish::Pr => "leaves it for the person to review and merge".to_owned(),
    };
    text.push_str(&format!(
        "\n\nYou are in {} on branch {}, cut from {}.\n\n1. Plan first. Read what you need, then ask every question you have at once with ask, before you change anything. If you cannot go on without an answer, ask with blocked true and end your turn: the answer comes back to you. Otherwise say what you assume and go on.\n2. Do the work, and run the tests and checks that cover it.\n3. Commit it on {}, and put a short summary in the card's result with board_update.\n\nDo not push, open a pull request or merge. When your turn ends, Sikemux commits anything left, pushes the branch, opens the pull request and {landing}. So do not end your turn before the work is done, unless you are waiting on an answer. If you start other agents to help, Sikemux ships only once none of them is still working.",
        tree.cwd.display(),
        tree.branch,
        tree.base,
        tree.branch,
    ));
    text
}

async fn start(core: &Arc<Core>, project: &str, id: u64, agent: &str) -> Result<(), String> {
    let (card, settings, ticket) = {
        let state = core.boards.state.locked();
        let board = state.boards.get(project).ok_or("the board is gone")?;
        let card = board.card(id)?.clone();
        let ticket = card.ticket.as_ref().map(|ticket| match &ticket.url {
            Some(url) => format!("{} {url}", ticket.key),
            None => ticket.key.clone(),
        });
        (card, board.settings.clone(), ticket)
    };
    let slug = worktree::slug(&match &card.ticket {
        Some(ticket) => format!("{} {}", ticket.key, card.title),
        None => format!("card {} {}", card.id, card.title),
    });
    let prepared = {
        let (core, project, agent, settings) = (
            core.clone(),
            project.to_owned(),
            agent.to_owned(),
            settings.clone(),
        );
        blocking(move || {
            let tree = worktree::cut_for_card(Path::new(&project), &slug)?;
            let started = core.workspaces.auto_launch(
                settings.launcher.as_deref(),
                &tree.cwd,
                settings.provider.as_deref(),
                settings.model.clone(),
            );
            let (mut launch, launcher) = match started {
                Ok(started) => started,
                Err(error) => {
                    worktree::remove_with_remote(&tree.repo, &tree.path, &tree.branch, true);
                    return Err(error.into());
                }
            };
            let (named, _) = core.names.give(
                Starting {
                    agent_id: &agent,
                    project: project.clone(),
                    provider: Some(&launch.provider),
                    resume_id: None,
                },
                core.open_agents(),
            );
            launch.agent_id = agent.clone();
            Ok((tree, launch, launcher, named.name))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (tree, launch, launcher, name) = prepared;
    let provider = launch.provider.clone();
    let text = task(&card, &tree, &settings, ticket);
    let placed = finish::change_run(core, project, id, agent, |card, run| {
        card.owner = Some(Owner {
            id: agent.to_owned(),
            name: Some(name.clone()),
            cli: Some(provider.clone()),
        });
        card.team = Some(Team {
            lead: agent.to_owned(),
            name: name.clone(),
        });
        run.repo = tree.repo.to_string_lossy().into_owned();
        run.worktree = tree.path.to_string_lossy().into_owned();
        run.branch = tree.branch.clone();
        run.base = tree.base.clone();
        run.start_sha = tree.start_sha.clone();
        run.stage = Stage::Working;
        run.note = None;
    });
    if !placed {
        blocking(move || {
            worktree::remove_with_remote(&tree.repo, &tree.path, &tree.branch, true);
            Ok(())
        })
        .await
        .ok();
        return Ok(());
    }
    super::tell_clients(core);
    let chat = chat::begin(core, launch, None, Some(launcher), false)
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
    core.broadcast_local(&Event::ChatBegun { chat: chat.info() });
    core.view_changed();
    chat::until_started(core, chat)
        .await
        .map_err(|error| error.to_string())?;
    messages::hand_task(core, agent, messages::auto_task(text)).await
}

/// The turn of one of Auto's agents ended: its work ships, or its card goes
/// back to the person when the turn did not finish.
pub(crate) fn turn_ended(core: &Arc<Core>, agent_id: &str, ending: &Ending) {
    if blind::turn_ended(core, agent_id, ending) {
        return;
    }
    let found = {
        let state = core.boards.state.locked();
        state.boards.iter().find_map(|(project, board)| {
            board.cards.iter().find_map(|card| {
                let run = card.auto.as_ref()?;
                (run.agent == agent_id && !run.stage.is_over() && run.stage != Stage::Starting)
                    .then(|| (project.clone(), card.id, run.stage))
            })
        })
    };
    let Some((project, id, stage)) = found else {
        worker_ended(core, agent_id);
        return;
    };
    let name = core
        .names
        .name(agent_id)
        .unwrap_or_else(|| "Its agent".to_owned());
    let held = |note: String| finish::hold(core, &project, id, agent_id, note);
    match ending {
        Ending::Finished => {
            if core.harness.questions.waits_on_answer(agent_id)
                || core.harness.mailbox.has_waiting(agent_id)
                || !stage.ships_on_turn_end()
                || workers_busy(core, agent_id, agent_id)
            {
                return;
            }
            finish::ship(core, project, id);
        }
        _ if !stage.busy() => {}
        Ending::Stopped => held(format!("{name} was stopped before it finished")),
        Ending::Failed(reason) => held(format!("{name}'s turn failed: {reason}")),
        Ending::Gone { reason, .. } => held(match reason {
            Some(reason) if !reason.trim().is_empty() => {
                format!("{name} stopped running: {reason}")
            }
            _ => format!("{name} stopped running"),
        }),
    }
}

/// Whether the agent is in the middle of a turn.
fn mid_turn(core: &Core, agent_id: &str) -> bool {
    if let Some(chat) = core.chats.get(agent_id) {
        return chat.turn_running();
    }
    core.agent_session(agent_id)
        .and_then(|session| session.agent.as_ref().map(|agent| agent.busy()))
        .unwrap_or(false)
}

/// Whether an agent `lead` started, or one that agent started, other than
/// `but`, is still at work or has messages on the way to it.
fn workers_busy(core: &Core, lead: &str, but: &str) -> bool {
    core.running_agents().iter().any(|agent| {
        agent != but
            && core.names.leads(lead, agent)
            && (mid_turn(core, agent) || core.harness.mailbox.has_waiting(agent))
    })
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
                let run = card.auto.as_ref()?;
                (run.agent == lead && matches!(run.stage, Stage::Working | Stage::Fixing))
                    .then(|| (project.clone(), card.id))
            })
        })
    };
    let Some((project, id)) = found else {
        return;
    };
    if mid_turn(core, &lead)
        || core.harness.questions.waits_on_answer(&lead)
        || core.harness.mailbox.has_waiting(&lead)
        || workers_busy(core, &lead, agent_id)
    {
        return;
    }
    finish::ship(core, project, id);
}

/// Whether the card is one of `agent`'s own runs, whose work ships when its
/// turn ends rather than the card closing before it lands.
pub(super) fn own_run(card: &Card, agent: &str) -> bool {
    card.auto
        .as_ref()
        .is_some_and(|run| run.agent == agent && !run.stage.is_over())
}
