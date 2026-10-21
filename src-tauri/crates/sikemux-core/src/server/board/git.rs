//! The git work Auto does in a card's worktree. Each of these blocks.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use crate::git::Git;
use crate::server::{edits, sandbox, Core};

const QUICK: Duration = Duration::from_secs(60);
/// Pushing and fetching wait on the network.
pub(crate) const SLOW: Duration = Duration::from_secs(5 * 60);
const CHECK: Duration = Duration::from_secs(30 * 60);
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How much of a failure's output a note or a fix request carries.
const TAIL_CHARS: usize = 3000;
const SLUG_MAX: usize = 40;

pub(crate) fn git_in(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    git_with(dir, args, &[], timeout)
}

/// [`git_in`] with `envs` set for git.
pub(crate) fn git_with(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, String> {
    let mut git = Git::at(dir).timeout(timeout).limit(OUTPUT_LIMIT);
    for (key, value) in envs {
        git = git.env(key, value);
    }
    let output = git.output(args)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    let said = said(&output);
    Err(format!(
        "git {} failed{}",
        args.first().copied().unwrap_or_default(),
        if said.is_empty() {
            String::new()
        } else {
            format!(": {said}")
        }
    ))
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_in(dir, args, QUICK)
}

/// The end of what a command wrote, which is where its error usually is.
pub(super) fn tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= TAIL_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - TAIL_CHARS).collect();
    format!("…{kept}")
}

fn said(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    let out = String::from_utf8_lossy(&output.stdout);
    if !out.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&out);
    }
    tail(&text)
}

/// A card's own checkout.
pub(super) struct Tree {
    pub repo: PathBuf,
    pub path: PathBuf,
    /// Where the agent works: the project's folder inside the worktree.
    pub cwd: PathBuf,
    pub branch: String,
    pub base: String,
    pub start_sha: String,
}

/// A few words of `text`, kebab-cased, to name a branch and folder after.
pub(crate) fn slug(text: &str) -> String {
    let mut slug = String::new();
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let next = if slug.is_empty() {
            word.to_owned()
        } else {
            format!("{slug}-{word}")
        };
        if next.len() > SLUG_MAX {
            break;
        }
        slug = next;
    }
    if slug.is_empty() {
        "card".to_owned()
    } else {
        slug
    }
}

pub(crate) fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// The branch to cut from and land on: the one the project's checkout is on,
/// else the remote's default.
fn base_branch(repo: &Path) -> Result<String, String> {
    if let Ok(branch) = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        return Ok(branch);
    }
    git(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .map(|remote| remote.trim_start_matches("origin/").to_owned())
    .map_err(|_| {
        "the project's checkout is on no branch, so Auto cannot tell which branch to work from"
            .to_owned()
    })
}

/// Where `project` sits inside the repository at `repo`.
pub(crate) fn inside(project: &Path, repo: &Path) -> PathBuf {
    project
        .canonicalize()
        .ok()
        .zip(repo.canonicalize().ok())
        .and_then(|(project, repo)| project.strip_prefix(&repo).ok().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// The folder beside the repository that Auto's checkouts go in.
pub(crate) fn worktrees_folder(repo: &Path) -> PathBuf {
    let name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    repo.with_file_name(format!("{name}.worktrees"))
}

/// Cuts a worktree for a card beside the project's checkout, on a branch of
/// its own from the latest of the base branch.
pub(super) fn add_worktree(project: &Path, slug: &str) -> Result<Tree, String> {
    let repo = PathBuf::from(
        git(
            project,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|_| {
            "the project is not a git repository, so Auto cannot give a card a worktree of its own"
                .to_owned()
        })?,
    );
    let inside = inside(project, &repo);
    let base = base_branch(&repo)?;
    let _ = git_in(&repo, &["fetch", "--quiet", "origin", &base], SLOW);
    let remote = format!("origin/{base}");
    let start = if git(&repo, &["rev-parse", "--verify", "--quiet", &remote]).is_ok() {
        remote
    } else {
        base.clone()
    };
    let start_sha = git(&repo, &["rev-parse", &start])?;
    let (branch, path) = sandbox::cut(&repo, slug, &start_sha)?;
    sandbox::copy_included(&repo, &path);
    Ok(Tree {
        cwd: path.join(&inside),
        repo,
        path,
        branch,
        base,
        start_sha,
    })
}

/// Commits whatever is left uncommitted in the worktree, naming the agents
/// that wrote it, else `agent`. False when there was nothing to commit.
pub(super) fn commit_all(
    core: &Core,
    tree: &Path,
    message: &str,
    agent: &str,
) -> Result<bool, String> {
    if git(tree, &["status", "--porcelain"])?.is_empty() {
        return Ok(false);
    }
    git(tree, &["add", "--all"])?;
    let output = edits::footer::commit(core, tree, &["--quiet"], message, Some(agent))?;
    if !output.status.success() {
        return Err(format!("git commit failed: {}", said(&output)));
    }
    Ok(true)
}

/// How many commits the worktree's branch has on top of where it started.
pub(super) fn ahead(tree: &Path, start_sha: &str) -> Result<usize, String> {
    git(
        tree,
        &["rev-list", "--count", &format!("{start_sha}..HEAD")],
    )
    .map(|count| count.parse().unwrap_or(0))
}

pub(super) fn head(tree: &Path) -> Result<String, String> {
    git(tree, &["rev-parse", "HEAD"])
}

pub(super) fn origin(repo: &Path) -> Option<String> {
    git(repo, &["remote", "get-url", "origin"])
        .ok()
        .filter(|url| !url.is_empty())
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
    git_in(tree, &args, SLOW).map(drop)
}

/// Puts the branch on top of the latest base. False when that conflicts,
/// which leaves the branch as it was.
pub(super) fn rebase(tree: &Path, base: &str) -> Result<bool, String> {
    git_in(tree, &["fetch", "--quiet", "origin", base], SLOW)?;
    if git_in(tree, &["rebase", &format!("origin/{base}")], SLOW).is_ok() {
        return Ok(true);
    }
    let _ = git(tree, &["rebase", "--abort"]);
    Ok(false)
}

/// Runs the project's check command in the worktree. Whether it passed, and
/// the end of what it wrote.
pub(super) fn check(cwd: &Path, command: &str) -> Result<(bool, String), String> {
    let mut shell = sikemux_process::user_environment::command("/bin/sh");
    shell.args(["-c", command]).current_dir(cwd);
    let output = sikemux_process::run(&mut shell, None, CHECK, OUTPUT_LIMIT, None)
        .map_err(|error| format!("the check did not run: {error}"))?;
    Ok((output.status.success(), said(&output)))
}

/// Takes away a card's worktree and its branch, here and, when `remote`, on
/// the remote too.
pub(super) fn remove_worktree(repo: &Path, tree: &Path, branch: &str, remote: bool) {
    sandbox::remove(repo, tree, branch);
    if !remote {
        return;
    }
    let _ = git_in(
        repo,
        &["push", "--quiet", "origin", "--delete", branch],
        SLOW,
    );
}

/// A reviewer's checkout of one commit, on no branch, that it cannot write to.
pub(super) struct Checkout {
    pub path: PathBuf,
    /// Where the reviewer works: the project's folder inside the checkout.
    pub cwd: PathBuf,
}

/// Checks `head` out beside the project's checkout, detached, for a reviewer
/// named by `slug`, and takes away the write permission on all of it.
pub(super) fn add_checkout(
    repo: &Path,
    project: &Path,
    head: &str,
    slug: &str,
) -> Result<Checkout, String> {
    let folder = worktrees_folder(repo);
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    let path = folder.join(format!("review-{slug}"));
    let target = path.to_string_lossy().into_owned();
    git_in(repo, &["worktree", "add", "--detach", &target, head], SLOW)?;
    set_writable(&path, false);
    Ok(Checkout {
        cwd: path.join(inside(project, repo)),
        path,
    })
}

/// Gives a reviewer's checkout its write permission back and takes it away.
pub(super) fn remove_checkout(repo: &Path, path: &Path) {
    if !path.exists() {
        let _ = git(repo, &["worktree", "prune"]);
        return;
    }
    set_writable(path, true);
    let target = path.to_string_lossy().into_owned();
    if git(repo, &["worktree", "remove", "--force", &target]).is_err() {
        if let Err(error) = std::fs::remove_dir_all(path) {
            eprintln!("sikemux core: Auto left the review checkout {target}: {error}");
        }
        let _ = git(repo, &["worktree", "prune"]);
    }
}

#[cfg(not(unix))]
fn set_writable(_path: &Path, _writable: bool) {}

#[cfg(unix)]
fn set_writable(path: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mode = meta.permissions().mode();
    let mode = if writable {
        mode | 0o200
    } else {
        mode & !0o222
    };
    if writable {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            set_writable(&entry.path(), writable);
        }
    }
    if !writable {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
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
    let stat = git(repo, &["diff", "--no-color", "--stat", &range])?;
    let patch = git_in(repo, &["diff", "--no-color", &range], SLOW)?;
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

/// Where the branch's own work starts on `base` as it is now.
pub(super) fn fork_point(repo: &Path, base: &str, head: &str) -> Result<String, String> {
    git(repo, &["merge-base", &format!("origin/{base}"), head])
}
