use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use git2::{BranchType, Repository, Status, StatusOptions};
use serde::Serialize;

use super::{flight_key, git_walk_permit, open_repo, run_blocking, shared, Flights};

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

impl GitStatus {
    fn clone_head(&self) -> Self {
        Self {
            branch: self.branch.clone(),
            upstream: self.upstream.clone(),
            ahead: self.ahead,
            behind: self.behind,
            files: Vec::new(),
        }
    }
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

pub(super) fn read_status(repo: &Repository) -> Result<GitStatus, String> {
    let mut status = read_head(repo);
    status.files = read_files(repo, &[])?;
    Ok(status)
}

/// The checked out branch and how far it is from its upstream, with no files.
fn read_head(repo: &Repository) -> GitStatus {
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
    status
}

/// The changed files, or only those at or below `paths` when there are any.
fn read_files(repo: &Repository, paths: &[String]) -> Result<Vec<GitFile>, String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true);
    if !paths.is_empty() {
        opts.disable_pathspec_match(true);
        for path in paths {
            opts.pathspec(path);
        }
    }
    let mut files = Vec::new();
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
        files.push(GitFile {
            path,
            index: x.to_string(),
            worktree: y.to_string(),
        });
    }
    Ok(files)
}

/// A walk of a repo stays good until its watcher says the tree moved. Saving a
/// file used to cost four of them: the editor invalidates eagerly, the file
/// tree asks for status on its own, and the watcher fires again 200 ms later.
///
/// A repo with no watcher running is never cached — nothing would tell us when
/// the answer stopped being true.
const REPO_WALK_CACHE_REPOS: usize = 32;

/// How long a status may be carried forward by reading again only the paths
/// the watcher saw move. The watcher skips build and dependency folders and
/// cannot see ignore rules kept outside the tree, so it is walked whole again
/// after this.
const PATCHED_STATUS_LIFETIME: Duration = Duration::from_secs(30);

/// More moved paths than this read faster in one whole walk.
const MAX_PATCHED_PATHS: usize = 1024;

pub(super) type WalkCache<T> = Mutex<HashMap<String, (u64, T)>>;

#[derive(Clone)]
pub(super) struct StatusWalk {
    /// When the files were last read in a whole walk rather than patched.
    walked_at: Instant,
    status: GitStatus,
}

fn status_cache() -> &'static WalkCache<StatusWalk> {
    static C: OnceLock<WalkCache<StatusWalk>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub(super) fn watched_generation(repo: &str) -> Option<(String, u64)> {
    let key = crate::files::canonical_repo_key(repo).ok()?;
    let generation = crate::fs_watch::scan_generation(&key)?;
    Some((key, generation))
}

pub(super) fn cached_walk<T: Clone>(cache: &WalkCache<T>, key: &str, generation: u64) -> Option<T> {
    stored_walk(cache, key)
        .filter(|(stored, _)| *stored == generation)
        .map(|(_, value)| value)
}

/// The latest walk of `key` and the generation it was read at, however old.
pub(super) fn stored_walk<T: Clone>(cache: &WalkCache<T>, key: &str) -> Option<(u64, T)> {
    cache.lock().ok()?.get(key).cloned()
}

pub(super) fn store_walk<T>(cache: &WalkCache<T>, key: String, generation: u64, value: T) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.len() >= REPO_WALK_CACHE_REPOS && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, (generation, value));
}

pub(super) fn cached_status(key: &str, generation: u64) -> Option<GitStatus> {
    cached_walk(status_cache(), key, generation).map(|walk| walk.status)
}

/// The status of the watched repo `key` at `generation`, reading again only
/// the paths that moved since the last read when that gives the same answer.
pub(super) fn watched_status(
    repo: &Repository,
    key: &str,
    generation: u64,
) -> Result<GitStatus, String> {
    if let Some((read_at, walk)) = stored_walk(status_cache(), key) {
        if read_at == generation {
            return Ok(walk.status);
        }
        if let Some(status) = patched_status(repo, key, read_at, &walk) {
            let walk = StatusWalk {
                walked_at: walk.walked_at,
                status: status.clone(),
            };
            store_walk(status_cache(), key.to_owned(), generation, walk);
            return Ok(status);
        }
    }
    let status = read_status(repo)?;
    let walk = StatusWalk {
        walked_at: Instant::now(),
        status: status.clone(),
    };
    store_walk(status_cache(), key.to_owned(), generation, walk);
    Ok(status)
}

/// Whether a moved path can only have changed its own status and those of the
/// paths below it.
pub(super) fn moves_only_itself(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.is_ascii()
        && path.split('/').next() != Some(".git")
        && !matches!(name, ".gitignore" | ".gitattributes" | ".gitmodules")
}

/// `walk`, read at `since`, with the paths moved after it read again; none
/// when only a whole walk is sure to give the same answer.
fn patched_status(
    repo: &Repository,
    key: &str,
    since: u64,
    walk: &StatusWalk,
) -> Option<GitStatus> {
    if walk.walked_at.elapsed() > PATCHED_STATUS_LIFETIME {
        return None;
    }
    // A staged rename is listed under its old path, so an edit to its new
    // path changes an entry the moved path does not name.
    if walk.status.files.iter().any(|file| file.index == "R") {
        return None;
    }
    let moved = moved_since(repo, key, since)?;
    let mut status = walk.status.clone_head();
    if moved.is_empty() {
        status.files = walk.status.files.clone();
        return Some(status);
    }
    let ignore_case = repo
        .config()
        .and_then(|config| config.get_bool("core.ignorecase"))
        .unwrap_or(false);
    let fresh = read_files(repo, &moved).ok()?;
    status.files = merge_files(&walk.status.files, fresh, &moved, ignore_case);
    Some(status)
}

/// The paths moved in the watched checkout `key` after change `since`, when
/// asking git again of only them gives what asking of everything would.
pub(super) fn moved_since(repo: &Repository, key: &str, since: u64) -> Option<Vec<String>> {
    let moved = crate::fs_watch::changed_paths_since(key, since)?;
    let patchable = moved.len() <= MAX_PATCHED_PATHS
        && moved.iter().all(|path| moves_only_itself(path))
        && is_plain_checkout(repo, key);
    patchable.then_some(moved)
}

/// A checkout whose own `.git` folder sits at the watched root, with no
/// submodules: the watcher sees everything its status can depend on.
fn is_plain_checkout(repo: &Repository, key: &str) -> bool {
    let root = Path::new(key);
    let same = |path: Option<&Path>| {
        path.and_then(|path| path.canonicalize().ok())
            .is_some_and(|path| path == root)
    };
    same(repo.workdir())
        && same(repo.path().parent())
        && repo.path().file_name() == Some(std::ffi::OsStr::new(".git"))
        && !root.join(".gitmodules").exists()
}

/// `cached` without the entries at or below `moved`, with `fresh` slotted in
/// where git's own walk orders them.
fn merge_files(
    cached: &[GitFile],
    fresh: Vec<GitFile>,
    moved: &[String],
    ignore_case: bool,
) -> Vec<GitFile> {
    let order = |a: &str, b: &str| {
        if ignore_case {
            let a = a.bytes().map(|byte| byte.to_ascii_lowercase());
            a.cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
        } else {
            a.cmp(b)
        }
    };
    let at_or_below = |path: &str, moved: &str| {
        let path = path.strip_suffix('/').unwrap_or(path);
        let Some(head) = path.get(..moved.len()) else {
            return false;
        };
        let same = if ignore_case {
            head.eq_ignore_ascii_case(moved)
        } else {
            head == moved
        };
        same && matches!(path.as_bytes().get(moved.len()), None | Some(b'/'))
    };
    let kept = cached
        .iter()
        .filter(|file| !moved.iter().any(|moved| at_or_below(&file.path, moved)));
    let mut merged = Vec::with_capacity(cached.len() + fresh.len());
    let mut fresh = fresh.into_iter().peekable();
    for file in kept {
        while let Some(next) =
            fresh.next_if(|next| order(&next.path, &file.path) == std::cmp::Ordering::Less)
        {
            merged.push(next);
        }
        merged.push(file.clone());
    }
    merged.extend(fresh);
    merged
}

// Not `files::should_skip_dir`: it hides `vendor`, `build` and `out`, which are ordinary repository names.
fn skip_repo_scan_dir(name: &str) -> bool {
    matches!(name, "node_modules" | ".git")
}

/// The repositories directly inside `root`, one level down like VS Code's default scan.
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
            // `open`, not `discover`, so a plain folder never reports the repository it sits in.
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
        found.sort_by_cached_key(|repo| repo.name.to_lowercase());
        Ok(found)
    })
    .await
}

#[tauri::command]
pub async fn git_status(repo: String) -> Result<GitStatus, String> {
    let watched = watched_generation(&repo);
    if let Some((key, generation)) = &watched {
        if let Some(hit) = cached_status(key, *generation) {
            return Ok(hit);
        }
    }
    let flight = flight_key(&repo, watched.as_ref());
    shared(status_flights(), flight, async move {
        let _permit = git_walk_permit().await?;
        run_blocking(move || -> Result<GitStatus, String> {
            let r = open_repo(&repo)?;
            match watched {
                Some((key, generation)) => watched_status(&r, &key, generation),
                None => read_status(&r),
            }
        })
        .await
    })
    .await
}

fn status_flights() -> &'static Flights<GitStatus> {
    static F: OnceLock<Flights<GitStatus>> = OnceLock::new();
    F.get_or_init(Default::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{commit_base, git, init_repo, repo_arg};
    use std::fs;
    use tempfile::tempdir;

    /// A walk is reused only while the watcher's count stands still; the next
    /// change moves it on and the stale answer is skipped.
    #[test]
    fn a_cached_walk_is_only_reused_for_the_generation_it_was_read_at() {
        let cache: WalkCache<String> = Mutex::new(HashMap::new());
        store_walk(&cache, "/repo".into(), 7, "walked".to_string());

        assert_eq!(cached_walk(&cache, "/repo", 7).as_deref(), Some("walked"));
        assert!(cached_walk(&cache, "/repo", 8).is_none());
        assert!(cached_walk(&cache, "/other", 7).is_none());
    }

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
}
