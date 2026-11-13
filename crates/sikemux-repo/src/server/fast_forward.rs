//! Keeping each project's checkout of its default branch up to date with the remote while
//! nobody works in it. Every so often the core fetches that branch and fast-forwards the
//! checkout, only when it has no changes, no untracked files, no commits of its own and no
//! merge or rebase under way, and no agent is at work in it. Anything else, it leaves alone.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::connection::blocking;
use super::Core;
use crate::git::Git;
use sikemux_process::procs::lowest_priority;

/// How long after the core starts it first looks, then how often it looks again.
const FIRST_LOOK: Duration = Duration::from_secs(2 * 60);
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);

/// Files git leaves in a repository's folder while it is partway through changing it.
const UNDER_WAY: [&str; 6] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
    "index.lock",
];

pub struct FastForward;

impl super::part::Part for FastForward {
    fn start(&self, core: &Arc<Core>) -> Vec<JoinHandle<()>> {
        vec![tokio::spawn(serve(core.clone()))]
    }
}

async fn serve(core: Arc<Core>) {
    tokio::time::sleep(FIRST_LOOK).await;
    let mut ticker = tokio::time::interval(LOOK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let projects = core.workspaces().project_paths();
        let core = core.clone();
        let _ = blocking(move || {
            lowest_priority();
            let mut checkouts: Vec<PathBuf> = (projects.iter())
                .filter_map(|project| {
                    Git::quick(project)
                        .trimmed(&["rev-parse", "--show-toplevel"])
                        .ok()
                })
                .map(PathBuf::from)
                .collect();
            checkouts.sort();
            checkouts.dedup();
            for checkout in checkouts {
                bring_up_to_date(&core, &checkout);
            }
            Ok(())
        })
        .await;
    }
}

/// Fast-forwards `checkout` to its remote branch when that is its default branch and it is
/// left as it was committed.
fn bring_up_to_date(core: &Core, checkout: &Path) {
    let Some((remote, branch)) = default_branch_upstream(checkout) else {
        return;
    };
    if !untouched(core, checkout) {
        return;
    }
    let ssh = sikemux_process::user_environment::var("GIT_SSH_COMMAND")
        .unwrap_or_else(|| "ssh -o BatchMode=yes -o ConnectTimeout=20".into());
    let fetched = Git::slow(checkout).env("GIT_SSH_COMMAND", &ssh).trimmed(&[
        "fetch",
        "--quiet",
        "--no-tags",
        &remote,
        &branch,
    ]);
    if fetched.is_err() {
        return;
    }
    let upstream = format!("refs/remotes/{remote}/{branch}");
    let Ok(counts) = Git::quick(checkout).trimmed(&[
        "rev-list",
        "--left-right",
        "--count",
        &format!("HEAD...{upstream}"),
    ]) else {
        return;
    };
    let mut counts = counts.split_whitespace().map(str::parse::<u64>);
    let (Some(Ok(ahead)), Some(Ok(behind))) = (counts.next(), counts.next()) else {
        return;
    };
    if ahead > 0 || behind == 0 || !untouched(core, checkout) {
        return;
    }
    if let Err(error) = Git::quick(checkout).trimmed(&["merge", "--ff-only", "--quiet", &upstream])
    {
        eprintln!(
            "sikemux core: {} was not brought up to date with {remote}/{branch}: {error}",
            checkout.display()
        );
    }
}

/// The remote and branch `checkout`'s branch follows, when that branch is the remote's
/// default: the one its `HEAD` names, or `main` or `master` when it names none.
fn default_branch_upstream(checkout: &Path) -> Option<(String, String)> {
    let branch = Git::quick(checkout)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()?;
    let remote = Git::quick(checkout)
        .trimmed(&["config", "--get", &format!("branch.{branch}.remote")])
        .ok()
        .filter(|remote| !remote.is_empty() && remote != ".")?;
    let merged = Git::quick(checkout)
        .trimmed(&["config", "--get", &format!("branch.{branch}.merge")])
        .ok()?;
    let followed = merged.strip_prefix("refs/heads/")?.to_owned();
    let default = Git::quick(checkout)
        .trimmed(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ])
        .ok()
        .and_then(|head| Some(head.strip_prefix(&format!("{remote}/"))?.to_owned()));
    let is_default = match default {
        Some(default) => default == followed,
        None => followed == "main" || followed == "master",
    };
    is_default.then_some((remote, followed))
}

/// Whether nobody is at work in `checkout`: no agent busy there, nothing changed or added in
/// it, and no merge or rebase under way.
fn untouched(core: &Core, checkout: &Path) -> bool {
    let agents_busy = (core.workspaces().agent_ids().into_iter())
        .filter(|id| (core.repo_above().worktree_busy)(core, id))
        .filter_map(|id| core.workspaces().agent(&id))
        .any(|agent| agent.cwd.starts_with(checkout));
    if agents_busy {
        return false;
    }
    let Ok(git_dir) = Git::quick(checkout).trimmed(&["rev-parse", "--absolute-git-dir"]) else {
        return false;
    };
    let git_dir = PathBuf::from(git_dir);
    if UNDER_WAY.iter().any(|file| git_dir.join(file).exists()) {
        return false;
    }
    Git::quick(checkout)
        .trimmed(&["status", "--porcelain", "--untracked-files=normal"])
        .is_ok_and(|status| status.is_empty())
}
