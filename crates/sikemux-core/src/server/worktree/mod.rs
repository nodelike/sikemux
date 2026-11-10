//! Each piece of work's own checkout: a git worktree on a branch of its own,
//! in a folder beside the project's repository so it never shows up in the
//! project's own tree or status. Chat agents, Auto's cards and workers for
//! another host's agents all get theirs here.

use std::path::{Path, PathBuf};

pub(crate) use sikemux_view::workspace::AgentWorktree;

use crate::git::{main_checkout, Git};
use crate::tasks::detect::PERSONAL_TASKS;

pub mod agent;
pub mod removal;
mod warm;

pub(crate) use warm::seed_in_background;

const NAME_TRIES: usize = 50;
const MAX_STRAYED_FILES: usize = 20;

/// Without a `.worktreeinclude`, the files a project usually keeps out of git
/// but needs to run.
const DEFAULT_INCLUDE: [&str; 6] = [
    ".env",
    ".env.*",
    "*/.env",
    "*/.env.*",
    ".sikemux/.gitignore",
    PERSONAL_TASKS,
];
const MAX_INCLUDED: usize = 50;
const SLUG_MAX: usize = 40;
const BRANCH_WORDS: usize = 5;
/// Words that only say what kind of change the work is, left out of its branch.
const KIND_WORDS: [&str; 9] = [
    "fix",
    "fixes",
    "fixing",
    "add",
    "adds",
    "adding",
    "implement",
    "feat",
    "feature",
];
/// Words that say the work mends something, which puts its branch under `fix/`.
const MENDING_WORDS: [&str; 11] = [
    "fix",
    "fixes",
    "fixing",
    "bug",
    "bugs",
    "broken",
    "crash",
    "crashes",
    "regression",
    "fails",
    "failing",
];
/// Words that say nothing about the work.
const FILLER_WORDS: [&str; 29] = [
    "an", "the", "and", "or", "of", "to", "for", "in", "on", "at", "by", "with", "from", "into",
    "how", "that", "this", "it", "its", "is", "are", "be", "so", "as", "please", "we", "our",
    "you", "your",
];

/// The branch for the work `work` describes, in the repository's way of
/// naming them: `fix/` when it mends something, else `feat/`, then a few words
/// of its first sentence that says what it is, past one that only says who the
/// agent is, such as "You are the core teammate".
pub(crate) fn branch_for(work: &str) -> String {
    let mut mends = false;
    let mut chosen: Vec<String> = Vec::new();
    for sentence in work.split(['.', ':', '!', '?', '\n']) {
        let lower = sentence.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| word.len() > 1 || word.starts_with(|c: char| c.is_ascii_digit()))
            .collect();
        if matches!(words.as_slice(), ["you", "are" | "re", ..]) {
            continue;
        }
        mends |= words.iter().any(|word| MENDING_WORDS.contains(word));
        let told: Vec<String> = words
            .into_iter()
            .filter(|word| !FILLER_WORDS.contains(word) && !KIND_WORDS.contains(word))
            .map(str::to_owned)
            .collect();
        if told.len() >= 2 || chosen.is_empty() {
            chosen = told;
        }
        if chosen.len() >= 2 {
            break;
        }
    }
    chosen.truncate(BRANCH_WORDS);
    let kind = if mends { "fix" } else { "feat" };
    match chosen.is_empty() {
        true => format!("{kind}/work"),
        false => format!("{kind}/{}", slug(&chosen.join(" "))),
    }
}

/// Cuts a worktree of `repo` at `start` on a new branch called `branch`, or
/// `branch` and a number when that is taken. The branch and the folder it is in.
pub(crate) fn cut(repo: &Path, branch: &str, start: &str) -> Result<(String, PathBuf), String> {
    let folder = worktrees_folder(repo);
    let base = Git::quick(repo)
        .trimmed(&["check-ref-format", "--branch", branch])
        .map_err(|_| format!("{branch} is not a name git takes for a branch"))?;
    let (branch, path) = (1..=NAME_TRIES)
        .map(|index| {
            let branch = if index == 1 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            let path = folder.join(branch.replace('/', "-"));
            (branch, path)
        })
        .find(|(branch, path)| !path.exists() && !branch_exists(repo, branch))
        .ok_or_else(|| format!("every worktree name for {base} is taken"))?;
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    let target = path.to_string_lossy().into_owned();
    Git::slow(repo).trimmed(&[
        "worktree",
        "add",
        "--no-track",
        "-b",
        &branch,
        &target,
        start,
    ])?;
    Ok((branch, path))
}

pub(crate) fn remove(repo: &Path, path: &Path, branch: &str) {
    let target = path.to_string_lossy().into_owned();
    if let Err(error) = Git::quick(repo).trimmed(&["worktree", "remove", "--force", &target]) {
        eprintln!("sikemux core: the worktree {target} is left: {error}");
    }
    let _ = Git::quick(repo).trimmed(&["branch", "-D", branch]);
}

/// Copies the files of [`included`] from `main` into the worktree at `tree`.
pub(crate) fn copy_included(main: &Path, tree: &Path) {
    for relative in included(main) {
        let (from, to) = (main.join(&relative), tree.join(&relative));
        if !from.is_file() || to.symlink_metadata().is_ok() {
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::copy(&from, &to) {
            eprintln!(
                "sikemux core: {} was not copied into the worktree: {error}",
                relative.display()
            );
        }
    }
}

/// What a fresh checkout of `main` lacks but needs to run, such as `.env`:
/// what `.worktreeinclude` lists, the file Claude Code and Codex read, or else
/// [`DEFAULT_INCLUDE`].
pub(crate) fn included(main: &Path) -> Vec<PathBuf> {
    let listed = std::fs::read_to_string(main.join(".worktreeinclude")).ok();
    let patterns: Vec<&str> = match &listed {
        Some(listed) => listed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        None => DEFAULT_INCLUDE.to_vec(),
    };
    let mut found: Vec<PathBuf> = patterns
        .iter()
        .flat_map(|pattern| expand(main, pattern))
        .collect();
    found.sort();
    found.dedup();
    found.truncate(MAX_INCLUDED);
    found
}

/// The paths below `root` that `pattern` names, where `*` matches within one
/// part of a path.
fn expand(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    let parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        if *part == ".." {
            return Vec::new();
        }
        let last = index + 1 == parts.len();
        let mut next = Vec::new();
        for base in &paths {
            if !part.contains('*') {
                next.push(base.join(part));
                continue;
            }
            let Ok(entries) = std::fs::read_dir(root.join(base)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                if matches_part(part, &name) && (last || is_dir) {
                    next.push(base.join(name));
                }
            }
        }
        paths = next;
    }
    paths
}

fn matches_part(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    for (index, piece) in pieces.iter().enumerate() {
        if index + 1 == pieces.len() {
            return rest.len() >= piece.len() && rest.ends_with(piece);
        }
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

/// A card's own checkout.
pub(crate) struct Tree {
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
    Git::quick(repo)
        .trimmed(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .is_ok()
}

/// The branch to cut from and land on: the one the project's checkout is on,
/// else the remote's default.
fn base_branch(repo: &Path) -> Result<String, String> {
    if let Ok(branch) = Git::quick(repo).trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        return Ok(branch);
    }
    Git::quick(repo)
        .trimmed(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
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

/// The folder beside the main checkout that every worktree of `repo` goes in,
/// even when `repo` is itself one of those worktrees.
pub(crate) fn worktrees_folder(repo: &Path) -> PathBuf {
    let main = main_checkout(repo).unwrap_or_else(|| repo.to_path_buf());
    let name = main
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    main.with_file_name(format!("{name}.worktrees"))
}

/// The branch Auto's work lands on in `repo` and its newest commit, on the remote when it has one.
pub(crate) fn latest_of_base(repo: &Path) -> Result<(String, String), String> {
    let base = base_branch(repo)?;
    let sha = latest_of(repo, &base)?;
    Ok((base, sha))
}

fn latest_of(repo: &Path, base: &str) -> Result<String, String> {
    let _ = Git::slow(repo).trimmed(&["fetch", "--quiet", "origin", base]);
    let remote = format!("origin/{base}");
    let start = if Git::quick(repo)
        .trimmed(&["rev-parse", "--verify", "--quiet", &remote])
        .is_ok()
    {
        remote
    } else {
        base.to_owned()
    };
    Git::quick(repo).trimmed(&["rev-parse", &start])
}

/// Cuts a worktree for a card beside the project's checkout, on a branch of
/// its own from the latest of the base branch.
pub(crate) fn cut_for_card(project: &Path, branch: &str) -> Result<Tree, String> {
    let repo = PathBuf::from(
        Git::quick(project).trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|_| {
            "the project is not a git repository, so Auto cannot give a card a worktree of its own"
                .to_owned()
        })?,
    );
    let inside = inside(project, &repo);
    let (base, start_sha) = latest_of_base(&repo)?;
    let (branch, path) = cut(&repo, branch, &start_sha)?;
    copy_included(&repo, &path);
    warm::seed_in_background(&repo, &path);
    Ok(Tree {
        cwd: path.join(&inside),
        repo,
        path,
        branch,
        base,
        start_sha,
    })
}

/// Where a card's commits are kept on the remote between ships. Code hosts start checks only
/// for branches and tags, so a ref outside those never starts one.
pub(crate) fn backup_ref(branch: &str) -> String {
    format!("refs/sikemux/auto/{branch}")
}

/// Where a card's worktree was made again.
pub(crate) enum MadeAgain {
    AtCommit(String),
    FromBase,
}

fn has_commit(repo: &Path, sha: &str) -> bool {
    Git::quick(repo)
        .trimmed(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ])
        .is_ok()
}

fn is_ancestor(repo: &Path, older: &str, newer: &str) -> bool {
    Git::quick(repo)
        .trimmed(&["merge-base", "--is-ancestor", older, newer])
        .is_ok()
}

/// Has `sha` here, fetching `name` for it when it is not.
fn fetched(repo: &Path, sha: &str, name: &str) -> Result<String, String> {
    if !has_commit(repo, sha) {
        Git::slow(repo).trimmed(&["fetch", "--quiet", "origin", name])?;
    }
    match has_commit(repo, sha) {
        true => Ok(sha.to_owned()),
        false => Err(format!("{name} on the remote no longer holds {sha}")),
    }
}

/// The newest of the commits Auto knows it pushed for the branch: `kept` as a backup and
/// `shipped` to the branch itself. A rebase that shipped replaces what was kept before it.
fn last_pushed(
    repo: &Path,
    branch: &str,
    kept: Option<&str>,
    shipped: Option<&str>,
) -> Result<Option<String>, String> {
    let kept = (kept.map(|sha| fetched(repo, sha, &backup_ref(branch)))).transpose()?;
    let shipped =
        (shipped.map(|sha| fetched(repo, sha, &format!("refs/heads/{branch}")))).transpose()?;
    Ok(match (kept, shipped) {
        (Some(kept), Some(shipped)) if is_ancestor(repo, &shipped, &kept) => Some(kept),
        (kept, shipped) => shipped.or(kept),
    })
}

/// Makes a card's lost worktree again where it was: at the local branch when that has
/// everything pushed, else at the last commit pushed, else on the latest base.
pub(crate) fn make_again(
    lost: Tree,
    kept: Option<&str>,
    shipped: Option<&str>,
) -> Result<(Tree, MadeAgain), String> {
    let repo = lost.repo.clone();
    let local = Git::quick(&repo)
        .trimmed(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", lost.branch),
        ])
        .ok()
        .filter(|sha| *sha != lost.start_sha);
    let at = match (local, last_pushed(&repo, &lost.branch, kept, shipped)) {
        (Some(local), Ok(Some(pushed))) if is_ancestor(&repo, &pushed, &local) => Some(local),
        (_, Ok(Some(pushed))) => Some(pushed),
        (Some(local), _) => Some(local),
        (None, Ok(None)) => None,
        (None, Err(error)) => return Err(error),
    };
    let (start, made, start_sha) = match at {
        Some(sha) => (
            sha.clone(),
            MadeAgain::AtCommit(sha),
            lost.start_sha.clone(),
        ),
        None => {
            let sha = latest_of(&repo, &lost.base)?;
            (sha.clone(), MadeAgain::FromBase, sha)
        }
    };
    add_again(&repo, &lost.path, &lost.branch, &start)?;
    Ok((Tree { start_sha, ..lost }, made))
}

/// Checks `branch` out at `start` in a worktree of `repo` at `path` again, after its folder
/// was taken away, with what it needs copied in.
pub(crate) fn add_again(repo: &Path, path: &Path, branch: &str, start: &str) -> Result<(), String> {
    Git::quick(repo).trimmed(&["worktree", "prune"])?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    }
    let target = path.to_string_lossy().into_owned();
    Git::slow(repo).trimmed(&[
        "worktree",
        "add",
        "--no-track",
        "-B",
        branch,
        &target,
        start,
    ])?;
    copy_included(repo, path);
    warm::seed_in_background(repo, path);
    Ok(())
}

/// Takes away a card's worktree, its branch and what was kept of it on the remote, and when
/// `remote` the branch on the remote too.
pub(crate) fn remove_with_remote(repo: &Path, tree: &Path, branch: &str, remote: bool) {
    remove(repo, tree, branch);
    let _ = Git::slow(repo).trimmed(&[
        "push",
        "--quiet",
        "--no-verify",
        "origin",
        "--delete",
        &backup_ref(branch),
    ]);
    if !remote {
        return;
    }
    let _ = Git::slow(repo).trimmed(&["push", "--quiet", "origin", "--delete", branch]);
}

/// A reviewer's checkout of one commit, on no branch, that it cannot write to.
pub(crate) struct Checkout {
    pub path: PathBuf,
    /// Where the reviewer works: the project's folder inside the checkout.
    pub cwd: PathBuf,
}

/// Checks `head` out beside the project's checkout, detached, for a reviewer
/// named by `slug`, and takes away the write permission on all of it.
pub(crate) fn add_checkout(
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
    Git::slow(repo).trimmed(&["worktree", "add", "--detach", &target, head])?;
    set_writable(&path, false);
    Ok(Checkout {
        cwd: path.join(inside(project, repo)),
        path,
    })
}

/// A writable copy of `head` beside a reviewer's read-only checkout, where it
/// builds and runs the change without being able to alter what it reviews.
pub(crate) fn add_build_checkout(
    repo: &Path,
    project: &Path,
    head: &str,
    slug: &str,
) -> Result<Checkout, String> {
    let path = worktrees_folder(repo).join(format!("review-{slug}-build"));
    let target = path.to_string_lossy().into_owned();
    Git::slow(repo).trimmed(&["worktree", "add", "--detach", &target, head])?;
    copy_included(repo, &path);
    seed_in_background(repo, &path);
    Ok(Checkout {
        cwd: path.join(inside(project, repo)),
        path,
    })
}

/// How the checkout at `path` strays from `head`, none when what it holds is that commit.
pub(crate) fn strays_from(path: &Path, head: &str) -> Result<Option<String>, String> {
    let now = Git::quick(path).trimmed(&["rev-parse", "HEAD"])?;
    if now != head {
        return Ok(Some(format!("it is at {now} now")));
    }
    let changed = Git::quick(path).trimmed(&["status", "--porcelain", "--untracked-files=no"])?;
    Ok((!changed.is_empty()).then(|| {
        let files: Vec<&str> = changed.lines().take(MAX_STRAYED_FILES).collect();
        format!("these tracked files changed:\n{}", files.join("\n"))
    }))
}

/// Gives a reviewer's checkout its write permission back and takes it away.
pub(crate) fn remove_checkout(repo: &Path, path: &Path) {
    if !path.exists() {
        let _ = Git::quick(repo).trimmed(&["worktree", "prune"]);
        return;
    }
    set_writable(path, true);
    let target = path.to_string_lossy().into_owned();
    if Git::quick(repo)
        .trimmed(&["worktree", "remove", "--force", &target])
        .is_err()
    {
        if let Err(error) = std::fs::remove_dir_all(path) {
            eprintln!("sikemux core: Auto left the review checkout {target}: {error}");
        }
        let _ = Git::quick(repo).trimmed(&["worktree", "prune"]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_goes_beside_the_repository_under_a_name_not_yet_taken() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
            &["branch", "fix/flaky-pty-test"],
        ] {
            Git::quick(&repo).trimmed(args).unwrap();
        }
        let (branch, path) = cut(&repo, "fix/flaky-pty-test", "HEAD").unwrap();
        assert_eq!(branch, "fix/flaky-pty-test-2");
        assert_eq!(
            path,
            parent.path().join("app.worktrees/fix-flaky-pty-test-2")
        );
        assert!(path.join(".git").exists());
        assert!(cut(&repo, "fix/flaky test", "HEAD").is_err());
    }

    #[test]
    fn a_branch_is_named_for_the_work_and_not_for_who_the_agent_is() {
        assert_eq!(
            branch_for(
                "You are the core teammate for the goals feature. Add a goal store to the core."
            ),
            "feat/goal-store-core"
        );
        assert_eq!(
            branch_for(
                "You're the sixth and last app teammate.\nThe rail's tab rows crash on resize"
            ),
            "fix/rail-tab-rows-crash-resize"
        );
        assert_eq!(
            branch_for("Fix how Sikemux names the git branches of agent worktrees."),
            "fix/sikemux-names-git-branches-agent"
        );
        assert_eq!(
            branch_for("ABC-123 Markdown preview find"),
            "feat/abc-123-markdown-preview-find"
        );
    }
}
