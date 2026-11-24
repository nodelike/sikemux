//! Origin's copy of a branch an agent pushed from its checkout. Once all its work is on origin's
//! default branch, nothing needs the copy, so it goes. Anything unsure keeps it: a tip this
//! repository did not push, work not there yet, or an open pull request from or into it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::git::Git;
use crate::server::codehost::repo as codehost;
use crate::server::connection::blocking;
use crate::server::edits::push;
use crate::server::landing::project_branch;
use crate::server::{Core, CoreError};

const REMOTE: &str = "origin";
/// The most pull requests every code host lists on one page.
const PULLS: usize = 50;

pub fn branch_of(tree: &Path) -> Option<String> {
    Git::quick(tree)
        .trimmed(&["branch", "--show-current"])
        .ok()
        .filter(|branch| !branch.is_empty())
}

/// Deletes origin's copy of `branch` of the repository at `repo`, if nothing needs it, in the
/// background.
pub fn clear_landed(core: &Arc<Core>, repo: &Path, branch: &str) {
    let (core, repo, branch) = (core.clone(), repo.to_owned(), branch.to_owned());
    tokio::spawn(async move {
        match try_clear(&core, repo, branch.clone()).await {
            Ok(true) => {
                eprintln!("sikemux core: deleted {REMOTE}/{branch}, as all its work is landed")
            }
            Ok(false) => {}
            Err(why) => eprintln!("sikemux core: kept {REMOTE}/{branch}: {why}"),
        }
    });
}

async fn try_clear(core: &Arc<Core>, repo: PathBuf, branch: String) -> Result<bool, String> {
    let judged = {
        let (repo, branch) = (repo.clone(), branch.clone());
        blocking(move || landed_tip(&repo, &branch).map_err(CoreError::from)).await
    };
    let Some(tip) = judged.map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    if has_open_pull(core, &repo, &branch).await? {
        return Ok(false);
    }
    blocking(move || delete(&repo, &branch, &tip).map_err(CoreError::from))
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// The tip of origin's copy of `branch`, when this repository made the copy and pushed that tip,
/// and origin's default branch holds each of its changes.
fn landed_tip(repo: &Path, branch: &str) -> Result<Option<String>, String> {
    if !pushed_from(repo, branch).any() || push::shared(repo, REMOTE, branch) {
        return Ok(None);
    }
    let name = format!("refs/heads/{branch}");
    let listed = Git::slow(repo).trimmed(&["ls-remote", "--heads", REMOTE, &name])?;
    let Some((tip, _)) = listed.split_once('\t') else {
        return Ok(None);
    };
    if !pushed_from(repo, branch).holds(tip) {
        return Ok(None);
    }
    let Some(line) = project_branch(repo).filter(|line| line != branch) else {
        return Ok(None);
    };
    let onto = push::fetched_tip(repo, REMOTE, &line)?;
    let merges = Git::quick(repo).trimmed(&[
        "rev-list",
        "--merges",
        "--count",
        &format!("{onto}..{tip}"),
    ])?;
    let missing = Git::quick(repo).trimmed(&["cherry", &onto, tip])?;
    let landed = merges == "0" && !missing.lines().any(|commit| commit.starts_with('+'));
    Ok(landed.then(|| tip.to_owned()))
}

/// The tips this repository pushed to origin's `branch`; none when the branch was there before
/// its first push, as Git's log of the tracking branch tells.
struct PushedTips(Vec<String>);

impl PushedTips {
    fn any(&self) -> bool {
        !self.0.is_empty()
    }

    fn holds(&self, tip: &str) -> bool {
        self.0.iter().any(|pushed| pushed == tip)
    }
}

fn pushed_from(repo: &Path, branch: &str) -> PushedTips {
    let log = Git::quick(repo)
        .trimmed(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &format!("logs/refs/remotes/{REMOTE}/{branch}"),
        ])
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let mut entries = log.lines().map(|entry| {
        let mut words = entry.splitn(3, ' ');
        let (old, new) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        (old, new, entry.ends_with("\tupdate by push"))
    });
    let mut tips = Vec::new();
    if let Some((old, new, true)) = entries.next() {
        if old.bytes().all(|digit| digit == b'0') {
            tips.push(new.to_owned());
            tips.extend(
                entries
                    .filter(|(_, _, pushed)| *pushed)
                    .map(|(_, new, _)| new.to_owned()),
            );
        }
    }
    PushedTips(tips)
}

/// Whether an open pull request is from `branch` or into it. A full page may leave one out, so
/// it counts as one.
async fn has_open_pull(core: &Arc<Core>, repo: &Path, branch: &str) -> Result<bool, String> {
    let (origin, picks) = {
        let (core, repo) = (core.clone(), repo.to_owned());
        blocking(move || Ok((crate::git::origin(&repo), codehost::picks(&core, &repo))))
            .await
            .map_err(|error| error.to_string())?
    };
    let origin = origin.ok_or("it has no origin")?;
    let host = codehost::find(core, &origin, &picks).await?;
    let open = host
        .call("pulls", json!({ "state": "open", "perPage": PULLS }))
        .await
        .map_err(|failure| failure.message)?;
    let open = open
        .as_array()
        .ok_or("the code host answered without a list of pull requests")?;
    Ok(open.len() >= PULLS
        || open.iter().any(|pull| {
            ["head", "base"]
                .iter()
                .any(|side| pull.get(side).and_then(Value::as_str) == Some(branch))
        }))
}

/// Deletes origin's copy as long as it still stands on `tip`. The pre-push hook checks what is
/// pushed, and a delete pushes nothing.
fn delete(repo: &Path, branch: &str, tip: &str) -> Result<(), String> {
    Git::slow(repo)
        .trimmed(&[
            "push",
            "--quiet",
            "--no-verify",
            &format!("--force-with-lease=refs/heads/{branch}:{tip}"),
            REMOTE,
            &format!(":refs/heads/{branch}"),
        ])
        .map(drop)
}
