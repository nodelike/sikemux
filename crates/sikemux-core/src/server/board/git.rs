//! The git work Auto does in a card's worktree. Each of these blocks.

use std::path::Path;

use crate::git::{said, Git};
use crate::server::{edits, Core};

/// Commits whatever is left uncommitted in the worktree, naming the agents
/// that wrote it, else `agent`. False when there was nothing to commit.
pub(super) fn commit_all(
    core: &Core,
    tree: &Path,
    message: &str,
    agent: &str,
) -> Result<bool, String> {
    if Git::quick(tree)
        .trimmed(&["status", "--porcelain"])?
        .is_empty()
    {
        return Ok(false);
    }
    Git::quick(tree).trimmed(&["add", "--all"])?;
    let output = edits::footer::commit(core, tree, &["--quiet"], message, Some(agent))?;
    if !output.status.success() {
        return Err(format!("git commit failed: {}", said(&output)));
    }
    if let Err(error) = edits::footer::head_made(core, agent, tree) {
        eprintln!("sikemux core: Auto's commit was not told: {error}");
    }
    Ok(true)
}

pub(super) fn ahead(tree: &Path, start_sha: &str) -> Result<usize, String> {
    Git::quick(tree)
        .trimmed(&["rev-list", "--count", &format!("{start_sha}..HEAD")])
        .map(|count| count.parse().unwrap_or(0))
}

pub(super) fn head(tree: &Path) -> Result<String, String> {
    Git::quick(tree).trimmed(&["rev-parse", "HEAD"])
}

/// Pushes the worktree's branch. `pushed` is the commit Auto last put there,
/// which a rebase may replace, but nothing someone else pushed since.
pub(super) fn push(tree: &Path, branch: &str, pushed: Option<&str>) -> Result<(), String> {
    let refspec = format!("HEAD:refs/heads/{branch}");
    let lease = pushed.map(|sha| format!("--force-with-lease=refs/heads/{branch}:{sha}"));
    let mut args = vec!["push", "--quiet"];
    if let Some(lease) = &lease {
        args.push(lease);
    }
    args.extend(["origin", &refspec]);
    Git::slow(tree).trimmed(&args).map(drop)
}

/// Puts the branch on top of the latest base. False when that conflicts,
/// which leaves the branch as it was.
pub(super) fn rebase(tree: &Path, base: &str) -> Result<bool, String> {
    Git::slow(tree).trimmed(&["fetch", "--quiet", "origin", base])?;
    if Git::slow(tree)
        .trimmed(&["rebase", &format!("origin/{base}")])
        .is_ok()
    {
        return Ok(true);
    }
    let _ = Git::quick(tree).trimmed(&["rebase", "--abort"]);
    Ok(false)
}

/// What changed from `from` to `to`: a summary of the files, then the
/// change itself, cut at `max` bytes. Whether it was cut.
pub(super) fn diff(
    repo: &Path,
    from: &str,
    to: &str,
    max: usize,
) -> Result<(String, bool), String> {
    let range = format!("{from}..{to}");
    let stat = Git::quick(repo).trimmed(&["diff", "--no-color", "--stat", &range])?;
    let patch = Git::slow(repo).trimmed(&["diff", "--no-color", &range])?;
    let mut text = format!("{stat}\n\n{patch}");
    let cut = text.len() > max;
    if cut {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok((text, cut))
}

/// The paths that changed from `from` to `to`.
pub(super) fn changed_paths(repo: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let range = format!("{from}..{to}");
    let listed = Git::slow(repo).trimmed(&["diff", "--name-only", "--no-renames", &range])?;
    Ok(listed.lines().map(str::to_owned).collect())
}

/// Where the branch's own work starts on `base` as it is now.
pub(super) fn fork_point(repo: &Path, base: &str, head: &str) -> Result<String, String> {
    Git::quick(repo).trimmed(&["merge-base", &format!("origin/{base}"), head])
}
