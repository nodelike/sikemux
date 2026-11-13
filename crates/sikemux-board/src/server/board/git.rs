//! The git work Auto does in a card's worktree. Each of these blocks.

use std::path::Path;

use crate::git::{said, Git};
use crate::server::{edits, Core};

/// Commits whatever is left uncommitted in the worktree, naming the agents
/// that wrote it, else `agent`. False when there was nothing to commit.
pub fn commit_all(core: &Core, tree: &Path, message: &str, agent: &str) -> Result<bool, String> {
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

pub fn ahead(tree: &Path, start_sha: &str) -> Result<usize, String> {
    Git::quick(tree)
        .trimmed(&["rev-list", "--count", &format!("{start_sha}..HEAD")])
        .map(|count| count.parse().unwrap_or(0))
}

/// Whether the worktree holds nothing beyond `start_sha`, committed or not.
pub fn untouched(tree: &Path, start_sha: &str) -> bool {
    ahead(tree, start_sha).is_ok_and(|ahead| ahead == 0)
        && Git::quick(tree)
            .trimmed(&["status", "--porcelain"])
            .is_ok_and(|changed| changed.is_empty())
}

pub fn head(tree: &Path) -> Result<String, String> {
    Git::quick(tree).trimmed(&["rev-parse", "HEAD"])
}

/// Pushes the worktree's branch. `pushed` is the commit Auto last put there,
/// which a rebase may replace, but nothing someone else pushed since.
pub fn push(tree: &Path, branch: &str, pushed: Option<&str>) -> Result<(), String> {
    let refspec = format!("HEAD:refs/heads/{branch}");
    let lease = pushed.map(|sha| format!("--force-with-lease=refs/heads/{branch}:{sha}"));
    let mut args = vec!["push", "--quiet"];
    if let Some(lease) = &lease {
        args.push(lease);
    }
    args.extend(["origin", &refspec]);
    Git::slow(tree).trimmed(&args).map(drop)
}

/// Keeps the worktree's commits on the card's own ref, answering the commit it put there, none
/// when that was there already. `kept` is the last commit Auto put there, so nothing else there
/// can be overwritten. The repository's pre-push hooks are for real pushes, not this.
pub fn back_up(tree: &Path, branch: &str, kept: Option<&str>) -> Result<Option<String>, String> {
    let head = head(tree)?;
    if kept == Some(head.as_str()) {
        return Ok(None);
    }
    let target = crate::server::worktree::backup_ref(branch);
    let lease = format!("--force-with-lease={target}:{}", kept.unwrap_or_default());
    let refspec = format!("HEAD:{target}");
    Git::slow(tree).trimmed(&["push", "--quiet", "--no-verify", &lease, "origin", &refspec])?;
    Ok(Some(head))
}

/// Puts the branch on top of the latest base. False when that conflicts,
/// which leaves the branch as it was.
pub fn rebase(tree: &Path, base: &str) -> Result<bool, String> {
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

/// Fetches `base` and says where it is now.
pub fn latest(tree: &Path, base: &str) -> Result<String, String> {
    Git::slow(tree).trimmed(&["fetch", "--quiet", "origin", base])?;
    latest_known(tree, base)
}

pub fn latest_known(tree: &Path, base: &str) -> Result<String, String> {
    Git::quick(tree).trimmed(&["rev-parse", &format!("origin/{base}")])
}

/// Replays `from..head` on `onto` in a checkout of its own, ending there.
/// None when that conflicts, which leaves the checkout at `onto`.
pub fn lay(checkout: &Path, onto: &str, from: &str, head: &str) -> Result<Option<String>, String> {
    let replayed = Git::slow(checkout)
        .config("rebase.updateRefs", "false")
        .trimmed(&["rebase", "--onto", onto, from, head]);
    if replayed.is_err() {
        let _ = Git::quick(checkout).trimmed(&["rebase", "--abort"]);
        go_to(checkout, onto)?;
        return Ok(None);
    }
    self::head(checkout).map(Some)
}

pub fn go_to(checkout: &Path, commit: &str) -> Result<(), String> {
    Git::quick(checkout)
        .trimmed(&["checkout", "--detach", "--force", "--quiet", commit])
        .map(drop)
}

/// The change from `from` to `to`, byte for byte, without what a rebase alone
/// moves: which blobs it runs between and where its hunks sit.
fn patch(repo: &Path, from: &str, to: &str) -> Result<Vec<u8>, String> {
    let range = format!("{from}..{to}");
    let written = Git::slow(repo).stdout(&[
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--binary",
        &range,
    ])?;
    let mut kept = Vec::with_capacity(written.len());
    for line in written.split(|byte| *byte == b'\n') {
        if line.starts_with(b"index ") {
            continue;
        }
        kept.extend_from_slice(if line.starts_with(b"@@ ") {
            b"@@"
        } else {
            line
        });
        kept.push(b'\n');
    }
    Ok(kept)
}

/// Whether two ranges make the same change: same lines, same lines around
/// them, same files. An approval follows a rebase only when they do.
pub fn same_patch(repo: &Path, first: (&str, &str), second: (&str, &str)) -> Result<bool, String> {
    Ok(patch(repo, first.0, first.1)? == patch(repo, second.0, second.1)?)
}

pub fn same_tree(repo: &Path, first: &str, second: &str) -> Result<bool, String> {
    let tree =
        |commit: &str| Git::quick(repo).trimmed(&["rev-parse", &format!("{commit}^{{tree}}")]);
    Ok(tree(first)? == tree(second)?)
}

pub fn merge_base(repo: &Path, first: &str, second: &str) -> Result<String, String> {
    Git::quick(repo).trimmed(&["merge-base", first, second])
}

/// The commits after `from` up to `to`, oldest first.
pub fn commits_between(repo: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let listed = Git::quick(repo).trimmed(&["rev-list", "--reverse", &format!("{from}..{to}")])?;
    Ok(listed.lines().map(str::to_owned).collect())
}

/// Pushes a rebased `head` over `pushed`, unless the branch is there already
/// because the core stopped right after the push and before noting it.
pub fn push_rebased(tree: &Path, branch: &str, pushed: &str, head: &str) -> Result<(), String> {
    let listed =
        Git::slow(tree).trimmed(&["ls-remote", "origin", &format!("refs/heads/{branch}")])?;
    if listed.split_whitespace().next() == Some(head) {
        return Ok(());
    }
    push(tree, branch, Some(pushed))
}

/// What changed from `from` to `to`: a summary of the files, then the
/// change itself, cut at `max` bytes. Whether it was cut.
pub fn diff(repo: &Path, from: &str, to: &str, max: usize) -> Result<(String, bool), String> {
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
pub fn changed_paths(repo: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let range = format!("{from}..{to}");
    let listed = Git::slow(repo).trimmed(&["diff", "--name-only", "--no-renames", &range])?;
    Ok(listed.lines().map(str::to_owned).collect())
}

/// Where the branch's own work starts on `base` as it is now.
pub fn fork_point(repo: &Path, base: &str, head: &str) -> Result<String, String> {
    Git::quick(repo).trimmed(&["merge-base", &format!("origin/{base}"), head])
}

/// What git needs to make a commit in `tree`, on a host with no name of its own for git.
pub fn identity(tree: &Path) -> Vec<&'static str> {
    match Git::quick(tree).trimmed(&["config", "user.email"]) {
        Ok(_) => Vec::new(),
        Err(_) => vec![
            "-c",
            "user.name=Sikemux",
            "-c",
            "user.email=sikemux@localhost",
        ],
    }
}
