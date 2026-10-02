use std::io;
use std::path::Path;
use std::sync::Mutex;

use git2::{ErrorCode, Repository};
use serde::Serialize;

use super::log::{children_before_parents, describe_commits, GitCommit};
use super::{default_remote, git_ok, git_walk_permit, open_repo, run_blocking};

fn revparse_commit<'a>(repo: &'a Repository, rev: &str) -> Result<git2::Commit<'a>, String> {
    repo.revparse_single(rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| e.message().to_string())
}

#[tauri::command]
pub async fn git_show(repo: String, rev: String) -> Result<String, String> {
    // git2's diff doesn't render the message + stat block the way `git show`
    // does — shelling out here costs us nothing and keeps the UI identical.
    run_blocking(move || {
        git_ok(
            &repo,
            &[
                "show",
                "--no-ext-diff",
                "--no-textconv",
                "--stat",
                "-p",
                "--end-of-options",
                &rev,
            ],
        )
    })
    .await
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
    bytes_to_inline_text(blob.content(), path)
}

fn too_large_for_inline_diff(path: &str, bytes: usize) -> String {
    format!(
        "{path} is too large for inline diff ({}). Open the file directly or use git diff in the terminal.",
        human_bytes(bytes)
    )
}

fn bytes_to_inline_text(bytes: &[u8], path: &str) -> Result<String, String> {
    if bytes.len() > GIT_FILE_AT_MAX_BYTES {
        return Err(too_large_for_inline_diff(path, bytes.len()));
    }
    if looks_binary_bytes(bytes) {
        return Err(format!("{path} is binary; inline diff is disabled."));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| format!("{path} is not UTF-8 text; inline diff is disabled."))
}

fn file_text_at(repo: &str, rev: &str, path: &str) -> Result<String, String> {
    let cacheable = is_immutable_rev(rev);
    let key = (repo.to_string(), rev.to_string(), path.to_string());
    if cacheable {
        if let Ok(mut cache) = file_at_cache().lock() {
            if let Some(hit) = cache.get(&key) {
                return Ok(hit);
            }
        }
    }
    let r = open_repo(repo)?;
    let content = if rev == ":index" {
        let idx = r.index().map_err(|e| e.message().to_string())?;
        match idx.get_path(Path::new(path), 0) {
            Some(entry) => {
                let blob = r.find_blob(entry.id).map_err(|e| e.message().to_string())?;
                blob_to_inline_text(&blob, path)?
            }
            None => String::new(),
        }
    } else {
        match revparse_commit(&r, rev) {
            Ok(commit) => {
                let tree = commit.tree().map_err(|e| e.message().to_string())?;
                match tree.get_path(Path::new(path)) {
                    Ok(entry) => {
                        let blob = r
                            .find_blob(entry.id())
                            .map_err(|e| e.message().to_string())?;
                        blob_to_inline_text(&blob, path)?
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
            cache.insert(key, content.clone());
        }
    }
    Ok(content)
}

fn worktree_text(repo: &str, path: &str) -> Result<String, String> {
    let full = Path::new(repo).join(path);
    match std::fs::metadata(&full) {
        Ok(meta) if meta.len() as usize > GIT_FILE_AT_MAX_BYTES => {
            return Err(too_large_for_inline_diff(path, meta.len() as usize));
        }
        Ok(meta) if !meta.is_file() => return Ok(String::new()),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.to_string()),
    }
    match std::fs::read(&full) {
        Ok(bytes) => bytes_to_inline_text(&bytes, path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn git_file_at(repo: String, rev: String, path: String) -> Result<String, String> {
    run_blocking(move || file_text_at(&repo, &rev, &path)).await
}

/// The rows of a unified diff of one file between two revisions, or between a
/// revision and the working tree when `head_rev` is absent.
#[tauri::command]
pub async fn git_file_diff(
    repo: String,
    path: String,
    base_rev: String,
    head_rev: Option<String>,
    full: bool,
) -> Result<Vec<crate::diff::DiffRow>, String> {
    run_blocking(move || -> Result<_, String> {
        let base = file_text_at(&repo, &base_rev, &path)?;
        let head = match head_rev {
            Some(rev) => file_text_at(&repo, &rev, &path)?,
            None => worktree_text(&repo, &path)?,
        };
        Ok(crate::diff::unified_rows(&base, &head, full))
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

#[derive(Serialize, Clone)]
pub struct GitCompare {
    /// Where the branch left its base; diffs are drawn from here.
    merge_base: String,
    files: Vec<GitCompareFile>,
    commits: Vec<GitCommit>,
}

#[derive(Serialize, Clone)]
pub struct GitCompareFile {
    path: String,
    /// `A`, `M`, `D` or `R`, as the status column shows it.
    status: &'static str,
}

/// Prefers the remote's copy of `branch`, since a local base is often behind.
fn remote_or_local(repo: &Repository, remote: Option<&str>, branch: &str) -> String {
    match remote {
        Some(remote)
            if repo
                .revparse_single(&format!("refs/remotes/{remote}/{branch}"))
                .is_ok() =>
        {
            format!("refs/remotes/{remote}/{branch}")
        }
        _ => branch.to_string(),
    }
}

fn read_compare(repo_path: &str, base: &str, head: &str) -> Result<GitCompare, String> {
    let r = open_repo(repo_path)?;
    let remote = default_remote(repo_path).ok();
    let base_commit = revparse_commit(&r, &remote_or_local(&r, remote.as_deref(), base))?;
    let head_commit = revparse_commit(&r, head)
        .or_else(|_| revparse_commit(&r, &remote_or_local(&r, remote.as_deref(), head)))?;
    let merge_base = r
        .merge_base(base_commit.id(), head_commit.id())
        .map_err(|e| e.message().to_string())?;

    let old_tree = r
        .find_commit(merge_base)
        .and_then(|c| c.tree())
        .map_err(|e| e.message().to_string())?;
    let new_tree = head_commit.tree().map_err(|e| e.message().to_string())?;
    let diff = r
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None)
        .map_err(|e| e.message().to_string())?;
    let files = diff
        .deltas()
        .filter_map(|d| {
            let path = d.new_file().path().or_else(|| d.old_file().path())?;
            let status = match d.status() {
                git2::Delta::Added => "A",
                git2::Delta::Deleted => "D",
                git2::Delta::Renamed => "R",
                _ => "M",
            };
            Some(GitCompareFile {
                path: path.to_string_lossy().into_owned(),
                status,
            })
        })
        .collect();

    let mut revwalk = r.revwalk().map_err(|e| e.message().to_string())?;
    revwalk
        .push(head_commit.id())
        .map_err(|e| e.message().to_string())?;
    revwalk
        .hide(merge_base)
        .map_err(|e| e.message().to_string())?;
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.message().to_string())?;
    let commits = revwalk
        .flatten()
        .take(250)
        .filter_map(|oid| r.find_commit(oid).ok())
        .collect();

    Ok(GitCompare {
        merge_base: merge_base.to_string(),
        files,
        commits: describe_commits(&r, children_before_parents(commits)),
    })
}

/// What `head` would bring into `base`: its files and its commits since the two parted.
#[tauri::command]
pub async fn git_compare(repo: String, base: String, head: String) -> Result<GitCompare, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || read_compare(&repo, &base, &head)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{git, init_repo, repo_arg};
    use std::fs;

    #[test]
    fn a_branch_compares_against_where_it_left_its_base() {
        let td = init_repo();
        let repo = td.path();
        fs::write(repo.join("kept.txt"), "one\n").unwrap();
        fs::write(repo.join("gone.txt"), "bye\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "start"]);
        git(repo, &["branch", "-M", "main"]);
        git(repo, &["checkout", "-b", "feat"]);
        fs::write(repo.join("kept.txt"), "two\n").unwrap();
        fs::write(repo.join("new.txt"), "hi\n").unwrap();
        fs::remove_file(repo.join("gone.txt")).unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-m", "feat work"]);
        git(repo, &["checkout", "main"]);
        fs::write(repo.join("later.txt"), "main moved on\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "main work"]);

        let compare = read_compare(&repo_arg(repo), "main", "feat").expect("compare");
        let files: Vec<(&str, &str)> = compare
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status))
            .collect();
        assert_eq!(
            files,
            vec![("gone.txt", "D"), ("kept.txt", "M"), ("new.txt", "A")]
        );
        let subjects: Vec<&str> = compare.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["feat work"]);
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
}
