//! Each piece of work's own checkout: a git worktree on a branch of its own,
//! in a folder beside the project's repository so it never shows up in the
//! project's own tree or status. Chat agents, Auto's cards and workers for
//! another host's agents all get theirs here.

use std::path::{Path, PathBuf};

pub(crate) use sikemux_wire::protocol::AgentWorktree;

use crate::git::{main_checkout, Git};
use crate::tasks::detect::PERSONAL_TASKS;

pub(crate) mod agent;
pub(crate) mod removal;
mod warm;

pub(crate) use warm::seed_in_background;

const BRANCH_PREFIX: &str = "sikemux/";
const NAME_TRIES: usize = 50;

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

/// Cuts a worktree of `repo` at `start` on a new branch named for `name`.
/// The branch and the folder it is in.
pub(crate) fn cut(repo: &Path, name: &str, start: &str) -> Result<(String, PathBuf), String> {
    let folder = worktrees_folder(repo);
    let base = slug(name);
    let (branch, path) = (1..=NAME_TRIES)
        .map(|index| {
            let slug = if index == 1 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            (format!("{BRANCH_PREFIX}{slug}"), folder.join(&slug))
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

/// Takes away a worktree and its branch.
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

/// Cuts a worktree for a card beside the project's checkout, on a branch of
/// its own from the latest of the base branch.
pub(crate) fn cut_for_card(project: &Path, slug: &str) -> Result<Tree, String> {
    let repo = PathBuf::from(
        Git::quick(project).trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|_| {
            "the project is not a git repository, so Auto cannot give a card a worktree of its own"
                .to_owned()
        })?,
    );
    let inside = inside(project, &repo);
    let base = base_branch(&repo)?;
    let _ = Git::slow(&repo).trimmed(&["fetch", "--quiet", "origin", &base]);
    let remote = format!("origin/{base}");
    let start = if Git::quick(&repo)
        .trimmed(&["rev-parse", "--verify", "--quiet", &remote])
        .is_ok()
    {
        remote
    } else {
        base.clone()
    };
    let start_sha = Git::quick(&repo).trimmed(&["rev-parse", &start])?;
    let (branch, path) = cut(&repo, slug, &start_sha)?;
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

/// Takes away a card's worktree and its branch, here and, when `remote`, on
/// the remote too.
pub(crate) fn remove_with_remote(repo: &Path, tree: &Path, branch: &str, remote: bool) {
    remove(repo, tree, branch);
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
            &["branch", "sikemux/fix-the-flaky-pty-test"],
        ] {
            Git::quick(&repo).trimmed(args).unwrap();
        }
        let (branch, path) = cut(&repo, "Fix the flaky PTY test", "HEAD").unwrap();
        assert_eq!(branch, "sikemux/fix-the-flaky-pty-test-2");
        assert_eq!(
            path,
            parent.path().join("app.worktrees/fix-the-flaky-pty-test-2")
        );
        assert!(path.join(".git").exists());
    }
}
