//! Keeping each project's checkout of its default branch up to date with the remote while
//! nobody works in it. Every so often the core fetches that branch and fast-forwards the
//! checkout, only when it has no changes, no untracked files, no commits of its own and no
//! merge or rebase under way, and no agent is at work in it. Anything else, it leaves alone.

use crate::server::agent_now::AgentNowCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::task::JoinHandle;

use super::connection::blocking;
use super::worktree::git_folder;
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
/// left as it was committed. Says why when it leaves it be, once for as long as the reason
/// stays the same.
fn bring_up_to_date(core: &Core, checkout: &Path) {
    static LAST_REASON: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let outcome = try_bring_up_to_date(core, checkout);
    let mut last = LAST_REASON.get_or_init(Default::default).lock().unwrap();
    match outcome {
        Ok(()) => {
            last.remove(checkout);
        }
        Err(reason) => {
            if last.get(checkout) != Some(&reason) {
                eprintln!(
                    "sikemux core: not bringing {} up to date: {reason}",
                    checkout.display()
                );
                last.insert(checkout.to_owned(), reason);
            }
        }
    }
}

fn try_bring_up_to_date(core: &Core, checkout: &Path) -> Result<(), String> {
    let (remote, branch) = default_branch_upstream(checkout)?;
    untouched(core, checkout)?;
    let ssh = sikemux_process::user_environment::var("GIT_SSH_COMMAND")
        .unwrap_or_else(|| "ssh -o BatchMode=yes -o ConnectTimeout=20".into());
    Git::slow(checkout)
        .env("GIT_SSH_COMMAND", &ssh)
        .trimmed(&["fetch", "--quiet", "--no-tags", &remote, &branch])
        .map_err(|error| format!("fetching {remote}/{branch} failed: {error}"))?;
    let upstream = format!("refs/remotes/{remote}/{branch}");
    let counts = Git::quick(checkout)
        .trimmed(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{upstream}"),
        ])
        .map_err(|error| format!("counting commits against {remote}/{branch} failed: {error}"))?;
    let mut counts = counts.split_whitespace().map(str::parse::<u64>);
    let (Some(Ok(ahead)), Some(Ok(behind))) = (counts.next(), counts.next()) else {
        return Err(format!(
            "could not read the commit counts against {remote}/{branch}"
        ));
    };
    if ahead > 0 {
        return Err(format!(
            "it has {ahead} commit(s) of its own not on {remote}/{branch}"
        ));
    }
    if behind == 0 {
        return Ok(());
    }
    untouched(core, checkout)?;
    Git::quick(checkout)
        .trimmed(&["merge", "--ff-only", "--quiet", &upstream])
        .map(drop)
        .map_err(|error| format!("fast-forwarding to {remote}/{branch} failed: {error}"))
}

/// The remote and branch `checkout`'s branch follows, when that branch is the remote's
/// default: the one its `HEAD` names, or `main` or `master` when it names none.
fn default_branch_upstream(checkout: &Path) -> Result<(String, String), String> {
    let branch = Git::quick(checkout)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| "it is not on a branch".to_owned())?;
    let remote = Git::quick(checkout)
        .trimmed(&["config", "--get", &format!("branch.{branch}.remote")])
        .ok()
        .filter(|remote| !remote.is_empty() && remote != ".")
        .ok_or_else(|| format!("{branch} follows no remote"))?;
    let followed = Git::quick(checkout)
        .trimmed(&["config", "--get", &format!("branch.{branch}.merge")])
        .ok()
        .and_then(|merged| merged.strip_prefix("refs/heads/").map(str::to_owned))
        .ok_or_else(|| format!("{branch} follows no branch on {remote}"))?;
    let default = Git::quick(checkout)
        .trimmed(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ])
        .ok()
        .and_then(|head| Some(head.strip_prefix(&format!("{remote}/"))?.to_owned()));
    let is_default = match &default {
        Some(default) => *default == followed,
        None => followed == "main" || followed == "master",
    };
    if is_default {
        Ok((remote, followed))
    } else {
        let default = default.unwrap_or_else(|| "main".into());
        Err(format!(
            "{branch} follows {remote}/{followed}, but {remote}'s default branch is {default}"
        ))
    }
}

/// Whether git is partway through changing `checkout`, or it can't be told.
pub(super) fn under_way(checkout: &Path) -> bool {
    git_folder(checkout)
        .is_none_or(|folder| UNDER_WAY.iter().any(|file| folder.join(file).exists()))
}

/// Whether nobody is at work in `checkout`: no agent busy or starting there, nothing changed
/// or added in it, and no merge or rebase under way.
fn untouched(core: &Core, checkout: &Path) -> Result<(), String> {
    let agents_busy = (core.workspaces().agent_ids().into_iter())
        .filter(|id| core.agent_now(id).working_or_starting())
        .filter_map(|id| core.workspaces().agent(&id))
        .any(|agent| agent.cwd.starts_with(checkout));
    if agents_busy {
        return Err("an agent is at work or starting in it".into());
    }
    if under_way(checkout) {
        return Err("git is partway through a merge or rebase there".into());
    }
    let status = Git::quick(checkout)
        .trimmed(&["status", "--porcelain", "--untracked-files=normal"])
        .map_err(|error| format!("reading its status failed: {error}"))?;
    if status.is_empty() {
        Ok(())
    } else {
        Err("it has changes or untracked files".into())
    }
}
