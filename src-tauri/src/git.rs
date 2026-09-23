use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use git2::{
    BranchType, DiffFormat, DiffLineType, DiffOptions, ErrorCode, Repository, Status, StatusOptions,
};
use serde::Serialize;
use tauri::async_runtime::spawn_blocking;

/// Run a synchronous closure off the Tauri worker pool. Every `pub fn`
/// command in this module used to block the worker thread while libgit2
/// walked the repo; with many projects open + an fs-watch storm, that
/// pool gets saturated and unrelated IPC (PTY input, etc.) stalls.
async fn run_blocking<T, E, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    spawn_blocking(f)
        .await
        .map_err(|e| format!("join: {e}"))
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// Cap on concurrent libgit2 tree walks (`git_status` / `git_log` /
/// `git_overview`). Each walk transiently opens a fistful of fds — index,
/// refs, packfiles, and the recursive untracked-dir scan. With many
/// projects fs-watched at once, a single `npm build` or a busy agent fans a
/// `git_changed` burst across every repo simultaneously; without a cap
/// that's N parallel walks all grabbing fds + CPU, a transient spike that
/// was a contributing factor to the EMFILE wall. 4 lets a few panes refresh
/// in parallel while bounding the peak.
const GIT_WALK_CONCURRENCY: usize = 4;

async fn git_walk_permit() -> Result<tokio::sync::SemaphorePermit<'static>, String> {
    static S: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(GIT_WALK_CONCURRENCY))
        .acquire()
        .await
        .map_err(|e| e.to_string())
}

// ---- helpers --------------------------------------------------------------

fn open_repo(path: &str) -> Result<Repository, String> {
    Repository::discover(path).map_err(|e| e.message().to_string())
}

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

fn kill_and_reap_process(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn read_bounded_pipe(mut pipe: impl Read, exceeded: Arc<AtomicBool>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > MAX_COMMAND_OUTPUT_BYTES {
            exceeded.store(true, Ordering::Release);
            return Err(io::Error::other("subprocess output exceeds 32 MiB limit"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// Capture both pipes while enforcing a deadline and an output cap. Stdin and
/// both output pipes are handled concurrently so no pipe-ordering deadlock is
/// possible. Every timeout/error path kills the process group.
fn run_command_with_timeout(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, String> {
    crate::bounded_process::run(command, input, timeout, MAX_COMMAND_OUTPUT_BYTES, None)
        .map_err(|error| error.to_string())
}

fn run_git(repo: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(repo)
        .args(args);
    let out = run_command_with_timeout(&mut command, None, GIT_COMMAND_TIMEOUT)?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn git_ok(repo: &str, args: &[&str]) -> Result<String, String> {
    let (ok, so, se) = run_git(repo, args)?;
    if ok {
        Ok(so)
    } else {
        Err(if se.trim().is_empty() { so } else { se })
    }
}

fn git_has_head(repo: &str) -> bool {
    git_ok(repo, &["rev-parse", "--verify", "HEAD"]).is_ok()
}

fn current_branch_name(repo: &str) -> Result<String, String> {
    let branch = git_ok(repo, &["branch", "--show-current"])?
        .trim()
        .to_string();
    if branch.is_empty() {
        Err("Cannot operate on a detached HEAD — checkout a branch first.".into())
    } else {
        Ok(branch)
    }
}

fn remote_names(repo: &str) -> Result<Vec<String>, String> {
    Ok(git_ok(repo, &["remote"])?
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn default_remote(repo: &str) -> Result<String, String> {
    let remotes = remote_names(repo)?;
    if remotes.iter().any(|r| r == "origin") {
        return Ok("origin".into());
    }
    if remotes.len() == 1 {
        return Ok(remotes[0].clone());
    }
    if remotes.is_empty() {
        Err("No git remotes configured — add a remote before publishing this branch.".into())
    } else {
        Err(format!(
            "No remote named origin. Pick/set an upstream from the remotes panel. Available remotes: {}",
            remotes.join(", ")
        ))
    }
}

fn has_upstream(repo: &str) -> bool {
    git_ok(
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false)
}

fn path_in_index(repo: &str, path: &str) -> bool {
    run_git(repo, &["ls-files", "--error-unmatch", "--", path])
        .map(|(ok, so, _)| ok && !so.trim().is_empty())
        .unwrap_or(false)
}

fn path_in_head(repo: &str, path: &str) -> bool {
    run_git(repo, &["ls-tree", "-r", "--name-only", "HEAD", "--", path])
        .map(|(ok, so, _)| ok && so.lines().any(|line| line == path))
        .unwrap_or(false)
}

// ---- types ----------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GitFile {
    path: String,
    index: String,
    worktree: String,
}

#[derive(Serialize, Clone)]
pub struct GitStatus {
    branch: String,
    upstream: Option<String>,
    ahead: i32,
    behind: i32,
    files: Vec<GitFile>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiscoveredRepo {
    path: String,
    name: String,
    branch: String,
    ahead: i32,
    behind: i32,
    changes: usize,
}

#[derive(Serialize, Clone)]
pub struct GitBranch {
    name: String,
    current: bool,
    upstream: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct GitCommit {
    /// Short, human-facing id (`8b075bd`).
    hash: String,
    /// Full oid — used by the frontend graph to match parents/children.
    full_hash: String,
    /// Full oids of this commit's parents (>1 == a merge).
    parents: Vec<String>,
    author: String,
    /// Stable key for the author colour chip (initials avatar).
    author_email: String,
    date: String,
    subject: String,
    /// Ref decorations pointing at this commit: `HEAD`, local branches,
    /// `origin/main`, `tag: v0.1.11`. Rendered as lazygit-style badges.
    refs: Vec<String>,
    /// True when this commit is ahead of the current branch's upstream
    /// (reachable from HEAD but not from `@{u}`) — i.e. not yet pushed.
    /// Drives the unpushed-vs-pushed lane colour in the graph.
    unpushed: bool,
}

#[derive(Serialize, Clone)]
pub struct GitOverview {
    status: GitStatus,
    branches: Vec<GitBranch>,
    log: Vec<GitCommit>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GitWorktree {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    reference: Option<String>,
    detached: bool,
    locked: bool,
    lock_reason: Option<String>,
    prunable: bool,
    prune_reason: Option<String>,
    bare: bool,
    current: bool,
    is_main: bool,
}

// ---- worktrees ------------------------------------------------------------

fn worktree_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("worktree path is empty".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("worktree path must be absolute".into());
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("worktree path must not contain '.' or '..' components".into());
    }
    if path.parent().is_none() {
        return Err("the filesystem root cannot be used as a worktree".into());
    }
    Ok(path)
}

fn same_worktree_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Resolve every existing path component, then append the still-missing tail.
/// This catches a target whose lexical parent is harmless but whose nearest
/// existing parent is a symlink into a protected Git/worktree directory.
fn resolved_path_for_containment(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("cannot resolve worktree target {}", path.display()))?;
        missing.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| format!("cannot resolve worktree target {}", path.display()))?;
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|error| format!("resolve worktree target {}: {error}", path.display()))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn validate_worktree_target(
    repo: &str,
    path: &Path,
    worktrees: &[GitWorktree],
) -> Result<(), String> {
    let resolved_target = resolved_path_for_containment(path)?;
    let common_dir = git_ok(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = resolved_path_for_containment(Path::new(common_dir.trim()))?;
    if resolved_target.starts_with(&common_dir) {
        return Err("the target path cannot be inside Git's common directory".into());
    }
    for worktree in worktrees {
        let root = resolved_path_for_containment(Path::new(&worktree.path))?;
        if resolved_target.starts_with(&root) {
            return Err(format!(
                "the target path cannot be inside registered worktree {}",
                worktree.path
            ));
        }
    }
    Ok(())
}

fn current_worktree_path(repo: &str) -> Option<PathBuf> {
    git_ok(
        repo,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )
    .ok()
    .map(|path| PathBuf::from(path.trim()))
    .or_else(|| {
        git_ok(repo, &["rev-parse", "--path-format=absolute", "--git-dir"])
            .ok()
            .map(|path| PathBuf::from(path.trim()))
    })
}

fn read_worktrees(repo: &str) -> Result<Vec<GitWorktree>, String> {
    // `-z` makes paths and optional reason strings unambiguous. Git emits a
    // double NUL between records; every other field is a single NUL token.
    let output = git_ok(repo, &["worktree", "list", "--porcelain", "-z"])?;
    let current = current_worktree_path(repo);
    let mut worktrees = Vec::new();
    let mut fields: Vec<&str> = Vec::new();

    let finish = |fields: &mut Vec<&str>, worktrees: &mut Vec<GitWorktree>| {
        if fields.is_empty() {
            return;
        }
        let mut path = None;
        let mut head = None;
        let mut reference = None;
        let mut detached = false;
        let mut bare = false;
        let mut lock_reason = None;
        let mut prune_reason = None;
        for field in fields.drain(..) {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(value.to_string());
            } else if let Some(value) = field.strip_prefix("HEAD ") {
                head = Some(value.to_string());
            } else if let Some(value) = field.strip_prefix("branch ") {
                reference = Some(value.to_string());
            } else if field == "detached" {
                detached = true;
            } else if field == "bare" {
                bare = true;
            } else if field == "locked" {
                lock_reason = Some(String::new());
            } else if let Some(value) = field.strip_prefix("locked ") {
                lock_reason = Some(value.to_string());
            } else if field == "prunable" {
                prune_reason = Some(String::new());
            } else if let Some(value) = field.strip_prefix("prunable ") {
                prune_reason = Some(value.to_string());
            }
        }
        let Some(path) = path else {
            return;
        };
        let branch = reference
            .as_deref()
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .map(ToOwned::to_owned);
        let path_buf = PathBuf::from(&path);
        let current = current
            .as_deref()
            .is_some_and(|candidate| same_worktree_path(candidate, &path_buf));
        worktrees.push(GitWorktree {
            path,
            head,
            branch,
            reference,
            detached,
            locked: lock_reason.is_some(),
            lock_reason: lock_reason.filter(|reason| !reason.is_empty()),
            prunable: prune_reason.is_some(),
            prune_reason: prune_reason.filter(|reason| !reason.is_empty()),
            bare,
            current,
            // `git worktree list` guarantees the primary worktree first.
            is_main: worktrees.is_empty(),
        });
    };

    for field in output.split('\0') {
        if field.is_empty() {
            finish(&mut fields, &mut worktrees);
        } else {
            fields.push(field);
        }
    }
    finish(&mut fields, &mut worktrees);
    if worktrees.is_empty() {
        Err("git returned no worktrees for this repository".into())
    } else {
        Ok(worktrees)
    }
}

fn validate_branch_name(repo: &str, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("worktree branch name is empty".into());
    }
    if branch.starts_with('-') {
        return Err("worktree branch name cannot start with '-'".into());
    }
    git_ok(repo, &["check-ref-format", "--branch", branch])?;
    Ok(branch.to_string())
}

fn validate_start_point(repo: &str, start_point: Option<String>) -> Result<Option<String>, String> {
    let Some(start_point) = start_point else {
        return Ok(None);
    };
    let start_point = start_point.trim();
    if start_point.is_empty() {
        return Ok(None);
    }
    if start_point.starts_with('-') {
        return Err("worktree start point cannot start with '-'".into());
    }
    let commit = format!("{start_point}^{{commit}}");
    git_ok(repo, &["rev-parse", "--verify", &commit])?;
    Ok(Some(start_point.to_string()))
}

fn create_worktree(
    repo: &str,
    path: &str,
    branch: &str,
    create_branch: bool,
    start_point: Option<String>,
) -> Result<GitWorktree, String> {
    // Discover up front so errors for non-repositories are returned before
    // any target-path or branch processing.
    open_repo(repo)?;
    let path = worktree_path(path)?;
    let path_arg = path.to_string_lossy().into_owned();
    let branch = validate_branch_name(repo, branch)?;
    let start_point = validate_start_point(repo, start_point)?;
    if !create_branch && start_point.is_some() {
        return Err("a start point is only valid when creating a new branch".into());
    }
    let worktrees = read_worktrees(repo)?;
    validate_worktree_target(repo, &path, &worktrees)?;
    let parent = path
        .parent()
        .ok_or_else(|| "worktree path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create worktree parent {}: {error}", parent.display()))?;

    let mut args = vec!["worktree", "add"];
    if create_branch {
        args.extend(["-b", branch.as_str()]);
    }
    args.extend(["--", path_arg.as_str()]);
    if let Some(start_point) = start_point.as_deref() {
        args.push(start_point);
    } else if !create_branch {
        args.push(branch.as_str());
    }
    git_ok(repo, &args)?;

    read_worktrees(repo)?
        .into_iter()
        .find(|worktree| same_worktree_path(Path::new(&worktree.path), &path))
        .ok_or_else(|| "worktree was created but could not be found in Git's registry".into())
}

fn remove_worktree(repo: &str, path: &str, force: bool) -> Result<GitWorktree, String> {
    open_repo(repo)?;
    let path = worktree_path(path)?;
    let worktree = read_worktrees(repo)?
        .into_iter()
        .find(|worktree| same_worktree_path(Path::new(&worktree.path), &path))
        .ok_or_else(|| "the target path is not a registered worktree".to_string())?;
    if worktree.is_main {
        return Err("the main worktree cannot be removed".into());
    }
    if worktree.bare {
        return Err("a bare repository cannot be removed as a linked worktree".into());
    }

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", worktree.path.as_str()]);
    git_ok(repo, &args)?;
    Ok(worktree)
}

#[tauri::command]
pub async fn git_worktree_list(repo: String) -> Result<Vec<GitWorktree>, String> {
    run_blocking(move || read_worktrees(&repo)).await
}

#[tauri::command]
pub async fn git_worktree_create(
    repo: String,
    path: String,
    branch: String,
    create_branch: bool,
    start_point: Option<String>,
) -> Result<GitWorktree, String> {
    run_blocking(move || create_worktree(&repo, &path, &branch, create_branch, start_point)).await
}

#[tauri::command]
pub async fn git_worktree_remove(
    repo: String,
    path: String,
    force: bool,
) -> Result<GitWorktree, String> {
    run_blocking(move || remove_worktree(&repo, &path, force)).await
}

// ---- status ---------------------------------------------------------------

fn status_chars(s: Status) -> (char, char) {
    // map (index, worktree) to porcelain X / Y chars
    let mut x = ' ';
    let mut y = ' ';
    if s.contains(Status::INDEX_NEW) {
        x = 'A';
    } else if s.contains(Status::INDEX_MODIFIED) {
        x = 'M';
    } else if s.contains(Status::INDEX_DELETED) {
        x = 'D';
    } else if s.contains(Status::INDEX_RENAMED) {
        x = 'R';
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        x = 'T';
    }

    if s.contains(Status::WT_NEW) {
        y = '?';
        if x == ' ' {
            x = '?';
        }
    } else if s.contains(Status::WT_MODIFIED) {
        y = 'M';
    } else if s.contains(Status::WT_DELETED) {
        y = 'D';
    } else if s.contains(Status::WT_RENAMED) {
        y = 'R';
    } else if s.contains(Status::WT_TYPECHANGE) {
        y = 'T';
    } else if s.contains(Status::CONFLICTED) {
        x = 'U';
        y = 'U';
    }

    (x, y)
}

fn read_status(repo: &Repository) -> Result<GitStatus, String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true);

    let mut status = GitStatus {
        branch: String::new(),
        upstream: None,
        ahead: 0,
        behind: 0,
        files: Vec::new(),
    };

    // branch + upstream tracking
    if let Ok(head) = repo.head() {
        if let Ok(name) = head.shorthand() {
            status.branch = name.to_string();
        }
        if let Ok(branch) = repo.find_branch(&status.branch, BranchType::Local) {
            if let Ok(up) = branch.upstream() {
                if let Some(n) = up.name().ok().flatten() {
                    status.upstream = Some(n.to_string());
                }
                if let (Some(local_oid), Some(up_oid)) = (head.target(), up.get().target()) {
                    if let Ok((ahead, behind)) = repo.graph_ahead_behind(local_oid, up_oid) {
                        status.ahead = ahead as i32;
                        status.behind = behind as i32;
                    }
                }
            }
        }
    } else if let Ok(rname) = repo.head_detached() {
        if rname {
            status.branch = "HEAD".to_string();
        }
    }
    if status.branch.is_empty() {
        // unborn branch — pull from HEAD reference name
        if let Ok(reference) = repo.find_reference("HEAD") {
            if let Ok(Some(target)) = reference.symbolic_target() {
                status.branch = target
                    .strip_prefix("refs/heads/")
                    .unwrap_or(target)
                    .to_string();
            }
        }
    }

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| e.message().to_string())?;
    for entry in statuses.iter() {
        let path = match entry.path() {
            Ok(p) => p.to_string(),
            Err(_) => continue,
        };
        let (x, y) = status_chars(entry.status());
        if x == ' ' && y == ' ' {
            continue;
        }
        status.files.push(GitFile {
            path,
            index: x.to_string(),
            worktree: y.to_string(),
        });
    }
    Ok(status)
}

/// A walk of a repo stays good until its watcher says the tree moved. Saving a
/// file used to cost four of them: the editor invalidates eagerly, the file
/// tree asks for status on its own, and the watcher fires again 200 ms later.
///
/// A repo with no watcher running is never cached — nothing would tell us when
/// the answer stopped being true.
const REPO_WALK_CACHE_REPOS: usize = 32;

type WalkCache<T> = Mutex<std::collections::HashMap<String, (u64, T)>>;

fn status_cache() -> &'static WalkCache<GitStatus> {
    static C: std::sync::OnceLock<WalkCache<GitStatus>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn overview_cache() -> &'static WalkCache<GitOverview> {
    static C: std::sync::OnceLock<WalkCache<GitOverview>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn watched_generation(repo: &str) -> Option<(String, u64)> {
    let key = crate::files::canonical_repo_key(repo).ok()?;
    let generation = crate::fs_watch::scan_generation(&key)?;
    Some((key, generation))
}

fn cached_walk<T: Clone>(cache: &WalkCache<T>, key: &str, generation: u64) -> Option<T> {
    let cache = cache.lock().ok()?;
    cache
        .get(key)
        .filter(|(stored, _)| *stored == generation)
        .map(|(_, value)| value.clone())
}

fn store_walk<T>(cache: &WalkCache<T>, key: String, generation: u64, value: T) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.len() >= REPO_WALK_CACHE_REPOS && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, (generation, value));
}

/* Only `node_modules` and the `.git` directory itself are skipped, matching what
VS Code leaves out of its own scan. The much wider `files::should_skip_dir` list
is deliberately not reused here, because it hides directory names like `vendor`,
`build` and `out` that are perfectly ordinary repository names. */
fn skip_repo_scan_dir(name: &str) -> bool {
    matches!(name, "node_modules" | ".git")
}

/* One level below the opened folder, like VS Code's default scan depth. A project
directory that is not itself a repository is usually a flat container of them. */
#[tauri::command]
pub async fn git_discover_repos(root: String) -> Result<Vec<DiscoveredRepo>, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<Vec<DiscoveredRepo>, String> {
        let mut found: Vec<DiscoveredRepo> = Vec::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if skip_repo_scan_dir(&name) {
                continue;
            }
            let path = entry.path();
            // `open` rather than `discover`, so a plain directory never reports
            // the repository it happens to sit inside.
            let Ok(repo) = Repository::open(&path) else {
                continue;
            };
            let Ok(status) = read_status(&repo) else {
                continue;
            };
            found.push(DiscoveredRepo {
                path: path.to_string_lossy().to_string(),
                name,
                branch: status.branch,
                ahead: status.ahead,
                behind: status.behind,
                changes: status.files.len(),
            });
        }
        found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(found)
    })
    .await
}

#[tauri::command]
pub async fn git_status(repo: String) -> Result<GitStatus, String> {
    let watched = watched_generation(&repo);
    if let Some((key, generation)) = &watched {
        if let Some(hit) = cached_walk(status_cache(), key, *generation) {
            return Ok(hit);
        }
    }
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<GitStatus, String> {
        let status = read_status(&open_repo(&repo)?)?;
        if let Some((key, generation)) = watched {
            store_walk(status_cache(), key, generation, status.clone());
        }
        Ok(status)
    })
    .await
}

// ---- branches & log -------------------------------------------------------

fn read_branches(repo: &Repository) -> Result<Vec<GitBranch>, String> {
    // Lazygit-style ordering: current branch always at top, then everything
    // else sorted by most-recently-committed-on (so branches you've actually
    // touched recently float up over stale `main` / `master` copies).
    struct Row {
        branch: GitBranch,
        committed_at: i64,
        is_current: bool,
    }
    let mut rows: Vec<Row> = Vec::new();
    let iter = repo
        .branches(Some(BranchType::Local))
        .map_err(|e| e.message().to_string())?;
    for b in iter {
        let (branch, _) = match b {
            Ok(p) => p,
            Err(_) => continue,
        };
        let name = match branch.name() {
            Ok(Some(n)) => n.to_string(),
            _ => continue,
        };
        let upstream = branch
            .upstream()
            .ok()
            .and_then(|up| up.name().ok().flatten().map(String::from));
        let is_current = branch.is_head();
        // Tip-of-branch commit time; 0 if we can't resolve (won't push it
        // above a real branch — the sort prefers larger timestamps).
        let committed_at = branch
            .get()
            .peel_to_commit()
            .map(|c| c.time().seconds())
            .unwrap_or(0);
        rows.push(Row {
            branch: GitBranch {
                name,
                current: is_current,
                upstream,
            },
            committed_at,
            is_current,
        });
    }
    rows.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current) // current = true sorts first
            .then(b.committed_at.cmp(&a.committed_at)) // newer first
            .then(a.branch.name.cmp(&b.branch.name)) // tie-break alphabetical
    });
    Ok(rows.into_iter().map(|r| r.branch).collect())
}

#[tauri::command]
pub async fn git_branches(repo: String) -> Result<Vec<GitBranch>, String> {
    run_blocking(move || read_branches(&open_repo(&repo)?)).await
}

fn relative_time(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - secs).max(0);
    if d < 60 {
        return format!("{}s ago", d);
    }
    if d < 3600 {
        return format!("{}m ago", d / 60);
    }
    if d < 86400 {
        return format!("{}h ago", d / 3600);
    }
    if d < 86400 * 30 {
        return format!("{}d ago", d / 86400);
    }
    if d < 86400 * 365 {
        return format!("{}mo ago", d / (86400 * 30));
    }
    format!("{}y ago", d / (86400 * 365))
}

/// Map commit oid → ref decorations (`HEAD`, local branches, remote branches,
/// tags) for the commits the log is about to return, so the graph timeline can
/// render lazygit-style ref badges without N extra git calls.
///
/// Only refs pointing straight at one of those commits are read. Resolving the
/// rest means loading an object per ref — every remote branch and every tag in
/// the repo — to decorate commits nobody is looking at.
fn build_ref_map(
    repo: &Repository,
    commits: &std::collections::HashSet<git2::Oid>,
) -> std::collections::HashMap<git2::Oid, Vec<String>> {
    let mut map: std::collections::HashMap<git2::Oid, Vec<String>> =
        std::collections::HashMap::new();
    // HEAD first so it renders leftmost on its commit.
    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target().filter(|oid| commits.contains(oid)) {
            map.entry(oid).or_default().push("HEAD".to_string());
        }
    }
    for glob in ["refs/heads/*", "refs/remotes/*", "refs/tags/*"] {
        let Ok(refs) = repo.references_glob(glob) else {
            continue;
        };
        for r in refs.flatten() {
            let Some(oid) = r.target().filter(|oid| commits.contains(oid)) else {
                continue;
            };
            let name = match r.shorthand() {
                Ok(n) => n.to_string(),
                Err(_) => continue,
            };
            // `HEAD` is handled above; `origin/HEAD` & friends are symbolic
            // pointers, not real branches — skip the noise.
            if name == "HEAD" || name.ends_with("/HEAD") {
                continue;
            }
            let label = if r.is_tag() {
                format!("tag: {name}")
            } else {
                name
            };
            map.entry(oid).or_default().push(label);
        }
    }
    map
}

/// Set of commit oids that are ahead of the current branch's upstream —
/// reachable from HEAD but not from `@{u}`. Empty when HEAD is detached or
/// the branch has no upstream (nothing to compare against → all "pushed").
fn unpushed_set(repo: &Repository) -> std::collections::HashSet<git2::Oid> {
    let mut set = std::collections::HashSet::new();
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return set,
    };
    let head_oid = match head.target() {
        Some(o) => o,
        None => return set,
    };
    let upstream_oid = head
        .shorthand()
        .ok()
        .and_then(|name| repo.find_branch(name, BranchType::Local).ok())
        .and_then(|b| b.upstream().ok())
        .and_then(|u| u.get().target());
    let upstream_oid = match upstream_oid {
        Some(o) => o,
        None => return set,
    };
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return set,
    };
    if revwalk.push(head_oid).is_err() || revwalk.hide(upstream_oid).is_err() {
        return set;
    }
    for oid in revwalk.flatten() {
        set.insert(oid);
    }
    set
}

/// Two commits made in the same second can come back parent-first, because the
/// time walk has no way to break the tie. Reorder the window so every commit
/// still precedes its own parents, and leave the time order alone otherwise.
fn children_before_parents(commits: Vec<git2::Commit<'_>>) -> Vec<git2::Commit<'_>> {
    let position: std::collections::HashMap<git2::Oid, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| (commit.id(), i))
        .collect();
    let mut waiting_on_children = vec![0usize; commits.len()];
    for commit in &commits {
        for parent in commit.parent_ids() {
            if let Some(&i) = position.get(&parent) {
                waiting_on_children[i] += 1;
            }
        }
    }

    let mut taken = vec![false; commits.len()];
    let mut order = Vec::with_capacity(commits.len());
    while order.len() < commits.len() {
        let next = (0..commits.len()).find(|&i| !taken[i] && waiting_on_children[i] == 0);
        let Some(next) = next else { break };
        taken[next] = true;
        for parent in commits[next].parent_ids() {
            if let Some(&i) = position.get(&parent) {
                waiting_on_children[i] -= 1;
            }
        }
        order.push(next);
    }
    order.extend((0..commits.len()).filter(|&i| !taken[i]));

    let mut slots: Vec<Option<git2::Commit<'_>>> = commits.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

fn read_log(repo: &Repository, limit: usize) -> Result<Vec<GitCommit>, String> {
    let mut revwalk = repo.revwalk().map_err(|e| e.message().to_string())?;
    if revwalk.push_head().is_err() {
        return Ok(Vec::new());
    }
    // Newest first by commit time. Adding TOPOLOGICAL makes libgit2 enumerate
    // every reachable commit before it can hand back even the first one.
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.message().to_string())?;
    let mut commits = Vec::with_capacity(limit);
    for oid in revwalk.flatten() {
        if commits.len() >= limit {
            break;
        }
        if let Ok(commit) = repo.find_commit(oid) {
            commits.push(commit);
        }
    }
    let commits = children_before_parents(commits);
    let oids: std::collections::HashSet<git2::Oid> = commits.iter().map(|c| c.id()).collect();
    let ref_map = build_ref_map(repo, &oids);
    let unpushed = unpushed_set(repo);
    let mut out = Vec::with_capacity(commits.len());
    for commit in commits {
        let oid = commit.id();
        let short = commit
            .as_object()
            .short_id()
            .ok()
            .and_then(|b| b.as_str().ok().map(String::from))
            .unwrap_or_else(|| oid.to_string()[..7].to_string());
        out.push(GitCommit {
            hash: short,
            full_hash: oid.to_string(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
            author: commit.author().name().unwrap_or("").to_string(),
            author_email: commit.author().email().unwrap_or("").to_string(),
            date: relative_time(commit.time().seconds()),
            subject: commit.summary().ok().flatten().unwrap_or("").to_string(),
            unpushed: unpushed.contains(&oid),
            refs: ref_map.get(&oid).cloned().unwrap_or_default(),
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn git_log(repo: String) -> Result<Vec<GitCommit>, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || read_log(&open_repo(&repo)?, 60)).await
}

#[tauri::command]
pub async fn git_overview(repo: String) -> Result<GitOverview, String> {
    let watched = watched_generation(&repo);
    if let Some((key, generation)) = &watched {
        if let Some(hit) = cached_walk(overview_cache(), key, *generation) {
            return Ok(hit);
        }
    }
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<GitOverview, String> {
        let r = open_repo(&repo)?;
        let overview = GitOverview {
            status: read_status(&r)?,
            branches: read_branches(&r)?,
            log: read_log(&r, 60)?,
        };
        if let Some((key, generation)) = watched {
            store_walk(
                status_cache(),
                key.clone(),
                generation,
                overview.status.clone(),
            );
            store_walk(overview_cache(), key, generation, overview.clone());
        }
        Ok(overview)
    })
    .await
}

#[tauri::command]
pub async fn git_checkout(repo: String, branch: String) -> Result<(), String> {
    // git2 checkout is fiddly with working-tree handling — shell out.
    run_blocking(move || git_ok(&repo, &["checkout", &branch]).map(|_| ())).await
}

fn local_branch_exists(repo: &str, branch: &str) -> bool {
    git_ok(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

fn remote_branch_exists(repo: &str, remote: &str, branch: &str) -> bool {
    git_ok(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{branch}"),
        ],
    )
    .is_ok()
}

fn normalize_branch_input(repo: &str, raw: &str) -> Result<(Option<String>, String), String> {
    let mut b = raw.trim().trim_start_matches("refs/heads/").to_string();
    if let Some(rest) = b.strip_prefix("refs/remotes/") {
        b = rest.to_string();
    }
    if b.is_empty() || b == "—" || b.eq_ignore_ascii_case("n/a") {
        return Err("No deployed branch to checkout for this environment.".into());
    }
    let remotes = remote_names(repo).unwrap_or_default();
    if let Some((maybe_remote, rest)) = b.split_once('/') {
        if remotes.iter().any(|r| r == maybe_remote) {
            return Ok((Some(maybe_remote.to_string()), rest.to_string()));
        }
    }
    Ok((None, b))
}

fn find_remote_branch(
    repo: &str,
    preferred: Option<&str>,
    branch: &str,
) -> Result<Option<String>, String> {
    let remotes = remote_names(repo)?;
    if let Some(r) = preferred {
        return Ok(remote_branch_exists(repo, r, branch).then(|| r.to_string()));
    }
    if remotes.iter().any(|r| r == "origin") && remote_branch_exists(repo, "origin", branch) {
        return Ok(Some("origin".into()));
    }
    let matches: Vec<String> = remotes
        .into_iter()
        .filter(|r| remote_branch_exists(repo, r, branch))
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => Err(format!(
            "Branch {branch} exists on multiple remotes ({}). Checkout from the remotes panel to choose one.",
            many.join(", ")
        )),
    }
}

#[tauri::command]
pub async fn git_checkout_smart(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let (preferred_remote, local) = normalize_branch_input(&repo, &branch)?;
        if local_branch_exists(&repo, &local) {
            git_ok(&repo, &["checkout", &local])?;
            return Ok(format!("checked out {local}"));
        }

        let mut remote = find_remote_branch(&repo, preferred_remote.as_deref(), &local)?;
        if remote.is_none() {
            match preferred_remote.as_deref() {
                Some(r) => {
                    let _ = git_ok(&repo, &["fetch", "--prune", r]);
                }
                None => {
                    let _ = git_ok(&repo, &["fetch", "--all", "--prune"]);
                }
            }
            remote = find_remote_branch(&repo, preferred_remote.as_deref(), &local)?;
        }
        let Some(remote) = remote else {
            return Err(format!(
                "Branch {branch} was not found locally or on any remote after fetch."
            ));
        };
        let full_ref = format!("{remote}/{local}");
        git_ok(&repo, &["checkout", "-b", &local, "--track", &full_ref])?;
        Ok(format!("checked out {local} tracking {full_ref}"))
    })
    .await
}

/// Create a new branch starting at `start_point` (default HEAD) and check it
/// out. Mirrors `git checkout -b name [start_point]` — the usual "branch
/// from where I am right now" flow.
#[tauri::command]
pub async fn git_branch_create(
    repo: String,
    name: String,
    start_point: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        let mut args: Vec<&str> = vec!["checkout", "-b", trimmed];
        if let Some(sp) = start_point.as_deref() {
            if !sp.is_empty() {
                args.push(sp);
            }
        }
        git_ok(&repo, &args).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_branch_delete(repo: String, name: String, force: bool) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        let flag = if force { "-D" } else { "-d" };
        git_ok(&repo, &["branch", flag, trimmed]).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_branch_rename(
    repo: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let old_trimmed = old_name.trim();
        let new_trimmed = new_name.trim();
        if old_trimmed.is_empty() || new_trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(&repo, &["branch", "-m", old_trimmed, new_trimmed]).map(|_| ())
    })
    .await
}

/// Merge `branch` into the current HEAD with a merge commit (--no-ff so the
/// branch topology stays visible — common lazygit / Tower convention).
/// Returns the merge command output; conflict text comes back as the Err
/// for the caller to surface.
#[tauri::command]
pub async fn git_merge(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let trimmed = branch.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(&repo, &["merge", "--no-ff", trimmed])
    })
    .await
}

#[tauri::command]
pub async fn git_merge_squash(repo: String, branch: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let trimmed = branch.trim();
        if trimmed.is_empty() {
            return Err("branch name is empty".into());
        }
        git_ok(&repo, &["merge", "--squash", trimmed])
    })
    .await
}

#[tauri::command]
pub async fn git_reset(repo: String, rev: String, mode: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = rev.trim();
        if trimmed.is_empty() {
            return Err("revision is empty".into());
        }
        let flag = match mode.as_str() {
            "soft" => "--soft",
            "mixed" => "--mixed",
            "hard" => "--hard",
            other => return Err(format!("unknown reset mode: {other}")),
        };
        git_ok(&repo, &["reset", flag, trimmed]).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_revert(repo: String, rev: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let trimmed = rev.trim();
        if trimmed.is_empty() {
            return Err("revision is empty".into());
        }
        git_ok(&repo, &["revert", "--no-edit", trimmed]).map(|_| ())
    })
    .await
}

// ---- diff -----------------------------------------------------------------

fn write_diff_to_string(diff: &git2::Diff) -> Result<String, String> {
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_d, _h, line| {
        match line.origin_value() {
            DiffLineType::Context => out.push(' '),
            DiffLineType::Addition => out.push('+'),
            DiffLineType::Deletion => out.push('-'),
            DiffLineType::Binary => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[binary file changed]\n");
                return true;
            }
            DiffLineType::FileHeader | DiffLineType::HunkHeader => {}
            _ => {}
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.message().to_string())?;
    Ok(out)
}

fn git_diff_sync(repo: String, path: String, staged: bool) -> Result<String, String> {
    let r = open_repo(&repo)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(&path).context_lines(3);

    if staged {
        let head_tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = r
            .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
            .map_err(|e| e.message().to_string())?;
        return write_diff_to_string(&diff);
    }

    let diff = r
        .diff_index_to_workdir(None, Some(&mut opts))
        .map_err(|e| e.message().to_string())?;
    let s = write_diff_to_string(&diff)?;
    if !s.trim().is_empty() {
        return Ok(s);
    }

    // Untracked — fall back to git no-index for parity with the old impl.
    let (_, so, _) = run_git(
        &repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-index",
            "--",
            "/dev/null",
            &path,
        ],
    )?;
    Ok(so)
}

#[tauri::command]
pub async fn git_diff(repo: String, path: String, staged: bool) -> Result<String, String> {
    run_blocking(move || git_diff_sync(repo, path, staged)).await
}

// ---- staging --------------------------------------------------------------

fn git_stage_sync(repo: String, path: String) -> Result<(), String> {
    let r = open_repo(&repo)?;
    let mut index = r.index().map_err(|e| e.message().to_string())?;
    let p = Path::new(&path);
    // If the file is gone, stage the deletion; else add the worktree content.
    let abs = Path::new(&repo).join(p);
    if !abs.exists() {
        index.remove_path(p).map_err(|e| e.message().to_string())?;
    } else {
        index.add_path(p).map_err(|e| e.message().to_string())?;
    }
    index.write().map_err(|e| e.message().to_string())
}

fn git_unstage_sync(repo: String, path: String) -> Result<(), String> {
    let r = open_repo(&repo)?;
    let head_commit = r.head().and_then(|h| h.peel_to_commit()).ok();
    let result = if let Some(head) = head_commit {
        r.reset_default(Some(head.as_object()), [&path])
            .map_err(|e| e.message().to_string())
    } else {
        // Pre-first-commit — remove from index.
        let mut idx = r.index().map_err(|e| e.message().to_string())?;
        idx.remove_path(Path::new(&path))
            .map_err(|e| e.message().to_string())?;
        idx.write().map_err(|e| e.message().to_string())
    };
    result
}

#[tauri::command]
pub async fn git_stage(repo: String, path: String) -> Result<(), String> {
    run_blocking(move || git_stage_sync(repo, path)).await
}

#[tauri::command]
pub async fn git_unstage(repo: String, path: String) -> Result<(), String> {
    run_blocking(move || git_unstage_sync(repo, path)).await
}

/// Paths per `git` invocation. Selecting a range in the git pane can name
/// thousands of files, and an argument list that long is rejected by the OS.
const GIT_PATH_BATCH: usize = 128;

fn git_over_paths(repo: &str, leading: &[&str], paths: &[String]) -> Result<(), String> {
    for chunk in paths.chunks(GIT_PATH_BATCH) {
        let mut args: Vec<&str> = leading.to_vec();
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        git_ok(repo, &args)?;
    }
    Ok(())
}

/// Which of `paths` the given listing command reports back. `-z` because a
/// file name may contain a newline.
fn paths_listed_by(
    repo: &str,
    leading: &[&str],
    paths: &[String],
) -> std::collections::HashSet<String> {
    let mut found = std::collections::HashSet::new();
    for chunk in paths.chunks(GIT_PATH_BATCH) {
        let mut args: Vec<&str> = leading.to_vec();
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        if let Ok((true, out, _)) = run_git(repo, &args) {
            found.extend(
                out.split('\0')
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
            );
        }
    }
    found
}

fn unstage_paths_with_git(repo: &str, paths: &[String]) -> Result<(), String> {
    git_over_paths(repo, &["restore", "--staged"], paths)
        .or_else(|_| git_over_paths(repo, &["reset", "HEAD"], paths))
        .or_else(|_| git_over_paths(repo, &["rm", "--cached", "--ignore-unmatch"], paths))
}

/// Stage a whole selection against one open repository and one index write.
/// Staging file by file re-opened the repo and rewrote the index per row.
#[tauri::command]
pub async fn git_stage_paths(repo: String, paths: Vec<String>) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let r = open_repo(&repo)?;
        let mut index = r.index().map_err(|e| e.message().to_string())?;
        for path in &paths {
            let relative = Path::new(path);
            // A file that is gone stages as a deletion.
            if Path::new(&repo).join(relative).exists() {
                index
                    .add_path(relative)
                    .map_err(|e| e.message().to_string())?;
            } else {
                index
                    .remove_path(relative)
                    .map_err(|e| e.message().to_string())?;
            }
        }
        index.write().map_err(|e| e.message().to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_unstage_paths(repo: String, paths: Vec<String>) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let r = open_repo(&repo)?;
        let head_commit = r.head().and_then(|h| h.peel_to_commit()).ok();
        match head_commit {
            Some(head) => r
                .reset_default(Some(head.as_object()), paths.iter().map(String::as_str))
                .map_err(|e| e.message().to_string()),
            None => {
                // Pre-first-commit — remove from the index.
                let mut index = r.index().map_err(|e| e.message().to_string())?;
                for path in &paths {
                    index
                        .remove_path(Path::new(path))
                        .map_err(|e| e.message().to_string())?;
                }
                index.write().map_err(|e| e.message().to_string())
            }
        }
    })
    .await
}

/// Discard a whole selection. Modes match `git_discard_file`; the paths are
/// grouped so each kind of git call happens once rather than once per file.
#[tauri::command]
pub async fn git_discard_files(
    repo: String,
    paths: Vec<String>,
    mode: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let restore_or_clean = |known: std::collections::HashSet<String>| -> Result<(), String> {
            let (restore, clean): (Vec<String>, Vec<String>) =
                paths.iter().cloned().partition(|path| known.contains(path));
            git_over_paths(&repo, &["restore", "--worktree"], &restore)?;
            git_over_paths(&repo, &["clean", "-f"], &clean)
        };
        match mode.as_str() {
            "staged" => unstage_paths_with_git(&repo, &paths),
            "unstaged" => restore_or_clean(paths_listed_by(&repo, &["ls-files", "-z"], &paths)),
            "all" => {
                let _ = unstage_paths_with_git(&repo, &paths);
                restore_or_clean(paths_listed_by(
                    &repo,
                    &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
                    &paths,
                ))
            }
            other => Err(format!("unknown discard mode: {other}")),
        }
    })
    .await
}

#[tauri::command]
pub async fn git_stage_all(repo: String) -> Result<(), String> {
    run_blocking(move || {
        let r = open_repo(&repo)?;
        let mut idx = r.index().map_err(|e| e.message().to_string())?;
        idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .map_err(|e| e.message().to_string())?;
        // Also stage deletions.
        idx.update_all(["*"], None)
            .map_err(|e| e.message().to_string())?;
        idx.write().map_err(|e| e.message().to_string())
    })
    .await
}

/// Reset every staged change back to HEAD — lazygit-style "unstage all".
/// Used by the `a` toggle in the files panel when everything is already
/// staged. Shells out to `git reset` because libgit2's mixed-reset path
/// is fiddlier than spawning the canonical command.
#[tauri::command]
pub async fn git_unstage_all(repo: String) -> Result<(), String> {
    run_blocking(move || git_ok(&repo, &["reset", "HEAD", "--"]).map(|_| ())).await
}

// ---- show / file_at -------------------------------------------------------

fn revparse_commit<'a>(repo: &'a Repository, rev: &str) -> Result<git2::Commit<'a>, String> {
    repo.revparse_single(rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| e.message().to_string())
}

#[tauri::command]
pub async fn git_show(repo: String, rev: String) -> Result<String, String> {
    // git2's diff doesn't render the message + stat block the way `git show`
    // does — shelling out here costs us nothing and keeps the UI identical.
    run_blocking(move || git_ok(&repo, &["show", "--no-ext-diff", "--stat", "-p", &rev])).await
}

// Content-addressed cache for immutable revs.
//
// LRU by insertion+touch order — entries fall off the front as new ones land
// at the back. `LinkedHashMap` gives us O(1) move-to-back on each hit so the
// ordering stays meaningful.
//
// The budget is bytes, not entries: a count of 500 file revisions is anywhere
// between a few hundred KB and half a gigabyte depending on what the user
// opened.
const FILE_AT_CACHE_BYTES: usize = 32 * 1024 * 1024;

type FileAtKey = (String, String, String);

#[derive(Default)]
struct FileAtCache {
    entries: linked_hash_map::LinkedHashMap<FileAtKey, String>,
    bytes: usize,
}

impl FileAtCache {
    fn get(&mut self, key: &FileAtKey) -> Option<String> {
        self.entries.get_refresh(key).cloned()
    }

    fn insert(&mut self, key: FileAtKey, content: String) {
        if content.len() > FILE_AT_CACHE_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes -= previous.len();
        }
        self.bytes += content.len();
        self.entries.insert(key, content);
        while self.bytes > FILE_AT_CACHE_BYTES {
            match self.entries.pop_front() {
                Some((_, evicted)) => self.bytes -= evicted.len(),
                None => break,
            }
        }
    }
}

fn file_at_cache() -> &'static Mutex<FileAtCache> {
    static C: std::sync::OnceLock<Mutex<FileAtCache>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(FileAtCache::default()))
}

fn is_immutable_rev(rev: &str) -> bool {
    let head = rev.split(['~', '^']).next().unwrap_or(rev);
    head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())
}

const GIT_FILE_AT_MAX_BYTES: usize = 1024 * 1024;

fn human_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn looks_binary_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

fn blob_to_inline_text(blob: &git2::Blob<'_>, path: &str) -> Result<String, String> {
    let bytes = blob.content();
    if bytes.len() > GIT_FILE_AT_MAX_BYTES {
        return Err(format!(
            "{path} is too large for inline diff ({}). Open the file directly or use git diff in the terminal.",
            human_bytes(bytes.len())
        ));
    }
    if looks_binary_bytes(bytes) {
        return Err(format!("{path} is binary; inline diff is disabled."));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| format!("{path} is not UTF-8 text; inline diff is disabled."))
}

#[tauri::command]
pub async fn git_file_at(repo: String, rev: String, path: String) -> Result<String, String> {
    let cacheable = is_immutable_rev(&rev);
    let key = (repo.clone(), rev.clone(), path.clone());
    if cacheable {
        if let Ok(mut cache) = file_at_cache().lock() {
            if let Some(hit) = cache.get(&key) {
                return Ok(hit);
            }
        }
    }
    let cache_key = key.clone();
    run_blocking(move || -> Result<String, String> {
        let r = open_repo(&repo)?;
        let content = if rev == ":index" {
            let idx = r.index().map_err(|e| e.message().to_string())?;
            match idx.get_path(Path::new(&path), 0) {
                Some(entry) => {
                    let blob = r.find_blob(entry.id).map_err(|e| e.message().to_string())?;
                    blob_to_inline_text(&blob, &path)?
                }
                None => String::new(),
            }
        } else {
            match revparse_commit(&r, &rev) {
                Ok(commit) => {
                    let tree = commit.tree().map_err(|e| e.message().to_string())?;
                    match tree.get_path(Path::new(&path)) {
                        Ok(entry) => {
                            let blob = r
                                .find_blob(entry.id())
                                .map_err(|e| e.message().to_string())?;
                            blob_to_inline_text(&blob, &path)?
                        }
                        Err(e) if e.code() == ErrorCode::NotFound => String::new(),
                        Err(e) => return Err(e.message().to_string()),
                    }
                }
                Err(_) => String::new(),
            }
        };
        if cacheable {
            if let Ok(mut cache) = file_at_cache().lock() {
                cache.insert(cache_key, content.clone());
            }
        }
        Ok(content)
    })
    .await
}

#[tauri::command]
pub async fn git_commit_files(repo: String, rev: String) -> Result<Vec<String>, String> {
    run_blocking(move || -> Result<Vec<String>, String> {
        let r = open_repo(&repo)?;
        let commit = revparse_commit(&r, &rev)?;
        let new_tree = commit.tree().map_err(|e| e.message().to_string())?;
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let diff = r
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&new_tree), None)
            .map_err(|e| e.message().to_string())?;
        let mut paths = Vec::new();
        diff.foreach(
            &mut |d, _| {
                if let Some(p) = d.new_file().path().or_else(|| d.old_file().path()) {
                    let s = p.to_string_lossy().into_owned();
                    if !paths.contains(&s) {
                        paths.push(s);
                    }
                }
                true
            },
            None,
            None,
            None,
        )
        .map_err(|e| e.message().to_string())?;
        Ok(paths)
    })
    .await
}

// ---- blame ----------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct BlameCommit {
    /// Full commit oid; all-zeros for not-yet-committed lines.
    sha: String,
    /// Short id for display (`1d3fa0b2`); empty when uncommitted.
    short: String,
    author: String,
    author_email: String,
    /// Pre-formatted relative time (`3 days ago`); empty when uncommitted.
    time: String,
    /// Raw author timestamp (unix seconds) — kept so the UI can re-format.
    timestamp: i64,
    summary: String,
    /// True for lines that only exist in the working buffer (zero sha).
    uncommitted: bool,
}

/// Compact per-file blame: the unique commits touched, plus a parallel array
/// mapping each 0-based line to its commit's index in `commits`. The split
/// keeps the IPC payload small (commit metadata isn't repeated per line) and
/// lets the editor do O(1) cursor-line lookups with no further backend calls.
#[derive(Serialize, Default)]
pub struct GitBlame {
    commits: Vec<BlameCommit>,
    lines: Vec<u32>,
}

fn is_zero_sha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b == b'0')
}

/// Parse `git blame --porcelain`. Commit metadata is emitted only the first
/// time each commit appears, so we accumulate it keyed by sha and remember
/// first-seen order for stable indices.
fn parse_blame_porcelain(out: &str) -> GitBlame {
    use std::collections::HashMap;

    struct Meta {
        author: String,
        author_email: String,
        timestamp: i64,
        summary: String,
    }

    let mut meta: HashMap<String, Meta> = HashMap::new();
    let mut index_of: HashMap<String, u32> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut line_sha: Vec<(usize, String)> = Vec::new();

    let mut cur = String::new();
    let mut cur_line = 0usize;
    let mut max_line = 0usize;

    for line in out.lines() {
        // Content line — closes the entry for the line we last saw a header for.
        if let Some(_content) = line.strip_prefix('\t') {
            if !cur.is_empty() && cur_line > 0 {
                line_sha.push((cur_line, cur.clone()));
                if cur_line > max_line {
                    max_line = cur_line;
                }
            }
            continue;
        }

        // Header: "<40-hex-sha> <orig-line> <final-line> [<group-count>]".
        let b = line.as_bytes();
        let is_header =
            b.len() > 40 && b[40] == b' ' && b[..40].iter().all(|c| c.is_ascii_hexdigit());
        if is_header {
            let mut parts = line.split(' ');
            let sha = parts.next().unwrap_or("").to_string();
            let _orig = parts.next();
            cur_line = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            cur = sha.clone();
            if !index_of.contains_key(&sha) {
                index_of.insert(sha.clone(), order.len() as u32);
                order.push(sha.clone());
                meta.insert(
                    sha,
                    Meta {
                        author: String::new(),
                        author_email: String::new(),
                        timestamp: 0,
                        summary: String::new(),
                    },
                );
            }
            continue;
        }

        // Metadata line for the current commit.
        if let Some(rest) = line.strip_prefix("author ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.author = rest.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("author-mail ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.author_email = rest.trim_matches(|c| c == '<' || c == '>').to_string();
            }
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.timestamp = rest.trim().parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("summary ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.summary = rest.to_string();
            }
        }
    }

    let commits: Vec<BlameCommit> = order
        .iter()
        .map(|sha| {
            let m = &meta[sha];
            let uncommitted = is_zero_sha(sha);
            BlameCommit {
                sha: sha.clone(),
                short: if uncommitted {
                    String::new()
                } else {
                    sha[..8.min(sha.len())].to_string()
                },
                author: if uncommitted {
                    "You".to_string()
                } else {
                    m.author.clone()
                },
                author_email: m.author_email.clone(),
                time: if uncommitted {
                    String::new()
                } else {
                    relative_time(m.timestamp)
                },
                timestamp: m.timestamp,
                summary: if uncommitted {
                    "Uncommitted changes".to_string()
                } else {
                    m.summary.clone()
                },
                uncommitted,
            }
        })
        .collect();

    let mut lines = vec![0u32; max_line];
    for (ln, sha) in line_sha {
        if ln >= 1 && ln <= max_line {
            if let Some(&idx) = index_of.get(&sha) {
                lines[ln - 1] = idx;
            }
        }
    }

    GitBlame { commits, lines }
}

/// Blame a single file. When `contents` is provided we blame that buffer via
/// `--contents -` so unsaved editor edits line up correctly (those lines come
/// back as the zero-sha "uncommitted" commit). Untracked / no-HEAD / binary
/// files have nothing to blame and yield an empty result rather than an error
/// so the editor just shows no inline blame.
#[tauri::command]
pub async fn git_blame(
    repo: String,
    path: String,
    contents: Option<String>,
) -> Result<GitBlame, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<GitBlame, String> {
        let out = match contents {
            Some(text) => {
                let mut command = Command::new("git");
                command.arg("-C").arg(&repo).args([
                    "blame",
                    "--porcelain",
                    "--contents",
                    "-",
                    "--",
                    &path,
                ]);
                let o = run_command_with_timeout(
                    &mut command,
                    Some(text.as_bytes()),
                    GIT_COMMAND_TIMEOUT,
                )?;
                if !o.status.success() {
                    return Ok(GitBlame::default());
                }
                String::from_utf8_lossy(&o.stdout).into_owned()
            }
            None => {
                let (ok, so, _se) = run_git(&repo, &["blame", "--porcelain", "--", &path])?;
                if !ok {
                    return Ok(GitBlame::default());
                }
                so
            }
        };
        Ok(parse_blame_porcelain(&out))
    })
    .await
}

// ---- commit / push / pull -------------------------------------------------

fn commit_with_message(repo: &str, message: &str) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(["commit", "-F", "-"]);
    let out =
        run_command_with_timeout(&mut command, Some(message.as_bytes()), GIT_COMMAND_TIMEOUT)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[tauri::command]
pub async fn git_commit(repo: String, message: String) -> Result<String, String> {
    run_blocking(move || commit_with_message(&repo, &message)).await
}

#[tauri::command]
pub async fn git_push(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let branch = current_branch_name(&repo)?;
        if !has_upstream(&repo) {
            let remote = default_remote(&repo)?;
            let (ok, so, se) = run_git(&repo, &["push", "--set-upstream", &remote, &branch])?;
            return if ok {
                let out = format!("{so}{se}").trim().to_string();
                Ok(if out.is_empty() {
                    format!("published {branch} → {remote}/{branch}")
                } else {
                    format!("published {branch} → {remote}/{branch}\n{out}")
                })
            } else {
                Err(if se.trim().is_empty() { so } else { se })
            };
        }

        let (ok, so, se) = run_git(&repo, &["push"])?;
        if ok {
            return Ok(format!("{so}{se}").trim().to_string());
        }
        // Race-proof fallback: if upstream disappeared between the preflight
        // check and push, publish with -u instead of dumping raw Git advice.
        if se.contains("has no upstream branch") || se.contains("--set-upstream") {
            let remote = default_remote(&repo)?;
            let (ok2, so2, se2) = run_git(&repo, &["push", "--set-upstream", &remote, &branch])?;
            return if ok2 {
                Ok(format!(
                    "published {branch} → {remote}/{branch}\n{}",
                    format!("{so2}{se2}").trim()
                ))
            } else {
                Err(if se2.trim().is_empty() { so2 } else { se2 })
            };
        }
        Err(if se.trim().is_empty() { so } else { se })
    })
    .await
}

fn looks_like_ff_only_divergence(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("not possible to fast-forward")
        || s.contains("divergent branches")
        || s.contains("need to specify how to reconcile")
        || s.contains("fatal: not possible to fast-forward")
}

#[tauri::command]
pub async fn git_pull(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        if !has_upstream(&repo) {
            return Err("No upstream configured for this branch — publish it or set an upstream from the remotes panel first.".into());
        }
        let (ok, so, se) = run_git(&repo, &["pull", "--ff-only"])?;
        if ok {
            return Ok(format!("{so}{se}").trim().to_string());
        }
        let err = format!("{so}{se}");
        if !looks_like_ff_only_divergence(&err) {
            return Err(err.trim().to_string());
        }

        // Git 2.27+ asks users to configure pull.rebase for divergent pulls.
        // Do the app-level sane default instead: rebase with autostash, without
        // mutating the user's global config.
        let (ok2, so2, se2) = run_git(&repo, &["pull", "--rebase", "--autostash"])?;
        if ok2 {
            let out = format!("{so2}{se2}").trim().to_string();
            Ok(if out.is_empty() { "rebased onto upstream".into() } else { format!("rebased onto upstream\n{out}") })
        } else {
            Err(format!(
                "Fast-forward was not possible, and rebase needs attention. Resolve in the git pane or terminal, then continue the rebase.\n\n{}",
                format!("{so2}{se2}").trim()
            ))
        }
    })
    .await
}

// ---- AI commit ------------------------------------------------------------

const PRIMARY_DIFF_BUDGET: usize = 18_000;
const STAT_BUDGET: usize = 4_000;
const AI_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const AI_RESPONSE_LIMIT: usize = 1024 * 1024;
const OMITTED_HUNK_MARKER: &str = "\n[... hunk body compacted ...]\n";
const OMITTED_GENERATED_MARKER: &str =
    "[generated, vendored, or lockfile patch omitted; file and hunk retained]\n";

fn clean_commit_message(raw: &str) -> Result<String, String> {
    let mut t = raw.trim().to_string();
    if t.starts_with("```") {
        if let Some(nl) = t.find('\n') {
            t = t[nl + 1..].to_string();
        }
    }
    let te = t.trim_end();
    if let Some(stripped) = te.strip_suffix("```") {
        t = stripped.to_string();
    }
    const CONV: [&str; 11] = [
        "feat", "fix", "refactor", "chore", "docs", "test", "perf", "build", "ci", "style",
        "revert",
    ];
    let mut lines: Vec<&str> = t.lines().collect();
    while let Some(first) = lines.first() {
        let f = first.trim();
        let is_conv = CONV.iter().any(|p| {
            f.strip_prefix(p)
                .map(|r| r.starts_with(':') || r.starts_with('(') || r.starts_with('!'))
                .unwrap_or(false)
        });
        if is_conv {
            break;
        }
        lines.remove(0);
    }
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if lines.is_empty() {
        return Err("AI returned no usable commit message".into());
    }
    Ok(lines.join("\n"))
}

#[derive(Clone, Copy, Debug)]
enum GitAiProvider {
    Hermes,
    Codex,
    Claude,
}

impl GitAiProvider {
    fn parse(raw: Option<String>) -> Result<Self, String> {
        match raw
            .as_deref()
            .unwrap_or("hermes")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "hermes" => Ok(Self::Hermes),
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            other => Err(format!("unknown local AI provider: {other}")),
        }
    }

    fn bin(self) -> &'static str {
        match self {
            Self::Hermes => "hermes",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::Hermes => "openai/gpt-5.5",
            Self::Codex => "gpt-5.5",
            Self::Claude => "sonnet",
        }
    }
}

#[derive(Clone, Copy)]
enum AiOutputFormat {
    Text,
    ClaudeJson,
}

struct AiStreamDecoder {
    format: AiOutputFormat,
    pending: Vec<u8>,
    message: String,
}

impl AiStreamDecoder {
    fn new(format: AiOutputFormat) -> Self {
        Self {
            format,
            pending: Vec::new(),
            message: String::new(),
        }
    }

    fn emit(
        &mut self,
        text: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        self.message.push_str(text);
        if let Some(channel) = on_chunk {
            channel
                .send(text.to_string())
                .map_err(|e| format!("Commit message stream closed: {e}"))?;
        }
        Ok(())
    }

    fn apply_snapshot(
        &mut self,
        text: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        if let Some(suffix) = text.strip_prefix(&self.message) {
            self.emit(suffix, on_chunk)
        } else {
            // The command's final snapshot is authoritative. The frontend
            // replaces its streamed draft with the returned cleaned message.
            self.message.clear();
            self.message.push_str(text);
            Ok(())
        }
    }

    fn structured_line(
        &mut self,
        line: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            return Ok(());
        }
        let event: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("Local AI returned invalid streaming JSON: {e}"))?;
        match self.format {
            AiOutputFormat::ClaudeJson => {
                if event.get("type").and_then(|value| value.as_str()) == Some("stream_event") {
                    let delta = event.pointer("/event/delta");
                    if delta
                        .and_then(|value| value.get("type"))
                        .and_then(|value| value.as_str())
                        == Some("text_delta")
                    {
                        if let Some(text) = delta
                            .and_then(|value| value.get("text"))
                            .and_then(|value| value.as_str())
                        {
                            self.emit(text, on_chunk)?;
                        }
                    }
                } else if event.get("type").and_then(|value| value.as_str()) == Some("result") {
                    if let Some(text) = event.get("result").and_then(|value| value.as_str()) {
                        self.apply_snapshot(text, on_chunk)?;
                    }
                }
            }
            AiOutputFormat::Text => unreachable!("text output is not line decoded"),
        }
        Ok(())
    }

    fn push(
        &mut self,
        bytes: &[u8],
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        self.pending.extend_from_slice(bytes);
        if matches!(self.format, AiOutputFormat::Text) {
            loop {
                match std::str::from_utf8(&self.pending) {
                    Ok(text) => {
                        let text = text.to_string();
                        self.pending.clear();
                        return self.emit(&text, on_chunk);
                    }
                    Err(error) if error.valid_up_to() > 0 => {
                        let valid = error.valid_up_to();
                        let text = std::str::from_utf8(&self.pending[..valid])
                            .expect("validated UTF-8 prefix")
                            .to_string();
                        self.pending.drain(..valid);
                        self.emit(&text, on_chunk)?;
                    }
                    Err(error) if error.error_len().is_none() => return Ok(()),
                    Err(error) => {
                        return Err(format!("Local AI returned invalid UTF-8: {error}"));
                    }
                }
            }
        }

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            let line = std::str::from_utf8(&line)
                .map_err(|e| format!("Local AI returned invalid UTF-8: {e}"))?;
            self.structured_line(line, on_chunk)?;
        }
        Ok(())
    }

    fn finish(mut self, on_chunk: Option<&tauri::ipc::Channel<String>>) -> Result<String, String> {
        if !self.pending.is_empty() {
            match self.format {
                AiOutputFormat::Text => {
                    let text = std::str::from_utf8(&self.pending)
                        .map_err(|e| format!("Local AI returned incomplete UTF-8: {e}"))?
                        .to_string();
                    self.pending.clear();
                    self.emit(&text, on_chunk)?;
                }
                AiOutputFormat::ClaudeJson => {
                    let line = std::str::from_utf8(&self.pending)
                        .map_err(|e| format!("Local AI returned invalid UTF-8: {e}"))?
                        .to_string();
                    self.pending.clear();
                    self.structured_line(&line, on_chunk)?;
                }
            }
        }
        Ok(self.message)
    }
}

fn command_candidates(name: &str) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![
        name.to_string(),
        format!("{home}/.local/bin/{name}"),
        format!("{home}/.cargo/bin/{name}"),
        format!("{home}/.opencode/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
    ];
    if let Some(path) = crate::system::find_executable(name) {
        candidates.insert(0, path.to_string_lossy().into_owned());
    }
    candidates.dedup();
    candidates
}

#[derive(Debug, PartialEq)]
enum CodexAppServerEvent {
    Initialized,
    ThreadStarted(String),
    TurnStarted,
    AgentMessageDelta(String),
    AgentMessageCompleted(String),
    TurnCompleted,
    Error(String),
    Ignore,
}

fn codex_error_message(value: &serde_json::Value) -> Option<String> {
    value
        .get("message")
        .and_then(|message| message.as_str())
        .map(str::to_string)
}

fn parse_codex_app_server_event(line: &str) -> Result<CodexAppServerEvent, String> {
    let event: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| format!("Codex app-server returned invalid JSON: {error}"))?;

    if let Some(error) = event.get("error") {
        return Ok(CodexAppServerEvent::Error(
            codex_error_message(error).unwrap_or_else(|| error.to_string()),
        ));
    }

    match event.get("id").and_then(|id| id.as_u64()) {
        Some(0) => return Ok(CodexAppServerEvent::Initialized),
        Some(1) => {
            let thread_id = event
                .pointer("/result/thread/id")
                .and_then(|id| id.as_str())
                .ok_or_else(|| "Codex app-server did not return a thread id".to_string())?;
            return Ok(CodexAppServerEvent::ThreadStarted(thread_id.to_string()));
        }
        Some(2) => return Ok(CodexAppServerEvent::TurnStarted),
        _ => {}
    }

    let method = event.get("method").and_then(|method| method.as_str());
    match method {
        Some("item/agentMessage/delta") => event
            .pointer("/params/delta")
            .and_then(|delta| delta.as_str())
            .map(|delta| CodexAppServerEvent::AgentMessageDelta(delta.to_string()))
            .ok_or_else(|| "Codex app-server emitted a text delta without text".to_string()),
        Some("item/completed")
            if event
                .pointer("/params/item/type")
                .and_then(|kind| kind.as_str())
                == Some("agentMessage") =>
        {
            event
                .pointer("/params/item/text")
                .and_then(|text| text.as_str())
                .map(|text| CodexAppServerEvent::AgentMessageCompleted(text.to_string()))
                .ok_or_else(|| {
                    "Codex app-server completed an assistant message without text".to_string()
                })
        }
        Some("turn/completed") => {
            let status = event
                .pointer("/params/turn/status")
                .and_then(|status| status.as_str())
                .unwrap_or("failed");
            if status == "completed" {
                Ok(CodexAppServerEvent::TurnCompleted)
            } else {
                let detail = event
                    .pointer("/params/turn/error/message")
                    .and_then(|message| message.as_str())
                    .unwrap_or(status);
                Ok(CodexAppServerEvent::Error(format!(
                    "Codex turn {status}: {detail}"
                )))
            }
        }
        Some("error") => {
            let will_retry = event
                .pointer("/params/willRetry")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if will_retry {
                Ok(CodexAppServerEvent::Ignore)
            } else {
                let error = event
                    .pointer("/params/error")
                    .and_then(codex_error_message)
                    .unwrap_or_else(|| "Codex turn failed".to_string());
                Ok(CodexAppServerEvent::Error(error))
            }
        }
        _ => Ok(CodexAppServerEvent::Ignore),
    }
}

fn write_codex_app_server_message(
    stdin: &mut impl Write,
    message: &serde_json::Value,
) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, message)
        .map_err(|error| format!("Could not encode Codex app-server request: {error}"))?;
    stdin
        .write_all(b"\n")
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("Could not write to Codex app-server: {error}"))
}

fn read_codex_app_server_lines(
    mut stdout: impl Read,
    sender: std::sync::mpsc::Sender<Result<String, String>>,
    exceeded: Arc<AtomicBool>,
) {
    let mut total = 0usize;
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => {
                if !pending.is_empty() {
                    let line = String::from_utf8(pending).map_err(|error| {
                        format!("Codex app-server returned invalid UTF-8: {error}")
                    });
                    let _ = sender.send(line);
                }
                return;
            }
            Ok(read) => {
                total = total.saturating_add(read);
                if total > AI_RESPONSE_LIMIT {
                    exceeded.store(true, Ordering::Release);
                    let _ = sender.send(Err("Codex app-server output exceeds 1 MiB limit".into()));
                    return;
                }
                pending.extend_from_slice(&chunk[..read]);
                while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut line = pending.drain(..=newline).collect::<Vec<_>>();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let line = String::from_utf8(line).map_err(|error| {
                        format!("Codex app-server returned invalid UTF-8: {error}")
                    });
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = sender.send(Err(format!("Could not read Codex app-server: {error}")));
                return;
            }
        }
    }
}

fn run_codex_app_server_candidate(
    bin: &str,
    repo: &str,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let repo = Path::new(repo)
        .canonicalize()
        .map_err(|error| format!("Could not resolve repository path: {error}"))?;
    let repo = repo.to_string_lossy().into_owned();
    let mut command = Command::new(bin);
    command
        .current_dir(&repo)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("codex failed to start: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stdout unavailable".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stderr unavailable".into());
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stdin unavailable".into());
        }
    };

    let output_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_exceeded = Arc::clone(&output_exceeded);
    let (line_sender, line_receiver) = std::sync::mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        read_codex_app_server_lines(stdout, line_sender, stdout_exceeded)
    });
    let stderr_exceeded = Arc::clone(&output_exceeded);
    let stderr_reader = std::thread::spawn(move || read_bounded_pipe(stderr, stderr_exceeded));

    let result = (|| -> Result<String, String> {
        write_codex_app_server_message(
            &mut stdin,
            &serde_json::json!({
                "id": 0,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "sikemux",
                        "title": "Sikemux",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;

        let deadline = Instant::now() + AI_COMMAND_TIMEOUT;
        let mut final_message = None;
        loop {
            if output_exceeded.load(Ordering::Acquire) {
                return Err("Local AI output exceeds its safety limit".into());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "codex timed out after {}s",
                    AI_COMMAND_TIMEOUT.as_secs()
                ));
            }
            match line_receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(line)) => match parse_codex_app_server_event(line.trim_end())? {
                    CodexAppServerEvent::Initialized => {
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({"method": "initialized"}),
                        )?;
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({
                                "id": 1,
                                "method": "thread/start",
                                "params": {
                                    "model": model,
                                    "cwd": repo,
                                    "approvalPolicy": "never",
                                    "sandbox": "read-only",
                                    "ephemeral": true,
                                    "serviceName": "sikemux",
                                    "developerInstructions": "Answer directly from the supplied prompt. Do not call tools or inspect the workspace. Return only the requested commit message."
                                }
                            }),
                        )?;
                    }
                    CodexAppServerEvent::ThreadStarted(thread_id) => {
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({
                                "id": 2,
                                "method": "turn/start",
                                "params": {
                                    "threadId": thread_id,
                                    "input": [{"type": "text", "text": prompt}]
                                }
                            }),
                        )?;
                    }
                    CodexAppServerEvent::AgentMessageDelta(delta) => {
                        if let Some(channel) = &on_chunk {
                            channel.send(delta).map_err(|error| {
                                format!("Commit message stream closed: {error}")
                            })?;
                        }
                    }
                    CodexAppServerEvent::AgentMessageCompleted(message) => {
                        final_message = Some(message);
                    }
                    CodexAppServerEvent::TurnCompleted => {
                        return final_message
                            .filter(|message| !message.trim().is_empty())
                            .ok_or_else(|| "codex returned no commit message".to_string());
                    }
                    CodexAppServerEvent::Error(error) => {
                        return Err(format!("codex failed: {error}"))
                    }
                    CodexAppServerEvent::TurnStarted | CodexAppServerEvent::Ignore => {}
                },
                Ok(Err(error)) => return Err(error),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => match child.try_wait() {
                    Ok(Some(status)) => {
                        return Err(format!(
                            "codex app-server exited unexpectedly with {status}"
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => return Err(format!("codex failed: {error}")),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("codex app-server closed its output unexpectedly".into());
                }
            }
        }
    })();

    drop(stdin);
    kill_and_reap_process(&mut child);
    let _ = stdout_reader.join();
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Codex app-server stderr reader panicked".to_string())?
        .map_err(|error| error.to_string())?;

    result.map_err(|error| {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        if detail.is_empty() || error.contains(&detail) {
            error
        } else {
            format!("{error}: {detail}")
        }
    })
}

fn run_codex_app_server(
    repo: &str,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    for bin in command_candidates("codex") {
        match run_codex_app_server_candidate(&bin, repo, model, prompt, on_chunk.clone()) {
            Err(error) if error.starts_with("codex failed to start: No such file") => continue,
            result => return result,
        }
    }
    Err("codex is not installed or could not be found on PATH".into())
}

fn read_ai_pipe(
    mut pipe: impl Read,
    format: AiOutputFormat,
    on_chunk: Option<tauri::ipc::Channel<String>>,
    exceeded: Arc<AtomicBool>,
) -> Result<String, String> {
    let mut decoder = AiStreamDecoder::new(format);
    let mut total = 0usize;
    let mut chunk = [0_u8; 4096];
    let mut decode_error = None;
    loop {
        let read = pipe.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            if let Some(error) = decode_error {
                return Err(error);
            }
            return decoder.finish(on_chunk.as_ref());
        }
        total = total.saturating_add(read);
        if total > AI_RESPONSE_LIMIT {
            exceeded.store(true, Ordering::Release);
            return Err("Local AI response exceeds 1 MiB limit".into());
        }
        if decode_error.is_none() {
            if let Err(error) = decoder.push(&chunk[..read], on_chunk.as_ref()) {
                decode_error = Some(error);
            }
        }
    }
}

fn run_ai_candidate<F>(
    provider: GitAiProvider,
    on_chunk: Option<tauri::ipc::Channel<String>>,
    mut build: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> (Command, Option<Vec<u8>>, AiOutputFormat),
{
    for bin in command_candidates(provider.bin()) {
        let (mut command, input, format) = build(&bin);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{} failed to start: {error}", provider.bin())),
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                kill_and_reap_process(&mut child);
                return Err(format!("{} stdout unavailable", provider.bin()));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                kill_and_reap_process(&mut child);
                return Err(format!("{} stderr unavailable", provider.bin()));
            }
        };

        let output_exceeded = Arc::new(AtomicBool::new(false));
        let stdout_exceeded = Arc::clone(&output_exceeded);
        let stream_channel = on_chunk.clone();
        let stdout_reader = std::thread::spawn(move || {
            read_ai_pipe(stdout, format, stream_channel, stdout_exceeded)
        });
        let stderr_exceeded = Arc::clone(&output_exceeded);
        let stderr_reader = std::thread::spawn(move || read_bounded_pipe(stderr, stderr_exceeded));
        let stdin_writer = input.map(|bytes| {
            let stdin = child.stdin.take();
            std::thread::spawn(move || {
                stdin
                    .ok_or_else(|| "Local AI stdin unavailable".to_string())
                    .and_then(|mut stdin| stdin.write_all(&bytes).map_err(|e| e.to_string()))
            })
        });

        let deadline = Instant::now() + AI_COMMAND_TIMEOUT;
        let status = loop {
            if output_exceeded.load(Ordering::Acquire) {
                kill_and_reap_process(&mut child);
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("Local AI output exceeds its safety limit".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    kill_and_reap_process(&mut child);
                    if let Some(writer) = stdin_writer {
                        let _ = writer.join();
                    }
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(format!(
                        "{} timed out after {}s",
                        provider.bin(),
                        AI_COMMAND_TIMEOUT.as_secs()
                    ));
                }
                Err(error) => {
                    kill_and_reap_process(&mut child);
                    return Err(format!("{} failed: {error}", provider.bin()));
                }
            }
        };
        if let Some(writer) = stdin_writer {
            writer
                .join()
                .map_err(|_| "Local AI stdin writer panicked".to_string())??;
        }
        let message = stdout_reader
            .join()
            .map_err(|_| "Local AI stdout reader panicked".to_string())?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| "Local AI stderr reader panicked".to_string())?
            .map_err(|e| e.to_string())?;
        if !status.success() {
            let detail = String::from_utf8_lossy(&stderr).trim().to_string();
            return Err(if detail.is_empty() {
                format!("{} exited with {status}", provider.bin())
            } else {
                format!("{} failed: {detail}", provider.bin())
            });
        }
        let message = message?;
        if message.trim().is_empty() {
            return Err(format!("{} returned no commit message", provider.bin()));
        }
        return Ok(message);
    }
    Err(format!(
        "{} is not installed or could not be found on PATH",
        provider.bin()
    ))
}

fn run_ai_commit_model(
    repo: &str,
    provider: GitAiProvider,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let model = if model.trim().is_empty() {
        provider.default_model()
    } else {
        model.trim()
    };
    match provider {
        GitAiProvider::Hermes => run_ai_candidate(provider, on_chunk, |bin| {
            let mut command = Command::new(bin);
            command
                .current_dir(repo)
                .args(["chat", "-Q", "-m", model, "-t", "safe", "-q", prompt]);
            (command, None, AiOutputFormat::Text)
        }),
        GitAiProvider::Codex => run_codex_app_server(repo, model, prompt, on_chunk),
        GitAiProvider::Claude => run_ai_candidate(provider, on_chunk, |bin| {
            let mut command = Command::new(bin);
            command.current_dir(repo).args([
                "--print",
                "--model",
                model,
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--no-session-persistence",
                "--tools",
                "",
            ]);
            (
                command,
                Some(prompt.as_bytes().to_vec()),
                AiOutputFormat::ClaudeJson,
            )
        }),
    }
}

/// Build the AI prompt from a stat + diff. Shared by the generate-only
/// (`git_ai_message`) and stage-and-commit (`git_ai_commit`) paths so the
/// message style stays identical.
fn commit_message_prompt(repo: &str, stat: &str, diff: &str, compacted: bool) -> String {
    let branch = git_ok(repo, &["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let repo_name = std::path::Path::new(repo)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let diff_note = if compacted {
        "Unchanged context was removed. Every changed file and hunk header is represented, but noisy files and oversized hunk bodies may be compacted. Infer the commit from the retained changed lines plus the stat."
    } else {
        "Unchanged context was removed; all changed lines are included."
    };
    format!(
        "You are generating a Git commit message from the staged diff below.\n\
         Return ONLY the commit message. No markdown, no explanation, no quotes, no code fences.\n\n\
         Rules:\n\
         - First line: conventional commit format: type(scope): subject\n\
         - Imperative mood, no trailing period, <=72 chars if possible\n\
         - For trivial changes: subject line only\n\
         - For non-trivial changes: blank line after subject, then 2-6 bullets starting with \"- \"\n\
         - Common types: feat, fix, refactor, chore, docs, test, perf, build, ci, style\n\
         - Scope should be short and inferred from files/package/service when obvious; omit scope if unclear\n\n\
         Repo: {repo_name}\n\
         Branch: {branch}\n\n\
         Staged stat:\n{stat}\n\n\
         Diff note: {diff_note}\n\n\
         Staged changed-line diff:\n{diff}\n"
    )
}

#[derive(Debug)]
struct DiffHunk {
    header: String,
    body: String,
}

#[derive(Debug, Default)]
struct DiffFile {
    header: String,
    hunks: Vec<DiffHunk>,
}

#[derive(Debug)]
struct PreparedDiff {
    text: String,
    compacted: bool,
}

fn parse_diff_files(diff: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;

    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(DiffFile {
                header: line.to_string(),
                hunks: Vec::new(),
            });
            continue;
        }

        let file = current.get_or_insert_with(DiffFile::default);
        if line.starts_with("@@") || line == "GIT binary patch\n" {
            file.hunks.push(DiffHunk {
                header: line.to_string(),
                body: String::new(),
            });
        } else if let Some(hunk) = file.hunks.last_mut() {
            hunk.body.push_str(line);
        } else {
            file.header.push_str(line);
        }
    }
    if let Some(file) = current {
        files.push(file);
    }
    files
}

fn is_noisy_diff(file: &DiffFile) -> bool {
    let header = file.header.to_ascii_lowercase();
    const NAMES: [&str; 12] = [
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "cargo.lock",
        "gemfile.lock",
        "poetry.lock",
        "uv.lock",
        "composer.lock",
        "go.sum",
        ".snap",
    ];
    NAMES.iter().any(|name| header.contains(name))
        || ["/dist/", "/vendor/", ".min.js", ".min.css", ".map"]
            .iter()
            .any(|part| header.contains(part))
}

/// Distribute a byte budget evenly at first, then give unused shares to larger
/// entries. This prevents an early large file from starving every later file.
fn fair_allocations(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut allocations = vec![0; sizes.len()];
    let mut remaining = budget;
    let mut active: Vec<usize> = sizes
        .iter()
        .enumerate()
        .filter_map(|(index, size)| (*size > 0).then_some(index))
        .collect();

    while remaining > 0 && !active.is_empty() {
        let share = (remaining / active.len()).max(1);
        let mut granted = 0;
        active.retain(|index| {
            let need = sizes[*index] - allocations[*index];
            let available = remaining.saturating_sub(granted);
            let give = need.min(share).min(available);
            allocations[*index] += give;
            granted += give;
            allocations[*index] < sizes[*index] && granted < remaining
        });
        if granted == 0 {
            break;
        }
        remaining -= granted;
    }
    allocations
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn utf8_suffix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut start = value.len() - max_bytes;
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn sample_hunk_body(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    format!(
        "{}{}{}",
        utf8_prefix(body, budget.div_ceil(2)),
        OMITTED_HUNK_MARKER,
        utf8_suffix(body, budget / 2)
    )
}

fn structure_manifest(files: &[DiffFile], budget: usize) -> String {
    let mut manifest = String::from(
        "[diff has more structural metadata than the request budget; patch bodies omitted]\n",
    );
    for file in files {
        if let Some(first) = file.header.lines().next() {
            manifest.push_str(first);
            manifest.push('\n');
        }
        for hunk in &file.hunks {
            manifest.push_str(&hunk.header);
        }
    }
    if manifest.len() <= budget {
        return manifest;
    }
    let marker = "\n[... structural manifest compacted ...]\n";
    let content_budget = budget.saturating_sub(marker.len());
    format!(
        "{}{}{}",
        utf8_prefix(&manifest, content_budget.div_ceil(2)),
        marker,
        utf8_suffix(&manifest, content_budget / 2)
    )
}

fn prepare_diff(diff: &str, budget: usize) -> PreparedDiff {
    if diff.len() <= budget {
        return PreparedDiff {
            text: diff.to_string(),
            compacted: false,
        };
    }

    let files = parse_diff_files(diff);
    let noisy: Vec<bool> = files.iter().map(is_noisy_diff).collect();
    let mut fixed_size = 0usize;
    let mut file_body_sizes = Vec::with_capacity(files.len());
    for (file, noisy) in files.iter().zip(&noisy) {
        fixed_size = fixed_size.saturating_add(file.header.len());
        let mut body_size = 0usize;
        if *noisy && file.hunks.is_empty() {
            fixed_size = fixed_size.saturating_add(OMITTED_GENERATED_MARKER.len());
        }
        for hunk in &file.hunks {
            fixed_size = fixed_size.saturating_add(hunk.header.len());
            if *noisy {
                fixed_size = fixed_size.saturating_add(OMITTED_GENERATED_MARKER.len());
            } else if !hunk.body.is_empty() {
                fixed_size = fixed_size.saturating_add(OMITTED_HUNK_MARKER.len());
                body_size = body_size.saturating_add(hunk.body.len());
            }
        }
        file_body_sizes.push(body_size);
    }

    if fixed_size >= budget {
        return PreparedDiff {
            text: structure_manifest(&files, budget),
            compacted: true,
        };
    }

    let file_allocations = fair_allocations(&file_body_sizes, budget - fixed_size);
    let mut output = String::with_capacity(budget);
    for ((file, noisy), file_budget) in files.iter().zip(&noisy).zip(file_allocations) {
        output.push_str(&file.header);
        if *noisy && file.hunks.is_empty() {
            output.push_str(OMITTED_GENERATED_MARKER);
        }
        let hunk_sizes: Vec<usize> = file.hunks.iter().map(|hunk| hunk.body.len()).collect();
        let hunk_allocations = fair_allocations(&hunk_sizes, file_budget);
        for (hunk, hunk_budget) in file.hunks.iter().zip(hunk_allocations) {
            output.push_str(&hunk.header);
            if *noisy {
                output.push_str(OMITTED_GENERATED_MARKER);
            } else {
                output.push_str(&sample_hunk_body(&hunk.body, hunk_budget));
            }
        }
    }
    PreparedDiff {
        text: output,
        compacted: true,
    }
}

fn compact_stat(stat: &str) -> String {
    if stat.len() <= STAT_BUDGET {
        return stat.to_string();
    }
    let marker = "\n[... stat compacted ...]\n";
    let budget = STAT_BUDGET - marker.len();
    format!(
        "{}{}{}",
        utf8_prefix(stat, budget.div_ceil(2)),
        marker,
        utf8_suffix(stat, budget / 2)
    )
}

fn staged_diff(repo: &str) -> Result<(String, String), String> {
    Ok((
        git_ok(repo, &["diff", "--cached", "--stat"])?,
        git_ok(repo, &["diff", "--cached", "--no-ext-diff", "--unified=0"])?,
    ))
}

fn worktree_diff(repo: &str) -> Result<(String, String), String> {
    if git_has_head(repo) {
        Ok((
            git_ok(repo, &["diff", "HEAD", "--stat"])?,
            git_ok(repo, &["diff", "HEAD", "--no-ext-diff", "--unified=0"])?,
        ))
    } else {
        Ok((
            git_ok(repo, &["diff", "--stat"])?,
            git_ok(repo, &["diff", "--no-ext-diff", "--unified=0"])?,
        ))
    }
}

fn describe_diff_with_local_ai(
    repo: &str,
    stat: &str,
    diff: &str,
    provider: GitAiProvider,
    model: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let stat = compact_stat(stat);
    let prepared = prepare_diff(diff, PRIMARY_DIFF_BUDGET);
    let prompt = commit_message_prompt(repo, &stat, &prepared.text, prepared.compacted);
    clean_commit_message(&run_ai_commit_model(
        repo, provider, model, &prompt, on_chunk,
    )?)
}

/// Generate a commit message through a locally installed CLI without staging
/// or committing. Output chunks are forwarded to the commit textarea.
#[tauri::command]
pub async fn git_ai_message(
    repo: String,
    provider: Option<String>,
    model: Option<String>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let (mut stat, mut diff) = staged_diff(&repo)?;
        if diff.trim().is_empty() {
            (stat, diff) = worktree_diff(&repo)?;
        }
        if diff.trim().is_empty() {
            return Err("Nothing to describe — stage changes or edit some files first.".into());
        }
        let provider = GitAiProvider::parse(provider)?;
        let model = model.unwrap_or_else(|| provider.default_model().to_string());
        describe_diff_with_local_ai(&repo, &stat, &diff, provider, &model, Some(on_chunk))
    })
    .await
}

#[tauri::command]
pub async fn git_ai_commit(
    repo: String,
    provider: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        // Auto-stage all working changes if nothing's been staged yet.
        if git_ok(&repo, &["diff", "--cached", "--name-only"])?
            .trim()
            .is_empty()
        {
            git_ok(&repo, &["add", "-A"])?;
            if git_ok(&repo, &["diff", "--cached", "--name-only"])?
                .trim()
                .is_empty()
            {
                return Err("Nothing to commit — working tree is clean.".into());
            }
        }
        let (stat, diff) = staged_diff(&repo)?;
        let provider = GitAiProvider::parse(provider)?;
        let model = model.unwrap_or_else(|| provider.default_model().to_string());
        let message = describe_diff_with_local_ai(&repo, &stat, &diff, provider, &model, None)?;
        commit_with_message(&repo, &message)?;
        Ok(message)
    })
    .await
}

// ---- open PR --------------------------------------------------------------

#[tauri::command]
pub async fn pr_open(repo: String) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let r = open_repo(&repo)?;
        let remote_url = r
            .find_remote("origin")
            .map_err(|e| e.message().to_string())?
            .url()
            .map_err(|e| e.message().to_string())?
            .to_string();

        let branch = r
            .head()
            .ok()
            .and_then(|h| h.shorthand().ok().map(String::from))
            .ok_or("no current branch (detached HEAD?)")?;

        let mut url = if let Some(rest) = remote_url.strip_prefix("git@") {
            match rest.split_once(':') {
                Some((host, path)) => format!("https://{host}/{}", path.trim_end_matches(".git")),
                None => remote_url.clone(),
            }
        } else {
            remote_url.trim_end_matches(".git").to_string()
        };

        if url.contains("github.com") {
            url = format!("{url}/compare/{branch}?expand=1");
        } else if url.contains("bitbucket.org") {
            url = format!("{url}/pull-requests/new?source={branch}");
        } else {
            return Err(format!("unsupported remote host: {url}"));
        }

        open::that_detached(&url).map_err(|e| e.to_string())?;
        Ok(url)
    })
    .await
}

// ---- discard --------------------------------------------------------------

/// Discard changes to a single file. `mode`:
///   - "unstaged"       → revert working tree to match the index
///                        (= `git restore --worktree <path>`). Staged changes are
///                        preserved.
///   - "staged"         → unstage but leave the worktree alone
///                        (= `git restore --staged <path>`).
///   - "all"            → discard staged AND unstaged changes: first
///                        unstage, then restore. For untracked files this
///                        deletes the file (= `git clean -f <path>`).
///
/// For new (untracked) files, "unstaged" and "all" both remove the file
/// since there's no index or HEAD version to restore from.
#[tauri::command]
pub async fn git_discard_file(repo: String, path: String, mode: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        match mode.as_str() {
            "staged" => {
                git_ok(&repo, &["restore", "--staged", "--", &path])
                    .or_else(|_| git_ok(&repo, &["reset", "HEAD", "--", &path]))
                    .or_else(|_| {
                        git_ok(&repo, &["rm", "--cached", "--ignore-unmatch", "--", &path])
                    })?;
            }
            "unstaged" => {
                if path_in_index(&repo, &path) {
                    // Restore the worktree from the index, preserving staged
                    // content. `checkout HEAD -- path` would also wipe staged edits.
                    git_ok(&repo, &["restore", "--worktree", "--", &path])?;
                } else {
                    git_ok(&repo, &["clean", "-f", "--", &path])?;
                }
            }
            "all" => {
                let _ = git_ok(&repo, &["restore", "--staged", "--", &path])
                    .or_else(|_| git_ok(&repo, &["reset", "HEAD", "--", &path]));
                if path_in_head(&repo, &path) {
                    git_ok(&repo, &["restore", "--worktree", "--", &path])?;
                } else {
                    git_ok(&repo, &["clean", "-f", "--", &path])?;
                }
            }
            other => return Err(format!("unknown discard mode: {other}")),
        }
        Ok(())
    })
    .await
}

// ---- stash ---------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GitStash {
    /// Reflog index (0 = top). We treat this as the stable id within a
    /// single session, but it shifts whenever the user drops/pops, so
    /// the UI re-reads after every mutation.
    index: usize,
    /// Stash commit id. This is the stable guard used before apply/pop/drop so
    /// external stash-list edits cannot make a stale UI row target another stash.
    sha: String,
    /// `stash@{N}` — the symbolic ref form, useful for `git stash apply <ref>`.
    refname: String,
    /// Branch name the stash was created from.
    branch: String,
    /// Free-form message (usually `WIP on <branch>: <sha> <subject>`).
    message: String,
}

#[tauri::command]
pub async fn git_stash_list(repo: String) -> Result<Vec<GitStash>, String> {
    run_blocking(move || -> Result<Vec<GitStash>, String> {
        // Format chosen so we don't depend on lazy field parsing — `%gd` is
        // the selector (`stash@{N}`), `%gs` is the message. We compute branch
        // from the message prefix (`WIP on <branch>:` / `On <branch>:`).
        let out = git_ok(&repo, &["stash", "list", "--format=%H%x09%gd%x09%gs"])?;
        let mut entries = Vec::new();
        for (idx, line) in out.lines().enumerate() {
            let mut parts = line.splitn(3, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            let refname = parts.next().unwrap_or("").to_string();
            let message = parts.next().unwrap_or("").to_string();
            let branch = parse_stash_branch(&message);
            entries.push(GitStash {
                index: idx,
                sha,
                refname,
                branch,
                message,
            });
        }
        Ok(entries)
    })
    .await
}

fn parse_stash_branch(message: &str) -> String {
    // `WIP on foo: 1234abc subject` or `On foo: custom message`
    let stripped = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))
        .unwrap_or(message);
    stripped.split(':').next().unwrap_or("").trim().to_string()
}

fn resolve_stash_ref(repo: &str, refname: &str, expected_sha: &str) -> Result<String, String> {
    let cur_sha = git_ok(repo, &["rev-parse", refname])
        .unwrap_or_default()
        .trim()
        .to_string();
    if !expected_sha.is_empty() && cur_sha == expected_sha {
        return Ok(refname.to_string());
    }

    let out = git_ok(repo, &["stash", "list", "--format=%H%x09%gd"])?;
    for line in out.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        if sha == expected_sha && !name.is_empty() {
            return Ok(name.to_string());
        }
    }

    if expected_sha.is_empty() && !cur_sha.is_empty() {
        return Ok(refname.to_string());
    }
    Err(format!(
        "stash {refname} changed or no longer exists; refresh the stash list"
    ))
}

/// Create a new stash. `mode`:
///   - "all" (default)     → `git stash push -u` (includes untracked).
///   - "staged"            → `git stash push --staged`.
///   - "unstaged"          → `git stash push --keep-index` then a fixup
///                           that leaves only the unstaged work in the
///                           stash. Implemented as `--keep-index` since
///                           that's the closest single-command match.
#[tauri::command]
pub async fn git_stash_push(
    repo: String,
    message: Option<String>,
    mode: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let mut args: Vec<String> = vec!["stash".into(), "push".into()];
        match mode.as_str() {
            "all" => {
                args.push("-u".into());
            }
            "staged" => {
                args.push("--staged".into());
            }
            "unstaged" => {
                args.push("--keep-index".into());
            }
            other => return Err(format!("unknown stash mode: {other}")),
        }
        if let Some(m) = message {
            if !m.trim().is_empty() {
                args.push("-m".into());
                args.push(m);
            }
        }
        let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
        git_ok(&repo, &str_args)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_stash_apply(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "apply", &r])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_stash_pop(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "pop", &r])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_stash_drop(repo: String, refname: String, sha: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "drop", &r])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_stash_branch(
    repo: String,
    refname: String,
    sha: String,
    name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        git_ok(&repo, &["stash", "branch", &name, &r])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_stash_rename(
    repo: String,
    refname: String,
    sha: String,
    new_message: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        // No native `git stash rename`. Store the replacement first, then remove
        // the now-shifted original. A failure can leave a harmless duplicate but
        // can never remove the only ordinary stash reference.
        let r = resolve_stash_ref(&repo, &refname, &sha)?;
        // Grab the underlying commit SHA for the stash so we can re-store.
        let sha = git_ok(&repo, &["rev-parse", &r])?.trim().to_string();
        if sha.is_empty() {
            return Err(format!("could not resolve {r}"));
        }
        git_ok(&repo, &["stash", "store", "-m", &new_message, &sha])?;
        let original_index = r
            .strip_prefix("stash@{")
            .and_then(|value| value.strip_suffix('}'))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                format!(
                    "unexpected stash reference {r}; replacement was stored but the original was kept"
                )
            })?;
        let shifted_original = format!("stash@{{{}}}", original_index + 1);
        let shifted_sha = git_ok(&repo, &["rev-parse", &shifted_original])?
            .trim()
            .to_string();
        if shifted_sha != sha {
            return Err(
                "stash list changed during rename; replacement was stored and the original was kept"
                    .to_string(),
            );
        }
        git_ok(&repo, &["stash", "drop", &shifted_original])?;
        Ok(())
    })
    .await
}

// ---- remotes -------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GitRemote {
    pub name: String,
    /// Fetch URL — the one we display + use for cloning context.
    pub url: String,
}

/// `git remote -v` shape: `<name>\t<url> (fetch|push)` per line. We keep
/// only the fetch URL since that's authoritative for branch listing.
#[tauri::command]
pub async fn git_remotes(repo: String) -> Result<Vec<GitRemote>, String> {
    run_blocking(move || -> Result<Vec<GitRemote>, String> {
        let out = git_ok(&repo, &["remote", "-v"])?;
        let mut seen: std::collections::HashMap<String, String> = Default::default();
        for line in out.lines() {
            // Format: "origin\tgit@github.com:foo/bar.git (fetch)"
            let mut parts = line.split('\t');
            let name = parts.next().unwrap_or("").trim().to_string();
            let rest = parts.next().unwrap_or("");
            if name.is_empty() || rest.is_empty() {
                continue;
            }
            // Only keep fetch URLs.
            let is_fetch = rest.trim_end().ends_with("(fetch)");
            if !is_fetch {
                continue;
            }
            let url = rest
                .rsplit_once(' ')
                .map(|(url, _)| url)
                .unwrap_or("")
                .trim()
                .to_string();
            if !url.is_empty() {
                seen.insert(name, url);
            }
        }
        let mut list: Vec<GitRemote> = seen
            .into_iter()
            .map(|(name, url)| GitRemote { name, url })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    })
    .await
}

#[tauri::command]
pub async fn git_remote_add(repo: String, name: String, url: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "add", &name, &url])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_remote_remove(repo: String, name: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "remove", &name])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_remote_rename(
    repo: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "rename", &old_name, &new_name])?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_remote_set_url(repo: String, name: String, url: String) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["remote", "set-url", &name, &url])?;
        Ok(())
    })
    .await
}

/// Fetch a single remote when `remote` is set, otherwise `--all`. Always
/// passes `--prune` so stale remote-tracking branches get reaped — this
/// matches lazygit's default and avoids the "branch shows up after it was
/// deleted upstream" trap.
#[tauri::command]
pub async fn git_fetch(repo: String, remote: Option<String>) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let out = match remote {
            Some(r) if !r.is_empty() => git_ok(&repo, &["fetch", "--prune", &r])?,
            _ => git_ok(&repo, &["fetch", "--all", "--prune"])?,
        };
        Ok(out)
    })
    .await
}

// ---- remote branches -----------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GitRemoteBranch {
    /// Branch name WITHOUT the remote prefix (`main` not `origin/main`).
    pub name: String,
    /// Full ref form (`origin/main`) — what `git checkout --track` expects.
    pub full_ref: String,
    /// True if this is the symbolic HEAD pointer for the remote (e.g.
    /// `origin/HEAD -> origin/main`). UI shows it differently and skips
    /// it from most ops.
    pub is_head_pointer: bool,
    /// Local branch currently tracking this remote ref, if any.
    pub tracked_by: Option<String>,
    /// Tip subject line for the branch (best-effort).
    pub subject: Option<String>,
}

/// List branches under `refs/remotes/<remote>/`. We use `for-each-ref` so
/// we can extract the upstream-of mapping + the tip subject in a single
/// command instead of fanning out N `log -1` calls.
#[tauri::command]
pub async fn git_remote_branches(
    repo: String,
    remote: String,
) -> Result<Vec<GitRemoteBranch>, String> {
    run_blocking(move || -> Result<Vec<GitRemoteBranch>, String> {
        let prefix = format!("refs/remotes/{remote}/");
        let format = "%(refname:short)%09%(symref)%09%(subject)";
        let out = git_ok(
            &repo,
            &[
                "for-each-ref",
                "--sort=refname",
                &format!("--format={format}"),
                &prefix,
            ],
        )?;

        // Build the local-branch → upstream map once so we can annotate each
        // remote branch with its tracking local. The cheap form:
        // `git for-each-ref refs/heads --format='%(refname:short)\t%(upstream:short)'`.
        let mut upstreams: std::collections::HashMap<String, String> = Default::default();
        if let Ok(locals) = git_ok(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname:short)\t%(upstream:short)",
                "refs/heads/",
            ],
        ) {
            for line in locals.lines() {
                let mut p = line.splitn(2, '\t');
                let local = p.next().unwrap_or("").to_string();
                let upstream = p.next().unwrap_or("").trim().to_string();
                if !upstream.is_empty() {
                    upstreams.insert(upstream, local);
                }
            }
        }

        let mut list = Vec::new();
        for line in out.lines() {
            let mut p = line.splitn(3, '\t');
            let full_ref = p.next().unwrap_or("").to_string();
            let symref = p.next().unwrap_or("").trim().to_string();
            let subject = p.next().unwrap_or("").to_string();
            if full_ref.is_empty() {
                continue;
            }
            let name = full_ref
                .strip_prefix(&format!("{remote}/"))
                .unwrap_or(&full_ref)
                .to_string();
            let is_head_pointer = !symref.is_empty() || name == "HEAD";
            list.push(GitRemoteBranch {
                name,
                full_ref: full_ref.clone(),
                is_head_pointer,
                tracked_by: upstreams.get(&full_ref).cloned(),
                subject: if subject.is_empty() {
                    None
                } else {
                    Some(subject)
                },
            });
        }
        Ok(list)
    })
    .await
}

/// Check out a remote branch into a new local tracking branch. If
/// `local_name` is omitted, uses the remote branch's leaf name (so
/// `origin/feat/foo` → local `feat/foo`).
#[tauri::command]
pub async fn git_checkout_remote_branch(
    repo: String,
    remote: String,
    branch: String,
    local_name: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        let full_ref = format!("{remote}/{branch}");
        let local = local_name.unwrap_or_else(|| branch.clone());
        // If the local already exists, just `checkout <local>`; otherwise
        // create-and-track.
        let exists = git_ok(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{local}"),
            ],
        )
        .is_ok();
        if exists {
            git_ok(&repo, &["checkout", &local])?;
        } else {
            git_ok(&repo, &["checkout", "-b", &local, "--track", &full_ref])?;
        }
        Ok(())
    })
    .await
}

/// Delete a remote branch by pushing the empty ref. Strict form
/// `git push <remote> --delete <branch>` — the one lazygit invokes.
#[tauri::command]
pub async fn git_delete_remote_branch(
    repo: String,
    remote: String,
    branch: String,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        git_ok(&repo, &["push", &remote, "--delete", &branch])?;
        Ok(())
    })
    .await
}

/// Point a local branch's upstream at the given remote ref. Pass `null` /
/// empty `upstream` to clear the upstream entirely (matches lazygit's
/// "unset upstream" flow).
#[tauri::command]
pub async fn git_set_upstream(
    repo: String,
    branch: String,
    upstream: Option<String>,
) -> Result<(), String> {
    run_blocking(move || -> Result<(), String> {
        match upstream {
            Some(u) if !u.is_empty() => {
                git_ok(
                    &repo,
                    &["branch", &format!("--set-upstream-to={u}"), &branch],
                )?;
            }
            _ => {
                git_ok(&repo, &["branch", "--unset-upstream", &branch])?;
            }
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};
    use tempfile::tempdir;

    fn repo_arg(repo: &Path) -> String {
        repo.to_string_lossy().into_owned()
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?}\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn git_at(repo: &Path, stamp: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_DATE", stamp)
            .env("GIT_COMMITTER_DATE", stamp)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn init_repo() -> tempfile::TempDir {
        let td = tempdir().expect("tempdir");
        git(td.path(), &["init"]);
        git(td.path(), &["config", "user.email", "sikemux@example.test"]);
        git(td.path(), &["config", "user.name", "sikemux"]);
        git(td.path(), &["config", "core.autocrlf", "false"]);
        td
    }

    #[test]
    fn decodes_claude_partial_stream_across_single_byte_chunks() {
        let stream = concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"feat(git): \"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"stream ✨\"}}}\n",
            "{\"type\":\"result\",\"result\":\"feat(git): stream ✨\"}\n",
        );
        let mut decoder = AiStreamDecoder::new(AiOutputFormat::ClaudeJson);
        for byte in stream.as_bytes() {
            decoder
                .push(std::slice::from_ref(byte), None)
                .expect("stream chunk");
        }

        assert_eq!(
            decoder.finish(None).expect("finish"),
            "feat(git): stream ✨"
        );
    }

    #[test]
    fn decodes_codex_app_server_token_delta_and_final_message() {
        let delta = parse_codex_app_server_event(
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"item","delta":"fix(ui): "}}"#,
        )
        .expect("delta");
        let completed = parse_codex_app_server_event(
            r#"{"method":"item/completed","params":{"threadId":"thread","turnId":"turn","completedAtMs":1,"item":{"id":"item","type":"agentMessage","text":"fix(ui): stream token output"}}}"#,
        )
        .expect("completed message");
        assert_eq!(
            delta,
            CodexAppServerEvent::AgentMessageDelta("fix(ui): ".into())
        );
        assert_eq!(
            completed,
            CodexAppServerEvent::AgentMessageCompleted("fix(ui): stream token output".into())
        );
    }

    #[test]
    fn decodes_codex_app_server_handshake_and_turn_completion() {
        assert_eq!(
            parse_codex_app_server_event(r#"{"id":0,"result":{"userAgent":"test"}}"#)
                .expect("initialize"),
            CodexAppServerEvent::Initialized
        );
        assert_eq!(
            parse_codex_app_server_event(r#"{"id":1,"result":{"thread":{"id":"thread-123"}}}"#)
                .expect("thread"),
            CodexAppServerEvent::ThreadStarted("thread-123".into())
        );
        assert_eq!(
            parse_codex_app_server_event(
                r#"{"method":"turn/completed","params":{"threadId":"thread-123","turn":{"id":"turn","items":[],"status":"completed"}}}"#
            )
            .expect("turn"),
            CodexAppServerEvent::TurnCompleted
        );
    }

    #[test]
    fn text_decoder_preserves_split_utf8() {
        let text = "feat(git): stream ✨";
        let mut decoder = AiStreamDecoder::new(AiOutputFormat::Text);
        for byte in text.as_bytes() {
            decoder
                .push(std::slice::from_ref(byte), None)
                .expect("text chunk");
        }

        assert_eq!(decoder.finish(None).expect("finish"), text);
    }

    #[test]
    fn keeps_small_diffs_verbatim() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-old\n+new ✨\n";
        let prepared = prepare_diff(diff, 1_000);

        assert!(!prepared.compacted);
        assert_eq!(prepared.text, diff);
    }

    #[test]
    fn compacted_diff_keeps_every_file_hunk_and_both_ends() {
        let large_body = |label: &str| {
            format!(
                "-{label}_BEGIN_✨{}\n+{}{label}_END_🚀\n",
                "old".repeat(220),
                "new".repeat(220)
            )
        };
        let diff = format!(
            "diff --git a/src/first.rs b/src/first.rs\n--- a/src/first.rs\n+++ b/src/first.rs\n@@ -1 +1 @@ FIRST_HUNK\n{}@@ -20 +20 @@ SECOND_HUNK\n{}diff --git a/src/last.rs b/src/last.rs\n--- a/src/last.rs\n+++ b/src/last.rs\n@@ -2 +2 @@ LAST_HUNK\n{}",
            large_body("FIRST"),
            large_body("SECOND"),
            large_body("LAST")
        );
        let budget = 1_000;
        let prepared = prepare_diff(&diff, budget);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= budget);
        for expected in [
            "diff --git a/src/first.rs b/src/first.rs",
            "FIRST_HUNK",
            "SECOND_HUNK",
            "diff --git a/src/last.rs b/src/last.rs",
            "LAST_HUNK",
            "FIRST_BEGIN_✨",
            "FIRST_END_🚀",
            "LAST_BEGIN_✨",
            "LAST_END_🚀",
        ] {
            assert!(
                prepared.text.contains(expected),
                "missing {expected}:\n{}",
                prepared.text
            );
        }
        assert!(prepared.text.contains(OMITTED_HUNK_MARKER.trim()));
    }

    #[test]
    fn compacted_diff_summarizes_lockfiles_but_keeps_their_structure() {
        let diff = format!(
            "diff --git a/package-lock.json b/package-lock.json\n--- a/package-lock.json\n+++ b/package-lock.json\n@@ -1 +1 @@ LOCK_HUNK\n-LOCK_PAYLOAD_START{}LOCK_PAYLOAD_END\n+LOCK_PAYLOAD_NEW{}\ndiff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1 +1 @@ APP_HUNK\n-old{}\n+new{}\n",
            "x".repeat(1_200),
            "y".repeat(1_200),
            "a".repeat(500),
            "b".repeat(500)
        );
        let prepared = prepare_diff(&diff, 900);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= 900);
        assert!(prepared.text.contains("package-lock.json"));
        assert!(prepared.text.contains("LOCK_HUNK"));
        assert!(prepared.text.contains(OMITTED_GENERATED_MARKER.trim()));
        assert!(!prepared.text.contains("LOCK_PAYLOAD_END"));
        assert!(prepared.text.contains("src/app.ts"));
        assert!(prepared.text.contains("APP_HUNK"));
    }

    #[test]
    fn compaction_is_utf8_safe_at_tiny_boundaries() {
        let diff = format!(
            "diff --git a/emoji.rs b/emoji.rs\n--- a/emoji.rs\n+++ b/emoji.rs\n@@ -1 +1 @@\n-{}\n+{}\n",
            "🦀".repeat(400),
            "✨".repeat(400)
        );
        let prepared = prepare_diff(&diff, 320);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= 320);
        assert!(prepared.text.contains("diff --git a/emoji.rs"));
        assert!(prepared.text.contains("@@ -1 +1 @@"));
    }

    #[test]
    fn fair_budget_does_not_starve_later_entries() {
        let allocations = fair_allocations(&[10_000, 20, 10_000], 300);

        assert_eq!(allocations.iter().sum::<usize>(), 300);
        assert_eq!(allocations[1], 20);
        assert!(allocations[0] > 0);
        assert!(allocations[2] > 0);
        assert!((allocations[0] as isize - allocations[2] as isize).abs() <= 1);
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_drains_output_before_child_reads_stdin() {
        let input = vec![b'i'; 2 * 1024 * 1024];
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "dd if=/dev/zero bs=1048576 count=2 2>/dev/null; cat >/dev/null",
        ]);
        let out = run_command_with_timeout(&mut command, Some(&input), Duration::from_secs(5))
            .expect("concurrent stdin/stdout handling must not deadlock");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 2 * 1024 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_output_is_bounded() {
        let mut command = Command::new("sh");
        command.args(["-c", "dd if=/dev/zero bs=1048576 count=33 2>/dev/null"]);
        let error = run_command_with_timeout(&mut command, None, Duration::from_secs(5))
            .expect_err("oversized output must be rejected");
        assert!(error.contains("output exceeds"), "{error}");
    }

    fn commit_base(repo: &Path) {
        fs::write(repo.join("f.txt"), "base\n").expect("write base");
        git(repo, &["add", "f.txt"]);
        git(repo, &["commit", "-m", "base"]);
    }

    /// A branch or tag that decorates a commit outside the window must not cost
    /// anything, and the ones inside it must still be labelled.
    #[test]
    fn ref_badges_cover_the_returned_commits_and_nothing_else() {
        let td = init_repo();
        commit_base(td.path());
        git(td.path(), &["tag", "on-base"]);
        git(td.path(), &["branch", "side"]);
        fs::write(td.path().join("f.txt"), "second\n").expect("write second");
        git(td.path(), &["commit", "-am", "second"]);
        git(td.path(), &["tag", "on-tip"]);

        let repo = open_repo(&repo_arg(td.path())).expect("open repo");
        let log = read_log(&repo, 1).expect("read log");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "second");

        let refs = &log[0].refs;
        assert!(refs.contains(&"HEAD".to_string()), "{refs:?}");
        assert!(refs.contains(&"tag: on-tip".to_string()), "{refs:?}");
        assert!(!refs.contains(&"side".to_string()), "{refs:?}");
        assert!(!refs.contains(&"tag: on-base".to_string()), "{refs:?}");
    }

    /// Commits made in the same second come out of the time walk in whatever
    /// order the heap likes. The log still has to read newest-first.
    #[test]
    fn a_commit_never_follows_its_own_parent_in_the_log() {
        let td = init_repo();
        let stamp = "2026-09-18T13:54:44+05:30";
        for subject in ["base", "second", "third", "fourth"] {
            fs::write(td.path().join("f.txt"), format!("{subject}\n")).expect("write file");
            git(td.path(), &["add", "f.txt"]);
            git_at(td.path(), stamp, &["commit", "-m", subject]);
        }

        let repo = open_repo(&repo_arg(td.path())).expect("open repo");
        let log = read_log(&repo, 10).expect("read log");

        let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["fourth", "third", "second", "base"]);
    }

    #[test]
    fn file_revision_cache_evicts_by_bytes_not_by_entry_count() {
        let mut cache = FileAtCache::default();
        let key = |n: usize| (format!("/repo{n}"), "rev".to_string(), "f.txt".to_string());
        let half = "x".repeat(FILE_AT_CACHE_BYTES / 2 + 1);

        cache.insert(key(1), half.clone());
        cache.insert(key(2), half.clone());
        assert!(cache.bytes <= FILE_AT_CACHE_BYTES);
        assert!(cache.get(&key(1)).is_none());
        assert_eq!(cache.get(&key(2)).as_deref(), Some(half.as_str()));

        // A single revision larger than the whole budget is served, never stored.
        cache.insert(key(3), "y".repeat(FILE_AT_CACHE_BYTES + 1));
        assert!(cache.get(&key(3)).is_none());
        assert_eq!(cache.get(&key(2)).as_deref(), Some(half.as_str()));
    }

    /// A walk is reused only while the watcher's count stands still; the next
    /// change moves it on and the stale answer is skipped.
    #[test]
    fn a_cached_walk_is_only_reused_for_the_generation_it_was_read_at() {
        let cache: WalkCache<String> = Mutex::new(std::collections::HashMap::new());
        store_walk(&cache, "/repo".into(), 7, "walked".to_string());

        assert_eq!(cached_walk(&cache, "/repo", 7).as_deref(), Some("walked"));
        assert!(cached_walk(&cache, "/repo", 8).is_none());
        assert!(cached_walk(&cache, "/other", 7).is_none());
    }

    #[test]
    fn worktree_create_list_and_remove_new_branch() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent
            .path()
            .join("missing")
            .join("project")
            .join("feature-worktree");

        let created = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "feature/worktrees",
            true,
            Some("HEAD".into()),
        )
        .expect("create worktree");

        assert_eq!(created.branch.as_deref(), Some("feature/worktrees"));
        assert_eq!(
            created.reference.as_deref(),
            Some("refs/heads/feature/worktrees")
        );
        assert!(!created.is_main);
        assert!(!created.current);
        assert!(target.join("f.txt").is_file());
        let listed = read_worktrees(td.path().to_str().expect("repo path")).expect("list");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].is_main);
        assert!(listed[0].current);

        let removed = remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            false,
        )
        .expect("remove worktree");
        assert_eq!(removed.branch.as_deref(), Some("feature/worktrees"));
        assert!(!target.exists());
        assert_eq!(
            read_worktrees(td.path().to_str().expect("repo path"))
                .expect("list after removal")
                .len(),
            1
        );
    }

    #[test]
    fn worktree_uses_existing_branch_and_tracks_current_context() {
        let td = init_repo();
        commit_base(td.path());
        git(td.path(), &["branch", "existing"]);
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("existing-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "existing",
            false,
            None,
        )
        .expect("create existing branch worktree");

        let listed = read_worktrees(target.to_str().expect("target path")).expect("linked list");
        assert_eq!(listed.len(), 2);
        assert!(!listed[0].current);
        assert!(listed[1].current);
        assert_eq!(listed[1].branch.as_deref(), Some("existing"));
    }

    #[test]
    fn worktree_remove_refuses_main_and_requires_force_for_dirty_tree() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("dirty-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "dirty",
            true,
            None,
        )
        .expect("create worktree");

        let main_error = remove_worktree(
            target.to_str().expect("linked repo path"),
            td.path().to_str().expect("main path"),
            true,
        )
        .expect_err("main worktree must be protected from every linked worktree");
        assert!(main_error.contains("main worktree"), "{main_error}");

        fs::write(target.join("dirty.txt"), "uncommitted\n").expect("dirty worktree");
        let dirty_error = remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            false,
        )
        .expect_err("dirty worktree needs force");
        assert!(
            dirty_error.contains("modified or untracked"),
            "{dirty_error}"
        );
        remove_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            true,
        )
        .expect("force remove dirty worktree");
    }

    #[test]
    fn worktree_list_reports_lock_reason() {
        let td = init_repo();
        commit_base(td.path());
        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("locked-worktree");
        create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "locked",
            true,
            None,
        )
        .expect("create worktree");
        git(
            td.path(),
            &[
                "worktree",
                "lock",
                "--reason",
                "agent session active",
                target.to_str().expect("target path"),
            ],
        );

        let listed = read_worktrees(td.path().to_str().expect("repo path")).expect("list");
        let locked = listed.iter().find(|item| !item.is_main).expect("linked");
        assert!(locked.locked);
        assert_eq!(locked.lock_reason.as_deref(), Some("agent session active"));
    }

    #[test]
    fn worktree_create_rejects_ambiguous_paths_and_option_like_refs() {
        let td = init_repo();
        commit_base(td.path());

        let relative = create_worktree(
            td.path().to_str().expect("repo path"),
            "relative/worktree",
            "feature",
            true,
            None,
        )
        .expect_err("relative targets must be rejected");
        assert!(relative.contains("must be absolute"), "{relative}");

        let parent = tempdir().expect("worktree parent");
        let target = parent.path().join("malicious-worktree");
        let branch = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "--force",
            true,
            None,
        )
        .expect_err("option-like branches must be rejected");
        assert!(branch.contains("cannot start with '-'"), "{branch}");

        let start = create_worktree(
            td.path().to_str().expect("repo path"),
            target.to_str().expect("target path"),
            "safe-feature",
            true,
            Some("--help".into()),
        )
        .expect_err("option-like revisions must be rejected");
        assert!(start.contains("cannot start with '-'"), "{start}");
        assert!(!target.exists());
    }

    #[test]
    fn worktree_create_rejects_protected_and_nested_targets_before_creating_dirs() {
        let td = init_repo();
        commit_base(td.path());

        let nested = td.path().join("nested/worktree");
        let nested_error = create_worktree(
            td.path().to_str().expect("repo"),
            nested.to_str().expect("nested target"),
            "nested-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("target inside a registered worktree must fail");
        assert!(
            nested_error.contains("inside registered worktree"),
            "{nested_error}"
        );
        assert!(!td.path().join("nested").exists());

        let common = td.path().join(".git/sikemux-test/worktree");
        let common_error = create_worktree(
            td.path().to_str().expect("repo"),
            common.to_str().expect("common-dir target"),
            "common-dir-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("target inside common Git dir must fail");
        assert!(common_error.contains("common directory"), "{common_error}");
        assert!(!td.path().join(".git/sikemux-test").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_create_rejects_symlink_parent_escape_before_creating_dirs() {
        use std::os::unix::fs::symlink;

        let td = init_repo();
        commit_base(td.path());
        let outside = tempdir().expect("outside");
        let alias = outside.path().join("apparently-external");
        symlink(td.path(), &alias).expect("symlink into worktree");
        let target = alias.join("escaped/worktree");

        let error = create_worktree(
            td.path().to_str().expect("repo"),
            target.to_str().expect("symlink target"),
            "symlink-target",
            true,
            Some("HEAD".into()),
        )
        .expect_err("symlink parent into registered worktree must fail");
        assert!(error.contains("inside registered worktree"), "{error}");
        assert!(!td.path().join("escaped").exists());
    }

    #[test]
    fn staged_ai_diff_excludes_unchanged_context() {
        let td = init_repo();
        fs::write(
            td.path().join("f.txt"),
            "keep-one\nkeep-two\nold-value\nkeep-three\nkeep-four\n",
        )
        .expect("write base");
        git(td.path(), &["add", "f.txt"]);
        git(td.path(), &["commit", "-m", "base"]);
        fs::write(
            td.path().join("f.txt"),
            "keep-one\nkeep-two\nnew-value\nkeep-three\nkeep-four\n",
        )
        .expect("write change");
        git(td.path(), &["add", "f.txt"]);

        let (_, diff) = staged_diff(td.path().to_str().expect("repo path")).expect("staged diff");

        assert!(diff.contains("-old-value"));
        assert!(diff.contains("+new-value"));
        assert!(!diff.contains(" keep-one"));
        assert!(!diff.contains(" keep-four"));
    }

    #[tokio::test]
    async fn discard_unstaged_preserves_staged_changes() {
        let td = init_repo();
        commit_base(td.path());

        fs::write(td.path().join("f.txt"), "staged\n").expect("write staged");
        git(td.path(), &["add", "f.txt"]);
        fs::write(td.path().join("f.txt"), "unstaged\n").expect("write unstaged");

        git_discard_file(repo_arg(td.path()), "f.txt".into(), "unstaged".into())
            .await
            .expect("discard unstaged");

        assert_eq!(
            fs::read_to_string(td.path().join("f.txt")).expect("read worktree"),
            "staged\n"
        );
        assert_eq!(git(td.path(), &["show", ":f.txt"]), "staged\n");
    }

    #[tokio::test]
    async fn blame_maps_committed_and_uncommitted_lines() {
        let td = init_repo();
        fs::write(td.path().join("f.txt"), "one\ntwo\nthree\n").expect("write");
        git(td.path(), &["add", "f.txt"]);
        git(td.path(), &["commit", "-m", "seed"]);

        // Disk blame: every line attributed to the single seed commit.
        let on_disk = git_blame(repo_arg(td.path()), "f.txt".into(), None)
            .await
            .expect("blame disk");
        assert_eq!(on_disk.lines.len(), 3);
        assert!(on_disk.lines.iter().all(|&i| i == on_disk.lines[0]));
        let c = &on_disk.commits[on_disk.lines[0] as usize];
        assert_eq!(c.author, "sikemux");
        assert!(!c.uncommitted);
        assert_eq!(c.summary, "seed");
        assert_eq!(c.short.len(), 8);

        // Buffer blame: an appended line shows as not-yet-committed.
        let buffer = "one\ntwo\nthree\nfour\n".to_string();
        let blame = git_blame(repo_arg(td.path()), "f.txt".into(), Some(buffer))
            .await
            .expect("blame buffer");
        assert_eq!(blame.lines.len(), 4);
        let last = &blame.commits[blame.lines[3] as usize];
        assert!(last.uncommitted, "appended line should be uncommitted");
        assert_eq!(last.summary, "Uncommitted changes");
        assert!(!blame.commits[blame.lines[0] as usize].uncommitted);
    }

    /// `vendor` is a real repository name, so the scan has to return it even
    /// though the file-palette walker treats that name as noise.
    #[tokio::test]
    async fn discovers_child_repositories_and_skips_only_node_modules() {
        let td = tempdir().expect("tempdir");
        let root = td.path();
        for name in ["beta", "alpha", "vendor", "node_modules"] {
            let child = root.join(name);
            fs::create_dir_all(&child).expect("child dir");
            git(&child, &["init"]);
            git(&child, &["config", "user.email", "sikemux@example.test"]);
            git(&child, &["config", "user.name", "sikemux"]);
            commit_base(&child);
        }
        fs::create_dir_all(root.join("plain/nested")).expect("plain dir");
        fs::write(root.join("alpha/dirty.txt"), "x\n").expect("write dirty");

        let found = git_discover_repos(repo_arg(root)).await.expect("discover");

        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "vendor"]);
        assert_eq!(found[0].changes, 1, "alpha has one untracked file");
        assert_eq!(found[1].changes, 0, "beta is clean");
    }

    /// A directory inside a repository is not itself one, so it must not be
    /// listed just because `discover` would have walked up and found the parent.
    #[tokio::test]
    async fn plain_directories_never_report_the_repository_above_them() {
        let td = init_repo();
        commit_base(td.path());
        fs::create_dir_all(td.path().join("src")).expect("src dir");

        let found = git_discover_repos(repo_arg(td.path()))
            .await
            .expect("discover");

        assert!(found.is_empty(), "{found:?} should be empty");
    }

    #[tokio::test]
    async fn blame_untracked_file_is_empty() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("new.txt"), "hi\n").expect("write");
        let blame = git_blame(repo_arg(td.path()), "new.txt".into(), None)
            .await
            .expect("blame untracked");
        assert!(blame.commits.is_empty());
        assert!(blame.lines.is_empty());
    }

    #[tokio::test]
    async fn batched_staging_and_unstaging_covers_every_path_and_deletions() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("a.txt"), "a\n").expect("write a");
        fs::write(td.path().join("b.txt"), "b\n").expect("write b");
        fs::remove_file(td.path().join("f.txt")).expect("remove base");

        git_stage_paths(
            repo_arg(td.path()),
            vec!["a.txt".into(), "b.txt".into(), "f.txt".into()],
        )
        .await
        .expect("stage batch");

        let staged = git(td.path(), &["diff", "--cached", "--name-status"]);
        assert!(staged.contains("A\ta.txt"), "{staged}");
        assert!(staged.contains("A\tb.txt"), "{staged}");
        assert!(staged.contains("D\tf.txt"), "{staged}");

        git_unstage_paths(repo_arg(td.path()), vec!["a.txt".into(), "f.txt".into()])
            .await
            .expect("unstage batch");

        let staged = git(td.path(), &["diff", "--cached", "--name-status"]);
        assert_eq!(staged.trim(), "A\tb.txt");
    }

    #[tokio::test]
    async fn batched_discard_handles_tracked_and_untracked_together() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("f.txt"), "edited\n").expect("edit tracked");
        fs::write(td.path().join("new.txt"), "new\n").expect("write untracked");
        git(td.path(), &["add", "new.txt"]);

        git_discard_files(
            repo_arg(td.path()),
            vec!["f.txt".into(), "new.txt".into()],
            "all".into(),
        )
        .await
        .expect("discard batch");

        assert_eq!(
            fs::read_to_string(td.path().join("f.txt")).expect("read tracked"),
            "base\n"
        );
        assert!(!td.path().join("new.txt").exists());
        assert_eq!(git(td.path(), &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn discard_all_removes_staged_new_file() {
        let td = init_repo();
        commit_base(td.path());

        fs::write(td.path().join("new.txt"), "new\n").expect("write new");
        git(td.path(), &["add", "new.txt"]);

        git_discard_file(repo_arg(td.path()), "new.txt".into(), "all".into())
            .await
            .expect("discard all");

        assert!(!td.path().join("new.txt").exists());
        assert_eq!(git(td.path(), &["status", "--porcelain"]), "");
    }
}
