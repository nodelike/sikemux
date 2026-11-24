//! Work left in a checkout nobody works in any more. Its folder goes, but not the work: that
//! stays on a branch here, what was uncommitted as one commit on top, and resuming the chat
//! makes the checkout again with it uncommitted, as it was.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::agent::branch_for_folder;
use super::branch_exists;
use super::published::branch_of;
use crate::git::Git;
use crate::server::fast_forward::under_way;

const UNCOMMITTED: &str = "Uncommitted work left in a checkout";
/// The most a checkout's uncommitted files may come to, as a repository keeps what is committed.
const MOST_BYTES: u64 = 200_000_000;
/// How many commits back a checkout's files are looked for.
const EARLIER: &str = "2000";
/// The mode Git lists a repository inside the checkout under, whose files it would not commit.
const REPOSITORY_MODE: &str = " 160000 ";

/// Keeps the work in the checkout at `tree` on a branch, answering which.
pub fn keep(repo: &Path, tree: &Path) -> Result<String, String> {
    if under_way(tree) {
        return Err("git is partway through changing it".into());
    }
    let folder = folder_name(tree);
    let on = branch_of(tree);
    let own = match &on {
        Some(branch) => branch.replace('/', "-") == folder,
        None => branch_for_folder(repo, &folder).is_none(),
    };
    let head = Git::quick(tree).trimmed(&["rev-parse", "HEAD"])?;
    let changed = changed_paths(tree)?;
    let bytes: u64 = (changed.iter())
        .filter_map(|path| path.symlink_metadata().ok())
        .map(|meta| meta.len())
        .sum();
    let tip = match changed.is_empty() {
        true => head.clone(),
        false if !own => {
            return Err("it has uncommitted work on a branch that is not its own".into())
        }
        false if bytes > MOST_BYTES => return Err("its uncommitted files are too big".into()),
        false => commit_uncommitted(tree, &head)?,
    };
    let branch = match on {
        Some(branch) => branch,
        None if own => folder,
        None => return Err("a branch of its name is already there".into()),
    };
    let expected = if branch_exists(repo, &branch) {
        head
    } else {
        "0".repeat(head.len())
    };
    Git::quick(repo).trimmed(&[
        "update-ref",
        &format!("refs/heads/{branch}"),
        &tip,
        &expected,
    ])?;
    Ok(branch)
}

/// Takes the commit [`keep`] made back off the branch in the checkout at `tree`, leaving its
/// changes uncommitted.
pub fn take_up(tree: &Path) -> Result<(), String> {
    let subject = Git::quick(tree).trimmed(&["log", "-1", "--format=%s"])?;
    if subject == UNCOMMITTED {
        Git::quick(tree).trimmed(&["reset", "-q", "HEAD~1"])?;
    }
    Ok(())
}

/// When an uncommitted file of the checkout at `tree` last changed.
pub fn last_changed(tree: &Path) -> Option<SystemTime> {
    (changed_paths(tree).ok()?.iter())
        .filter_map(|path| {
            path.symlink_metadata()
                .and_then(|meta| meta.modified())
                .ok()
        })
        .max()
}

fn folder_name(tree: &Path) -> String {
    tree.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn commit_uncommitted(tree: &Path, head: &str) -> Result<String, String> {
    Git::slow(tree).trimmed(&["add", "--all"])?;
    let staged = Git::slow(tree).trimmed(&["diff", "--cached", "--raw", "--no-renames", head])?;
    if staged.contains(REPOSITORY_MODE) {
        return Err("it holds another repository".into());
    }
    let files = Git::quick(tree).trimmed(&["write-tree"])?;
    if was_at(tree, &files)? {
        return Ok(head.to_owned());
    }
    Git::quick(tree).trimmed(&["commit-tree", &files, "-p", head, "-m", UNCOMMITTED])
}

/// Whether an earlier commit of the branch at `tree` held exactly `files`. Landing a teammate's
/// work moves a lead's branch on under its checkout, whose older files then read as changes.
fn was_at(tree: &Path, files: &str) -> Result<bool, String> {
    let earlier = Git::quick(tree).trimmed(&["log", "-n", EARLIER, "--format=%T"])?;
    Ok(earlier.lines().any(|older| older == files))
}

fn changed_paths(tree: &Path) -> Result<Vec<PathBuf>, String> {
    let listed = Git::slow(tree).text(&[
        "status",
        "--porcelain",
        "--no-renames",
        "--untracked-files=all",
        "-z",
    ])?;
    Ok((listed.split('\0'))
        .filter_map(|entry| entry.get(3..))
        .map(|path| tree.join(path))
        .collect())
}
