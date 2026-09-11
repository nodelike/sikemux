// Watch a repo's working tree and emit `git_changed` events to the frontend
// whenever files (or `.git/index`, refs, HEAD) move. Replaces the 3s polling
// loop in GitPane. Per-repo watcher; a `repo_watch_stop` releases one exact
// opaque lease token.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::files::WatcherChange;

fn watch_err<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Watch(e.to_string())
}

struct WatchHandle {
    _watcher: RecommendedWatcher,
    lifetime: Arc<WatchLifetime>,
}

struct WatchLifetime {
    active: AtomicBool,
    update_lock: Mutex<()>,
}

impl WatchLifetime {
    fn new() -> Self {
        Self {
            active: AtomicBool::new(true),
            update_lock: Mutex::new(()),
        }
    }

    fn apply_if_active(&self, update: impl FnOnce()) -> bool {
        let _update = self
            .update_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        update();
        true
    }

    fn deactivate(&self) {
        let _update = self
            .update_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // Serialize the last release with an in-flight cache mutation. The
        // registry remains locked while this runs, so a replacement watcher
        // cannot start until the old debouncer is irrevocably inactive.
        self.lifetime.deactivate();
    }
}

const MAX_WATCH_LEASES: u16 = 1_024;
const MAX_TOTAL_WATCH_LEASES: usize = 4_096;
const MAX_ACTIVE_REPO_WATCHERS: usize = 64;
const MAX_REPO_ALIASES_PER_WATCHER: usize = 32;
const WATCH_TOKEN_BYTES: usize = 36;
const WATCH_LEASE_LIMIT_ERROR: &str = "repo watcher lease limit reached";
const TOTAL_WATCH_LEASE_LIMIT_ERROR: &str = "total repo watcher lease limit reached";
const WATCHER_CAPACITY_ERROR: &str = "active repo watcher limit reached";
const WATCH_ALIAS_LIMIT_ERROR: &str = "repo watcher alias limit reached";
const WATCH_ALIAS_CONFLICT_ERROR: &str = "repo watcher alias changed while still leased";
const WATCH_TOKEN_ERROR: &str = "invalid repo watcher lease token";
const WATCH_TOKEN_CONFLICT_ERROR: &str = "repo watcher lease token is already in use";

struct WatchEntry<H> {
    _handle: H,
    leases: u16,
    routes: HashMap<String, u16>,
}

#[derive(Clone)]
struct WatchLeaseIdentity {
    repo_key: String,
    route: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseAcquire {
    Created,
    Acquired(u16),
    Existing,
}

#[derive(Debug, Eq, PartialEq)]
enum LeaseAcquireError<E> {
    LeaseLimit,
    TotalLeaseLimit,
    WatcherLimit,
    AliasLimit,
    AliasConflict,
    TokenConflict,
    Create(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseRelease {
    Unknown,
    Retained(u16),
    Removed,
}

struct WatchRegistry<H> {
    entries: HashMap<String, WatchEntry<H>>,
    route_keys: HashMap<String, String>,
    lease_tokens: HashMap<String, WatchLeaseIdentity>,
}

impl<H> Default for WatchRegistry<H> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            route_keys: HashMap::new(),
            lease_tokens: HashMap::new(),
        }
    }
}

impl<H> WatchRegistry<H> {
    fn acquire_or_insert<E>(
        &mut self,
        repo_key: String,
        route: String,
        token: String,
        create: impl FnOnce() -> Result<H, E>,
    ) -> Result<LeaseAcquire, LeaseAcquireError<E>> {
        if let Some(existing) = self.lease_tokens.get(&token) {
            return if existing.repo_key == repo_key && existing.route == route {
                Ok(LeaseAcquire::Existing)
            } else {
                Err(LeaseAcquireError::TokenConflict)
            };
        }
        if self.lease_tokens.len() >= MAX_TOTAL_WATCH_LEASES {
            return Err(LeaseAcquireError::TotalLeaseLimit);
        }
        if self
            .route_keys
            .get(&route)
            .is_some_and(|existing| existing != &repo_key)
        {
            return Err(LeaseAcquireError::AliasConflict);
        }
        let watcher_count = self.entries.len();
        let identity = WatchLeaseIdentity {
            repo_key: repo_key.clone(),
            route: route.clone(),
        };
        match self.entries.entry(repo_key.clone()) {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                if entry.leases >= MAX_WATCH_LEASES {
                    return Err(LeaseAcquireError::LeaseLimit);
                }
                if !entry.routes.contains_key(&route)
                    && entry.routes.len() >= MAX_REPO_ALIASES_PER_WATCHER
                {
                    return Err(LeaseAcquireError::AliasLimit);
                }
                entry.leases += 1;
                let route_leases = entry.routes.entry(route.clone()).or_insert(0);
                *route_leases += 1;
                self.route_keys.insert(route, repo_key);
                self.lease_tokens.insert(token, identity);
                Ok(LeaseAcquire::Acquired(entry.leases))
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                if watcher_count >= MAX_ACTIVE_REPO_WATCHERS {
                    return Err(LeaseAcquireError::WatcherLimit);
                }
                let handle = create().map_err(LeaseAcquireError::Create)?;
                let mut routes = HashMap::new();
                routes.insert(route.clone(), 1);
                vacant.insert(WatchEntry {
                    _handle: handle,
                    leases: 1,
                    routes,
                });
                self.route_keys.insert(route, repo_key);
                self.lease_tokens.insert(token, identity);
                Ok(LeaseAcquire::Created)
            }
        }
    }

    fn release(&mut self, token: &str) -> LeaseRelease {
        let Some(identity) = self.lease_tokens.get(token).cloned() else {
            return LeaseRelease::Unknown;
        };
        let Some(entry) = self.entries.get_mut(&identity.repo_key) else {
            self.lease_tokens.remove(token);
            return LeaseRelease::Unknown;
        };
        let Some(route_leases) = entry.routes.get_mut(&identity.route) else {
            self.lease_tokens.remove(token);
            return LeaseRelease::Unknown;
        };

        self.lease_tokens.remove(token);
        let remove_route = *route_leases == 1;
        if *route_leases > 1 {
            *route_leases -= 1;
        }
        if remove_route {
            entry.routes.remove(&identity.route);
        }
        entry.leases = entry.leases.saturating_sub(1);
        let release = if entry.leases > 0 {
            LeaseRelease::Retained(entry.leases)
        } else {
            LeaseRelease::Removed
        };

        if remove_route {
            self.route_keys.remove(&identity.route);
        }
        if release == LeaseRelease::Removed {
            self.entries.remove(&identity.repo_key);
        }
        release
    }

    fn counts(&self) -> (usize, u64, usize) {
        let leases = self.entries.values().fold(0_u64, |total, entry| {
            total.saturating_add(u64::from(entry.leases))
        });
        debug_assert_eq!(leases, self.lease_tokens.len() as u64);
        (self.entries.len(), leases, self.route_keys.len())
    }

    fn routes(&self, repo_key: &str) -> Vec<String> {
        self.entries
            .get(repo_key)
            .map(|entry| entry.routes.keys().cloned().collect())
            .unwrap_or_default()
    }
}

fn registry() -> &'static Mutex<WatchRegistry<WatchHandle>> {
    static R: OnceLock<Mutex<WatchRegistry<WatchHandle>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(WatchRegistry::default()))
}

fn lock_registry() -> MutexGuard<'static, WatchRegistry<WatchHandle>> {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn valid_watch_token(token: &str) -> bool {
    if token.len() != WATCH_TOKEN_BYTES {
        return false;
    }
    Uuid::parse_str(token).is_ok_and(|uuid| {
        uuid.get_version_num() == 4
            && uuid.get_variant() == uuid::Variant::RFC4122
            && uuid.hyphenated().to_string() == token
    })
}

pub fn watch_count() -> usize {
    lock_registry().entries.len()
}

fn record_registry_gauges(watchers: usize, leases: u64, routes: usize) {
    let observer = crate::observability::global_observability();
    observer.set_gauge("fs_watch.active_watchers", watchers as f64);
    observer.set_gauge("fs_watch.active_leases", leases as f64);
    observer.set_gauge("fs_watch.active_routes", routes as f64);
}

fn increment_watch_counter(name: &'static str) {
    let _ = crate::observability::global_observability().increment_counter(name, 1);
}

fn active_routes(repo_key: &str) -> Vec<String> {
    lock_registry().routes(repo_key)
}

fn emit_changed_to_active_routes(app: &AppHandle, repo_key: &str, paths: Option<Vec<String>>) {
    for repo in active_routes(repo_key) {
        let _ = app.emit(
            "git_changed",
            ChangePayload {
                repo,
                paths: paths.clone(),
            },
        );
    }
}

#[derive(Serialize, Clone)]
struct ChangePayload {
    repo: String,
    paths: Option<Vec<String>>,
}

fn changed_relative_paths(
    repo: &Path,
    changes: &[WatcherChange],
    force_rescan: bool,
) -> Option<Vec<String>> {
    if force_rescan {
        return None;
    }
    let mut paths = Vec::with_capacity(changes.len());
    for change in changes {
        let path = match change {
            WatcherChange::Reconcile(path) | WatcherChange::Remove(path) => path,
        };
        if is_git_signal(path) {
            return None;
        }
        let relative = path.strip_prefix(repo).ok()?.to_str()?;
        if relative.is_empty() {
            return None;
        }
        paths.push(relative.replace('\\', "/"));
    }
    paths.sort_unstable();
    paths.dedup();
    Some(paths)
}

// 200ms debounce — fsevents on macOS fires bursts for a single save.
const DEBOUNCE_MS: u64 = 200;
const MAX_DEBOUNCE_MS: u64 = 1_000;
const EVENT_CHANNEL_CAPACITY: usize = 1_024;
const MAX_BATCH_CHANGES: usize = 512;

enum WatchMessage {
    Changes(Vec<WatcherChange>),
    Rescan,
}

fn merge_message(message: WatchMessage, changes: &mut Vec<WatcherChange>, force_rescan: &mut bool) {
    match message {
        WatchMessage::Rescan => {
            *force_rescan = true;
            changes.clear();
        }
        WatchMessage::Changes(mut incoming) if !*force_rescan => {
            if changes.len().saturating_add(incoming.len()) > MAX_BATCH_CHANGES {
                *force_rescan = true;
                changes.clear();
            } else {
                changes.append(&mut incoming);
            }
        }
        WatchMessage::Changes(_) => {}
    }
}

fn spawn_debouncer(
    app: AppHandle,
    repo_key: String,
    mut rx: tokio::sync::mpsc::Receiver<WatchMessage>,
    rescan_requested: Arc<AtomicBool>,
    lifetime: Arc<WatchLifetime>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut changes = Vec::new();
            let mut force_rescan = rescan_requested.swap(false, Ordering::AcqRel);
            merge_message(first, &mut changes, &mut force_rescan);

            let sleep = tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS));
            tokio::pin!(sleep);
            let max_sleep = tokio::time::sleep(Duration::from_millis(MAX_DEBOUNCE_MS));
            tokio::pin!(max_sleep);
            let mut closed = false;
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    _ = &mut max_sleep => break,
                    msg = rx.recv() => {
                        match msg {
                            Some(message) => {
                                merge_message(message, &mut changes, &mut force_rescan);
                                force_rescan |= rescan_requested.swap(false, Ordering::AcqRel);
                                sleep.as_mut().reset(
                                    tokio::time::Instant::now() + Duration::from_millis(DEBOUNCE_MS)
                                );
                            }
                            None => {
                                closed = true;
                                break;
                            }
                        }
                    }
                }
            }

            force_rescan |= rescan_requested.swap(false, Ordering::AcqRel);
            let paths = changed_relative_paths(Path::new(&repo_key), &changes, force_rescan);
            let repo_for_update = repo_key.clone();
            let update_lifetime = lifetime.clone();
            let update = tauri::async_runtime::spawn_blocking(move || {
                update_lifetime.apply_if_active(|| {
                    crate::files::apply_watcher_batch(&repo_for_update, changes, force_rescan);
                })
            })
            .await;
            match update {
                Ok(true) => emit_changed_to_active_routes(&app, &repo_key, paths),
                Ok(false) => return,
                Err(_) => {
                    // A panicked blocking task must not leave a trusted stale
                    // snapshot behind. The next palette open performs a full scan.
                    crate::files::invalidate(&repo_key);
                    emit_changed_to_active_routes(&app, &repo_key, None);
                }
            }
            if closed {
                return;
            }
        }
    });
}

fn is_git_signal(path: &Path) -> bool {
    let mut after_git = false;
    for c in path.components() {
        let s = c.as_os_str().to_string_lossy();
        if !after_git {
            if s == ".git" {
                after_git = true;
            }
            continue;
        }
        return matches!(
            s.as_ref(),
            "HEAD"
                | "index"
                | "packed-refs"
                | "MERGE_HEAD"
                | "REBASE_HEAD"
                | "CHERRY_PICK_HEAD"
                | "BISECT_LOG"
                | "refs"
                | "logs"
                | "rebase-merge"
                | "rebase-apply"
        );
    }
    false
}

fn should_ignore(repo: &Path, path: &Path) -> bool {
    if is_git_signal(path) {
        return false;
    }
    // Share the file-palette denylist (build artifacts, dep caches, VCS
    // metadata) so a `cargo test` / `pytest` / `npm run build` in the
    // watched repo doesn't fire thousands of fs events that all clear the
    // resource cache and re-run `git_overview`. `.DS_Store` is a file so
    // it's not covered by should_skip_dir; keep the explicit check.
    let Ok(relative) = path.strip_prefix(repo) else {
        // Let the file cache classify outside-root paths as ambiguous and use
        // its correctness-preserving full-rescan fallback.
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let name = component.as_os_str().to_string_lossy();
        if name == ".DS_Store" {
            return true;
        }
        let is_leaf = index + 1 == components.len();
        // Keep an event for the denied directory itself: an included file
        // named `target` may just have been replaced by a denied directory,
        // and the incremental cache must remove that stale file. Descendant
        // churn remains ignored.
        if crate::files::should_skip_dir(&name) && !is_leaf {
            return true;
        }
    }
    false
}

fn changes_for_event(repo: &Path, event: Event) -> Option<WatchMessage> {
    if event.need_rescan() {
        return Some(WatchMessage::Rescan);
    }
    if event.paths.len() > MAX_BATCH_CHANGES {
        return Some(WatchMessage::Rescan);
    }

    let mut changes = match event.kind {
        EventKind::Access(_) => return None,
        EventKind::Create(_) => event
            .paths
            .into_iter()
            .map(WatcherChange::Reconcile)
            .collect::<Vec<_>>(),
        EventKind::Remove(_) => event
            .paths
            .into_iter()
            .map(WatcherChange::Remove)
            .collect::<Vec<_>>(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if event.paths.len() != 2 {
                return Some(WatchMessage::Rescan);
            }
            vec![
                WatcherChange::Remove(event.paths[0].clone()),
                WatcherChange::Reconcile(event.paths[1].clone()),
            ]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event
            .paths
            .into_iter()
            .map(WatcherChange::Remove)
            .collect::<Vec<_>>(),
        EventKind::Modify(ModifyKind::Name(
            RenameMode::To | RenameMode::Any | RenameMode::Other,
        ))
        | EventKind::Modify(_) => event
            .paths
            .into_iter()
            .map(WatcherChange::Reconcile)
            .collect::<Vec<_>>(),
        EventKind::Any | EventKind::Other => return Some(WatchMessage::Rescan),
    };

    if changes.is_empty() {
        return Some(WatchMessage::Rescan);
    }
    changes.retain(|change| {
        let path: &PathBuf = match change {
            WatcherChange::Reconcile(path) | WatcherChange::Remove(path) => path,
        };
        !should_ignore(repo, path)
    });
    if changes.is_empty() {
        None
    } else {
        Some(WatchMessage::Changes(changes))
    }
}

fn try_send_message(
    tx: &tokio::sync::mpsc::Sender<WatchMessage>,
    rescan_requested: &AtomicBool,
    message: WatchMessage,
) {
    if matches!(
        tx.try_send(message),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_))
    ) {
        // A dropped path delta makes the whole batch untrustworthy. The
        // bounded channel already contains a wakeup, and the debouncer
        // will convert the shared flag into one complete rescan.
        rescan_requested.store(true, Ordering::Release);
    }
}

fn create_watch(app: AppHandle, repo_key: String) -> AppResult<WatchHandle> {
    let (tx, rx) = tokio::sync::mpsc::channel::<WatchMessage>(EVENT_CHANNEL_CAPACITY);
    let rescan_requested = Arc::new(AtomicBool::new(false));
    let lifetime = Arc::new(WatchLifetime::new());
    let callback_repo = PathBuf::from(&repo_key);
    let callback_rescan = rescan_requested.clone();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let message = match res {
            Ok(event) => changes_for_event(&callback_repo, event),
            Err(_) => Some(WatchMessage::Rescan),
        };
        if let Some(message) = message {
            try_send_message(&tx, &callback_rescan, message);
        }
    })
    .map_err(watch_err)?;

    watcher
        .watch(Path::new(&repo_key), RecursiveMode::Recursive)
        .map_err(watch_err)?;

    // Start the consumer only after the OS watcher is live. Events produced
    // during `watch` remain bounded in the channel; a failed setup drops both
    // ends without publishing a registry entry or leaking a background task.
    spawn_debouncer(app, repo_key, rx, rescan_requested, lifetime.clone());
    Ok(WatchHandle {
        _watcher: watcher,
        lifetime,
    })
}

#[tauri::command]
pub fn repo_watch_start(app: AppHandle, repo: String, token: String) -> AppResult<()> {
    if !valid_watch_token(&token) {
        increment_watch_counter("fs_watch.start_invalid_token");
        return Err(AppError::Watch(WATCH_TOKEN_ERROR.to_owned()));
    }
    let repo_key = crate::files::canonical_repo_key(&repo).map_err(watch_err)?;
    // The registry lock deliberately covers construction. Starts are rare,
    // and serializing them closes the check/create/insert race that could
    // otherwise produce two native event streams for one repo.
    let mut registry = lock_registry();
    let acquisition = registry.acquire_or_insert(repo_key.clone(), repo.clone(), token, || {
        create_watch(app.clone(), repo_key.clone())
    });
    let counts = registry.counts();
    drop(registry);

    match acquisition {
        Ok(LeaseAcquire::Acquired(_)) => {
            increment_watch_counter("fs_watch.lease_acquires");
            record_registry_gauges(counts.0, counts.1, counts.2);
        }
        Ok(LeaseAcquire::Existing) => {
            increment_watch_counter("fs_watch.start_replays");
        }
        Ok(LeaseAcquire::Created) => {
            increment_watch_counter("fs_watch.lease_acquires");
            increment_watch_counter("fs_watch.watcher_starts");
            record_registry_gauges(counts.0, counts.1, counts.2);
        }
        Err(LeaseAcquireError::LeaseLimit) => {
            increment_watch_counter("fs_watch.lease_overflows");
            return Err(AppError::Watch(WATCH_LEASE_LIMIT_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::TotalLeaseLimit) => {
            increment_watch_counter("fs_watch.total_lease_overflows");
            return Err(AppError::Watch(TOTAL_WATCH_LEASE_LIMIT_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::WatcherLimit) => {
            increment_watch_counter("fs_watch.watcher_overflows");
            return Err(AppError::Watch(WATCHER_CAPACITY_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::AliasLimit) => {
            increment_watch_counter("fs_watch.alias_overflows");
            return Err(AppError::Watch(WATCH_ALIAS_LIMIT_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::AliasConflict) => {
            increment_watch_counter("fs_watch.alias_conflicts");
            return Err(AppError::Watch(WATCH_ALIAS_CONFLICT_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::TokenConflict) => {
            increment_watch_counter("fs_watch.token_conflicts");
            return Err(AppError::Watch(WATCH_TOKEN_CONFLICT_ERROR.to_owned()));
        }
        Err(LeaseAcquireError::Create(error)) => {
            increment_watch_counter("fs_watch.start_errors");
            return Err(error);
        }
    }

    // First repo-scoped synthetic emit so the frontend refreshes this project
    // after subscribing. Keep it scoped; an empty repo means "invalidate all"
    // on the JS side and causes an O(open projects) refetch storm.
    crate::files::invalidate(&repo_key);
    let _ = app.emit::<ChangePayload>("git_changed", ChangePayload { repo, paths: None });
    Ok(())
}

#[tauri::command]
pub fn repo_watch_stop(token: String) -> AppResult<()> {
    if !valid_watch_token(&token) {
        increment_watch_counter("fs_watch.stop_invalid");
        return Err(AppError::Watch(WATCH_TOKEN_ERROR.to_owned()));
    }
    let mut registry = lock_registry();
    let release = registry.release(&token);
    let counts = registry.counts();
    drop(registry);

    match release {
        LeaseRelease::Unknown => {
            // Cleanup paths may race or retry, so an already-consumed token is
            // explicitly idempotent. Count it for lifecycle diagnostics.
            increment_watch_counter("fs_watch.stop_unknown");
        }
        LeaseRelease::Retained(_) => {
            increment_watch_counter("fs_watch.lease_releases");
            record_registry_gauges(counts.0, counts.1, counts.2);
        }
        LeaseRelease::Removed => {
            increment_watch_counter("fs_watch.lease_releases");
            increment_watch_counter("fs_watch.watcher_stops");
            record_registry_gauges(counts.0, counts.1, counts.2);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::Flag;

    #[test]
    fn change_payload_bounds_refreshes_and_falls_back_for_ambiguous_batches() {
        let repo = Path::new("/repo");
        let changes = vec![
            WatcherChange::Remove(repo.join("src/old.rs")),
            WatcherChange::Reconcile(repo.join("src/new.rs")),
            WatcherChange::Reconcile(repo.join("src/new.rs")),
        ];
        assert_eq!(
            changed_relative_paths(repo, &changes, false),
            Some(vec!["src/new.rs".into(), "src/old.rs".into()])
        );
        assert_eq!(changed_relative_paths(repo, &changes, true), None);
        for path in [
            repo.join(".git/HEAD"),
            repo.to_path_buf(),
            PathBuf::from("/outside/file"),
        ] {
            assert_eq!(
                changed_relative_paths(repo, &[WatcherChange::Reconcile(path)], false),
                None
            );
        }
    }

    #[test]
    fn watcher_registry_leases_reuse_one_handle_and_remove_only_at_zero() {
        let mut registry = WatchRegistry::<u8>::default();
        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token-a".to_owned(),
                || Ok::<_, &'static str>(7),
            ),
            Ok(LeaseAcquire::Created)
        );
        assert_eq!(
            registry.acquire_or_insert::<&'static str>(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token-b".to_owned(),
                || panic!("existing watcher must not be recreated"),
            ),
            Ok(LeaseAcquire::Acquired(2))
        );
        assert_eq!(registry.counts(), (1, 2, 1));
        assert_eq!(registry.entries.get("canonical").unwrap()._handle, 7);

        assert_eq!(registry.release("token-a"), LeaseRelease::Retained(1));
        assert_eq!(registry.counts(), (1, 1, 1));
        assert_eq!(registry.release("token-a"), LeaseRelease::Unknown);
        assert_eq!(registry.release("token-b"), LeaseRelease::Removed);
        assert_eq!(registry.counts(), (0, 0, 0));
        assert_eq!(registry.release("token-b"), LeaseRelease::Unknown);
    }

    #[test]
    fn failed_first_watcher_start_does_not_publish_a_lease() {
        let mut registry = WatchRegistry::<u8>::default();
        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token".to_owned(),
                || Err("setup failed"),
            ),
            Err(LeaseAcquireError::Create("setup failed"))
        );
        assert_eq!(registry.counts(), (0, 0, 0));

        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token".to_owned(),
                || Ok::<_, &'static str>(9),
            ),
            Ok(LeaseAcquire::Created)
        );
        assert_eq!(registry.counts(), (1, 1, 1));
    }

    #[test]
    fn watcher_lease_overflow_is_rejected_without_mutation() {
        let mut registry = WatchRegistry::<u8>::default();
        registry
            .acquire_or_insert(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token-a".to_owned(),
                || Ok::<_, &'static str>(11),
            )
            .unwrap();
        registry.entries.get_mut("canonical").unwrap().leases = MAX_WATCH_LEASES;

        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "repo".to_owned(),
                "token-b".to_owned(),
                || Ok::<_, &'static str>(12),
            ),
            Err(LeaseAcquireError::LeaseLimit)
        );
        let entry = registry.entries.get("canonical").unwrap();
        assert_eq!(entry.leases, MAX_WATCH_LEASES);
        assert_eq!(entry._handle, 11);
    }

    #[test]
    fn canonical_watcher_shares_alias_routes_and_releases_them_exactly() {
        let mut registry = WatchRegistry::<u8>::default();
        registry
            .acquire_or_insert(
                "canonical".to_owned(),
                "alias-a".to_owned(),
                "token-a".to_owned(),
                || Ok::<_, &'static str>(1),
            )
            .unwrap();
        assert_eq!(
            registry.acquire_or_insert::<&'static str>(
                "canonical".to_owned(),
                "alias-b".to_owned(),
                "token-b".to_owned(),
                || panic!("alias must reuse the canonical watcher"),
            ),
            Ok(LeaseAcquire::Acquired(2))
        );
        let mut routes = registry.routes("canonical");
        routes.sort_unstable();
        assert_eq!(routes, ["alias-a", "alias-b"]);
        assert_eq!(registry.counts(), (1, 2, 2));

        assert_eq!(registry.release("token-a"), LeaseRelease::Retained(1));
        assert_eq!(registry.routes("canonical"), ["alias-b"]);
        assert_eq!(registry.release("token-b"), LeaseRelease::Removed);
        assert_eq!(registry.counts(), (0, 0, 0));
    }

    #[test]
    fn watcher_start_replay_is_idempotent_but_conflicting_token_reuse_is_rejected() {
        let mut registry = WatchRegistry::<u8>::default();
        registry
            .acquire_or_insert(
                "canonical".to_owned(),
                "route".to_owned(),
                "token".to_owned(),
                || Ok::<_, &'static str>(1),
            )
            .unwrap();

        assert_eq!(
            registry.acquire_or_insert::<&'static str>(
                "canonical".to_owned(),
                "route".to_owned(),
                "token".to_owned(),
                || panic!("a replay must not recreate its watcher"),
            ),
            Ok(LeaseAcquire::Existing)
        );
        assert_eq!(registry.counts(), (1, 1, 1));
        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "other-route".to_owned(),
                "token".to_owned(),
                || Ok::<_, &'static str>(2),
            ),
            Err(LeaseAcquireError::TokenConflict)
        );
        assert_eq!(
            registry.acquire_or_insert(
                "other-canonical".to_owned(),
                "route".to_owned(),
                "token".to_owned(),
                || Ok::<_, &'static str>(3),
            ),
            Err(LeaseAcquireError::TokenConflict)
        );
        assert_eq!(registry.counts(), (1, 1, 1));
    }

    #[test]
    fn watcher_registry_bounds_cardinality_and_rejects_retargeted_aliases() {
        let mut registry = WatchRegistry::<usize>::default();
        registry
            .acquire_or_insert(
                "canonical-0".to_owned(),
                "alias-0".to_owned(),
                "token-0".to_owned(),
                || Ok::<_, &'static str>(0),
            )
            .unwrap();
        assert_eq!(
            registry.acquire_or_insert(
                "canonical-retargeted".to_owned(),
                "alias-0".to_owned(),
                "retarget-token".to_owned(),
                || Ok::<_, &'static str>(usize::MAX),
            ),
            Err(LeaseAcquireError::AliasConflict)
        );

        for index in 1..MAX_ACTIVE_REPO_WATCHERS {
            registry
                .acquire_or_insert(
                    format!("canonical-{index}"),
                    format!("alias-{index}"),
                    format!("token-{index}"),
                    || Ok::<_, &'static str>(index),
                )
                .unwrap();
        }
        assert_eq!(registry.counts().0, MAX_ACTIVE_REPO_WATCHERS);
        assert_eq!(
            registry.acquire_or_insert(
                "one-too-many".to_owned(),
                "one-too-many".to_owned(),
                "one-too-many".to_owned(),
                || Ok::<_, &'static str>(usize::MAX),
            ),
            Err(LeaseAcquireError::WatcherLimit)
        );
        assert_eq!(registry.counts().0, MAX_ACTIVE_REPO_WATCHERS);
        assert!(!registry.route_keys.contains_key("one-too-many"));
    }

    #[test]
    fn watcher_registry_bounds_alias_fanout() {
        let mut registry = WatchRegistry::<u8>::default();
        for index in 0..MAX_REPO_ALIASES_PER_WATCHER {
            registry
                .acquire_or_insert(
                    "canonical".to_owned(),
                    format!("alias-{index}"),
                    format!("token-{index}"),
                    || Ok::<_, &'static str>(1),
                )
                .unwrap();
        }
        assert_eq!(
            registry.routes("canonical").len(),
            MAX_REPO_ALIASES_PER_WATCHER
        );
        assert_eq!(
            registry.acquire_or_insert(
                "canonical".to_owned(),
                "one-too-many".to_owned(),
                "one-too-many".to_owned(),
                || Ok::<_, &'static str>(2),
            ),
            Err(LeaseAcquireError::AliasLimit)
        );
        assert!(!registry.route_keys.contains_key("one-too-many"));
    }

    #[test]
    fn watcher_registry_bounds_the_global_token_index() {
        let mut registry = WatchRegistry::<usize>::default();
        for index in 0..MAX_TOTAL_WATCH_LEASES {
            let repo = index / usize::from(MAX_WATCH_LEASES);
            registry
                .acquire_or_insert(
                    format!("canonical-{repo}"),
                    format!("route-{repo}"),
                    format!("token-{index}"),
                    || Ok::<_, &'static str>(repo),
                )
                .unwrap();
        }
        assert_eq!(registry.counts(), (4, MAX_TOTAL_WATCH_LEASES as u64, 4));
        assert_eq!(
            registry.acquire_or_insert(
                "overflow".to_owned(),
                "overflow".to_owned(),
                "overflow".to_owned(),
                || Ok::<_, &'static str>(usize::MAX),
            ),
            Err(LeaseAcquireError::TotalLeaseLimit)
        );
        assert_eq!(registry.counts(), (4, MAX_TOTAL_WATCH_LEASES as u64, 4));
    }

    #[test]
    fn watch_tokens_require_canonical_lowercase_uuid_v4() {
        let valid = "d9428888-122b-4a87-a4ca-2b87c5d47f25";
        assert!(valid_watch_token(valid));
        assert!(!valid_watch_token(&valid.to_uppercase()));
        assert!(!valid_watch_token("d9428888-122b-1a87-a4ca-2b87c5d47f25"));
        assert!(!valid_watch_token("not-a-token"));
        assert!(!valid_watch_token(&"x".repeat(WATCH_TOKEN_BYTES + 1)));
    }

    #[test]
    fn watcher_deactivation_serializes_with_an_inflight_update() {
        let lifetime = Arc::new(WatchLifetime::new());
        let update = lifetime.update_lock.lock().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let stopping_lifetime = lifetime.clone();
        let stopping = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            stopping_lifetime.deactivate();
            done_tx.send(()).unwrap();
        });

        entered_rx.recv().unwrap();
        assert!(matches!(
            done_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        drop(update);
        done_rx.recv().unwrap();
        stopping.join().unwrap();

        assert!(!lifetime.apply_if_active(|| {
            panic!("a stopped watcher must never mutate the replacement cache")
        }));
    }

    #[test]
    fn paired_rename_preserves_source_remove_and_target_reconcile() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("before.txt");
        let target = temp.path().join("after.txt");
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(source.clone())
            .add_path(target.clone());

        let Some(WatchMessage::Changes(changes)) = changes_for_event(temp.path(), event) else {
            panic!("paired rename should produce incremental changes");
        };
        assert_eq!(changes.len(), 2);
        assert!(matches!(
            &changes[0],
            WatcherChange::Remove(path) if path == &source
        ));
        assert!(matches!(
            &changes[1],
            WatcherChange::Reconcile(path) if path == &target
        ));
    }

    #[test]
    fn denied_subtrees_are_ignored_but_git_signals_are_retained() {
        let temp = tempfile::tempdir().unwrap();
        let denied_root = temp.path().join("target");
        let denied_root_event =
            Event::new(EventKind::Create(notify::event::CreateKind::Folder)).add_path(denied_root);
        assert!(matches!(
            changes_for_event(temp.path(), denied_root_event),
            Some(WatchMessage::Changes(_))
        ));

        let denied = temp.path().join("target/debug/output");
        let denied_event =
            Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(denied);
        assert!(changes_for_event(temp.path(), denied_event).is_none());

        let git_index = temp.path().join(".git/index");
        let git_event = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(git_index.clone());
        let Some(WatchMessage::Changes(changes)) = changes_for_event(temp.path(), git_event) else {
            panic!("git index should remain a git refresh signal");
        };
        assert!(matches!(
            &changes[0],
            WatcherChange::Reconcile(path) if path == &git_index
        ));
    }

    #[test]
    fn notify_rescan_flag_forces_a_complete_rescan() {
        let temp = tempfile::tempdir().unwrap();
        let event = Event::new(EventKind::Any).set_flag(Flag::Rescan);
        assert!(matches!(
            changes_for_event(temp.path(), event),
            Some(WatchMessage::Rescan)
        ));
    }

    #[test]
    fn oversized_debounce_batch_collapses_to_one_rescan() {
        let incoming = (0..=MAX_BATCH_CHANGES)
            .map(|index| WatcherChange::Remove(PathBuf::from(format!("file-{index}"))))
            .collect();
        let mut changes = Vec::new();
        let mut force_rescan = false;

        merge_message(
            WatchMessage::Changes(incoming),
            &mut changes,
            &mut force_rescan,
        );

        assert!(force_rescan);
        assert!(changes.is_empty());
    }
}
