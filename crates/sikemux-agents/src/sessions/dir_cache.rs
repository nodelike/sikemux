//! Per-directory mtime and entry cache for file-backed session scanners.
//! Unchanged cold directories skip `read_dir` and file `stat` calls until their
//! directory mtime changes or a periodic fallback sweep runs.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::index::Indexed;
use super::{file_stamp, FileStamp, MAX_AGENT_TRANSCRIPT_PATHS};

const MAX_CACHED_DIRS: usize = 8_192;
const FALLBACK_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);
const ACTIVE_DIR_WINDOW_MS: u64 = 30 * 60 * 1000;
const ACTIVE_FILE_WINDOW_MS: u64 = 2 * 60 * 60 * 1000;

#[derive(Clone)]
pub(super) struct CachedFile {
    pub(super) path: PathBuf,
    pub(super) key: String,
    pub(super) stamp: FileStamp,
    pub(super) indexed: Option<Arc<Indexed>>,
}

struct CachedDir {
    mtime_ns: u128,
    checked_at: Instant,
    newest_ms: u64,
    subdirs: Vec<PathBuf>,
    files: Vec<CachedFile>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, CachedDir>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedDir>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dir_mtime_ns(dir: &Path) -> Option<u128> {
    let meta = fs::metadata(dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(mtime)
}

/// Drops cached metadata for `path`'s parent directory after Sikemux mutates a transcript in place.
pub(super) fn invalidate_path(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    cache().locked().remove(parent);
}

/// Remembers a parsed `Indexed` record on the cached file entry so repeat scans avoid re-locking
/// the session index for unchanged files.
pub(super) fn remember_indexed(path: &Path, stamp: FileStamp, read: Arc<Indexed>) {
    let Some(parent) = path.parent() else {
        return;
    };
    let mut guard = cache().locked();
    let Some(entry) = guard.get_mut(parent) else {
        return;
    };
    if let Some(file) = entry
        .files
        .iter_mut()
        .find(|file| file.path == path && file.stamp == stamp)
    {
        file.indexed = Some(read);
    }
}

/// Lists subdirectories of `dir`, skipping `read_dir` when `dir`'s mtime has not changed.
pub(super) fn list_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Some(mtime_ns) = dir_mtime_ns(dir) else {
        cache().locked().remove(dir);
        return Vec::new();
    };
    if let Some(cached) = cache().locked().get(dir) {
        if cached.mtime_ns == mtime_ns {
            return cached.subdirs.clone();
        }
    }
    let Ok(entries) = fs::read_dir(dir) else {
        cache().locked().remove(dir);
        return Vec::new();
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|kind| kind.is_dir() || (kind.is_symlink() && path.is_dir()))
            .unwrap_or_else(|_| path.is_dir());
        if is_dir {
            subdirs.push(path);
        }
    }
    let mut guard = cache().locked();
    if guard.len() >= MAX_CACHED_DIRS && !guard.contains_key(dir) {
        guard.clear();
    }
    if let Some(existing) = guard.get_mut(dir) {
        existing.mtime_ns = mtime_ns;
        existing.subdirs = subdirs.clone();
    } else {
        guard.insert(
            dir.to_path_buf(),
            CachedDir {
                mtime_ns,
                checked_at: Instant::now(),
                newest_ms: 0,
                subdirs: subdirs.clone(),
                files: Vec::new(),
            },
        );
    }
    subdirs
}

/// Lists matching transcript files in a single directory, reusing cached entries when the
/// directory's mtime is unchanged. When `recheck_all` is true (explicit project queries),
/// cached files are re-statted without calling `read_dir`; otherwise cold directories skip
/// per-file `stat` calls between fallback sweeps.
pub(super) fn list_dir(
    dir: &Path,
    wanted: fn(&Path) -> bool,
    recheck_all: bool,
) -> Vec<CachedFile> {
    scan_dir(dir, wanted, recheck_all, false).1
}

/// Recursively collects matching transcript files under `root` up to depth 6, caching each
/// directory by mtime.
pub(super) fn list_tree(
    root: &Path,
    wanted: fn(&Path) -> bool,
    recheck_all: bool,
) -> Vec<CachedFile> {
    let mut out = Vec::new();
    collect_tree(root, wanted, recheck_all, &mut out, 0);
    out
}

fn collect_tree(
    dir: &Path,
    wanted: fn(&Path) -> bool,
    recheck_all: bool,
    out: &mut Vec<CachedFile>,
    depth: u32,
) {
    if depth > 6 || out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
        return;
    }
    let (subdirs, files) = scan_dir(dir, wanted, recheck_all, true);
    let room = MAX_AGENT_TRANSCRIPT_PATHS.saturating_sub(out.len());
    out.extend(files.into_iter().take(room));
    for sub in subdirs {
        if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
            break;
        }
        collect_tree(&sub, wanted, recheck_all, out, depth + 1);
    }
}

fn scan_dir(
    dir: &Path,
    wanted: fn(&Path) -> bool,
    recheck_all: bool,
    need_subdirs: bool,
) -> (Vec<PathBuf>, Vec<CachedFile>) {
    let Some(mtime_ns) = dir_mtime_ns(dir) else {
        cache().locked().remove(dir);
        return (Vec::new(), Vec::new());
    };
    let now = Instant::now();
    let now_ms = epoch_ms();

    let hit = {
        let guard = cache().locked();
        guard
            .get(dir)
            .filter(|cached| cached.mtime_ns == mtime_ns)
            .map(|cached| {
                let sweep_due = recheck_all
                    || now.saturating_duration_since(cached.checked_at) >= FALLBACK_SWEEP_INTERVAL;
                let active = now_ms.saturating_sub(cached.newest_ms) <= ACTIVE_DIR_WINDOW_MS;
                let subdirs = if need_subdirs {
                    cached.subdirs.clone()
                } else {
                    Vec::new()
                };
                (sweep_due, active, subdirs, cached.files.clone())
            })
    };

    if let Some((sweep_due, active, subdirs, mut files)) = hit {
        if !sweep_due && !active {
            return (subdirs, files);
        }
        let mut changed = false;
        let mut newest_ms = 0u64;
        for file in &mut files {
            let check = sweep_due
                || now_ms.saturating_sub(file.stamp.unix_millis()) <= ACTIVE_FILE_WINDOW_MS;
            if check {
                let stamp = file_stamp(&file.path);
                if stamp != file.stamp {
                    file.stamp = stamp;
                    file.indexed = None;
                    changed = true;
                }
            }
            newest_ms = newest_ms.max(file.stamp.unix_millis());
        }
        if sweep_due || changed {
            let mut guard = cache().locked();
            if let Some(entry) = guard.get_mut(dir) {
                if entry.mtime_ns == mtime_ns {
                    if sweep_due {
                        entry.checked_at = now;
                    }
                    entry.newest_ms = newest_ms;
                    entry.files = files.clone();
                }
            }
        }
        return (subdirs, files);
    }

    let Ok(entries) = fs::read_dir(dir) else {
        cache().locked().remove(dir);
        return (Vec::new(), Vec::new());
    };
    let prior_indexed: HashMap<PathBuf, (FileStamp, Arc<Indexed>)> = cache()
        .locked()
        .get(dir)
        .map(|cached| {
            cached
                .files
                .iter()
                .filter_map(|file| Some((file.path.clone(), (file.stamp, file.indexed.clone()?))))
                .collect()
        })
        .unwrap_or_default();

    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    let mut newest_ms = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|kind| kind.is_dir() || (kind.is_symlink() && path.is_dir()))
            .unwrap_or_else(|_| path.is_dir());
        if is_dir {
            subdirs.push(path);
        } else if wanted(&path) {
            let stamp = file_stamp(&path);
            newest_ms = newest_ms.max(stamp.unix_millis());
            let indexed = prior_indexed
                .get(&path)
                .filter(|(prior_stamp, _)| *prior_stamp == stamp)
                .map(|(_, read)| read.clone());
            let key = path.to_string_lossy().into_owned();
            files.push(CachedFile {
                path,
                key,
                stamp,
                indexed,
            });
        }
    }

    let returned_subdirs = if need_subdirs {
        subdirs.clone()
    } else {
        Vec::new()
    };
    let mut guard = cache().locked();
    if guard.len() >= MAX_CACHED_DIRS && !guard.contains_key(dir) {
        guard.clear();
    }
    guard.insert(
        dir.to_path_buf(),
        CachedDir {
            mtime_ns,
            checked_at: now,
            newest_ms,
            subdirs,
            files: files.clone(),
        },
    );
    (returned_subdirs, files)
}
