//! Taking away the worktree an agent was given. A client first asks what
//! removing it would cost, so it can ask the person; then it removes it with
//! their answers, and the agent goes back to the main checkout.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::connection::blocking;
use super::sandbox::AgentWorktree;
use super::{board, ports, tasks};
use super::{Core, CoreError, CoreResult};
use crate::git::Git;
use crate::protocol::WorkspaceAgent;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeParams {
    agent_id: String,
}

/// What removing an agent's worktree would cost.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Removal {
    branch: String,
    path: PathBuf,
    /// The agent is at work, so it keeps its worktree until its turn ends.
    busy: bool,
    /// Other agents and projects in the worktree, by name, which keep it.
    shared_with: Vec<String>,
    /// Files with changes not committed, which removing it loses.
    dirty_files: usize,
    /// Commits on its branch its base does not have, which deleting the
    /// branch loses.
    unmerged_commits: usize,
}

fn worktree_of(core: &Core, agent_id: &str) -> CoreResult<(WorkspaceAgent, AgentWorktree)> {
    let agent = core
        .workspaces
        .agent(agent_id)
        .ok_or_else(|| CoreError::from("that agent is no longer on this host"))?;
    let worktree = agent
        .worktree
        .clone()
        .and_then(|worktree| serde_json::from_value(worktree).ok())
        .ok_or_else(|| CoreError::from("that agent works in the main checkout"))?;
    Ok((agent, worktree))
}

fn sharers(core: &Core, agent_id: &str, worktree: &Path) -> Vec<String> {
    let projects = core
        .workspaces
        .project_paths()
        .into_iter()
        .filter(|path| path.starts_with(worktree))
        .map(|path| path.to_string_lossy().into_owned());
    let agents = core
        .workspaces
        .agent_ids()
        .into_iter()
        .filter(|id| id != agent_id)
        .filter_map(|id| core.workspaces.agent(&id))
        .filter(|other| other.cwd.starts_with(worktree))
        .map(|other| core.names.name(&other.id).unwrap_or(other.title));
    projects.chain(agents).collect()
}

fn count(output: Result<String, String>) -> usize {
    output.map_or(0, |text| {
        text.lines().filter(|line| !line.is_empty()).count()
    })
}

fn costs(worktree: &AgentWorktree) -> (usize, usize) {
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

fn busy(core: &Core, agent_id: &str) -> bool {
    core.chats
        .get(agent_id)
        .is_some_and(|chat| chat.state_now() != "idle")
}

/// What an agent's worktree shows of its work, as its review lays it out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Review {
    branch: String,
    path: PathBuf,
    /// The first line of what it put on its card as the result.
    summary: Option<String>,
    /// Files changed since it branched, committed or not.
    changed: Vec<String>,
    /// How many of them are not committed.
    uncommitted: usize,
    /// Test results were found.
    tested: bool,
    passed: u64,
    failed: u64,
    /// Where a server it started listens.
    url: Option<String>,
    /// What to tell it about the tests that fail, while some do.
    failures: Option<String>,
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

/// The `agentReview` call.
/// Whether the agent's branch is merged into the one it was cut from: it has commits of its
/// own, and that branch has every one of them.
pub(crate) async fn merged(core: &Arc<Core>, params: WorktreeParams) -> CoreResult<bool> {
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

pub(crate) async fn review(core: &Arc<Core>, params: WorktreeParams) -> CoreResult<Review> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    let summary = board::agent_card(core, &params.agent_id)
        .and_then(|card| card.result)
        .and_then(|result| {
            result
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(str::to_owned)
        });
    let tests = tasks::module_tests(core, worktree.path.to_string_lossy().into_owned()).await?;
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
    Ok(Review {
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

pub(crate) async fn removal(core: &Core, params: WorktreeParams) -> CoreResult<Removal> {
    let (_, worktree) = worktree_of(core, &params.agent_id)?;
    let shared_with = sharers(core, &params.agent_id, &worktree.path);
    let busy = busy(core, &params.agent_id);
    let (dirty_files, unmerged_commits) = {
        let worktree = worktree.clone();
        blocking(move || Ok(costs(&worktree))).await?
    };
    Ok(Removal {
        branch: worktree.branch,
        path: worktree.path,
        busy,
        shared_with,
        dirty_files,
        unmerged_commits,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoveParams {
    agent_id: String,
    /// The person agreed to lose the worktree's uncommitted changes.
    #[serde(default)]
    discard_changes: bool,
    /// The person agreed to lose commits only its branch has.
    #[serde(default)]
    delete_branch: bool,
}

/// What removing did to the branch.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Removed {
    branch_deleted: bool,
}

/// The agent's folder once the worktree is gone: the same place in the main
/// checkout.
fn outside(worktree: &AgentWorktree, cwd: &Path) -> PathBuf {
    match cwd.strip_prefix(&worktree.path) {
        Ok(inside) => worktree.repo.join(inside),
        Err(_) => worktree.repo.clone(),
    }
}

pub(crate) async fn remove(core: &Core, params: RemoveParams) -> CoreResult<Removed> {
    let (agent, worktree) = worktree_of(core, &params.agent_id)?;
    if busy(core, &params.agent_id) {
        return Err("stop the agent's turn before removing its worktree".into());
    }
    if !sharers(core, &params.agent_id, &worktree.path).is_empty() {
        return Err("another agent or project still uses this worktree".into());
    }
    let (discard, delete) = (params.discard_changes, params.delete_branch);
    let deleted = {
        let worktree = worktree.clone();
        blocking(move || {
            let (dirty, unmerged) = costs(&worktree);
            if dirty > 0 && !discard {
                return Err(CoreError::from(format!(
                    "{dirty} uncommitted change{} would be lost",
                    if dirty == 1 { "" } else { "s" }
                )));
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
    let mut patch = Map::new();
    patch.insert("worktree".into(), Value::Null);
    patch.insert(
        "cwd".into(),
        Value::String(
            outside(&worktree, &agent.cwd)
                .to_string_lossy()
                .into_owned(),
        ),
    );
    core.workspaces.update_agent(&params.agent_id, patch)?;
    core.view_changed();
    Ok(Removed {
        branch_deleted: deleted,
    })
}
