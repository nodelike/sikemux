//! Wakes whoever should land an idle agent's commits that never landed: the agent that started
//! it, else the project's chief once that lead has closed. It waits a few minutes after the turn
//! ends and tells once per idle stretch. An agent the person started is left to its row, which
//! says "committed, not landed" already.

use crate::server::agent_now::AgentNowCore;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sikemux_view::turn::{TurnPhase, TurnState, WorkAt};
use sikemux_view::workspace::WorkspaceAgent;

use super::Core;
use crate::git::Git;

const GRACE_MS: u64 = 5 * 60 * 1000;
const MOST_LISTED: usize = 10;

pub(crate) struct Left {
    agent: String,
    to: String,
    text: String,
}

/// The idle agents whose commits are due to be told of now, and when the next one falls due.
/// `told` keeps, per agent, the end of the turn it was last told about.
pub(crate) fn read(
    core: &Core,
    turns: &HashMap<String, TurnState>,
    told: &mut HashMap<String, u64>,
    now: u64,
) -> (Vec<Left>, Option<u64>) {
    told.retain(|agent, _| core.workspaces().agent(agent).is_some());
    let mut left = Vec::new();
    let mut next: Option<u64> = None;
    for agent in core.workspaces().agents() {
        let Some(turn) = turns.get(&agent.id) else {
            continue;
        };
        if !idle_with_commits(core, &agent.id, turn) || told.get(&agent.id) == Some(&turn.since) {
            continue;
        }
        let due = turn.since + GRACE_MS;
        if now < due {
            next = Some(next.map_or(due, |next| next.min(due)));
            continue;
        }
        let Some(to) = who_lands(core, &agent) else {
            continue;
        };
        told.insert(agent.id.clone(), turn.since);
        if let Some(text) = told_text(core, &agent, &to) {
            left.push(Left {
                agent: agent.id,
                to,
                text,
            });
        }
    }
    (left, next)
}

pub(crate) fn tell(core: &Arc<Core>, left: Vec<Left>) {
    for left in left {
        super::messages::report_unlanded(core, left.agent, left.to, left.text);
    }
}

fn idle_with_commits(core: &Core, agent_id: &str, turn: &TurnState) -> bool {
    let committed = turn.state == TurnPhase::Done
        && turn.call.is_none()
        && (turn.done.as_ref()).is_some_and(|done| done.changes == WorkAt::Committed);
    if !committed {
        return false;
    }
    let now = core.agent_now(agent_id);
    !now.at_work() && !now.waits_on_something() && !super::turn_state::set_a_wake(core, agent_id)
}

/// The agent that started this one while it is open, else the project's chief. None for an
/// agent the person started.
fn who_lands(core: &Core, agent: &WorkspaceAgent) -> Option<String> {
    let lead = agent.lead.as_ref()?;
    if core.open_agents().contains(lead) {
        return Some(lead.clone());
    }
    super::chief::chief_over(core, &agent.id)
}

fn told_text(core: &Core, agent: &WorkspaceAgent, to: &str) -> Option<String> {
    let project = core.workspaces().project_path(&agent.project).ok()?;
    let repo: PathBuf = (agent.worktree.as_ref()).map_or(project.clone(), |tree| tree.repo.clone());
    let tip = super::turn_state::landing_tip(core, &project.to_string_lossy(), &repo)?;
    let listed = Git::quick(&agent.cwd)
        .text(&[
            "log",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            "--format=%h %s",
            &format!("{tip}...HEAD"),
        ])
        .ok()?;
    let commits: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    if commits.is_empty() {
        return None;
    }
    let mut list: Vec<String> = (commits.iter().take(MOST_LISTED))
        .map(|commit| format!("- {commit}"))
        .collect();
    if commits.len() > MOST_LISTED {
        list.push(format!("- and {} more", commits.len() - MOST_LISTED));
    }
    let branch = (tip.strip_prefix("refs/heads/"))
        .or_else(|| tip.strip_prefix("refs/remotes/"))
        .unwrap_or(&tip);
    let whose = match agent.lead.as_deref() == Some(to) {
        true => "It",
        false => "Its lead has closed, so this comes to you as the project's chief. It",
    };
    Some(format!(
        "{whose} has been idle for {} minutes with {} that are not on {branch}:\n{}\n\nLand them (ask it to run git_commit with land: true, or land them yourself), or close it if they are not wanted. You hear this once until it works again.",
        GRACE_MS / 60_000,
        match commits.len() {
            1 => "a commit".to_owned(),
            count => format!("{count} commits"),
        },
        list.join("\n"),
    ))
}
