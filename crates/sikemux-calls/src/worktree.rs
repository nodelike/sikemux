use serde::{Deserialize, Serialize};
use sikemux_view::workspace::AgentWorktree;
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeParams {
    pub agent_id: String,
}

/// What removing an agent's worktree would cost.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRemovalInfo {
    pub branch: String,
    pub path: PathBuf,
    /// The agent is at work, so it keeps its worktree until its turn ends.
    pub busy: bool,
    /// Other agents and projects in the worktree, by name, which keep it.
    pub shared_with: Vec<String>,
    /// Files with changes not committed, which removing it loses.
    pub dirty_files: usize,
    /// Commits on its branch its base does not have, which deleting the
    /// branch loses.
    pub unmerged_commits: usize,
}

/// What an agent's worktree shows of its work, as its review lays it out.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeReview {
    pub branch: String,
    pub path: PathBuf,
    /// The first line of what it put on its card as the result.
    pub summary: Option<String>,
    /// Files changed since it branched, committed or not.
    pub changed: Vec<String>,
    /// How many of them are not committed.
    pub uncommitted: usize,
    /// Test results were found.
    pub tested: bool,
    pub passed: u64,
    pub failed: u64,
    /// Where a server it started listens.
    pub url: Option<String>,
    /// What to tell it about the tests that fail, while some do.
    pub failures: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveWorktreeParams {
    pub agent_id: String,
    /// The person agreed to lose the worktree's uncommitted changes.
    #[serde(default)]
    pub discard_changes: bool,
    /// The person agreed to lose commits only its branch has.
    #[serde(default)]
    pub delete_branch: bool,
}

/// What removing did to the branch.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRemoved {
    pub branch_deleted: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorktreeParams {
    pub agent_id: String,
    /// What the agent was asked first, which names the branch.
    pub message: String,
    /// The repository to use when the agent's folder holds several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MadeAgentWorktree {
    pub worktree: AgentWorktree,
    pub cwd: PathBuf,
    /// Why the project's worktree setup did not all finish, when it did not.
    pub setup_error: Option<String>,
}
