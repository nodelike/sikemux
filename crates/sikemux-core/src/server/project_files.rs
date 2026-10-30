//! Every file in a project, for Cmd-P, kept current from the watcher. A client
//! that names the scan it holds gets only what was added and removed since.
//!
//! .gitignore is not honoured: .env, local.properties and *.local.json are
//! exactly what the person jumps to, so a fixed list of build, dependency and
//! cache folders is left out instead.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use crate::git::watch::{repo_key, should_skip_dir, GIT_STATE};

/// A full rescan backstops platform events the watcher missed.
const TTL: Duration = Duration::from_secs(60);
const MAX_INCREMENTAL_PATHS: usize = 512;
const MAX_INCREMENTAL_FILES: usize = 4_096;
const MAX_PROJECT_FILES: usize = 250_000;
/// Bounds the kept list and every answer by the JSON its paths take.
const MAX_PROJECT_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROJECT_ENCODED_PATH_BYTES: usize = MAX_PROJECT_SNAPSHOT_BYTES - 1024;
const MAX_PROJECT_CACHE_ENTRIES: usize = 128;
/// How many changes back a client may still be answered with only the change.
const KEPT_CHANGES: usize = 16;
/// A change bigger than this is not kept; its readers get the whole list.
const MAX_KEPT_CHANGE_PATHS: usize = 20_000;

static NEXT_SCAN_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CACHE_ACCESS: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct Entry {
    files: Arc<Vec<String>>,
    full_scan_at: Instant,
    scan_id: u64,
}

/// What the files became after scan `from`, up to the next kept change or
/// the current list.
struct Change {
    from: u64,
    added: Vec<String>,
    removed: Vec<String>,
}

/// Paths the watcher saw move since the last read; `None` means anything may
/// have.
#[derive(Default)]
enum Pending {
    #[default]
    Nothing,
    Paths(Vec<String>),
    Anything,
}

#[derive(Default)]
struct Slot {
    entry: Option<Entry>,
    pending: Pending,
    changes: VecDeque<Change>,
}

struct ProjectCache {
    slot: Mutex<Slot>,
    last_used: AtomicU64,
}

impl ProjectCache {
    fn new() -> Self {
        Self {
            slot: Mutex::new(Slot::default()),
            last_used: AtomicU64::new(next_cache_access()),
        }
    }

    fn touch(&self) {
        self.last_used.store(next_cache_access(), Ordering::Release);
    }
}

type ProjectCaches = Mutex<HashMap<String, Arc<ProjectCache>>>;

fn cache() -> &'static ProjectCaches {
    static CACHE: OnceLock<ProjectCaches> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn next_cache_access() -> u64 {
    NEXT_CACHE_ACCESS.fetch_add(1, Ordering::Relaxed)
}

/// Lookups clone a cell's `Arc` only while holding the map's lock, so under it
/// a count of one means nobody is using the cell.
fn evict_one_idle_project_cache_from(caches: &mut HashMap<String, Arc<ProjectCache>>) -> bool {
    let idle = caches
        .iter()
        .filter(|(_, project)| Arc::strong_count(project) == 1)
        .min_by_key(|(_, project)| project.last_used.load(Ordering::Acquire))
        .map(|(key, _)| key.clone());
    idle.is_some_and(|key| caches.remove(&key).is_some())
}

fn project_cache_from(
    caches: &ProjectCaches,
    repo: &str,
    capacity: usize,
) -> Result<Arc<ProjectCache>, String> {
    let mut caches = caches.locked();
    if let Some(project) = caches.get(repo) {
        let project = project.clone();
        project.touch();
        return Ok(project);
    }
    while caches.len() >= capacity {
        if !evict_one_idle_project_cache_from(&mut caches) {
            return Err(format!(
                "project file cache is busy at its {capacity}-project safety limit"
            ));
        }
    }
    let project = Arc::new(ProjectCache::new());
    caches.insert(repo.to_owned(), project.clone());
    Ok(project)
}

fn project_cache(repo: &str) -> Result<Arc<ProjectCache>, String> {
    project_cache_from(cache(), repo, MAX_PROJECT_CACHE_ENTRIES)
}

/// The answer to `projectFiles`: the whole list, only what changed since the
/// client's scan, or neither when nothing did.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectFiles {
    scan_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectFilesParams {
    project: String,
    #[serde(default)]
    since: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntriesParams {
    project: String,
    #[serde(default)]
    query: String,
    #[serde(default = "entries_shown")]
    limit: usize,
}

fn entries_shown() -> usize {
    8
}

/// The most entries `projectEntries` answers with.
const MAX_ENTRIES: usize = 50;

enum LimitedWalk {
    Complete(Vec<String>),
    LimitExceeded,
    Unreliable,
}

#[derive(Clone, Copy)]
struct WalkLimits {
    max_files: usize,
    max_encoded_path_bytes: usize,
    strict_errors: bool,
}

fn encoded_path_upper_bound(path: &str) -> usize {
    // serde_json can expand ASCII control characters to `\u00XX`; all other
    // UTF-8 is emitted verbatim except quotes and backslashes. Include the two
    // string quotes and one array separator byte.
    path.chars().fold(3usize, |total, character| {
        total.saturating_add(match character {
            '\"' | '\\' => 2,
            '\u{0}'..='\u{1f}' => 6,
            _ => character.len_utf8(),
        })
    })
}

fn snapshot_encoded_path_bytes_with_limit(files: &[String], limit: usize) -> Option<usize> {
    let mut total = 0usize;
    for file in files {
        total = total.checked_add(encoded_path_upper_bound(file))?;
        if total > limit {
            return None;
        }
    }
    Some(total)
}

fn snapshot_encoded_path_bytes(files: &[String]) -> Option<usize> {
    snapshot_encoded_path_bytes_with_limit(files, MAX_PROJECT_ENCODED_PATH_BYTES)
}

fn walk(repo_root: &Path, scan_root: &Path, limits: WalkLimits) -> LimitedWalk {
    let mut files = Vec::new();
    let mut encoded_path_bytes = 0usize;
    let walker = WalkBuilder::new(scan_root)
        .hidden(false) // show dotfiles — .env, .vscode/, etc. are findable
        .git_ignore(false) // gitignored does not mean uninteresting
        .git_exclude(false)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| {
            // Always allow the explicit scan root. For descendants, returning
            // false for a denied directory prunes its complete subtree.
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_name() == ".DS_Store" {
                return false;
            }
            let is_dir = entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false);
            !is_dir || !should_skip_dir(&entry.file_name().to_string_lossy())
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) if limits.strict_errors => return LimitedWalk::Unreliable,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            if limits.strict_errors {
                return LimitedWalk::Unreliable;
            }
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(repo_root) else {
            if limits.strict_errors {
                return LimitedWalk::Unreliable;
            }
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let Some(relative) = wire_relative_path(relative) else {
            if limits.strict_errors {
                return LimitedWalk::Unreliable;
            }
            continue;
        };
        encoded_path_bytes =
            match encoded_path_bytes.checked_add(encoded_path_upper_bound(&relative)) {
                Some(bytes) if bytes <= limits.max_encoded_path_bytes => bytes,
                _ => return LimitedWalk::LimitExceeded,
            };
        files.push(relative);
        if files.len() > limits.max_files {
            return LimitedWalk::LimitExceeded;
        }
    }

    files.sort_unstable();
    files.dedup();
    LimitedWalk::Complete(files)
}

fn next_scan_id() -> u64 {
    NEXT_SCAN_ID.fetch_add(1, Ordering::Relaxed)
}

fn full_scan_with_limits(repo: &str, limits: WalkLimits) -> Result<Vec<String>, String> {
    match walk(Path::new(repo), Path::new(repo), limits) {
        LimitedWalk::Complete(files) => Ok(files),
        LimitedWalk::LimitExceeded => Err(format!(
            "project file snapshot exceeds its {}-file or {}-byte safety limit",
            limits.max_files, limits.max_encoded_path_bytes
        )),
        LimitedWalk::Unreliable => Err("project file snapshot could not be read reliably".into()),
    }
}

fn full_scan(repo: &str) -> Result<Vec<String>, String> {
    full_scan_with_limits(
        repo,
        WalkLimits {
            max_files: MAX_PROJECT_FILES,
            max_encoded_path_bytes: MAX_PROJECT_ENCODED_PATH_BYTES,
            strict_errors: false,
        },
    )
}

/// What `new` added to `old` and removed from it; both are sorted.
fn difference(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    let (mut at_old, mut at_new) = (0, 0);
    while at_old < old.len() || at_new < new.len() {
        match (old.get(at_old), new.get(at_new)) {
            (Some(was), Some(is)) if was == is => {
                at_old += 1;
                at_new += 1;
            }
            (Some(was), Some(is)) if was < is => {
                removed.push(was.clone());
                at_old += 1;
            }
            (_, Some(is)) => {
                added.push(is.clone());
                at_new += 1;
            }
            (Some(was), None) => {
                removed.push(was.clone());
                at_old += 1;
            }
            (None, None) => break,
        }
    }
    (added, removed)
}

impl Slot {
    /// Takes `files` as the project's list, keeping what changed for clients
    /// that hold the one before. An unchanged list keeps its scan.
    fn replace(&mut self, files: Vec<String>, full_scan_at: Instant) {
        let scan_id = next_scan_id();
        if let Some(previous) = self.entry.take() {
            let (added, removed) = difference(&previous.files, &files);
            if added.is_empty() && removed.is_empty() {
                self.entry = Some(Entry {
                    full_scan_at,
                    ..previous
                });
                return;
            }
            if added.len() + removed.len() <= MAX_KEPT_CHANGE_PATHS {
                if self.changes.len() == KEPT_CHANGES {
                    self.changes.pop_front();
                }
                self.changes.push_back(Change {
                    from: previous.scan_id,
                    added,
                    removed,
                });
            } else {
                self.changes.clear();
            }
        }
        self.entry = Some(Entry {
            files: Arc::new(files),
            full_scan_at,
            scan_id,
        });
    }

    fn rescan(&mut self, repo: &str) -> Result<(), String> {
        let files = full_scan(repo)?;
        self.replace(files, Instant::now());
        Ok(())
    }

    /// Brings the list up to date with what the watcher saw move. Ambiguous,
    /// outside-root, oversized or unreadable changes fall back to a rescan.
    fn catch_up(&mut self, repo: &str) -> Result<(), String> {
        let Some(current) = self.entry.clone() else {
            self.pending = Pending::Nothing;
            return self.rescan(repo);
        };
        match std::mem::take(&mut self.pending) {
            Pending::Nothing if current.full_scan_at.elapsed() < TTL => Ok(()),
            Pending::Nothing | Pending::Anything => self.rescan(repo),
            Pending::Paths(paths) => match updated(repo, &current.files, &paths) {
                Some(Some(files)) => {
                    self.replace(files, current.full_scan_at);
                    Ok(())
                }
                Some(None) => Ok(()),
                None => self.rescan(repo),
            },
        }
    }

    fn answer(&self, since: Option<u64>) -> ProjectFiles {
        let entry = self.entry.as_ref().expect("caught up");
        let whole = || ProjectFiles {
            scan_id: entry.scan_id,
            files: Some((*entry.files).clone()),
            added: None,
            removed: None,
        };
        let Some(since) = since else {
            return whole();
        };
        if since == entry.scan_id {
            return ProjectFiles {
                scan_id: entry.scan_id,
                files: None,
                added: None,
                removed: None,
            };
        }
        let Some(start) = self.changes.iter().position(|change| change.from == since) else {
            return whole();
        };
        let (mut added, mut removed) = (HashSet::new(), HashSet::new());
        for change in self.changes.range(start..) {
            for path in &change.removed {
                if !added.remove(path) {
                    removed.insert(path.clone());
                }
            }
            for path in &change.added {
                if !removed.remove(path) {
                    added.insert(path.clone());
                }
            }
        }
        let sorted = |set: HashSet<String>| {
            let mut paths: Vec<String> = set.into_iter().collect();
            paths.sort_unstable();
            paths
        };
        ProjectFiles {
            scan_id: entry.scan_id,
            files: None,
            added: Some(sorted(added)),
            removed: Some(sorted(removed)),
        }
    }
}

/// The list with `changes` applied, `Some(None)` when none of them touch it,
/// or none when only a rescan can tell.
fn updated(repo: &str, current: &[String], changes: &[String]) -> Option<Option<Vec<String>>> {
    let changes: Vec<&String> = changes
        .iter()
        .filter(|path| path.as_str() != GIT_STATE)
        .collect();
    if changes.len() > MAX_INCREMENTAL_PATHS {
        return None;
    }
    let repo_root = Path::new(repo);
    let mut removals = Vec::new();
    let mut additions = Vec::new();
    let mut remaining_files = MAX_INCREMENTAL_FILES;
    for change in changes {
        let relative = match normalized_relative(repo_root, &repo_root.join(change)) {
            Ok(Some(relative)) => relative,
            Ok(None) => continue,
            Err(()) => return None,
        };
        removals.push(relative.clone());
        let absolute = repo_root.join(&relative);
        let metadata = match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if metadata.file_type().is_file() {
            remaining_files = remaining_files.checked_sub(1)?;
            additions.push(wire_relative_path(&relative)?);
        } else if metadata.file_type().is_dir() && !leaf_is_denied_directory(&relative) {
            let limits = WalkLimits {
                max_files: remaining_files,
                max_encoded_path_bytes: MAX_PROJECT_ENCODED_PATH_BYTES,
                strict_errors: true,
            };
            let LimitedWalk::Complete(mut subtree) = walk(repo_root, &absolute, limits) else {
                return None;
            };
            remaining_files = remaining_files.saturating_sub(subtree.len());
            additions.append(&mut subtree);
        }
    }
    if removals.is_empty() {
        return Some(None);
    }
    let removals = collapse_prefixes(removals);
    let ranges = removal_ranges(current, &removals);
    if ranges.is_empty() && additions.is_empty() {
        return Some(None);
    }
    let files = merge_snapshot(current, &ranges, additions);
    snapshot_encoded_path_bytes(&files)?;
    (files.len() <= MAX_PROJECT_FILES).then_some(Some(files))
}

/// Notes that the watcher saw `paths` move in `project`, or anything when
/// none. Only a project someone listed is kept, and it catches up when it is
/// next read.
pub(crate) fn moved(project: &str, paths: Option<&[String]>) {
    let Ok(repo) = repo_key(project) else {
        return;
    };
    let Some(cell) = cache().locked().get(&repo).cloned() else {
        return;
    };
    let mut slot = cell.slot.locked();
    slot.pending = match (std::mem::take(&mut slot.pending), paths) {
        (Pending::Anything, _) | (_, None) => Pending::Anything,
        (Pending::Nothing, Some(paths)) => Pending::Paths(paths.to_vec()),
        (Pending::Paths(mut known), Some(paths)) => {
            known.extend_from_slice(paths);
            if known.len() > MAX_INCREMENTAL_PATHS {
                Pending::Anything
            } else {
                Pending::Paths(known)
            }
        }
    };
}

/// Every file in the project, or what changed since the scan the client holds.
pub(crate) fn read(params: ProjectFilesParams) -> Result<ProjectFiles, String> {
    let repo = repo_key(&params.project)?;
    let cell = project_cache(&repo)?;
    let mut slot = cell.slot.locked();
    slot.catch_up(&repo)?;
    Ok(slot.answer(params.since))
}

/// The project's files and folders that best match the query, for a client that keeps no
/// list of its own.
pub(crate) fn entries(params: ProjectEntriesParams) -> Result<Vec<crate::fuzzy::Entry>, String> {
    let repo = repo_key(&params.project)?;
    let cell = project_cache(&repo)?;
    let files = {
        let mut slot = cell.slot.locked();
        slot.catch_up(&repo)?;
        slot.entry.as_ref().expect("caught up").files.clone()
    };
    let limit = params.limit.min(MAX_ENTRIES);
    Ok(crate::fuzzy::rank_entries(&params.query, &files, limit))
}

fn normalized_relative(repo_root: &Path, path: &Path) -> Result<Option<PathBuf>, ()> {
    let relative = path.strip_prefix(repo_root).map_err(|_| ())?;
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(component) => normalized.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(()),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(());
    }

    // The final component might be an ordinary file named `target` or
    // `vendor`, which the full walker includes. Only ancestors are known to be
    // directories without consulting the filesystem.
    let mut components = normalized.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        if should_skip_dir(&component.as_os_str().to_string_lossy()) {
            return Ok(None);
        }
    }
    Ok(Some(normalized))
}

/// Project file paths cross the IPC boundary and must not depend on the host
/// separator. A stable slash-delimited representation also keeps full scans
/// and incremental watcher updates comparable on Windows.
fn wire_relative_path(relative: &Path) -> Option<String> {
    let mut wire = String::new();
    for component in relative.components() {
        match component {
            Component::Normal(component) => {
                if !wire.is_empty() {
                    wire.push('/');
                }
                wire.push_str(&component.to_string_lossy());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!wire.is_empty()).then_some(wire)
}

fn leaf_is_denied_directory(relative: &Path) -> bool {
    relative
        .file_name()
        .is_some_and(|name| should_skip_dir(&name.to_string_lossy()))
}

fn collapse_prefixes(mut prefixes: Vec<PathBuf>) -> Vec<PathBuf> {
    prefixes.sort_unstable();
    prefixes.dedup();
    let mut collapsed: Vec<PathBuf> = Vec::with_capacity(prefixes.len());
    for prefix in prefixes {
        if collapsed.iter().any(|parent| prefix.starts_with(parent)) {
            continue;
        }
        collapsed.push(prefix);
    }
    collapsed
}

fn removal_ranges(files: &[String], prefixes: &[PathBuf]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for relative in prefixes {
        let Some(exact) = wire_relative_path(relative) else {
            continue;
        };
        if let Ok(index) = files.binary_search_by(|file| file.as_str().cmp(&exact)) {
            ranges.push(index..index + 1);
        }

        let subtree_prefix = format!("{exact}/");
        let start = files.partition_point(|file| file < &subtree_prefix);
        let mut end = start;
        while end < files.len() && files[end].starts_with(&subtree_prefix) {
            end += 1;
        }
        if start < end {
            ranges.push(start..end);
        }
    }

    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

/// The new snapshot: what survived the removals, merged with what the batch
/// added. Both sides are already in order, so this is one pass. Re-sorting the
/// whole project list costs a quarter of a million comparisons in a big repo,
/// and a watcher batch arrives for every file a build or an agent writes.
fn merge_snapshot(
    current: &[String],
    removed: &[Range<usize>],
    mut additions: Vec<String>,
) -> Vec<String> {
    additions.sort_unstable();
    additions.dedup();
    let dropped: usize = removed.iter().map(|range| range.end - range.start).sum();
    let mut out = Vec::with_capacity(current.len() - dropped + additions.len());
    let mut additions = additions.into_iter().peekable();
    let mut removed = removed.iter().peekable();
    let mut index = 0;
    while index < current.len() {
        if removed.peek().is_some_and(|range| range.start <= index) {
            index = removed.next().map_or(index, |range| range.end);
            continue;
        }
        let survivor = &current[index];
        while let Some(added) = additions.next_if(|added| added <= survivor) {
            if out.last() != Some(&added) {
                out.push(added);
            }
        }
        if out.last() != Some(survivor) {
            out.push(survivor.clone());
        }
        index += 1;
    }
    for added in additions {
        if out.last() != Some(&added) {
            out.push(added);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn canonical_temp_root(temp: &tempfile::TempDir) -> (String, PathBuf) {
        let repo = repo_key(temp.path().to_string_lossy().as_ref()).unwrap();
        let root = PathBuf::from(&repo);
        (repo, root)
    }

    fn snapshot(repo: &str) -> (u64, Vec<String>) {
        let answer = read(ProjectFilesParams {
            project: repo.to_owned(),
            since: None,
        })
        .unwrap();
        (answer.scan_id, answer.files.unwrap())
    }

    #[test]
    fn merged_snapshot_keeps_order_drops_removed_runs_and_never_repeats_a_path() {
        let current = vec![
            "a.rs".to_string(),
            "src/one.rs".to_string(),
            "src/two.rs".to_string(),
            "y.rs".to_string(),
            "z.rs".to_string(),
        ];
        let removed = vec![1..3, 4..5];
        let merged = merge_snapshot(
            &current,
            &removed,
            vec![
                "src/two.rs".to_string(),
                "b.rs".to_string(),
                "b.rs".to_string(),
                "z.rs".to_string(),
            ],
        );

        assert_eq!(merged, vec!["a.rs", "b.rs", "src/two.rs", "y.rs", "z.rs"]);
    }

    #[test]
    fn snapshot_is_sorted_deduped_and_scan_ids_are_monotonic() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("z.txt"), "z");
        write(&temp.path().join("a.txt"), "a");
        write(&temp.path().join(".DS_Store"), "junk");
        write(&temp.path().join("target/ignored.txt"), "ignored");
        let (repo, _) = canonical_temp_root(&temp);

        let (first, files) = snapshot(&repo);
        assert_eq!(files, vec!["a.txt", "z.txt"]);
        assert_eq!(snapshot(&repo).0, first);

        write(&temp.path().join("b.txt"), "b");
        moved(&repo, Some(&["b.txt".to_owned()]));
        let (second, files) = snapshot(&repo);
        assert!(second > first);
        assert_eq!(files, vec!["a.txt", "b.txt", "z.txt"]);

        write(&temp.path().join("b.txt"), "modified");
        moved(&repo, Some(&["b.txt".to_owned()]));
        let (after_modify, unchanged) = snapshot(&repo);
        assert_eq!(after_modify, second);
        assert_eq!(unchanged, files);
    }

    #[test]
    fn incremental_create_remove_rename_and_directory_subtrees_converge() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("old.txt"), "old");
        let (repo, root) = canonical_temp_root(&temp);
        let (initial, _) = snapshot(&repo);

        let directory = root.join("nested");
        write(&directory.join("one.txt"), "one");
        write(&directory.join("deep/two.txt"), "two");
        moved(&repo, Some(&["nested".to_owned()]));
        let (after_directory, files) = snapshot(&repo);
        assert!(after_directory > initial);
        assert_eq!(
            files,
            vec!["nested/deep/two.txt", "nested/one.txt", "old.txt"]
        );

        let old = root.join("old.txt");
        let renamed = root.join("renamed.txt");
        std::fs::rename(&old, &renamed).unwrap();
        moved(
            &repo,
            Some(&["old.txt".to_owned(), "renamed.txt".to_owned()]),
        );
        std::fs::remove_dir_all(&directory).unwrap();
        moved(&repo, Some(&["nested".to_owned()]));

        let (last, files) = snapshot(&repo);
        assert!(last > after_directory);
        assert_eq!(files, vec!["renamed.txt"]);
    }

    #[test]
    fn outside_path_and_forced_batches_use_full_scan_fallback() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("initial.txt"), "initial");
        let (repo, _root) = canonical_temp_root(&temp);
        let (initial, _) = snapshot(&repo);

        write(&temp.path().join("missed.txt"), "missed");
        moved(&repo, Some(&["../outside.txt".to_owned()]));
        let (after_outside, files) = snapshot(&repo);
        assert!(after_outside > initial);
        assert_eq!(files, vec!["initial.txt", "missed.txt"]);

        write(&temp.path().join("forced.txt"), "forced");
        moved(&repo, None);
        let (after_forced, files) = snapshot(&repo);
        assert!(after_forced > after_outside);
        assert_eq!(files, vec!["forced.txt", "initial.txt", "missed.txt"]);
    }

    #[test]
    fn cache_miss_defers_work_until_the_next_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("one.txt"), "one");
        let (repo, _) = canonical_temp_root(&temp);

        moved(&repo, Some(&["one.txt".to_owned()]));
        let (scan_id, files) = snapshot(&repo);
        assert_eq!(files, vec!["one.txt"]);
        assert!(scan_id > 0);
    }

    #[test]
    fn project_cache_cap_evicts_only_idle_cells() {
        let caches = ProjectCaches::default();
        let first = project_cache_from(&caches, "first", 2).unwrap();
        let second = project_cache_from(&caches, "second", 2).unwrap();

        // Both cells have active borrowers, so admitting a third cannot split an
        // in-flight scan/invalidation across a replacement lock cell.
        assert!(project_cache_from(&caches, "third", 2).is_err());
        assert_eq!(caches.locked().len(), 2);

        drop(first);
        let third = project_cache_from(&caches, "third", 2).unwrap();
        let held = caches.locked();
        assert!(!held.contains_key("first"));
        assert!(held.contains_key("second"));
        assert!(held.contains_key("third"));
        assert_eq!(held.len(), 2);
        drop(held);

        drop(second);
        drop(third);
    }

    #[test]
    fn full_scan_limits_file_count_and_encoded_path_bytes() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("one.txt"), "one");
        write(&temp.path().join("two.txt"), "two");
        let (repo, _) = canonical_temp_root(&temp);
        let file_limit = full_scan_with_limits(
            &repo,
            WalkLimits {
                max_files: 1,
                max_encoded_path_bytes: 128,
                strict_errors: false,
            },
        )
        .expect_err("file limit should reject the scan");
        assert!(file_limit.contains("1-file"));

        let byte_limit = full_scan_with_limits(
            &repo,
            WalkLimits {
                max_files: 10,
                max_encoded_path_bytes: encoded_path_upper_bound("one.txt") - 1,
                strict_errors: false,
            },
        )
        .expect_err("byte limit should reject the scan");
        assert!(byte_limit.contains("byte safety limit"));
    }

    #[test]
    fn encoded_path_budget_covers_json_escaping_and_incremental_rejection() {
        let files = vec![
            "plain.txt".to_owned(),
            "quote\"and\\slash".to_owned(),
            "line\nfeed".to_owned(),
            "snowman-☃".to_owned(),
        ];
        let encoded = snapshot_encoded_path_bytes_with_limit(&files, usize::MAX).unwrap();
        let serialized = serde_json::to_vec(&ProjectFiles {
            scan_id: u64::MAX,
            files: Some(files.clone()),
            added: None,
            removed: None,
        })
        .unwrap();
        assert!(serialized.len() <= encoded + 1024);
        assert_eq!(
            snapshot_encoded_path_bytes_with_limit(&files, encoded),
            Some(encoded)
        );
        assert_eq!(
            snapshot_encoded_path_bytes_with_limit(&files, encoded - 1),
            None
        );
    }
}
