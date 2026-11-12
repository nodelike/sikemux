//! How each agent's turn stands: one function over what the core knows of it, which every agent
//! row and chat head carries, so no client and no other part of the core works a state out again.
//! Nothing here is stored but where git said the agent's work was when its turn ended, read off
//! the view's path and again on each commit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::calls::CallResult;
use sikemux_view::chat::FailureKind;
use sikemux_view::inbox::AgentShown;
use sikemux_view::slice::Verdict;
use sikemux_view::turn::{
    ChecksStand, Landed, NextStep, Remedy, TurnCall, TurnDone, TurnFailed, TurnPhase, TurnState,
    TurnStep, TurnStopped, TurnWait, TurnWaitKind, WorkAt,
};
use sikemux_view::workspace::AgentWorktree;
use sikemux_wire::protocol::{AgentStatus, ChatContext, FOR_AGENT_URI};

use super::changed::What;
use super::inbox::{self, Fault};
use super::{Core, CoreError, CoreResult};
use crate::git::Git;

/// Where git said an agent's work was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Work {
    pub changes: WorkAt,
    /// The commit its checkout was at, which the project's check is looked up by.
    pub head: Option<String>,
    pub project: String,
}

/// How the agent's latest turn ended, as [`inbox::Turns`] keeps it.
pub(crate) struct Ended {
    /// None while it runs, and once the person stopped it.
    pub at: Option<u64>,
    pub started_at: u64,
    /// A deploy of its went to a place before production during it, and passed.
    pub staged: bool,
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
    /// Owners of slices waiting on the project's check: since when, and how long it usually takes.
    on_check: HashMap<String, (u64, Option<u64>)>,
}

struct Ask {
    id: String,
    question: String,
    at: u64,
}

impl Gathered {
    pub(crate) fn of(core: &Core, agents: &[AgentShown]) -> Self {
        let mut asks: HashMap<String, Vec<Ask>> = HashMap::new();
        for attention in core.chats.attentions() {
            let choices = attention.choices;
            asks.entry(attention.agent_id).or_default().push(Ask {
                id: attention.id,
                question: choices.title.or(choices.command).unwrap_or_default(),
                at: attention.at,
            });
        }
        let leads = core.workspaces.agent_leads();
        let mut workers = HashMap::new();
        let working = agents
            .iter()
            .filter(|agent| agent.status == AgentStatus::Working);
        for worker in working {
            let Some(lead) = leads.get(&worker.agent_id) else {
                continue;
            };
            let since = core.seen.started_at(&worker.agent_id).unwrap_or_default();
            let name = worker.name.clone().unwrap_or_else(|| worker.title.clone());
            workers.entry(lead.clone()).or_insert((name, since));
        }
        Self {
            asks,
            workers,
            on_check: super::board::owners_on_check(core),
        }
    }
}

/// What the agent waits on the person for: its oldest permission or call, with how many wait.
fn call_of(core: &Core, gathered: &Gathered, agent_id: &str) -> Option<TurnCall> {
    let asked = gathered.asks.get(agent_id).map_or(&[][..], Vec::as_slice);
    let call = core.person_calls.of_agent(agent_id);
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
    let chat = core.chats.get(agent_id);
    if matches!(status, AgentStatus::Working | AgentStatus::Blocked) {
        let since = (core.seen.started_at(agent_id))
            .or_else(|| core.inbox.ending(agent_id).map(|ended| ended.started_at))
            .unwrap_or_default();
        let mut state = standing(TurnPhase::Working, since);
        state.on = chat.and_then(|chat| chat.feed.plan_steps(since).0);
        return Some(state);
    }
    let ended = core.inbox.ending(agent_id);
    let rested_at = (ended.as_ref())
        .and_then(|ended| ended.at)
        .or_else(|| core.seen.active_at(agent_id))
        .unwrap_or_default();
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
    let left = chat.and_then(|chat| chat.feed.plan_steps(ended.started_at).1);
    if let Some(left) = left {
        let mut state = standing(TurnPhase::Stopped, at);
        state.stopped = Some(TurnStopped { left: Some(left) });
        return Some(state);
    }
    let mut state = standing(TurnPhase::Done, at);
    state.done = Some(done(core, agent_id, &ended));
    Some(state)
}

/// What set to wake the agent it waits on, its turn having ended at `rested_at`: CI or a timer
/// it set, a worker of its at work, or the project's check on its slice.
fn wait_of(core: &Core, gathered: &Gathered, agent_id: &str, rested_at: u64) -> Option<TurnWait> {
    let set = core
        .triggers
        .wait_of(agent_id, rested_at, |on| core.waits.expected(on));
    if set
        .as_ref()
        .is_some_and(|wait| wait.kind == TurnWaitKind::Ci)
    {
        return set;
    }
    if let Some((name, since)) = gathered.workers.get(agent_id) {
        return Some(TurnWait {
            kind: TurnWaitKind::Agent,
            what: name.clone(),
            started_at: *since,
            expected_ms: None,
            until: None,
        });
    }
    if let Some((since, took)) = gathered.on_check.get(agent_id) {
        return Some(TurnWait {
            kind: TurnWaitKind::Check,
            what: String::new(),
            started_at: *since,
            expected_ms: *took,
            until: None,
        });
    }
    set
}

fn failed(core: &Core, agent_id: &str, ended: &Ended) -> Option<TurnFailed> {
    let problem = ended.problem.as_ref()?;
    let why = problem.kind.unwrap_or(FailureKind::Other);
    let resets_at = core.chats.resets.resumes_at(agent_id);
    let can = match why {
        FailureKind::Limit => vec![Remedy::OtherAccount, Remedy::Retry],
        FailureKind::SignIn => vec![Remedy::SignIn, Remedy::OtherAccount],
        FailureKind::Network if problem.fault == Some(Fault::Retrying) => vec![Remedy::Reconnect],
        FailureKind::Network | FailureKind::Other => vec![Remedy::Retry],
    };
    Some(TurnFailed {
        why,
        said: Some(problem.said.clone()).filter(|said| !said.is_empty()),
        resets_at,
        can,
    })
}

fn done(core: &Core, agent_id: &str, ended: &Ended) -> TurnDone {
    let work = ended.work.as_ref();
    let shipped = (core.person_calls.latest_outcome(agent_id))
        .filter(|outcome| outcome.result == CallResult::Shipped)
        .filter(|outcome| outcome.at >= ended.started_at);
    let landed = work.map_or(WorkAt::None, |work| work.changes);
    let changes = match (&shipped, ended.staged) {
        (Some(_), _) => WorkAt::Shipped,
        (None, true) if landed == WorkAt::Landed => WorkAt::Staged,
        (None, _) => landed,
    };
    let verdict = work.and_then(|work| {
        let head = work.head.as_deref()?;
        super::board::verdict_on(core, &work.project, head)
    });
    let checks = match verdict {
        Some(Verdict::Passed) => ChecksStand::Pass,
        Some(Verdict::Failed) => ChecksStand::Failed,
        None => ChecksStand::NotChecked,
    };
    let places = work.map(|work| super::board::deploys_to(core, &work.project));
    let can_roll_back = shipped.as_ref().is_some_and(|outcome| outcome.roll_back);
    let mut next = next_steps(changes, places.unwrap_or_default());
    next.retain(|next| next.step != TurnStep::RollBack || can_roll_back);
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
    tokio::task::spawn_blocking(move || {
        let Some(work) = work_of(&core, &agent_id) else {
            return;
        };
        if core.inbox.keep_work(&agent_id, work) {
            core.changed(What::View);
        }
    });
}

fn work_of(core: &Arc<Core>, agent_id: &str) -> Option<Work> {
    let agent = core.workspaces.agent(agent_id);
    let cwd: PathBuf = match core.chats.get(agent_id) {
        Some(chat) => chat.launch.cwd.clone(),
        None => agent.as_ref()?.cwd.clone(),
    };
    let project = (core.names.get(agent_id).map(|named| named.project))
        .unwrap_or_else(|| core.workspaces.project_of(&cwd));
    let handoff = inbox::handoff(core, agent_id);
    let head = Git::quick(&cwd).trimmed(&["rev-parse", "HEAD"]).ok();
    let changes = if handoff.uncommitted {
        WorkAt::Uncommitted
    } else if handoff.edited == 0 && !core.inbox.changed_in_turn(agent_id) {
        WorkAt::None
    } else {
        let worktree = agent.and_then(|agent| agent.worktree);
        match head.as_deref() {
            Some(head) if landed(core, &project, &cwd, worktree.as_ref(), head) => WorkAt::Landed,
            _ => WorkAt::Committed,
        }
    };
    Some(Work {
        changes,
        head,
        project,
    })
}

/// Whether `head` has landed on the project's branch, as the project counts landing: on the
/// branch itself once committed, else on its remote.
fn landed(
    core: &Core,
    project: &str,
    cwd: &Path,
    worktree: Option<&AgentWorktree>,
    head: &str,
) -> bool {
    let (repo, branch) = match worktree {
        Some(worktree) => match &worktree.base {
            Some(base) => (worktree.repo.as_path(), base.clone()),
            None => return false,
        },
        None => match Git::quick(cwd).trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Ok(branch) => (cwd, branch),
            Err(_) => return false,
        },
    };
    let holds = |tip: &str| {
        Git::quick(repo)
            .output(&["merge-base", "--is-ancestor", head, tip])
            .is_ok_and(|output| output.status.success())
    };
    let upstream = Git::quick(repo)
        .trimmed(&[
            "rev-parse",
            "--symbolic-full-name",
            &format!("{branch}@{{upstream}}"),
        ])
        .ok()
        .filter(|name| name.starts_with("refs/remotes/"));
    let on_commit = super::landing::means_now(core, project).means == Landed::Committed;
    match upstream.filter(|_| !on_commit) {
        Some(upstream) => holds(&upstream),
        None => holds(&format!("refs/heads/{branch}")),
    }
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
                Some(left) => {
                    format!("stopped with this left open: {left}; nothing is set to wake it")
                }
                None => "stopped before it finished; nothing is set to wake it".to_owned(),
            }
        }
        TurnPhase::Failed => {
            let said = (turn.failed.as_ref()).and_then(|failed| failed.said.as_deref());
            format!("failed: {}", said.unwrap_or("the turn did not finish"))
        }
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            let on = match wait.kind {
                TurnWaitKind::Ci => "CI",
                TurnWaitKind::Deploy => "a deploy",
                TurnWaitKind::Agent => return Some(format!("waiting on @{}", wait.what)),
                TurnWaitKind::Timer => "a timer",
                TurnWaitKind::Process => "a process",
                TurnWaitKind::Check | TurnWaitKind::Unknown => "the project's check",
            };
            match wait.what.is_empty() {
                true => format!("waiting on {on}"),
                false => format!("waiting on {on}: {}", wait.what),
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

/// Each agent's turn as the view last worked it out, for the core's other readers.
#[derive(Default)]
pub(crate) struct Latest(Mutex<HashMap<String, TurnState>>);

impl Latest {
    pub(crate) fn keep(&self, agents: &[AgentShown]) {
        let kept = (agents.iter())
            .filter_map(|agent| Some((agent.agent_id.clone(), agent.turn.clone()?)))
            .collect();
        *self.0.locked() = kept;
    }

    pub(crate) fn of(&self, agent_id: &str) -> Option<TurnState> {
        self.0.locked().get(agent_id).cloned()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeStepParams {
    pub agent_id: String,
    pub step: TurnStep,
}

/// The person takes the next step on an agent's work with one click: it goes to the agent as
/// their message. Shipping to production goes that way too, so the gate at production holds
/// it and asks the person on its call; this ships nothing itself.
pub(crate) async fn take_step(core: &Arc<Core>, params: TakeStepParams) -> CoreResult<()> {
    let text = match params.step {
        TurnStep::Commit => "Commit your changes.",
        TurnStep::Push => "Land your commits on the project's branch, as the project lands them.",
        TurnStep::DeployStaging => "Deploy this to staging.",
        TurnStep::ShipProduction => "Ship this to production.",
        TurnStep::RollBack => "Roll back the last production deploy.",
        TurnStep::CarryOn => "Carry on with what is left.",
        TurnStep::TryIt | TurnStep::Unknown => {
            return Err(CoreError::from(
                "This step is not something Sikemux can take yet",
            ));
        }
    };
    super::chat::calls::prompt_chat(
        core,
        super::chat::calls::PromptChatParams::new(params.agent_id, text.to_owned()),
    )
    .await
}
