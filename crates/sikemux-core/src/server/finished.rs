//! Closes an agent once its own facts say it is finished: its work is on the project's branch on
//! its remote, or it changed nothing; its last turn since it opened asked nothing; no agent waits
//! on it; its lead has read how that turn ended; nobody has it on screen; it holds no live browser
//! tab on its desk; the person has not written to it in the last hour; and, for a chat the person
//! started, they have opened its last answer. It runs when one of those facts may have moved,
//! never as a poll.

use crate::server::agent_now::AgentNowCore;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::now::{ClosedWhy, FixAt, NowClosed};
use sikemux_view::turn::{TurnPhase, TurnState, WorkAt};
use sikemux_view::workspace::WorkspaceAgent;

use super::wake::Wake;
use super::Core;
use crate::git::Git;

/// How long an agent the person wrote to stays open after they last did.
const PERSON_KEEPS_MS: u64 = 60 * 60 * 1000;

pub(crate) struct Finishing {
    pub(crate) wake: Wake,
    /// When this core started: the person's writes before it are not known, so none counts as
    /// earlier than this.
    started: u64,
    reading: AtomicBool,
    again: AtomicBool,
    /// Per agent, the start of the long watch it was last told about.
    told_long_watch: Mutex<HashMap<String, u64>>,
    /// Per agent, whether it was last seen waiting or since when it has been idle with work.
    left_seen: Mutex<HashMap<String, super::left_behind::IdleSeen>>,
}

impl Default for Finishing {
    fn default() -> Self {
        Self {
            wake: Wake::default(),
            started: epoch_ms(),
            reading: AtomicBool::new(false),
            again: AtomicBool::new(false),
            told_long_watch: Mutex::default(),
            left_seen: Mutex::default(),
        }
    }
}

impl super::part::Part for Finishing {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![super::part::on_wake(
            core,
            |core| &core.finishing().wake,
            settle,
        )]
    }

    fn client_gone(&self, core: &Arc<Core>, _client: super::connection::ClientId) {
        moved(core);
    }
}

/// Something an agent's being finished rests on may have moved.
pub(crate) fn moved(core: &Core) {
    core.finishing().wake.now();
}

/// How a finished agent's work stood when it closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Left {
    Landed,
    NothingChanged,
}

fn settle(core: &Arc<Core>) {
    let finishing = core.finishing();
    if finishing.reading.swap(true, Ordering::AcqRel) {
        finishing.again.store(true, Ordering::Release);
        return;
    }
    let core = core.clone();
    tokio::task::spawn_blocking(move || {
        let turns = super::turn_state::all(&core);
        let (finished, next) = read(&core, &turns);
        if !finished.is_empty() {
            close(&core, finished);
        }
        let (left, due) = {
            let mut seen = core.finishing().left_seen.locked();
            super::left_behind::read(&core, &turns, &mut seen, epoch_ms())
        };
        super::left_behind::tell(&core, left);
        let (asks, asks_due) = super::left_behind::asks(&core, epoch_ms());
        super::left_behind::tell_chiefs(&core, asks);
        let (long, watch_due) = {
            let mut told = core.finishing().told_long_watch.locked();
            super::long_watch::read(&core, &turns, &mut told, epoch_ms())
        };
        super::long_watch::tell(&core, long);
        if let Some(next) = [next, due, asks_due, watch_due].into_iter().flatten().min() {
            core.finishing().wake.at(next);
        }
        let finishing = core.finishing();
        finishing.reading.store(false, Ordering::Release);
        if finishing.again.swap(false, Ordering::AcqRel) {
            finishing.wake.now();
        }
    });
}

/// The agents finished now, and when the soonest of those the person wrote to lately would be.
fn read(
    core: &Core,
    turns: &HashMap<String, TurnState>,
) -> (Vec<(WorkspaceAgent, Left)>, Option<u64>) {
    let waited = waited_on(core, turns);
    let now = epoch_ms();
    let mut finished = Vec::new();
    let mut next: Option<u64> = None;
    for agent in core.workspaces().agents() {
        let Some(turn) =
            (turns.get(&agent.id).cloned()).or_else(|| super::turn_state::now(core, &agent.id))
        else {
            continue;
        };
        if !at_rest(core, &agent, &turn, &waited) {
            continue;
        }
        let wrote = core.seen().person_at(&agent.id).unwrap_or_default();
        let free_at = wrote.max(core.finishing().started) + PERSON_KEEPS_MS;
        if now < free_at {
            next = Some(next.map_or(free_at, |next| next.min(free_at)));
            continue;
        }
        if let Some(left) = work_left(core, &agent, &turn) {
            finished.push((agent, left));
        }
    }
    (finished, next)
}

/// The open agents some other agent waits on: one an agent's wait names, the lead of an open
/// worker, whose report comes to it, and a lead an open question is put to.
fn waited_on(core: &Core, turns: &HashMap<String, TurnState>) -> HashSet<String> {
    let mut waited: HashSet<String> = (turns.iter())
        .filter_map(|(waiter, turn)| turn.wait.as_ref()?.agent.clone().filter(|on| on != waiter))
        .collect();
    waited.extend(core.workspaces().agent_leads().into_values());
    waited.extend((core.harness().questions.waiting_on_leads().into_values()).map(|lead| lead.id));
    waited
}

/// Whether the agent's last turn ended done and asked nothing, with nothing left to bring it back
/// and nothing of its still running. A chat the person started waits until they have opened its
/// last answer.
fn at_rest(
    core: &Core,
    agent: &WorkspaceAgent,
    turn: &TurnState,
    waited: &HashSet<String>,
) -> bool {
    if agent.terminal
        || super::chief::is_chief(core, &agent.id)
        || waited.contains(&agent.id)
        || core.seen().viewed(&agent.id)
        || super::goals::has_live_page(core, &agent.id)
    {
        return false;
    }
    // A chat taken up again keeps the turn it closed on, which says nothing of it now.
    if turn.since < agent.created_at {
        return false;
    }
    if turn.state != TurnPhase::Done
        || turn.call.is_some()
        || agent.lead.is_none() && core.seen().unread(&agent.id)
    {
        return false;
    }
    if matches!(
        turn.done.as_ref().map(|done| done.changes),
        Some(WorkAt::Uncommitted | WorkAt::Unknown) | None
    ) {
        return false;
    }
    if core.harness().mailbox.report_unread(&agent.id) {
        return false;
    }
    let now = core.agent_now(&agent.id);
    if now.at_work() || now.waits_on_something() {
        return false;
    }
    if core.triggers().count(&agent.id) > 0 || runs(core, &agent.id) {
        return false;
    }
    let answer = (core.chats().heard(&agent.id)).and_then(|feed| feed.last_answer());
    !answer.is_some_and(|answer| super::questions::asks_in(&answer))
}

/// Whether a process the agent started still runs.
fn runs(core: &Core, agent_id: &str) -> bool {
    (core.harness().all_runs().iter())
        .any(|run| run.status.is_active() && run.agent_id.as_deref() == Some(agent_id))
}

/// How the agent left its work, when nothing of it is off the project's branch on its remote.
fn work_left(core: &Core, agent: &WorkspaceAgent, turn: &TurnState) -> Option<Left> {
    if super::spawn::held_work(core, &agent.id).is_some() {
        return None;
    }
    let changed = turn.done.as_ref()?.changes != WorkAt::None;
    if !changed {
        return Some(Left::NothingChanged);
    }
    let repo = match &agent.worktree {
        Some(tree) => tree.repo.clone(),
        None => core.workspaces().project_path(&agent.project).ok()?,
    };
    let repo = repo.as_path();
    let branch = super::landing::project_branch(repo)?;
    let local = format!("refs/heads/{branch}");
    let Some(remote) = super::landing::remote_of(repo, &branch) else {
        return on(&agent.cwd, &local).then_some(Left::Landed);
    };
    let unpushed = Git::quick(repo)
        .text(&[
            "log",
            "-n",
            "1",
            "--fixed-strings",
            &format!("--grep={}", agent.id),
            "--format=%H",
            &format!("{remote}..{local}"),
        ])
        .is_ok_and(|found| !found.trim().is_empty());
    (on(&agent.cwd, &remote) && !unpushed).then_some(Left::Landed)
}

/// Whether the commit checked out in `cwd` is on `tip`.
fn on(cwd: &Path, tip: &str) -> bool {
    Git::quick(cwd)
        .output(&["merge-base", "--is-ancestor", "HEAD", tip])
        .is_ok_and(|output| output.status.success())
}

fn close(core: &Arc<Core>, finished: Vec<(WorkspaceAgent, Left)>) {
    let ids: Vec<String> = finished.iter().map(|(agent, _)| agent.id.clone()).collect();
    let names: Vec<String> = (ids.iter())
        .map(|id| core.names().name(id).unwrap_or_else(|| id.clone()))
        .collect();
    if let Err(error) = super::workspace::close(core, &ids) {
        eprintln!("sikemux core: finished agents stayed open: {error}");
        return;
    }
    for ((agent, left), name) in finished.into_iter().zip(names) {
        let (why, work) = match left {
            Left::Landed => (ClosedWhy::Landed, Some(FixAt::Landed)),
            Left::NothingChanged => (ClosedWhy::NothingChanged, None),
        };
        let entry = NowClosed {
            agent: agent.id,
            name,
            at: epoch_ms(),
            why,
            work,
            lead: agent.lead,
        };
        super::now::closed(core, &agent.project, entry);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::server::ServerCore;

    /// A worker's first turn changed nothing and ended on a finding its lead had to act on. The
    /// lead was mid-turn, so the report waited for it, and 191 ms after the turn ended the worker
    /// was closed as having changed nothing. The lead's list showed it closed 91 s before its
    /// report came, and then had no open agent to answer.
    #[test]
    fn a_worker_stays_open_until_its_lead_has_read_its_report() {
        let core = Core::new(Default::default(), None).unwrap();
        let worker: WorkspaceAgent = serde_json::from_value(json!({
            "id": "agent-otter",
            "provider": "claude",
            "project": "shop",
            "cwd": "/shop.worktrees/feat-mail-domain/",
            "title": "Mail domain verification",
            "lead": "agent-chief",
            "permissionMode": "bypass",
            "worktree": {
                "repo": "/shop",
                "path": "/shop.worktrees/feat-mail-domain",
                "branch": "feat/mail-domain",
                "base": "main",
                "startSha": "1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e",
            },
            "createdAt": 1_795_454_491_472_u64,
        }))
        .unwrap();
        let turn: TurnState = serde_json::from_value(json!({
            "state": "done",
            "since": 1_795_454_507_240_u64,
            "done": { "changes": "none", "checks": "notChecked" },
        }))
        .unwrap();
        let waited = HashSet::new();
        assert!(at_rest(&core, &worker, &turn, &waited));

        let mailbox = &core.harness().mailbox;
        mailbox.hold_report(
            "agent-otter",
            "agent-chief",
            "The DNS is not managed there.",
        );
        assert!(!at_rest(&core, &worker, &turn, &waited));
    }

    fn finished_worker(id: &str) -> (WorkspaceAgent, TurnState) {
        let worker: WorkspaceAgent = serde_json::from_value(json!({
            "id": id,
            "provider": "claude",
            "project": "shop",
            "cwd": "/shop",
            "title": "Open Udyam registration",
            "lead": "agent-chief",
            "permissionMode": "bypass",
            "createdAt": 1_795_454_491_472_u64,
        }))
        .unwrap();
        let turn: TurnState = serde_json::from_value(json!({
            "state": "done",
            "since": 1_795_454_507_240_u64,
            "done": { "changes": "none", "checks": "notChecked" },
        }))
        .unwrap();
        (worker, turn)
    }

    #[test]
    fn a_worker_with_a_live_browser_tab_stays_open_until_the_tab_closes() {
        let core = Core::new(Default::default(), None).unwrap();
        let (worker, turn) = finished_worker("agent-gabi");
        let waited = HashSet::new();
        assert!(at_rest(&core, &worker, &turn, &waited));

        let client = 1;
        let desks = |pages: Vec<sikemux_view::goals::DeskPage>| sikemux_view::goals::DesksParams {
            desks: [(
                worker.id.clone(),
                sikemux_view::goals::AgentDesk {
                    pages,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        };

        super::super::goals::set_desks(
            &core,
            client,
            desks(vec![sikemux_view::goals::DeskPage {
                id: "tab-1".to_owned(),
                title: "Udyam Registration".to_owned(),
                url: "https://udyamregistration.gov.in/UdyamRegistration.aspx".to_owned(),
                background: false,
            }]),
        );
        assert!(!at_rest(&core, &worker, &turn, &waited));

        super::super::goals::set_desks(
            &core,
            client,
            desks(vec![sikemux_view::goals::DeskPage {
                id: "tab-2".to_owned(),
                title: "Background check".to_owned(),
                url: "https://example.com".to_owned(),
                background: true,
            }]),
        );
        assert!(at_rest(&core, &worker, &turn, &waited));

        super::super::goals::set_desks(&core, client, desks(Vec::new()));
        assert!(at_rest(&core, &worker, &turn, &waited));

        super::super::goals::set_desks(
            &core,
            client,
            desks(vec![sikemux_view::goals::DeskPage {
                id: "tab-3".to_owned(),
                title: "Udyam Registration".to_owned(),
                url: "https://udyamregistration.gov.in/UdyamRegistration.aspx".to_owned(),
                background: false,
            }]),
        );
        assert!(!at_rest(&core, &worker, &turn, &waited));
        use crate::server::part::Part;
        core.goals().client_gone(&core, client);
        assert!(at_rest(&core, &worker, &turn, &waited));
    }

    #[test]
    fn an_agent_currently_on_screen_stays_open_until_the_person_switches_away() {
        let core = Core::new(Default::default(), None).unwrap();
        let (worker, turn) = finished_worker("agent-gabi");
        let waited = HashSet::new();
        assert!(at_rest(&core, &worker, &turn, &waited));

        core.seen().show(1, [worker.id.clone()], true);
        assert!(!at_rest(&core, &worker, &turn, &waited));

        core.seen().show(1, [worker.id.clone()], false);
        assert!(!at_rest(&core, &worker, &turn, &waited));

        core.seen().show(1, ["agent-chief".to_owned()], true);
        assert!(at_rest(&core, &worker, &turn, &waited));
    }
}
