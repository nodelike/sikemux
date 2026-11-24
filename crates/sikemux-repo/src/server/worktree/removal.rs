//! Taking away the worktree an agent was given. A client first asks what
//! removing it would cost, so it can ask the person; then it removes it with
//! their answers, and the agent goes back to the main checkout.

use crate::server::agent_now::AgentNowCore;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::worktree::{
    RemoveWorktreeParams, WorktreeParams, WorktreeRemovalInfo, WorktreeRemoved, WorktreeReview,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::build_output::LEFT_UNTOUCHED_FOR;
use super::{branch_exists, left, own, published, take_folder, AgentWorktree};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::ports;
use crate::server::{Core, CoreError, CoreResult};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::workspace::WorkspaceAgent;
use sikemux_wire::protocol::AgentState;

fn worktree_of(core: &Core, agent_id: &str) -> CoreResult<(WorkspaceAgent, AgentWorktree)> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    let worktree = agent
        .worktree
        .clone()
        .ok_or_else(|| CoreError::from("that agent works in the main checkout"))?;
    Ok((agent, worktree))
}

fn sharers(core: &Core, agent_id: &str, worktree: &Path) -> Vec<String> {
    let projects = core
        .workspaces()
        .project_paths()
        .into_iter()
        .filter(|path| path.starts_with(worktree))
        .map(|path| path.to_string_lossy().into_owned());
    let agents = core
        .workspaces()
        .agent_ids()
        .into_iter()
        .filter(|id| id != agent_id)
        .filter_map(|id| core.workspaces().agent(&id))
        .filter(|other| other.cwd.starts_with(worktree))
        .map(|other| core.names().name(&other.id).unwrap_or(other.title));
    projects.chain(agents).collect()
}

fn count(output: Result<String, String>) -> usize {
    output.map_or(0, |text| {
        text.lines().filter(|line| !line.is_empty()).count()
    })
}

/// Whether the worktree already went with its branch, as an agent that slept for days leaves it.
fn gone(worktree: &AgentWorktree) -> bool {
    !worktree.path.exists() && !branch_exists(&worktree.repo, &worktree.branch)
}

fn costs(worktree: &AgentWorktree) -> (usize, usize) {
    if gone(worktree) {
        return (0, 0);
    }
    let dirty = count(Git::at(&worktree.path).text(&["status", "--porcelain"]));
    let base = worktree.base.as_deref().unwrap_or(&worktree.start_sha);
    let unmerged = Git::at(&worktree.repo)
        .text(&[
            "rev-list",
            "--count",
            &format!("{base}..{}", worktree.branch),
        ])
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(1);
    (dirty, unmerged)
}

pub fn busy(core: &Core, agent_id: &str) -> bool {
    let state = core.agent_now(agent_id).chat_state();
    state.is_some_and(|state| state != AgentState::Idle)
}

/// The paths `git status --porcelain` lists, a rename by its new name.
fn status_paths(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.get(3..))
        .map(|path| path.rsplit(" -> ").next().unwrap_or(path).to_owned())
        .collect()
}

fn changes(worktree: &AgentWorktree) -> (Vec<String>, usize) {
    let base = worktree.base.as_deref().unwrap_or(&worktree.start_sha);
    let committed = Git::at(&worktree.path)
        .text(&["diff", "--name-only", &format!("{base}...HEAD")])
        .unwrap_or_default();
    let uncommitted = status_paths(
        &Git::at(&worktree.path)
            .text(&["status", "--porcelain"])
            .unwrap_or_default(),
    );
    let mut changed: Vec<String> = Vec::new();
    for path in committed
        .lines()
        .map(str::to_owned)
        .chain(uncommitted.iter().cloned())
    {
        if !path.is_empty() && !changed.contains(&path) {
            changed.push(path);
        }
    }
    (changed, uncommitted.len())
}

/// The tests that fail, each with its message, as a note asking the agent to fix them.
fn failure_note(modules: &[Value]) -> Option<String> {
    let lines: Vec<String> = modules
        .iter()
        .filter_map(|module| module["failures"].as_array())
        .flatten()
        .map(|failure| {
            let mut line = format!(
                "- {}: {}",
                failure["test"].as_str().unwrap_or_default(),
                failure["message"].as_str().unwrap_or_default()
            );
            if let Some(at) = failure["line"].as_u64() {
                line.push_str(&format!(" (line {at})"));
            }
            line
        })
        .collect();
    (!lines.is_empty()).then(|| {
        format!(
            "These tests fail in your worktree:\n{}\nFix them and run the tests again.",
            lines.join("\n")
        )
    })
}

/// Whether the agent's branch is merged into the one it was cut from: it has commits of its
/// own, and that branch has every one of them.
pub async fn merged(core: &Arc<Core>, params: WorktreeParams) -> CoreResult<bool> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    let Some(base) = worktree.base.clone() else {
        return Ok(false);
    };
    blocking(move || {
        let moved = Git::at(&worktree.repo)
            .text(&["rev-parse", &worktree.branch])
            .is_ok_and(|tip| tip.trim() != worktree.start_sha);
        let unmerged = Git::at(&worktree.repo)
            .text(&[
                "rev-list",
                "--count",
                &format!("{base}..{}", worktree.branch),
            ])
            .ok()
            .and_then(|text| text.trim().parse::<usize>().ok())
            .unwrap_or(1);
        Ok(moved && unmerged == 0)
    })
    .await
}

/// The `agentReview` call.
pub async fn review(core: &Arc<Core>, params: WorktreeParams) -> CoreResult<WorktreeReview> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    let summary =
        (core.repo_above().board_agent_result)(core, &params.agent_id).and_then(|result| {
            result
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(str::to_owned)
        });
    let tests =
        (core.repo_above().tasks_module_tests)(core, worktree.path.to_string_lossy().into_owned())
            .await?;
    let modules = tests.as_array().cloned().unwrap_or_default();
    let sum = |key: &str| -> u64 {
        modules
            .iter()
            .filter_map(|module| module[key].as_u64())
            .sum()
    };
    let (passed, failed) = (sum("passed"), sum("failed"));
    let (changed, uncommitted, url) = {
        let (core, worktree, agent_id) = (core.clone(), worktree.clone(), params.agent_id.clone());
        blocking(move || {
            let (changed, uncommitted) = changes(&worktree);
            Ok((changed, uncommitted, ports::agent_url(&core, &agent_id)))
        })
        .await?
    };
    Ok(WorktreeReview {
        branch: worktree.branch,
        path: worktree.path,
        summary,
        changed,
        uncommitted,
        tested: !modules.is_empty(),
        passed,
        failed,
        url,
        failures: (failed > 0).then(|| failure_note(&modules)).flatten(),
    })
}

pub async fn removal(core: &Core, params: WorktreeParams) -> CoreResult<WorktreeRemovalInfo> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    let shared_with = sharers(core, &params.agent_id, &worktree.path);
    let busy = busy(core, &params.agent_id);
    let (dirty_files, unmerged_commits) = {
        let worktree = worktree.clone();
        blocking(move || Ok(costs(&worktree))).await?
    };
    Ok(WorktreeRemovalInfo {
        branch: worktree.branch,
        path: worktree.path,
        busy,
        shared_with,
        dirty_files,
        unmerged_commits,
    })
}

/// The agent's folder once the worktree is gone: the same place in the main
/// checkout.
fn outside(worktree: &AgentWorktree, cwd: &Path) -> PathBuf {
    match cwd.strip_prefix(&worktree.path) {
        Ok(inside) => worktree.repo.join(inside),
        Err(_) => worktree.repo.clone(),
    }
}

pub async fn remove(core: &Arc<Core>, params: RemoveWorktreeParams) -> CoreResult<WorktreeRemoved> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    if busy(core, &params.agent_id) {
        return Err("stop the agent's turn before removing its worktree".into());
    }
    if !sharers(core, &params.agent_id, &worktree.path).is_empty() {
        return Err("another agent or project still uses this worktree".into());
    }
    let (discard, delete) = (params.discard_changes, params.delete_branch);
    let unmerged = {
        let worktree = worktree.clone();
        blocking(move || {
            let (dirty, unmerged) = costs(&worktree);
            if dirty > 0 && !discard {
                return Err(CoreError::from(format!(
                    "{dirty} uncommitted change{} would be lost",
                    if dirty == 1 { "" } else { "s" }
                )));
            }
            Ok(unmerged)
        })
        .await?
    };
    ports::checkout::removing(core, &worktree.path).await;
    let deleted = {
        let worktree = worktree.clone();
        blocking(move || {
            if gone(&worktree) {
                return Ok(false);
            }
            let path = worktree.path.to_string_lossy().into_owned();
            let mut args = vec!["worktree", "remove"];
            if discard {
                args.push("--force");
            }
            args.push(&path);
            Git::at(&worktree.repo)
                .text(&args)
                .map_err(CoreError::from)?;
            let deleted = unmerged == 0 || delete;
            if deleted {
                Git::at(&worktree.repo)
                    .text(&["branch", "-D", &worktree.branch])
                    .map_err(CoreError::from)?;
            }
            Ok(deleted)
        })
        .await?
    };
    core.workspaces().edit_agent(&params.agent_id, |agent| {
        agent.worktree = None;
        agent.cwd = outside(&worktree, &agent.cwd);
    })?;
    if deleted {
        published::clear_landed(core, &worktree.repo, &worktree.branch);
    }
    Ok(WorktreeRemoved {
        branch_deleted: deleted,
    })
}

/// The work in a worktree no agent works in any more.
#[derive(Clone, Copy, PartialEq)]
pub enum Work {
    /// All on the base branch, so the worktree's own branch goes with it.
    Landed,
    /// Not all on the base branch, so it is kept on a branch first.
    Left,
}

/// Removes the worktree at `tree` that no agent works in any more, when Sikemux made it and it is
/// still there. Git refuses while anything in it is uncommitted, which [`left::keep`] commits
/// first, and one with an ignored file only it has stays, so what goes can be made again.
pub async fn remove_left(core: &Arc<Core>, repo: &Path, tree: &Path, work: Work) {
    let removal = core.build_outputs().removal.lock().await;
    let (main, path) = (repo.to_owned(), tree.to_owned());
    let kept_for = blocking(move || {
        if !own::made_here(&path) {
            return Ok(None);
        }
        Ok(Some(own::only_here(&main, &path)?))
    })
    .await;
    match kept_for {
        Ok(None) => return,
        Ok(Some(None)) => {}
        Ok(Some(Some(file))) => {
            eprintln!(
                "sikemux core: kept the worktree {}, as Git ignores {} and only it has that",
                tree.display(),
                file.display()
            );
            return;
        }
        Err(error) => {
            eprintln!(
                "sikemux core: kept the worktree {}: {error}",
                tree.display()
            );
            return;
        }
    }
    if let Some(holder) = held_now(core, tree, work) {
        eprintln!(
            "sikemux core: kept the worktree {}, as {holder} works in it again",
            tree.display()
        );
        return;
    }
    ports::checkout::removing(core, tree).await;
    let (main, tree) = (repo.to_owned(), tree.to_owned());
    let removed = blocking(move || {
        let path = tree.to_string_lossy().into_owned();
        let branch = match work {
            Work::Landed => published::branch_of(&tree),
            Work::Left => Some(left::keep(&main, &tree).map_err(|why| format!("{path}: {why}"))?),
        };
        take_folder(&main, &tree, work == Work::Left).map_err(CoreError::from)?;
        if let (Some(branch), Work::Landed) = (&branch, work) {
            Git::at(&main)
                .text(&["branch", "-D", branch])
                .map_err(CoreError::from)?;
        }
        Ok((path, branch))
    })
    .await;
    drop(removal);
    match removed {
        Ok((path, Some(branch))) if work == Work::Left => eprintln!(
            "sikemux core: removed the worktree {path}, as nothing touched it for days; its work is on {branch}"
        ),
        Ok((path, branch)) => {
            eprintln!("sikemux core: removed the worktree {path}, as all its work is landed");
            if let Some(branch) = branch {
                published::clear_landed(core, repo, &branch);
            }
        }
        Err(error) => eprintln!("sikemux core: kept a worktree no agent works in: {error}"),
    }
}

/// Who works in `tree` at this moment: an open agent, or one being reopened or woken there. An
/// agent at rest holds only work that is not landed.
fn held_now(core: &Core, tree: &Path, work: Work) -> Option<String> {
    let reopening =
        (core.build_outputs().reopening.locked().iter()).any(|cwd| cwd.starts_with(tree));
    if reopening {
        return Some("an agent being reopened or woken".into());
    }
    (core.workspaces().agent_ids().into_iter())
        .filter_map(|id| core.workspaces().agent(&id))
        .find(|agent| works_in(agent, tree) && !(work == Work::Landed && rests(core, agent)))
        .map(|agent| agent.id)
}

pub(super) fn works_in(agent: &WorkspaceAgent, tree: &Path) -> bool {
    agent.cwd.starts_with(tree)
        || (agent.worktree.as_ref()).is_some_and(|held| held.path.starts_with(tree))
}

/// Whether the agent finished its last turn and has slept since, for days, so it needs its
/// folder only once it wakes, which makes it again.
pub(super) fn rests(core: &Core, agent: &WorkspaceAgent) -> bool {
    agent.asleep
        && core.agent_now(&agent.id).chat_state().is_none()
        && !core.agent_now(&agent.id).waits_on_something()
        && (core.repo_above().turn_state_done_at)(core, &agent.id).is_some_and(|at| {
            epoch_ms().saturating_sub(at) >= LEFT_UNTOUCHED_FOR.as_millis() as u64
        })
}

/// Keeps the folder an agent is being reopened or woken in from removal until this is dropped, by
/// which time the agent's record holds it or the agent is awake. A removal already under way
/// finishes first, so the agent finds the folder gone and makes it again.
pub struct Reopening {
    core: Arc<Core>,
    cwd: PathBuf,
}

pub async fn reopening(core: &Arc<Core>, cwd: &Path) -> Reopening {
    let _removal = core.build_outputs().removal.lock().await;
    core.build_outputs().reopening.locked().push(cwd.to_owned());
    Reopening {
        core: core.clone(),
        cwd: cwd.to_owned(),
    }
}

impl Drop for Reopening {
    fn drop(&mut self) {
        let mut reopening = self.core.build_outputs().reopening.locked();
        if let Some(at) = reopening.iter().position(|cwd| *cwd == self.cwd) {
            reopening.swap_remove(at);
        }
    }
}
