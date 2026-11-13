//! How each agent's turn stands: one function over what the core knows of it, which every agent
//! row and chat head carries, so no client and no other part of the core works a state out again.
//! What it reads of a turn is kept with the turn by `inbox::Turns`, through a restart; where git
//! said the chat's work was is read off the view's path when a turn ends and on each commit.

use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::chat_state::ChatParts;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::repo_state::RepoParts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::turn_state::TakeStepParams;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_view::calls::CallResult;
use sikemux_view::chat::FailureKind;
use sikemux_view::slice::Verdict;
use sikemux_view::turn::{
    ChecksStand, Landed, NextStep, Remedy, TurnCall, TurnDone, TurnFailed, TurnPhase, TurnState,
    TurnStep, TurnStopped, TurnWait, TurnWaitKind, WorkAt,
};
use sikemux_view::workspace::AgentWorktree;
use sikemux_wire::protocol::{AgentStatus, ChatContext, ChatInfo, SessionInfo, FOR_AGENT_URI};

use super::changed::What;
use super::inbox::{self, Fault};
use super::{Core, CoreError, CoreResult};
use crate::git::Git;
use crate::text::clipped_line;

/// The most of the agent's own words a line it is told carries.
const MAX_TOLD_CHARS: usize = 200;

/// Where git said a chat's work was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Work {
    pub changes: WorkAt,
    /// The commit its checkout was at, which the project's check is looked up by.
    pub head: Option<String>,
    /// When that commit was made, in Unix milliseconds.
    pub head_at: Option<u64>,
    pub project: String,
    /// The turn changed a page, and the agent's checkout can be served to look at it.
    pub can_try: bool,
    /// Its landed commits that are its alone and not taken back out yet, each as its short id
    /// and subject, newest first; read only while its work reads landed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub own: Vec<String>,
    /// How many of its landed commits name other agents too, so are not its alone to take out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shared: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// How the agent's latest turn ended, as [`inbox::Turns`] keeps it.
pub(crate) struct Ended {
    /// None while it runs, and once the person stopped it.
    pub at: Option<u64>,
    pub started_at: u64,
    /// When the ask it works on began, once the person or its own lead started a turn of its.
    pub asked_at: Option<u64>,
    /// What its plan left open as it ended by itself.
    pub left: Option<String>,
    /// The commit a deploy to a place before production passed at.
    pub staged: Option<String>,
    pub problem: Option<Problem>,
    pub work: Option<Work>,
}

pub(crate) struct Problem {
    pub said: String,
    pub fault: Option<Fault>,
    pub kind: Option<FailureKind>,
}

/// What the turn of every agent reads of the others, gathered once per view.
pub(crate) struct Gathered {
    /// The permission requests each agent's chat waits on the person for. Calls are kept
    /// apart, by `person_calls`.
    asks: HashMap<String, Vec<Ask>>,
    /// A worker each lead waits on, mid-turn, with when it started.
    workers: HashMap<String, (String, u64)>,
    /// The lead each worker stopped to hear an answer from, with when it asked.
    leads: HashMap<String, (String, u64)>,
    /// The workers of each lead.
    teams: HashMap<String, Vec<String>>,
    /// Owners of slices waiting on the project's check: since when, and how long it usually takes.
    on_check: HashMap<String, (u64, Option<u64>)>,
}

struct Ask {
    id: String,
    question: String,
    at: u64,
}

/// An open agent, as the turns of the others read it.
pub(crate) struct Seen {
    pub id: String,
    pub status: AgentStatus,
    pub name: String,
}

impl Gathered {
    pub(crate) fn of(core: &Core, agents: &[Seen]) -> Self {
        let mut asks: HashMap<String, Vec<Ask>> = HashMap::new();
        for attention in core.chats().attentions() {
            let agent = attention.agent_id.clone();
            asks.entry(agent).or_default().push(ask_of(attention));
        }
        let leads = core.workspaces().agent_leads();
        let mut teams: HashMap<String, Vec<String>> = HashMap::new();
        for (worker, lead) in &leads {
            teams.entry(lead.clone()).or_default().push(worker.clone());
        }
        let mut workers = HashMap::new();
        let working = agents
            .iter()
            .filter(|agent| agent.status == AgentStatus::Working);
        for worker in working {
            let Some(lead) = leads.get(&worker.id) else {
                continue;
            };
            let since = (core.seen().started_at(&worker.id))
                .or_else(|| core.seen().active_at(&worker.id))
                .unwrap_or_else(epoch_ms);
            workers
                .entry(lead.clone())
                .or_insert((worker.name.clone(), since));
        }
        Self {
            asks,
            workers,
            leads: core.harness().questions.waiting_on_leads(),
            teams,
            on_check: super::board::owners_on_check(core),
        }
    }
}

/// What the agent waits on the person for: its oldest permission or call, with how many wait.
fn call_of(core: &Core, gathered: &Gathered, agent_id: &str) -> Option<TurnCall> {
    let asked = gathered.asks.get(agent_id).map_or(&[][..], Vec::as_slice);
    let call = core.person_calls().of_agent(agent_id);
    let count = (asked.len() + call.as_ref().map_or(0, |call| call.count)) as u32;
    let older = |ask: &&Ask| {
        call.as_ref()
            .is_none_or(|call| ask.at <= call.waiting_since)
    };
    let permission = asked.first().filter(older).map(|ask| TurnCall {
        id: ask.id.clone(),
        question: ask.question.clone(),
        waiting_since: ask.at,
        count,
    });
    permission.or_else(|| {
        call.map(|call| TurnCall {
            id: call.id,
            question: call.question,
            waiting_since: call.waiting_since,
            count,
        })
    })
}

/// How every open agent's turn stands, worked out now from the agents `chats` and `sessions` list.
pub(crate) fn all_from(
    core: &Core,
    chats: &[ChatInfo],
    sessions: &[SessionInfo],
) -> HashMap<String, TurnState> {
    let mut seen: Vec<Seen> = (chats.iter())
        .map(|chat| Seen {
            id: chat.agent_id.clone(),
            status: chat.status,
            name: (chat.name.clone())
                .or_else(|| chat.title.clone())
                .unwrap_or_default(),
        })
        .collect();
    seen.extend(
        super::roster::terminals(chats, sessions).filter_map(|(session, agent_id)| {
            Some(Seen {
                id: agent_id.to_owned(),
                status: session.status?,
                name: session.agent_name.clone().unwrap_or_default(),
            })
        }),
    );
    let gathered = Gathered::of(core, &seen);
    (seen.iter())
        .filter_map(|agent| {
            Some((
                agent.id.clone(),
                of(core, &gathered, &agent.id, agent.status)?,
            ))
        })
        .collect()
}

/// How every open agent's turn stands, worked out now.
pub(crate) fn all(core: &Core) -> HashMap<String, TurnState> {
    let (chats, sessions) = core.ranked_infos();
    all_from(core, &chats, &sessions)
}

/// How `agent_id`'s turn stands, worked out now from its own facts alone.
pub(crate) fn now(core: &Core, agent_id: &str) -> Option<TurnState> {
    let mid_turn = |id: &str| core.agent_now(id).mid_turn();
    let status = match mid_turn(agent_id) {
        true => AgentStatus::Working,
        false => AgentStatus::Idle,
    };
    let mut asks: HashMap<String, Vec<Ask>> = HashMap::new();
    if let Some(chat) = core.chats().get(agent_id) {
        asks.insert(
            agent_id.to_owned(),
            (chat.attentions().into_iter()).map(ask_of).collect(),
        );
    }
    let leads = core.workspaces().agent_leads();
    let team: Vec<String> = (leads.iter())
        .filter(|(_, lead)| *lead == agent_id)
        .map(|(worker, _)| worker.clone())
        .collect();
    let worker = (leads.iter()).find(|(worker, lead)| *lead == agent_id && mid_turn(worker));
    let workers = (worker.into_iter())
        .map(|(worker, _)| {
            let since = (core.seen().started_at(worker))
                .or_else(|| core.seen().active_at(worker))
                .unwrap_or_else(epoch_ms);
            let name = core.names().name(worker).unwrap_or_default();
            (agent_id.to_owned(), (name, since))
        })
        .collect();
    let mut on_check = super::board::owners_on_check(core);
    on_check.retain(|owner, _| owner == agent_id);
    let mut leads = core.harness().questions.waiting_on_leads();
    leads.retain(|worker, _| worker == agent_id);
    let gathered = Gathered {
        asks,
        workers,
        leads,
        teams: HashMap::from([(agent_id.to_owned(), team)]),
        on_check,
    };
    of(core, &gathered, agent_id, status)
}

fn ask_of(attention: sikemux_wire::protocol::Attention) -> Ask {
    let choices = attention.choices;
    Ask {
        id: attention.id,
        question: choices.title.or(choices.command).unwrap_or_default(),
        at: attention.at,
    }
}

fn standing(state: TurnPhase, since: u64) -> TurnState {
    TurnState {
        state,
        since,
        on: None,
        wait: None,
        call: None,
        done: None,
        stopped: None,
        failed: None,
        seen: false,
    }
}

/// How the turn of `agent_id`, which shows `status`, stands; none before its first turn.
pub(crate) fn of(
    core: &Core,
    gathered: &Gathered,
    agent_id: &str,
    status: AgentStatus,
) -> Option<TurnState> {
    if let Some(call) = call_of(core, gathered, agent_id) {
        let mut state = standing(TurnPhase::NeedsYou, call.waiting_since);
        state.call = Some(call);
        return Some(state);
    }
    let ended = core.inbox().ending(agent_id);
    if matches!(status, AgentStatus::Working | AgentStatus::Blocked) {
        let since = (core.seen().started_at(agent_id))
            .or_else(|| ended.as_ref().map(|ended| ended.started_at))
            .or_else(|| core.seen().active_at(agent_id))
            .unwrap_or_else(epoch_ms);
        let mut state = standing(TurnPhase::Working, since);
        state.on = (core.chats().get(agent_id)).and_then(|chat| chat.feed.plan_on(since));
        return Some(state);
    }
    let rested_at = (ended.as_ref())
        .and_then(|ended| ended.at)
        .or_else(|| core.seen().active_at(agent_id))
        .unwrap_or_else(epoch_ms);
    if let Some(failed) = ended
        .as_ref()
        .and_then(|ended| failed(core, agent_id, ended))
    {
        let mut state = standing(TurnPhase::Failed, rested_at);
        state.failed = Some(failed);
        return Some(state);
    }
    if let Some(wait) = wait_of(core, gathered, agent_id, rested_at) {
        let mut state = standing(TurnPhase::Waiting, wait.started_at);
        state.wait = Some(wait);
        return Some(state);
    }
    let ended = ended?;
    let Some(at) = ended.at else {
        let mut state = standing(TurnPhase::Stopped, rested_at);
        state.stopped = Some(TurnStopped { left: None });
        return Some(state);
    };
    if let Some(left) = &ended.left {
        let mut state = standing(TurnPhase::Stopped, at);
        state.stopped = Some(TurnStopped {
            left: Some(left.clone()),
        });
        return Some(state);
    }
    let mut state = standing(TurnPhase::Done, at);
    state.done = (ended.work.as_ref()).map(|work| done(core, agent_id, &ended, work));
    if let Some(done) = &mut state.done {
        if let Some(changes) = team_work(core, gathered, agent_id, &ended, done.changes) {
            done.changes = changes;
            done.next.clear();
        }
    }
    state.seen = !core.seen().unread(agent_id);
    Some(state)
}

/// Where a lead's work stands with that of the workers it ran for its current ask, when a
/// worker's is the least advanced of theirs and its own. None when no such worker changed
/// anything, or the lead's own work is no further along, so its own line and buttons stand.
fn team_work(
    core: &Core,
    gathered: &Gathered,
    lead: &str,
    ended: &Ended,
    own: WorkAt,
) -> Option<WorkAt> {
    let asked = ended.asked_at.unwrap_or(ended.started_at);
    let least = (gathered.teams.get(lead)?.iter())
        .filter_map(|worker| core.inbox().worked_since(worker, asked))
        .filter(|changes| advanced(*changes).is_some())
        .min_by_key(|changes| advanced(*changes))?;
    let own_behind = advanced(own).is_some() && advanced(own) <= advanced(least);
    (!own_behind).then_some(least)
}

/// Whether the person or the agent's own lead started its latest turn, which begins a new ask;
/// any other turn, such as a worker's report, a trigger or the board's nudge, carries one on.
pub(crate) fn began_ask(core: &Core, agent_id: &str) -> bool {
    let Some(chat) = core.chats().get(agent_id) else {
        return false;
    };
    let prompt = chat.feed.turn_prompt().unwrap_or_default();
    match sender_of(&prompt) {
        Some(sender) => {
            (core.workspaces().agent_leads().get(agent_id)).is_some_and(|lead| lead == sender)
        }
        None => !chat.feed.person_said().is_empty(),
    }
}

/// The id of the agent whose message `prompt` is, when it is one.
fn sender_of(prompt: &str) -> Option<&str> {
    let (opening, _) = prompt
        .trim_start()
        .strip_prefix("<agent-message ")?
        .split_once('>')?;
    let (_, rest) = opening.split_once(" agent-id=\"")?;
    Some(rest.split_once('"')?.0)
}

/// How far work that changed something has gone; none when nothing changed or it is not known.
fn advanced(changes: WorkAt) -> Option<u8> {
    match changes {
        WorkAt::Uncommitted => Some(0),
        WorkAt::Committed => Some(1),
        WorkAt::Landed => Some(2),
        WorkAt::Staged => Some(3),
        WorkAt::Shipped => Some(4),
        WorkAt::None | WorkAt::Unknown => None,
    }
}

/// What set to wake the agent it waits on, its turn having ended at `rested_at`: CI or a timer
/// it set, its lead's answer, a worker of its at work, or the project's check on its slice.
fn wait_of(core: &Core, gathered: &Gathered, agent_id: &str, rested_at: u64) -> Option<TurnWait> {
    let set = core
        .triggers()
        .wait_of(agent_id, rested_at, |on| core.waits().expected(on))
        .map(|mut wait| {
            let in_line = matches!(wait.kind, TurnWaitKind::Line | TurnWaitKind::InUse);
            if let Some(line) = in_line
                .then(|| core.line().waiting(core, agent_id))
                .flatten()
            {
                wait.what = line.what;
                wait.place = Some(line.place);
                wait.expected_ms = line.expected_ms;
                wait.by = line.by;
            }
            wait
        });
    if set
        .as_ref()
        .is_some_and(|wait| wait.kind == TurnWaitKind::Ci)
    {
        return set;
    }
    let on_agent = (gathered.leads.get(agent_id)).or_else(|| gathered.workers.get(agent_id));
    if let Some((name, since)) = on_agent {
        return Some(TurnWait {
            kind: TurnWaitKind::Agent,
            what: name.clone(),
            started_at: *since,
            expected_ms: None,
            until: None,
            place: None,
            by: None,
        });
    }
    if let Some((since, took)) = gathered.on_check.get(agent_id) {
        return Some(TurnWait {
            kind: TurnWaitKind::Check,
            what: String::new(),
            started_at: *since,
            expected_ms: *took,
            until: None,
            place: None,
            by: None,
        });
    }
    set
}

fn failed(core: &Core, agent_id: &str, ended: &Ended) -> Option<TurnFailed> {
    let problem = ended.problem.as_ref()?;
    let why = problem.kind.unwrap_or(FailureKind::Other);
    let resets_at = core.chats().resets.resumes_at(agent_id);
    let can = match why {
        FailureKind::Limit => vec![Remedy::OtherAccount, Remedy::Retry],
        FailureKind::SignIn => vec![Remedy::SignIn, Remedy::OtherAccount],
        FailureKind::Network if problem.fault == Some(Fault::Retrying) => vec![Remedy::Reconnect],
        FailureKind::Network | FailureKind::Other | FailureKind::Unknown => vec![Remedy::Retry],
    };
    Some(TurnFailed {
        why,
        said: Some(problem.said.clone()).filter(|said| !said.is_empty()),
        resets_at,
        can,
    })
}

/// Where the chat's work is, as git last said, and what can be done with it. Landed work that
/// shipped or went to staging since its commit was made reads so.
fn done(core: &Core, agent_id: &str, ended: &Ended, work: &Work) -> TurnDone {
    let shipped = (core.person_calls().latest_outcome(agent_id))
        .filter(|outcome| outcome.result == CallResult::Shipped)
        .filter(|outcome| work.head_at.is_none_or(|made| outcome.at >= made));
    let staged = ended.staged.is_some() && ended.staged == work.head;
    let changes = match work.changes {
        WorkAt::Landed if shipped.is_some() => WorkAt::Shipped,
        WorkAt::Landed if staged => WorkAt::Staged,
        changes => changes,
    };
    let verdict = (work.changes != WorkAt::Uncommitted)
        .then_some(work.head.as_deref())
        .flatten()
        .and_then(|head| super::board::verdict_on(core, &work.project, head));
    let checks = match verdict {
        Some(Verdict::Passed) => ChecksStand::Pass,
        Some(Verdict::Failed) => ChecksStand::Failed,
        None => ChecksStand::NotChecked,
    };
    let places = super::board::deploys_to(core, &work.project);
    let can_roll_back = shipped.as_ref().is_some_and(|outcome| outcome.roll_back);
    let mut next = next_steps(changes, places);
    next.retain(|next| next.step != TurnStep::RollBack || can_roll_back);
    if work.can_try && changes != WorkAt::None {
        for step in &mut next {
            step.primary = false;
        }
        next.insert(
            0,
            NextStep {
                step: TurnStep::TryIt,
                primary: true,
            },
        );
    }
    TurnDone {
        changes,
        checks,
        tests: None,
        healthy: shipped.and_then(|outcome| outcome.healthy),
        next,
    }
}

/// The steps that take the work on from `changes`, by where the project deploys: before
/// production, and at all. The first that needs no call is the main one.
fn next_steps(changes: WorkAt, (staging, production): (bool, bool)) -> Vec<NextStep> {
    let steps: Vec<TurnStep> = match changes {
        WorkAt::None | WorkAt::Unknown => Vec::new(),
        WorkAt::Uncommitted => vec![TurnStep::Commit],
        WorkAt::Committed => vec![TurnStep::Push],
        WorkAt::Landed => [
            staging.then_some(TurnStep::DeployStaging),
            production.then_some(TurnStep::ShipProduction),
        ]
        .into_iter()
        .flatten()
        .collect(),
        WorkAt::Staged => [production.then_some(TurnStep::ShipProduction)]
            .into_iter()
            .flatten()
            .collect(),
        WorkAt::Shipped => vec![TurnStep::RollBack],
    };
    let primary = steps
        .iter()
        .position(|step| !matches!(step, TurnStep::ShipProduction | TurnStep::RollBack));
    (steps.into_iter().enumerate())
        .map(|(at, step)| NextStep {
            step,
            primary: Some(at) == primary,
        })
        .collect()
}

/// Reads again where the agent's work is in git, off the caller's thread, and tells clients
/// when that moved.
pub(crate) fn read_work(core: &Core, agent_id: &str) {
    let Some(core) = core.arc() else {
        return;
    };
    let agent_id = agent_id.to_owned();
    tokio::task::spawn_blocking(move || keep_work(&core, &agent_id));
}

/// The watch credited the agent with files a command of its wrote: once its turn has ended, where
/// its work is is read again off the watch's thread, since that credit lands only after the write
/// settles.
pub(crate) fn wrote(core: &Core, agent_id: &str) {
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    if core.agent_now(agent_id).mid_turn() || !core.inbox().wrote_after(agent_id) {
        return;
    }
    let agent_id = agent_id.to_owned();
    runtime.spawn_blocking(move || {
        core.inbox().rereading(&agent_id);
        keep_work(&core, &agent_id);
    });
}

fn keep_work(core: &Arc<Core>, agent_id: &str) {
    let Some(work) = work_of(core, agent_id) else {
        return;
    };
    let shows = (work.can_try).then(|| (work.project.clone(), work.changes, work.head.clone()));
    if core.inbox().keep_work(agent_id, work) {
        core.changed(What::View);
    }
    let Some((project, changes, head)) = shows else {
        return;
    };
    let unchecked =
        changes == WorkAt::Uncommitted || super::checks::project_check(core, &project).is_err();
    let started = core.inbox().ending(agent_id).map(|ended| ended.started_at);
    if let (true, Some(turn)) = (unchecked, started) {
        super::pictures::take(core, agent_id, &project, turn, head);
    }
}

fn work_of(core: &Arc<Core>, agent_id: &str) -> Option<Work> {
    let agent = core.workspaces().agent(agent_id);
    let cwd: PathBuf = match core.chats().get(agent_id) {
        Some(chat) => chat.launch.cwd.clone(),
        None => agent.as_ref()?.cwd.clone(),
    };
    let project = (core.names().get(agent_id).map(|named| named.project))
        .unwrap_or_else(|| core.workspaces().project_of(&cwd));
    let handoff = inbox::handoff(core, agent_id);
    let head = Git::quick(&cwd).trimmed(&["rev-parse", "HEAD"]).ok();
    let head_at = Git::quick(&cwd)
        .trimmed(&["log", "-1", "--format=%ct", "HEAD"])
        .ok()
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(|seconds| seconds * 1000);
    let pages = (handoff.files.iter()).any(|file| super::pictures::is_page_file(file));
    let changed =
        inbox::ever_edited(core, &project, agent_id) || core.inbox().changed_in_turn(agent_id);
    let changes = if handoff.uncommitted {
        WorkAt::Uncommitted
    } else if !changed {
        WorkAt::None
    } else {
        let worktree = agent.as_ref().and_then(|agent| agent.worktree.as_ref());
        match head.as_deref() {
            Some(head) if landed(core, &project, worktree, head) => WorkAt::Landed,
            _ => WorkAt::Committed,
        }
    };
    let (own, shared) = match changes {
        WorkAt::Landed => {
            let worktree = agent.as_ref().and_then(|agent| agent.worktree.as_ref());
            own_landed(core, &project, worktree, agent_id)
        }
        _ => (Vec::new(), 0),
    };
    let can_try = pages && changes != WorkAt::None && super::pictures::servable(core, &cwd);
    Some(Work {
        changes,
        head,
        head_at,
        own,
        shared,
        project,
        can_try,
    })
}

/// Whether `head` has landed on the project's branch, as the project counts landing: on that
/// branch once committed, else on its remote. A side branch, pushed or not, has not landed.
pub(crate) fn landed(
    core: &Core,
    project: &str,
    worktree: Option<&AgentWorktree>,
    head: &str,
) -> bool {
    let repo = worktree.map_or(Path::new(project), |worktree| worktree.repo.as_path());
    landing_tip(core, project, repo).is_some_and(|tip| {
        Git::quick(repo)
            .output(&["merge-base", "--is-ancestor", head, &tip])
            .is_ok_and(|output| output.status.success())
    })
}

/// What a commit in `repo` must be on to have landed in `project`, as the project counts
/// landing: its branch once committed, else that branch's remote. None without a branch.
pub(crate) fn landing_tip(core: &Core, project: &str, repo: &Path) -> Option<String> {
    let branch = super::landing::project_branch(repo)?;
    let local = format!("refs/heads/{branch}");
    Some(match super::landing::means_now(core, project).means {
        Landed::Committed => local,
        _ => super::landing::remote_of(repo, &branch).unwrap_or(local),
    })
}

/// What the agent's plan leaves open as its turn ends by itself.
pub(crate) fn left_open(core: &Core, agent_id: &str) -> Option<String> {
    let started = core.inbox().ending(agent_id)?.started_at;
    core.chats().get(agent_id)?.feed.plan_left(started)
}

/// Whether the agent ends its turn on a wake it set itself: CI, a one-time timer, or a worker
/// of its at work.
pub(crate) fn set_a_wake(core: &Core, agent_id: &str) -> bool {
    core.triggers()
        .wait_of(agent_id, epoch_ms(), |_| None)
        .is_some()
        || (core.workspaces().agent_leads().iter())
            .any(|(worker, lead)| lead == agent_id && core.agent_now(worker).mid_turn())
}

/// How a turn that ended stands, in words for agents: the agent's own line, and another agent's
/// in `agent_list`. None while it works or waits on the person, which its status says already.
pub(crate) fn line(turn: &TurnState) -> Option<String> {
    Some(match turn.state {
        TurnPhase::Done => {
            let done = turn.done.as_ref()?;
            let changes = match done.changes {
                WorkAt::None => "no changes",
                WorkAt::Uncommitted => "uncommitted",
                WorkAt::Committed => "committed, not landed",
                WorkAt::Landed => "landed",
                WorkAt::Staged => "on staging",
                WorkAt::Shipped => "shipped",
                WorkAt::Unknown => return None,
            };
            let checks = match done.checks {
                ChecksStand::Pass => "checks pass",
                ChecksStand::Failed => "checks failed",
                ChecksStand::NotChecked | ChecksStand::Unknown => "not checked",
            };
            format!("done, {changes}, {checks}")
        }
        TurnPhase::Stopped => {
            match (turn.stopped.as_ref()).and_then(|stopped| stopped.left.as_ref()) {
                Some(left) => format!(
                    "stopped with this left open: {}; nothing is set to wake it",
                    clipped_line(left, MAX_TOLD_CHARS)
                ),
                None => "stopped before it finished; nothing is set to wake it".to_owned(),
            }
        }
        TurnPhase::Failed => {
            let said = (turn.failed.as_ref()).and_then(|failed| failed.said.as_deref());
            let said = said.map(|said| clipped_line(said, MAX_TOLD_CHARS));
            format!(
                "failed: {}",
                said.as_deref().unwrap_or("the turn did not finish")
            )
        }
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            let on = match wait.kind {
                TurnWaitKind::Ci => "CI",
                TurnWaitKind::Deploy => "a deploy",
                TurnWaitKind::Agent => return Some(format!("waiting on @{}", wait.what)),
                TurnWaitKind::Timer => "a timer",
                TurnWaitKind::Process => "a process",
                TurnWaitKind::Line => "its turn on the host",
                TurnWaitKind::InUse => "something in use",
                TurnWaitKind::Check | TurnWaitKind::Unknown => "the project's check",
            };
            match wait.what.is_empty() {
                true => format!("waiting on {on}"),
                false => format!(
                    "waiting on {on}: {}",
                    clipped_line(&wait.what, MAX_TOLD_CHARS)
                ),
            }
        }
        TurnPhase::Working | TurnPhase::NeedsYou | TurnPhase::Unknown => return None,
    })
}

/// The agent's own line, sent with the person's next message so it knows where its work stands
/// as the person sees it.
pub(crate) fn told(turn: &TurnState) -> Option<ChatContext> {
    Some(ChatContext {
        uri: format!("{FOR_AGENT_URI}turn"),
        title: "Your last turn".to_owned(),
        text: format!(
            "How your last turn stands, as the person sees it: {}.",
            line(turn)?
        ),
    })
}

/// The person takes a next step on an agent's work with one click, one its line offers now: it
/// goes to the agent as a step, not as the person's own words, so it answers no call. Shipping
/// to production goes that way too, so the gate at production holds it and asks the person on
/// its call; this ships nothing itself.
pub(crate) async fn take_step(core: &Arc<Core>, params: TakeStepParams) -> CoreResult<()> {
    let turn = {
        let (core, agent_id) = (core.clone(), params.agent_id.clone());
        super::connection::blocking(move || Ok(now(&core, &agent_id))).await?
    };
    let done = turn.as_ref().and_then(|turn| turn.done.as_ref());
    let offered = match (&turn, params.step) {
        (Some(turn), TurnStep::CarryOn) => turn.state == TurnPhase::Stopped,
        (_, TurnStep::Undo) => done.is_some_and(|done| done.changes == WorkAt::Landed),
        (_, step) => done.is_some_and(|done| done.next.iter().any(|next| next.step == step)),
    };
    if !offered {
        return Err(CoreError::from(
            "That step is not one this agent's work offers now; its line shows the steps it does",
        ));
    }
    let text = match params.step {
        TurnStep::Commit => "Commit your changes.".to_owned(),
        TurnStep::Push => {
            "Land your commits on the project's branch, as the project lands them.".to_owned()
        }
        TurnStep::DeployStaging => "Deploy this to staging.".to_owned(),
        TurnStep::ShipProduction => "Ship this to production.".to_owned(),
        TurnStep::RollBack => "Roll back the last production deploy.".to_owned(),
        TurnStep::CarryOn => "Carry on with what is left.".to_owned(),
        TurnStep::Undo => {
            let (reading, agent_id) = (core.clone(), params.agent_id.clone());
            super::connection::blocking(move || Ok(undo_words(&reading, &agent_id))).await??
        }
        TurnStep::TryIt => return try_it(core, &params.agent_id).await,
        TurnStep::Unknown => {
            return Err(CoreError::from(
                "This step is not something Sikemux can take yet",
            ));
        }
    };
    super::messages::step_from_person(core, params.agent_id, text).await?;
    Ok(())
}

/// Opens the page the agent's checkout serves on its desk, starting the project's process there
/// when nothing serves it yet.
async fn try_it(core: &Arc<Core>, agent_id: &str) -> CoreResult<()> {
    let url = super::pictures::serve(core, agent_id).await?;
    let project = (core.names().get(agent_id).map(|named| named.project)).unwrap_or_default();
    let opened = crate::cli::protocol::HarnessRequest {
        id: format!("try-it-{}", epoch_ms()),
        project,
        agent_id: Some(agent_id.to_owned()),
        method: "browser.navigate".to_owned(),
        params: serde_json::json!({ "url": url, "newTab": true, "report": "outcome" }),
    };
    super::harness::forward(core, opened).await?;
    Ok(())
}

/// The commits of the project's branch that are `agent_id`'s alone and not taken back out: among
/// its latest, those whose signed footer names it and no other agent, less any a revert there
/// names, and its own reverts. Also how many name it with others. Each as its short id and subject.
fn own_landed(
    core: &Core,
    project: &str,
    worktree: Option<&AgentWorktree>,
    agent_id: &str,
) -> (Vec<String>, usize) {
    let repo = worktree.map_or(Path::new(project), |worktree| worktree.repo.as_path());
    let Some(branch) = super::landing::project_branch(repo) else {
        return (Vec::new(), 0);
    };
    let tip = super::landing::remote_of(repo, &branch).unwrap_or(format!("refs/heads/{branch}"));
    let log = |grep: &str, format: &str| {
        Git::quick(repo)
            .text(&["log", "-n", "200", "--fixed-strings", grep, format, &tip])
            .unwrap_or_default()
    };
    let reverted: HashSet<String> = (log("--grep=This reverts commit ", "--format=%b%x1e")
        .split("This reverts commit "))
    .skip(1)
    .filter_map(|rest| rest.get(..40).map(str::to_owned))
    .collect();
    let (mut own, mut shared) = (Vec::new(), 0);
    let named = log(&format!("--grep={agent_id}"), "--format=%H%x1f%s%x1e");
    for entry in named.split('\x1e') {
        let Some((sha, subject)) = entry.trim().split_once('\x1f') else {
            continue;
        };
        if subject.starts_with("Revert \"") || reverted.contains(sha) {
            continue;
        }
        let agents = super::edits::footer::verified(core, repo, sha).unwrap_or_default();
        if !agents.iter().any(|agent| agent.chat == agent_id) {
            continue;
        }
        match agents.iter().all(|agent| agent.chat == agent_id) {
            true => own.push(format!("{} {subject}", &sha[..sha.len().min(9)])),
            false => shared += 1,
        }
    }
    (own, shared)
}

/// What tells a worker to take its landed work back out: its own landed commits, by name. Why
/// not, when it has none of its own.
fn undo_words(core: &Core, agent_id: &str) -> Result<String, String> {
    let work = (core.inbox().ending(agent_id))
        .and_then(|ended| ended.work)
        .ok_or("Sikemux has not read where this agent's work is yet")?;
    if work.own.is_empty() {
        return Err(match work.shared {
            0 => "Sikemux found no commits of this agent's own on the project's branch to take out".to_owned(),
            shared => format!(
                "Its {shared} landed commits name other agents too, so none is this agent's alone to take out"
            ),
        });
    }
    Ok(format!(
        "Take these commits of yours back out of the project's branch, by reverting them there as the project lands changes:\n{}",
        work.own.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::activity::{Activity, ActivityCore};
    use crate::server::workspace::AddProjectParamsExt;
    use crate::server::ServerCore;
    use serde_json::{json, Value};

    #[tokio::test]
    async fn a_finished_turn_is_seen_once_a_focused_client_shows_it() {
        let core = Core::new(Default::default(), None).unwrap();
        core.note_activity("agent", Activity::Started);
        core.note_activity("agent", Activity::Finished { cancelled: false });
        let turn = now(&core, "agent").expect("the turn ended");
        assert_eq!(turn.state, TurnPhase::Done);
        assert!(!turn.seen);

        core.seen().show(1, ["agent".to_owned()], true);
        let turn = now(&core, "agent").expect("the turn ended");
        assert_eq!(turn.state, TurnPhase::Done);
        assert!(turn.seen);
    }

    fn work(changes: WorkAt) -> Work {
        Work {
            changes,
            head: None,
            head_at: None,
            project: "widget".to_owned(),
            can_try: false,
            own: Vec::new(),
            shared: 0,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lead_whose_worker_committed_what_it_asked_for_reads_committed_without_a_button() {
        let core = Core::new(Default::default(), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = crate::server::workspace::add_project(
            &core,
            crate::server::workspace::AddProjectParams::at(dir.path().to_owned()),
        )
        .await
        .expect("the project");
        let worker = json!({
            "id": "worker",
            "provider": "claude",
            "project": project.id,
            "cwd": dir.path(),
            "title": "Fix the header",
            "lead": "lead",
            "permissionMode": "auto",
        });
        let Value::Object(worker) = worker else {
            unreachable!()
        };
        core.workspaces()
            .put_agent_draft(worker)
            .expect("the worker");
        core.note_activity("lead", Activity::Started);
        core.note_activity("worker", Activity::Started);
        core.note_activity("worker", Activity::Finished { cancelled: false });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while (core.inbox().ending("worker")).is_none_or(|ended| ended.work.is_none()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the worker's work was never read"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        core.inbox().keep_work("worker", work(WorkAt::Committed));
        core.note_activity("lead", Activity::Finished { cancelled: false });
        core.inbox().keep_work("lead", work(WorkAt::None));

        let turn = now(&core, "lead").expect("the turn ended");
        assert_eq!(turn.state, TurnPhase::Done);
        let done = turn.done.expect("where the work is");
        assert_eq!(done.changes, WorkAt::Committed);
        assert!(done.next.is_empty());

        core.inbox().keep_work("lead", work(WorkAt::Uncommitted));
        let done = now(&core, "lead").and_then(|turn| turn.done);
        let done = done.expect("where the work is");
        assert_eq!(done.changes, WorkAt::Uncommitted);
        assert!(done.next.iter().any(|next| next.step == TurnStep::Commit));
    }
}
