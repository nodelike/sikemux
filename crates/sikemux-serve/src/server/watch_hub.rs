//! Multiplexes filesystem watches across one `notify::RecommendedWatcher` per core,
//! avoiding a separate FSEvents runloop thread per watched folder or checkout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sikemux_process::sync::Locked;

type Callback = Arc<dyn Fn(&Event) + Send + Sync>;
type Subscribers = RwLock<HashMap<u64, Subscriber>>;

struct Subscriber {
    paths: HashMap<PathBuf, RecursiveMode>,
    callback: Callback,
}

#[derive(Default)]
struct WatcherState {
    watcher: Option<RecommendedWatcher>,
    active: HashMap<PathBuf, RecursiveMode>,
}

/// Shared filesystem watcher owned by a core instance.
///
/// Subscriptions hold RAII [`WatchHandle`]s; once the last watched path is removed the
/// underlying watcher is dropped so no background runloop thread remains alive.
pub struct WatchHub {
    next_id: AtomicU64,
    watcher: Mutex<WatcherState>,
    subscribers: Arc<Subscribers>,
}

impl Default for WatchHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchHub {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            watcher: Mutex::new(WatcherState::default()),
            subscribers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Creates a subscription with no watched paths yet; update it with [`WatchHandle::set_paths`].
    pub fn subscribe(
        self: &Arc<Self>,
        callback: impl Fn(&Event) + Send + Sync + 'static,
    ) -> WatchHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut subscribers = self.subscribers.write().unwrap_or_else(|e| e.into_inner());
        subscribers.insert(
            id,
            Subscriber {
                paths: HashMap::new(),
                callback: Arc::new(callback),
            },
        );
        WatchHandle {
            hub: Arc::downgrade(self),
            id,
        }
    }

    /// Watches a single `path` at `mode`, failing if the OS watcher rejects `path`.
    pub fn watch(
        self: &Arc<Self>,
        path: &Path,
        mode: RecursiveMode,
        callback: impl Fn(&Event) + Send + Sync + 'static,
    ) -> notify::Result<WatchHandle> {
        let handle = self.subscribe(callback);
        let canonical = canonical_path(path);
        let watched = handle.set_paths([(canonical.clone(), mode)]);
        if watched.contains_key(&canonical) {
            Ok(handle)
        } else {
            drop(handle);
            Err(notify::Error::path_not_found().add_path(path.to_path_buf()))
        }
    }

    fn update_subscriber(
        &self,
        id: u64,
        requested: HashMap<PathBuf, RecursiveMode>,
    ) -> HashMap<PathBuf, RecursiveMode> {
        let mut state = self.watcher.locked();
        let desired = {
            let mut subscribers = self.subscribers.write().unwrap_or_else(|e| e.into_inner());
            let Some(subscriber) = subscribers.get_mut(&id) else {
                return HashMap::new();
            };
            if subscriber.paths == requested {
                let mut kept = HashMap::new();
                for (path, mode) in &requested {
                    if covers(state.active.get(path).copied(), *mode) {
                        kept.insert(path.clone(), *mode);
                    }
                }
                return kept;
            }
            subscriber.paths = requested.clone();
            merged_paths(&subscribers)
        };
        sync_watcher(&mut state, &desired, &self.subscribers);
        let requested_len = requested.len();
        let mut kept = HashMap::new();
        for (path, mode) in requested {
            if covers(state.active.get(&path).copied(), mode) {
                kept.insert(path, mode);
            }
        }
        // Keep only paths that actually succeeded on the watcher so future updates stay accurate.
        if kept.len() != requested_len {
            let mut subscribers = self.subscribers.write().unwrap_or_else(|e| e.into_inner());
            if let Some(subscriber) = subscribers.get_mut(&id) {
                subscriber.paths = kept.clone();
            }
        }
        kept
    }

    fn remove_subscriber(&self, id: u64) {
        let mut state = self.watcher.locked();
        let desired = {
            let mut subscribers = self.subscribers.write().unwrap_or_else(|e| e.into_inner());
            if subscribers.remove(&id).is_none() {
                return;
            }
            merged_paths(&subscribers)
        };
        sync_watcher(&mut state, &desired, &self.subscribers);
    }
}

/// Active subscription on a [`WatchHub`]. Dropping it unregisters its callback and unwatches
/// any paths no other subscription needs.
pub struct WatchHandle {
    hub: Weak<WatchHub>,
    id: u64,
}

impl WatchHandle {
    /// Replaces this subscription's watched paths and returns the subset now actively watched.
    pub fn set_paths(
        &self,
        paths: impl IntoIterator<Item = (PathBuf, RecursiveMode)>,
    ) -> HashMap<PathBuf, RecursiveMode> {
        let Some(hub) = self.hub.upgrade() else {
            return HashMap::new();
        };
        let mut normalized: HashMap<PathBuf, RecursiveMode> = HashMap::new();
        for (path, mode) in paths {
            let canonical = canonical_path(&path);
            let entry = normalized.entry(canonical).or_insert(mode);
            if mode == RecursiveMode::Recursive {
                *entry = RecursiveMode::Recursive;
            }
        }
        hub.update_subscriber(self.id, normalized)
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.remove_subscriber(self.id);
        }
    }
}

fn canonical_path(path: &Path) -> PathBuf {
    sikemux_process::files::real_path(path).unwrap_or_else(|_| path.to_path_buf())
}

fn covers(active: Option<RecursiveMode>, wanted: RecursiveMode) -> bool {
    matches!(
        (active, wanted),
        (Some(RecursiveMode::Recursive), _)
            | (
                Some(RecursiveMode::NonRecursive),
                RecursiveMode::NonRecursive
            )
    )
}

fn merged_paths(subscribers: &HashMap<u64, Subscriber>) -> HashMap<PathBuf, RecursiveMode> {
    let mut merged: HashMap<PathBuf, RecursiveMode> = HashMap::new();
    for subscriber in subscribers.values() {
        for (path, mode) in &subscriber.paths {
            let entry = merged.entry(path.clone()).or_insert(*mode);
            if *mode == RecursiveMode::Recursive {
                *entry = RecursiveMode::Recursive;
            }
        }
    }
    merged
}

fn sync_watcher(
    state: &mut WatcherState,
    desired: &HashMap<PathBuf, RecursiveMode>,
    subscribers: &Arc<Subscribers>,
) {
    if desired.is_empty() {
        state.watcher = None;
        state.active.clear();
        return;
    }
    if state.watcher.is_none() {
        let weak = Arc::downgrade(subscribers);
        let Ok(watcher) = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let Some(subscribers) = weak.upgrade() else {
                return;
            };
            let targets: Vec<Callback> = {
                let subscribers = subscribers.read().unwrap_or_else(|e| e.into_inner());
                subscribers
                    .values()
                    .filter(|subscriber| matches_event(&subscriber.paths, &event))
                    .map(|subscriber| subscriber.callback.clone())
                    .collect()
            };
            for callback in targets {
                callback(&event);
            }
        }) else {
            return;
        };
        state.watcher = Some(watcher);
    }
    let Some(watcher) = state.watcher.as_mut() else {
        return;
    };
    let to_remove: Vec<PathBuf> = state
        .active
        .iter()
        .filter(|(path, mode)| desired.get(*path) != Some(*mode))
        .map(|(path, _)| path.clone())
        .collect();
    let to_add: Vec<(PathBuf, RecursiveMode)> = desired
        .iter()
        .filter(|(path, mode)| state.active.get(*path) != Some(*mode))
        .map(|(path, mode)| (path.clone(), *mode))
        .collect();
    if to_remove.is_empty() && to_add.is_empty() {
        return;
    }
    let mut paths = watcher.paths_mut();
    for path in &to_remove {
        let _ = paths.remove(path);
        state.active.remove(path);
    }
    for (path, mode) in to_add {
        if paths.add(&path, mode).is_ok() {
            state.active.insert(path, mode);
        }
    }
    let _ = paths.commit();
    if state.active.is_empty() {
        state.watcher = None;
    }
}

fn matches_event(roots: &HashMap<PathBuf, RecursiveMode>, event: &Event) -> bool {
    if roots.is_empty() {
        return false;
    }
    if event.need_rescan() || event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| {
        roots.iter().any(|(root, mode)| match mode {
            RecursiveMode::Recursive => path.starts_with(root),
            RecursiveMode::NonRecursive => path == root || path.parent() == Some(root.as_path()),
        })
    })
}
