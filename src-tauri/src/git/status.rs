use std::sync::Mutex;

use git2::{BranchType, Repository, Status, StatusOptions};
use serde::Serialize;

use super::log::GitOverview;
use super::{git_walk_permit, open_repo, run_blocking};

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

pub(super) fn status_cache() -> &'static WalkCache<GitStatus> {
    static C: std::sync::OnceLock<WalkCache<GitStatus>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub(super) fn overview_cache() -> &'static WalkCache<GitOverview> {
    static C: std::sync::OnceLock<WalkCache<GitOverview>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub(super) fn watched_generation(repo: &str) -> Option<(String, u64)> {
    let key = crate::files::canonical_repo_key(repo).ok()?;
    let generation = crate::fs_watch::scan_generation(&key)?;
    Some((key, generation))
}

pub(super) fn cached_walk<T: Clone>(cache: &WalkCache<T>, key: &str, generation: u64) -> Option<T> {
    let cache = cache.lock().ok()?;
    cache
        .get(key)
        .filter(|(stored, _)| *stored == generation)
        .map(|(_, value)| value.clone())
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
        let cache: WalkCache<String> = Mutex::new(std::collections::HashMap::new());
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
