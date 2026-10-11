// Project-wide file snapshots for the Cmd-P palette.
//
// Why we don't respect .gitignore:
//   .env, local.properties, *.local.json — these are exactly the files the
//   user often needs to jump to, but they're gitignored. Honouring gitignore
//   would hide them. Instead we use a fixed denylist of well-known build /
//   dependency / cache directories.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use ignore::WalkBuilder;
use serde::Serialize;

use crate::observability::{global_observability, Metadata, ScalarValue, SpanOutcome};

// Watcher deltas keep a snapshot current, while this TTL remains a periodic
// correctness backstop for missed platform events and repos without a watcher.
const TTL: Duration = Duration::from_secs(60);
const MAX_INCREMENTAL_PATHS: usize = 512;
const MAX_INCREMENTAL_FILES: usize = 4_096;
const MAX_PROJECT_FILES: usize = 250_000;
// Counts a conservative JSON-encoded upper bound for every relative path,
// including quotes and separators. This bounds the retained cache, the clone
// made for IPC, and the eventual command response before any of them are
// allocated without limit.
const MAX_PROJECT_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROJECT_ENCODED_PATH_BYTES: usize = MAX_PROJECT_SNAPSHOT_BYTES - 1024;
pub(crate) const MAX_REPO_PATH_BYTES: usize = 4 * 1024;
const MAX_PROJECT_CACHE_ENTRIES: usize = 128;
const FULL_SCAN_SLOW_THRESHOLD: Duration = Duration::from_millis(100);
const INCREMENTAL_SLOW_THRESHOLD: Duration = Duration::from_millis(16);

static NEXT_SCAN_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CACHE_ACCESS: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct Entry {
    files: Arc<Vec<String>>,
    encoded_path_bytes: usize,
    full_scan_at: Instant,
    scan_id: u64,
}

struct ProjectCache {
    entry: Mutex<Option<Entry>>,
    last_used: AtomicU64,
}

impl ProjectCache {
    fn new() -> Self {
        Self {
            entry: Mutex::new(None),
            last_used: AtomicU64::new(next_cache_access()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Entry>> {
        match self.entry.lock() {
            Ok(entry) => entry,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn touch(&self) {
        self.last_used.store(next_cache_access(), Ordering::Release);
    }
}

fn cache() -> &'static DashMap<String, Arc<ProjectCache>> {
    static CACHE: OnceLock<DashMap<String, Arc<ProjectCache>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn cache_admission_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn next_cache_access() -> u64 {
    NEXT_CACHE_ACCESS.fetch_add(1, Ordering::Relaxed)
}

/// Canonical repository identity shared by snapshots and native watchers.
/// Raw aliases remain a frontend routing concern; retaining only this key keeps
/// symlinks, `.` components, and trailing separators from multiplying native
/// cache/watcher ownership.
pub(crate) fn canonical_repo_key(repo: &str) -> Result<String, String> {
    if repo.is_empty()
        || repo.len() > MAX_REPO_PATH_BYTES
        || repo.contains('\0')
        || !Path::new(repo).is_absolute()
    {
        return Err(format!(
            "repository path must be an absolute path of at most {MAX_REPO_PATH_BYTES} bytes"
        ));
    }
    let canonical = std::fs::canonicalize(repo)
        .map_err(|error| format!("canonicalize repository path: {error}"))?;
    if !canonical.is_dir() {
        return Err("repository path is not a directory".into());
    }
    let canonical = canonical
        .into_os_string()
        .into_string()
        .map_err(|_| "canonical repository path is not valid UTF-8".to_string())?;
    if canonical.len() > MAX_REPO_PATH_BYTES {
        return Err(format!(
            "canonical repository path exceeds {MAX_REPO_PATH_BYTES} bytes"
        ));
    }
    Ok(canonical)
}

fn evict_one_idle_project_cache_from(caches: &DashMap<String, Arc<ProjectCache>>) -> bool {
    let mut candidates = caches
        .iter()
        .map(|entry| {
            (
                entry.value().last_used.load(Ordering::Acquire),
                entry.key().clone(),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    for (_, key) in candidates {
        // A concurrent lookup holds the DashMap shard while cloning the Arc.
        // `remove_if` therefore observes its strong reference before deciding;
        // only the map-owned, fully idle cell can be evicted.
        if caches
            .remove_if(&key, |_, project| Arc::strong_count(project) == 1)
            .is_some()
        {
            return true;
        }
    }
    false
}

fn project_cache_from(
    caches: &DashMap<String, Arc<ProjectCache>>,
    admission_lock: &Mutex<()>,
    repo: &str,
    capacity: usize,
) -> Result<Arc<ProjectCache>, String> {
    if let Some(project) = caches.get(repo) {
        let project = project.clone();
        project.touch();
        return Ok(project);
    }

    let _admission = admission_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(project) = caches.get(repo) {
        let project = project.clone();
        project.touch();
        return Ok(project);
    }
    while caches.len() >= capacity {
        if !evict_one_idle_project_cache_from(caches) {
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
    project_cache_from(
        cache(),
        cache_admission_lock(),
        repo,
        MAX_PROJECT_CACHE_ENTRIES,
    )
}

/// Explicit invalidation keeps compatibility with callers that need the next
/// read to be a complete scan. The per-project lock object is retained so an
/// in-flight scan and invalidation cannot split across different cache cells.
pub fn invalidate(repo: &str) {
    if let Some(project) = cache().get(repo) {
        *project.lock() = None;
    }
}

// Directories we never descend into. Keep this list conservative — anything
// here is permanently invisible to the file palette.
pub fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        // dependency / package managers
        "node_modules" | "bower_components" | "vendor"
        | ".pnpm-store" | ".yarn" | ".npm" | ".pnpm" | ".bun" | ".deno"
        // VCS
        | ".git" | ".hg" | ".svn"
        // build outputs (js / rust / generic)
        | "target" | "dist" | "build" | "out" | "_build"
        | ".next" | ".nuxt" | ".svelte-kit" | ".vercel" | ".turbo" | ".astro"
        | ".output"
        // caches (js / generic)
        | ".cache" | ".parcel-cache" | ".eslintcache" | ".vite"
        | "coverage" | ".nyc_output"
        // go
        | ".gocache" | ".go-cache" | "gocache" | "go-build"
        // rust
        | ".cargo" | ".rustup"
        // python
        | ".venv" | "venv" | ".env_venv" | "__pycache__"
        | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".tox"
        // jvm / scala
        | ".gradle" | ".m2" | ".bloop" | ".metals" | ".bsp"
        // infra / cloud
        | ".terraform" | ".serverless" | ".firebase" | ".vagrant"
        // ios / xcode
        | "Pods" | "Carthage" | "DerivedData" | "xcuserdata"
        // editor / OS junk
        | ".idea" | ".vscode-test" | ".DS_Store"
        // cmake / clion
        | "cmake-build-debug" | "cmake-build-release"
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilesSnapshot {
    pub scan_id: u64,
    pub files: Vec<String>,
}

impl Entry {
    fn snapshot(&self) -> ProjectFilesSnapshot {
        ProjectFilesSnapshot {
            scan_id: self.scan_id,
            files: (*self.files).clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum WatcherChange {
    /// Reconcile the current on-disk type: insert a file, replace a directory
    /// subtree, or remove a path which no longer exists.
    Reconcile(PathBuf),
    /// Remove the exact path and any cached descendants without consulting a
    /// potentially reused on-disk rename source.
    Remove(PathBuf),
}

enum FullScanReason {
    CacheMiss,
    Ttl,
    WatcherFallback,
}

impl FullScanReason {
    fn label(&self) -> &'static str {
        match self {
            Self::CacheMiss => "cache_miss",
            Self::Ttl => "ttl",
            Self::WatcherFallback => "watcher_fallback",
        }
    }
}

enum LimitedWalk {
    Complete {
        files: Vec<String>,
        encoded_path_bytes: usize,
    },
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
    LimitedWalk::Complete {
        files,
        encoded_path_bytes,
    }
}

fn next_scan_id() -> u64 {
    loop {
        let current = NEXT_SCAN_ID.load(Ordering::Relaxed);
        let Some(next) = current.checked_add(1) else {
            std::process::abort();
        };
        if NEXT_SCAN_ID
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return current;
        }
    }
}

fn full_scan_locked_with_limits(
    repo: &str,
    slot: &mut Option<Entry>,
    reason: FullScanReason,
    limits: WalkLimits,
) -> Result<Entry, String> {
    let observer = global_observability();
    let mut metadata = Metadata::new();
    metadata.insert("reason".to_owned(), ScalarValue::from(reason.label()));
    let timer =
        observer.slow_operation("files.full_scan", FULL_SCAN_SLOW_THRESHOLD, None, metadata);

    let (files, encoded_path_bytes) = match walk(Path::new(repo), Path::new(repo), limits) {
        LimitedWalk::Complete {
            files,
            encoded_path_bytes,
        } => (files, encoded_path_bytes),
        LimitedWalk::LimitExceeded => {
            let _ = observer.increment_counter("files.full_scan.limit_errors", 1);
            timer.finish(SpanOutcome::Error);
            return Err(format!(
                "project file snapshot exceeds its {}-file or {}-byte safety limit",
                limits.max_files, limits.max_encoded_path_bytes
            ));
        }
        LimitedWalk::Unreliable => {
            let _ = observer.increment_counter("files.full_scan.read_errors", 1);
            timer.finish(SpanOutcome::Error);
            return Err("project file snapshot could not be read reliably".into());
        }
    };
    let entry = Entry {
        files: Arc::new(files),
        encoded_path_bytes,
        full_scan_at: Instant::now(),
        scan_id: next_scan_id(),
    };
    *slot = Some(entry.clone());

    let _ = observer.increment_counter("files.full_scans", 1);
    observer.set_gauge("files.full_scan.last_file_count", entry.files.len() as f64);
    observer.set_gauge(
        "files.full_scan.last_encoded_path_bytes",
        entry.encoded_path_bytes as f64,
    );
    timer.finish(SpanOutcome::Success);
    Ok(entry)
}

fn full_scan_locked(
    repo: &str,
    slot: &mut Option<Entry>,
    reason: FullScanReason,
) -> Result<Entry, String> {
    full_scan_locked_with_limits(
        repo,
        slot,
        reason,
        WalkLimits {
            max_files: MAX_PROJECT_FILES,
            max_encoded_path_bytes: MAX_PROJECT_ENCODED_PATH_BYTES,
            strict_errors: false,
        },
    )
}

fn snapshot_for_key_blocking(repo: &str) -> Result<ProjectFilesSnapshot, String> {
    let project = project_cache(repo)?;
    let entry = {
        let mut slot = project.lock();
        if let Some(entry) = slot.as_ref() {
            if entry.full_scan_at.elapsed() < TTL {
                entry.clone()
            } else {
                full_scan_locked(repo, &mut slot, FullScanReason::Ttl)?
            }
        } else {
            full_scan_locked(repo, &mut slot, FullScanReason::CacheMiss)?
        }
    };
    // The IPC-owned Vec clone can be large; do it after releasing the
    // per-project mutation lock so watcher deltas do not wait on allocation.
    Ok(entry.snapshot())
}

fn snapshot_blocking(repo: &str) -> Result<ProjectFilesSnapshot, String> {
    let repo = canonical_repo_key(repo)?;
    snapshot_for_key_blocking(&repo)
}

#[tauri::command]
pub async fn list_project_files_snapshot(repo: String) -> Result<ProjectFilesSnapshot, String> {
    let repo_for_blocking = repo;
    tauri::async_runtime::spawn_blocking(move || snapshot_blocking(&repo_for_blocking))
        .await
        .map_err(|error| format!("walk join: {error}"))
        .and_then(|snapshot| snapshot)
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

fn fallback_full_scan(
    repo: &str,
    slot: &mut Option<Entry>,
    timer: crate::observability::SlowOperationGuard,
) {
    timer.finish(SpanOutcome::Cancelled);
    let _ = global_observability().increment_counter("files.incremental_fallbacks", 1);
    if full_scan_locked(repo, slot, FullScanReason::WatcherFallback).is_err() {
        // Keep the previous bounded snapshot rather than replacing it with an
        // empty or partial view. The next explicit read retries and surfaces
        // the stable limit/read error to the renderer.
        let _ = global_observability().increment_counter("files.incremental_fallback_errors", 1);
    }
}

/// Applies one debounced watcher batch to a cached sorted snapshot. Ambiguous,
/// outside-root, oversized, or unreadable changes fall back to one full scan.
/// A missing cache remains missing so the next command performs its normal
/// complete scan rather than doing duplicate work in the watcher task.
pub(crate) fn apply_watcher_batch(
    repo: &str,
    changes: Vec<WatcherChange>,
    force_full_rescan: bool,
) {
    let observer = global_observability();
    let mut metadata = Metadata::new();
    metadata.insert("change_count".to_owned(), ScalarValue::from(changes.len()));
    metadata.insert(
        "forced_rescan".to_owned(),
        ScalarValue::Bool(force_full_rescan),
    );
    let timer = observer.slow_operation(
        "files.incremental_update",
        INCREMENTAL_SLOW_THRESHOLD,
        None,
        metadata,
    );

    let project = match project_cache(repo) {
        Ok(project) => project,
        Err(_) => {
            let _ = observer.increment_counter("files.incremental_cache_capacity_errors", 1);
            timer.finish(SpanOutcome::Error);
            return;
        }
    };
    let mut slot = project.lock();
    let Some(current) = slot.as_ref().cloned() else {
        let _ = observer.increment_counter("files.incremental_cache_misses", 1);
        timer.finish(SpanOutcome::Cancelled);
        return;
    };

    if force_full_rescan || changes.len() > MAX_INCREMENTAL_PATHS {
        fallback_full_scan(repo, &mut slot, timer);
        return;
    }

    let repo_root = Path::new(repo);
    let mut removals = Vec::new();
    let mut additions = Vec::new();
    let mut relevant_changes = 0usize;
    let mut remaining_files = MAX_INCREMENTAL_FILES;

    for change in changes {
        let path = match &change {
            WatcherChange::Reconcile(path) | WatcherChange::Remove(path) => path,
        };
        let relative = match normalized_relative(repo_root, path) {
            Ok(Some(relative)) => relative,
            Ok(None) => continue,
            Err(()) => {
                fallback_full_scan(repo, &mut slot, timer);
                return;
            }
        };
        relevant_changes = relevant_changes.saturating_add(1);
        removals.push(relative.clone());

        if matches!(change, WatcherChange::Remove(_)) {
            continue;
        }

        let absolute = repo_root.join(&relative);
        let metadata = match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                fallback_full_scan(repo, &mut slot, timer);
                return;
            }
        };

        if metadata.file_type().is_file() {
            if remaining_files == 0 {
                fallback_full_scan(repo, &mut slot, timer);
                return;
            }
            remaining_files -= 1;
            let Some(relative) = wire_relative_path(&relative) else {
                fallback_full_scan(repo, &mut slot, timer);
                return;
            };
            additions.push(relative);
        } else if metadata.file_type().is_dir() && !leaf_is_denied_directory(&relative) {
            match walk(
                repo_root,
                &absolute,
                WalkLimits {
                    max_files: remaining_files,
                    max_encoded_path_bytes: MAX_PROJECT_ENCODED_PATH_BYTES,
                    strict_errors: true,
                },
            ) {
                LimitedWalk::Complete {
                    files: mut subtree, ..
                } => {
                    remaining_files = remaining_files.saturating_sub(subtree.len());
                    additions.append(&mut subtree);
                }
                LimitedWalk::LimitExceeded | LimitedWalk::Unreliable => {
                    fallback_full_scan(repo, &mut slot, timer);
                    return;
                }
            }
        }
    }

    if relevant_changes == 0 {
        timer.finish(SpanOutcome::Success);
        return;
    }

    let removals = collapse_prefixes(removals);
    let ranges = removal_ranges(&current.files, &removals);
    if ranges.is_empty() && additions.is_empty() {
        timer.finish(SpanOutcome::Success);
        return;
    }
    let files = merge_snapshot(&current.files, &ranges, additions);
    let Some(encoded_path_bytes) = snapshot_encoded_path_bytes(&files) else {
        fallback_full_scan(repo, &mut slot, timer);
        return;
    };
    if files.len() > MAX_PROJECT_FILES {
        fallback_full_scan(repo, &mut slot, timer);
        return;
    }

    let updated = Entry {
        files: Arc::new(files),
        encoded_path_bytes,
        // Incremental events do not postpone the periodic full-scan backstop.
        full_scan_at: current.full_scan_at,
        scan_id: next_scan_id(),
    };
    *slot = Some(updated.clone());

    let _ = observer.increment_counter("files.incremental_updates", 1);
    observer.set_gauge(
        "files.incremental.last_change_count",
        relevant_changes as f64,
    );
    observer.set_gauge(
        "files.incremental.last_file_count",
        updated.files.len() as f64,
    );
    observer.set_gauge(
        "files.incremental.last_encoded_path_bytes",
        updated.encoded_path_bytes as f64,
    );
    timer.finish(SpanOutcome::Success);
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
        let repo = canonical_repo_key(temp.path().to_string_lossy().as_ref()).unwrap();
        let root = PathBuf::from(&repo);
        (repo, root)
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
        let (repo, root) = canonical_temp_root(&temp);

        let first = snapshot_blocking(&repo).unwrap();
        assert_eq!(first.files, vec!["a.txt", "z.txt"]);
        assert_eq!(snapshot_blocking(&repo).unwrap().scan_id, first.scan_id);

        write(&temp.path().join("b.txt"), "b");
        apply_watcher_batch(
            &repo,
            vec![WatcherChange::Reconcile(root.join("b.txt"))],
            false,
        );
        let second = snapshot_blocking(&repo).unwrap();
        assert!(second.scan_id > first.scan_id);
        assert_eq!(second.files, vec!["a.txt", "b.txt", "z.txt"]);

        write(&temp.path().join("b.txt"), "modified");
        apply_watcher_batch(
            &repo,
            vec![WatcherChange::Reconcile(root.join("b.txt"))],
            false,
        );
        let after_modify = snapshot_blocking(&repo).unwrap();
        assert!(after_modify.scan_id > second.scan_id);
        assert_eq!(after_modify.files, second.files);
    }

    #[test]
    fn incremental_create_remove_rename_and_directory_subtrees_converge() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("old.txt"), "old");
        let (repo, root) = canonical_temp_root(&temp);
        let initial = snapshot_blocking(&repo).unwrap();

        let directory = root.join("nested");
        write(&directory.join("one.txt"), "one");
        write(&directory.join("deep/two.txt"), "two");
        apply_watcher_batch(
            &repo,
            vec![WatcherChange::Reconcile(directory.clone())],
            false,
        );
        let after_directory = snapshot_blocking(&repo).unwrap();
        assert!(after_directory.scan_id > initial.scan_id);
        assert_eq!(
            after_directory.files,
            vec!["nested/deep/two.txt", "nested/one.txt", "old.txt"]
        );

        let old = root.join("old.txt");
        let renamed = root.join("renamed.txt");
        std::fs::rename(&old, &renamed).unwrap();
        apply_watcher_batch(
            &repo,
            vec![
                WatcherChange::Remove(old),
                WatcherChange::Reconcile(renamed),
            ],
            false,
        );
        std::fs::remove_dir_all(&directory).unwrap();
        apply_watcher_batch(&repo, vec![WatcherChange::Remove(directory)], false);

        let final_snapshot = snapshot_blocking(&repo).unwrap();
        assert!(final_snapshot.scan_id > after_directory.scan_id);
        assert_eq!(final_snapshot.files, vec!["renamed.txt"]);
    }

    #[test]
    fn outside_path_and_forced_batches_use_full_scan_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&temp.path().join("initial.txt"), "initial");
        let (repo, _root) = canonical_temp_root(&temp);
        let initial = snapshot_blocking(&repo).unwrap();

        write(&temp.path().join("missed.txt"), "missed");
        apply_watcher_batch(
            &repo,
            vec![WatcherChange::Reconcile(outside.path().join("outside.txt"))],
            false,
        );
        let after_outside = snapshot_blocking(&repo).unwrap();
        assert!(after_outside.scan_id > initial.scan_id);
        assert_eq!(after_outside.files, vec!["initial.txt", "missed.txt"]);

        write(&temp.path().join("forced.txt"), "forced");
        apply_watcher_batch(&repo, Vec::new(), true);
        let after_forced = snapshot_blocking(&repo).unwrap();
        assert!(after_forced.scan_id > after_outside.scan_id);
        assert_eq!(
            after_forced.files,
            vec!["forced.txt", "initial.txt", "missed.txt"]
        );
    }

    #[test]
    fn cache_miss_defers_work_until_the_next_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("one.txt"), "one");
        let (repo, root) = canonical_temp_root(&temp);

        invalidate(&repo);
        apply_watcher_batch(
            &repo,
            vec![WatcherChange::Reconcile(root.join("one.txt"))],
            false,
        );
        let snapshot = snapshot_blocking(&repo).unwrap();
        assert_eq!(snapshot.files, vec!["one.txt"]);
        assert!(snapshot.scan_id > 0);
    }

    #[test]
    fn canonical_repo_keys_collapse_aliases_and_bound_input() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = canonical_repo_key(temp.path().to_string_lossy().as_ref()).unwrap();
        let dotted = temp.path().join(".");
        assert_eq!(
            canonical_repo_key(dotted.to_string_lossy().as_ref()).unwrap(),
            canonical
        );
        assert!(canonical_repo_key("relative/repo").is_err());
        assert!(canonical_repo_key(&format!("/{}", "x".repeat(MAX_REPO_PATH_BYTES))).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let parent = tempfile::tempdir().unwrap();
            let alias = parent.path().join("repo-alias");
            symlink(temp.path(), &alias).unwrap();
            assert_eq!(
                canonical_repo_key(alias.to_string_lossy().as_ref()).unwrap(),
                canonical
            );
        }
    }

    #[test]
    fn project_cache_cap_evicts_only_idle_cells() {
        let caches = DashMap::new();
        let admission = Mutex::new(());
        let first = project_cache_from(&caches, &admission, "first", 2).unwrap();
        let second = project_cache_from(&caches, &admission, "second", 2).unwrap();

        // Both cells have active borrowers, so admitting a third cannot split an
        // in-flight scan/invalidation across a replacement lock cell.
        assert!(project_cache_from(&caches, &admission, "third", 2).is_err());
        assert_eq!(caches.len(), 2);

        drop(first);
        let third = project_cache_from(&caches, &admission, "third", 2).unwrap();
        assert!(!caches.contains_key("first"));
        assert!(caches.contains_key("second"));
        assert!(caches.contains_key("third"));
        assert_eq!(caches.len(), 2);

        drop(second);
        drop(third);
    }

    #[test]
    fn full_scan_limits_file_count_and_encoded_path_bytes_without_replacing_cache() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("one.txt"), "one");
        let (repo, _) = canonical_temp_root(&temp);
        let mut slot = None;
        let first = full_scan_locked_with_limits(
            &repo,
            &mut slot,
            FullScanReason::CacheMiss,
            WalkLimits {
                max_files: 1,
                max_encoded_path_bytes: 128,
                strict_errors: false,
            },
        )
        .unwrap();

        write(&temp.path().join("two.txt"), "two");
        let file_limit = full_scan_locked_with_limits(
            &repo,
            &mut slot,
            FullScanReason::Ttl,
            WalkLimits {
                max_files: 1,
                max_encoded_path_bytes: 128,
                strict_errors: false,
            },
        )
        .err()
        .expect("file limit should reject the scan");
        assert!(file_limit.contains("1-file"));
        assert_eq!(slot.as_ref().unwrap().scan_id, first.scan_id);
        assert_eq!(slot.as_ref().unwrap().files.as_slice(), ["one.txt"]);

        let byte_limit = full_scan_locked_with_limits(
            &repo,
            &mut slot,
            FullScanReason::WatcherFallback,
            WalkLimits {
                max_files: 10,
                max_encoded_path_bytes: encoded_path_upper_bound("one.txt") - 1,
                strict_errors: false,
            },
        )
        .err()
        .expect("byte limit should reject the scan");
        assert!(byte_limit.contains("byte safety limit"));
        assert_eq!(slot.as_ref().unwrap().scan_id, first.scan_id);
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
        let serialized = serde_json::to_vec(&ProjectFilesSnapshot {
            scan_id: u64::MAX,
            files: files.clone(),
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
