// Read-only pre-deploy plan. Mirrors `cmd_plan` from the bash CLI: inspect
// the local checkout, the branch the job last deployed successfully, and how
// the two relate, so the user can sanity-check before running the job. Repo
// state is read with git2; refreshing remote refs shells out to `git fetch`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use git2::{BranchType, ErrorCode, Repository};
use serde::Serialize;

use crate::error::RundeckResult;

use crate::executions::{deployed_branch, newest_succeeded};

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const FETCH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Serialize, Clone)]
pub struct PlanResult {
    pub project: String,
    pub service: String,
    pub target_branch: String,

    pub deployed_branch: Option<String>,
    pub branch_relation: BranchRelation,
    pub branch_relation_detail: Option<String>,

    pub git_root: Option<String>,
    pub current_branch: Option<String>,
    pub head_sha: Option<String>,
    pub dirty: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub remote_target_exists: bool,
    pub push_action: PushAction,
}

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BranchRelation {
    Same,
    TargetContainsDeployed,
    TargetMissingDeployed,
    UnknownNoDeployedBranch,
    UnknownDeployedNotOnOrigin,
    UnknownTargetNotOnOrigin,
    UnknownNoRepo,
}

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PushAction {
    #[serde(rename = "will-push-current")]
    PushCurrent,
    #[serde(rename = "will-not-push-different-branch")]
    NotPushDifferentBranch,
    #[serde(rename = "will-not-push-no-repo")]
    NotPushNoRepo,
    #[serde(rename = "will-not-push-detached")]
    NotPushDetached,
}

pub struct PlanRequest {
    pub job_id: String,
    pub project: String,
    pub service: String,
    pub target_branch: String,
    pub repo_path: String,
    pub branch_options: Vec<String>,
}

fn relation_without_repo(deployed: Option<&str>, target: &str) -> BranchRelation {
    match deployed {
        None => BranchRelation::UnknownNoDeployedBranch,
        Some(deployed) if deployed == target => BranchRelation::Same,
        Some(_) => BranchRelation::UnknownNoRepo,
    }
}

/// Compute a read-only deploy plan. An empty `repo_path` skips the git side.
pub async fn plan(request: PlanRequest) -> RundeckResult<PlanResult> {
    let target_branch = request.target_branch.trim().to_string();

    // The git-side analysis is still useful when the deploy history is not.
    let deployed_branch = newest_succeeded(&request.job_id)
        .await
        .ok()
        .flatten()
        .and_then(|execution| deployed_branch(&execution, &request.branch_options));

    let plan = PlanResult {
        project: request.project,
        service: request.service,
        branch_relation: relation_without_repo(deployed_branch.as_deref(), &target_branch),
        target_branch,
        deployed_branch,
        branch_relation_detail: None,
        git_root: None,
        current_branch: None,
        head_sha: None,
        dirty: false,
        upstream: None,
        ahead: None,
        behind: None,
        remote_target_exists: false,
        push_action: PushAction::NotPushNoRepo,
    };

    let repo_path = request.repo_path;
    if repo_path.trim().is_empty() {
        return Ok(plan);
    }
    let fallback = plan.clone();
    tokio::task::spawn_blocking(move || inspect_repo(plan, &repo_path))
        .await
        .unwrap_or(Ok(fallback))
}

/// True when this repo has not been fetched in the last `FETCH_INTERVAL`, and
/// marks it as fetched now.
fn claim_fetch(root: PathBuf) -> bool {
    static LAST_FETCH: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    let mut last = LAST_FETCH
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    if last
        .get(&root)
        .is_some_and(|at| now.duration_since(*at) < FETCH_INTERVAL)
    {
        return false;
    }
    last.insert(root, now);
    true
}

fn fetch_origin(root: &std::path::Path) {
    let mut fetch = std::process::Command::new("git");
    fetch
        .arg("-C")
        .arg(root)
        .args(["fetch", "origin", "--quiet"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(std::process::Stdio::null());
    let _ = sikemux_process::run(&mut fetch, None, FETCH_TIMEOUT, 4 * 1024 * 1024, None);
}

/// Sets the branch and commit HEAD is on. A detached HEAD has no branch; a
/// branch with no commits yet has no commit.
fn read_head(repo: &Repository, plan: &mut PlanResult) -> bool {
    match repo.head() {
        Ok(head) => {
            if let Some(oid) = head.target() {
                plan.head_sha = Some(oid.to_string().chars().take(7).collect());
            }
            let detached = repo.head_detached().unwrap_or(false);
            if !detached {
                plan.current_branch = head.shorthand().ok().map(String::from);
            }
            detached
        }
        Err(error) if error.code() == ErrorCode::UnbornBranch => {
            plan.current_branch = repo.find_reference("HEAD").ok().and_then(|head| {
                head.symbolic_target()
                    .ok()
                    .flatten()
                    .map(|target| target.trim_start_matches("refs/heads/").to_string())
            });
            false
        }
        Err(_) => false,
    }
}

/// Fetches and walks the local checkout, which can take as long as the fetch
/// timeout, so it runs off the async threads.
fn inspect_repo(mut plan: PlanResult, repo_path: &str) -> RundeckResult<PlanResult> {
    let target_branch = plan.target_branch.clone();
    let deployed_branch = plan.deployed_branch.clone();
    let (project, service) = (plan.project.clone(), plan.service.clone());
    let repo = match Repository::discover(repo_path) {
        Ok(r) => r,
        Err(_) => return Ok(plan),
    };
    plan.git_root = repo.workdir().map(|p| p.to_string_lossy().to_string());

    // Best-effort fetch — we want fresh refs but tolerate offline machines.
    let root = repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf();
    if claim_fetch(root.clone()) {
        fetch_origin(&root);
    }

    let detached = read_head(&repo, &mut plan);

    // Dirty tree
    if let Ok(statuses) = repo.statuses(None) {
        plan.dirty = statuses
            .iter()
            .any(|s| !s.status().is_ignored() && !(s.status().is_empty()));
    }

    // Upstream + ahead/behind
    if let Some(branch_name) = &plan.current_branch {
        if let Ok(branch) = repo.find_branch(branch_name, BranchType::Local) {
            if let Ok(upstream) = branch.upstream() {
                if let Some(uname) = upstream.name().ok().flatten() {
                    plan.upstream = Some(uname.to_string());
                    if let (Some(local_oid), Some(up_oid)) = (
                        branch.into_reference().target(),
                        upstream.into_reference().target(),
                    ) {
                        if let Ok((ahead, behind)) = repo.graph_ahead_behind(local_oid, up_oid) {
                            plan.ahead = Some(ahead as u32);
                            plan.behind = Some(behind as u32);
                        }
                    }
                }
            }
        }
    }

    // Remote target ref exists?
    plan.remote_target_exists = repo
        .find_reference(&format!("refs/remotes/origin/{target_branch}"))
        .is_ok();

    // Push action prediction
    plan.push_action = match &plan.current_branch {
        _ if detached => PushAction::NotPushDetached,
        Some(cb) if cb == &target_branch => PushAction::PushCurrent,
        Some(_) => PushAction::NotPushDifferentBranch,
        None => PushAction::NotPushDetached,
    };

    // Branch relation
    plan.branch_relation = match (&deployed_branch, plan.remote_target_exists) {
        (None, _) => BranchRelation::UnknownNoDeployedBranch,
        (Some(d), _) if d == &target_branch => BranchRelation::Same,
        (Some(d), true) => {
            let deployed_ref = repo.find_reference(&format!("refs/remotes/origin/{d}"));
            if deployed_ref.is_err() {
                BranchRelation::UnknownDeployedNotOnOrigin
            } else {
                // Resolve OIDs and check ancestry.
                let target_ref = repo
                    .find_reference(&format!("refs/remotes/origin/{target_branch}"))
                    .ok()
                    .and_then(|r| r.target());
                let deployed_oid = deployed_ref.ok().and_then(|r| r.target());
                match (target_ref, deployed_oid) {
                    (Some(t), Some(d_oid)) => {
                        let is_ancestor =
                            repo.graph_descendant_of(t, d_oid).unwrap_or(false) || t == d_oid;
                        if is_ancestor {
                            BranchRelation::TargetContainsDeployed
                        } else {
                            plan.branch_relation_detail = Some(format!(
                                "Deploying will switch {}/{} to a different line of work.",
                                project, service
                            ));
                            BranchRelation::TargetMissingDeployed
                        }
                    }
                    _ => BranchRelation::UnknownTargetNotOnOrigin,
                }
            }
        }
        (Some(_), false) => BranchRelation::UnknownTargetNotOnOrigin,
    };

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_repo_only_an_exact_match_is_known() {
        assert!(relation_without_repo(Some("main"), "main") == BranchRelation::Same);
        assert!(relation_without_repo(Some("dev"), "main") == BranchRelation::UnknownNoRepo);
        assert!(relation_without_repo(None, "main") == BranchRelation::UnknownNoDeployedBranch);
    }

    #[test]
    fn fetches_a_repo_at_most_once_per_interval() {
        let root = PathBuf::from("/tmp/sikemux-plan-fetch-throttle-test");
        assert!(claim_fetch(root.clone()));
        assert!(!claim_fetch(root));
    }
}
