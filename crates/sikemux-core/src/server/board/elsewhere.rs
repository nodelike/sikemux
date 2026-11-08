//! Auto's agents on linked hosts. Past this host's places, a card's builder starts on a host
//! the project names, on that host's own sign-in. The run still has its worktree here: each
//! time the builder's turn ends its commits come back into it, and shipping, the reviewer and
//! the merge queue read that worktree as they do for an agent here.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;

use super::auto::{self, AutoRun, RunStage, Seat};
use super::finish::{self, Step};
use super::{guard, recover, Board, Card};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::hosts::code;
use crate::server::hosts::route::{pin, stop_worker};
use crate::server::hosts::spawn::{hand_over, AutoTurn, Handed, Handing, LeadParams};
use crate::server::hosts::team::{Foreign, Role};
use crate::server::hosts::{find, name_of};
use crate::server::names::Starting;
use crate::server::spawn::Ending;
use crate::server::worktree;
use crate::server::Core;
use sikemux_wire::protocol::AgentStatus;

/// Long enough for the first bundle of a large repository.
const HANDING_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a host may be out of reach before Auto stops waiting for its agent. A laptop that
/// sleeps for a moment should not cost a card its agent.
const OUT_OF_REACH_MS: u64 = recover::HUNG_AFTER_MS;
const MAX_RESULT_CHARS: usize = 4_000;
const MAX_HOSTS: usize = 16;

/// Where a run's builder works, when that is not this host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunElsewhere {
    /// The key of the linked host.
    pub host: String,
    /// Who Auto goes by on that host, as its builder's lead.
    pub lead: String,
    /// The builder's commit the run's worktree last took in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken: Option<String>,
    /// The branch here its newest commits came back on, until the worktree takes them in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrived: Option<String>,
}

pub(super) fn checked_hosts(hosts: Vec<String>) -> Result<Vec<String>, String> {
    if hosts.len() > MAX_HOSTS {
        return Err(format!("hosts must name at most {MAX_HOSTS} linked hosts"));
    }
    let mut kept: Vec<String> = Vec::with_capacity(hosts.len());
    for host in hosts {
        if !kept.contains(&host) {
            kept.push(host);
        }
    }
    Ok(kept)
}

impl RunElsewhere {
    /// A place on `host`, kept from when the card is picked so a slow start cannot overfill it.
    pub(super) fn on(host: String) -> Self {
        Self {
            host,
            lead: crate::server::names::new_agent_id(),
            taken: None,
            arrived: None,
        }
    }
}

fn busy_on(board: &Board, host: &str) -> usize {
    (board.cards.iter())
        .filter_map(Card::run)
        .filter(|run| run.stage.busy())
        .filter(|run| (run.elsewhere.as_ref()).is_some_and(|elsewhere| elsewhere.host == host))
        .count()
}

/// One entry for each place free on the hosts the project names, in the order it names them.
/// Each host takes as many of Auto's agents as this one does.
pub(super) fn free_places(core: &Core, board: &Board) -> Vec<String> {
    let slots = board.settings.slots as usize;
    (board.settings.hosts.iter())
        .filter(|host| core.remote.link(host).is_some() && core.hosts.reachable(host))
        .flat_map(|host| {
            std::iter::repeat_n(host.clone(), slots.saturating_sub(busy_on(board, host)))
        })
        .collect()
}

fn at_work(core: &Core, worker: &Foreign) -> bool {
    worker.role == Role::Worker
        && !matches!(worker.status, AgentStatus::Stopped | AgentStatus::Asleep)
        && core.hosts.reachable(&worker.host)
}

/// The agents at work on linked hosts as far as this host can tell.
pub(super) fn working(core: &Core) -> impl Iterator<Item = String> + '_ {
    (core.hosts.team.all().into_iter())
        .filter(|worker| at_work(core, worker))
        .map(|worker| worker.agent_id)
}

pub(super) fn works_now(core: &Core, agent: &str) -> bool {
    (core.hosts.team.get(agent)).is_some_and(|worker| at_work(core, &worker))
}

fn out_of_reach(core: &Core, run: &AutoRun) -> bool {
    (run.elsewhere.as_ref()).is_some_and(|elsewhere| {
        !core.hosts.reachable(&elsewhere.host) && core.hosts.team.get(&run.agent).is_some()
    })
}

/// How long the run's agent may be missing before Auto acts on it.
pub(super) fn gone_after(core: &Core, run: &AutoRun, otherwise: u64) -> u64 {
    let building = matches!(run.stage, RunStage::Working | RunStage::Fixing);
    if building && out_of_reach(core, run) {
        OUT_OF_REACH_MS
    } else {
        otherwise
    }
}

/// Why the run's agent no longer counts as running, when its host went out of reach.
pub(super) fn why_gone(core: &Core, run: &AutoRun, name: &str) -> Option<String> {
    let elsewhere = run.elsewhere.as_ref().filter(|_| out_of_reach(core, run))?;
    Some(format!(
        "{name} runs on {}, which has been out of reach for {} minutes",
        name_of(core, &elsewhere.host),
        OUT_OF_REACH_MS / 60_000
    ))
}

fn run_of(core: &Core, agent: &str, matches: impl Fn(&AutoRun) -> bool) -> Option<(String, u64)> {
    let state = core.boards.state.locked();
    state.boards.iter().find_map(|(project, board)| {
        (board.cards.iter()).find_map(|card| {
            let run = card.kept_run()?;
            (run.agent == agent && run.elsewhere.is_some() && matches(run))
                .then(|| (project.clone(), card.id))
        })
    })
}

/// Whether `agent` is the builder of one of Auto's runs on a linked host.
pub(crate) fn holds(core: &Core, agent: &str) -> bool {
    run_of(core, agent, |_| true).is_some()
}

/// Whether `lead` is who Auto goes by on a linked host, for one of its runs.
pub(crate) fn leads_a_run(core: &Core, lead: &str) -> bool {
    let state = core.boards.state.locked();
    (state.boards.values())
        .flat_map(|board| board.cards.iter())
        .filter_map(Card::kept_run)
        .filter_map(|run| run.elsewhere.as_ref())
        .any(|elsewhere| elsewhere.lead == lead)
}

/// Starts the card's builder on `host`, from `tree` here. The worktree goes when it does not
/// start.
pub(super) async fn start(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    agent: &str,
    place: RunElsewhere,
    tree: worktree::Tree,
    prompt: String,
) -> Result<(), String> {
    let started = start_on(core, project, id, agent, place, &tree, prompt).await;
    if !matches!(started, Ok(true)) {
        let _ = blocking(move || {
            worktree::remove_with_remote(&tree.repo, &tree.path, &tree.branch, true);
            Ok(())
        })
        .await;
    }
    started.map(drop)
}

/// False when the run moved on while its builder started, which then stops again.
async fn start_on(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    agent: &str,
    place: RunElsewhere,
    tree: &worktree::Tree,
    prompt: String,
) -> Result<bool, String> {
    let settings = {
        let state = core.boards.state.locked();
        let board = state.boards.get(project).ok_or("the board is gone")?;
        board.settings.clone()
    };
    let link = find(core, &place.host)?;
    let (packed, provider, mode): (code::Packed, String, _) = {
        let (core, cwd, settings) = (core.clone(), tree.cwd.clone(), settings.clone());
        let folder = Path::new(project)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        blocking(move || {
            let (launch, _) = core.workspaces.board_launch(
                settings.launcher.as_deref(),
                &cwd,
                None,
                settings.model.clone(),
                auto::PICKED_IN,
            )?;
            let mut packed = code::package(&cwd)?;
            packed.code.project = folder.unwrap_or(packed.code.project);
            Ok((packed, launch.provider, launch.permission_mode))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (named, _) = core.names.give(
        Starting {
            agent_id: agent,
            project: project.to_owned(),
            provider: Some(&provider),
            resume_id: None,
        },
        core.open_agents(),
    );
    core.names.count_as_started(agent);
    let Handed { name, .. } = hand_over(
        core,
        &link,
        Handing {
            agent_id: agent.to_owned(),
            name: named.name,
            provider: provider.clone(),
            project: project.to_owned(),
            lead: LeadParams {
                agent_id: place.lead.clone(),
                name: "Auto".to_owned(),
                cli: provider.clone(),
                mode: Some(mode.as_str().to_owned()),
                depth: 0,
            },
            model: settings.model.clone(),
            reports: true,
            prompt,
            branch: Some(tree.branch.clone()),
            packed,
            auto: true,
            wait: HANDING_WAIT,
        },
    )
    .await?;
    let seated = auto::seat(
        core,
        project,
        id,
        agent,
        agent,
        Seat {
            name: &name,
            provider,
            elsewhere: Some(place),
        },
        tree,
    );
    if !seated {
        stop_worker(core, agent);
        return Ok(false);
    }
    finish::change_run(core, project, id, agent, |_, run| {
        run.note = Some(format!("Its agent runs on {}", link.name));
    });
    Ok(true)
}

/// How a turn of a builder on `host` ended. Its commits got here first, so this host treats
/// the ending as it does one of its own agents'.
pub(crate) fn turn_ended(core: &Arc<Core>, host: &str, turn: AutoTurn) -> Result<Value, String> {
    let AutoTurn {
        agent_id,
        ending,
        answer,
        cost_usd,
    } = turn;
    if (core.hosts.team.get(&agent_id)).is_none_or(|worker| worker.host != host) {
        return Ok(Value::Null);
    }
    let mut landed = None;
    core.hosts
        .team
        .update(&agent_id, |worker| landed = worker.landed.take());
    if let Some((project, id)) = run_of(core, &agent_id, |_| true) {
        core.boards.edit_if(&project, id, |card| {
            if let (Some(elsewhere), Some((branch, _))) = (
                card.auto.as_mut().and_then(|run| run.elsewhere.as_mut()),
                landed.take(),
            ) {
                elsewhere.arrived = Some(branch);
            }
            if let (Ending::Finished, Some(answer)) = (&ending, answer) {
                card.result = Some(recover::clipped(&answer, MAX_RESULT_CHARS));
            }
            Some(())
        });
    }
    if let Some(cost) = cost_usd {
        guard::heard_cost(core, &agent_id, cost);
    }
    auto::turn_ended(core, &agent_id, &ending);
    if matches!(ending, Ending::Gone { .. }) {
        core.hosts.team.remove(&agent_id);
        pin(core);
    }
    Ok(Value::Null)
}

/// Puts what the builder sent on the run's worktree. Nothing is lost when this fails: the
/// commits stay on the branch they came back on.
pub(super) async fn take_in(core: &Arc<Core>, step: &Step) -> Result<(), String> {
    let Some(elsewhere) = step.run.elsewhere.as_ref() else {
        return Ok(());
    };
    let Some(branch) = elsewhere.arrived.clone() else {
        return Ok(());
    };
    let (tree, start, taken) = (
        step.tree(),
        step.run.start_sha.clone(),
        elsewhere.taken.clone(),
    );
    let from_branch = branch.clone();
    let head = blocking(move || {
        bring_in(&tree, &from_branch, taken.as_deref(), &start).map_err(Into::into)
    })
    .await
    .map_err(|error| {
        format!(
            "Its commits came back on the branch {branch} but did not go on its worktree: {error}"
        )
    })?;
    finish::change_run(core, &step.project, step.id, step.agent(), |_, run| {
        if let Some(elsewhere) = run.elsewhere.as_mut() {
            elsewhere.taken = Some(head);
            elsewhere.arrived = None;
        }
    });
    Ok(())
}

/// The worktree moves to the builder's commits when it has nothing the builder lacks. When
/// Auto rebased it meanwhile only the builder's new commits go on top; when the builder
/// rebased, its branch replaces the worktree's.
fn bring_in(tree: &Path, branch: &str, taken: Option<&str>, start: &str) -> Result<String, String> {
    let git = || Git::slow(tree);
    let head = git().trimmed(&["rev-parse", &format!("{branch}^{{commit}}")])?;
    let descends = |from: &str| {
        git()
            .trimmed(&["merge-base", "--is-ancestor", from, &head])
            .is_ok()
    };
    let from = taken.unwrap_or(start);
    if descends("HEAD") {
        git().trimmed(&["merge", "--ff-only", "--quiet", &head])?;
    } else if descends(from) {
        let mut args = code::identity(tree);
        let range = format!("{from}..{head}");
        args.extend(["cherry-pick", range.as_str()]);
        if let Err(error) = git().trimmed(&args) {
            let _ = git().trimmed(&["cherry-pick", "--abort"]);
            return Err(error);
        }
    } else {
        git().trimmed(&["reset", "--hard", "--quiet", &head])?;
    }
    let _ = git().trimmed(&["branch", "-D", branch]);
    Ok(head)
}
