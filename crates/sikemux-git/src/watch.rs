//! Watches each project's tree, so every client hears when its files or its
//! git state move, and the walks [`super::status`] and [`super::log`] keep of
//! it stay good until then.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

#[cfg(any(not(target_os = "macos"), test))]
use notify::{Event, EventKind};
#[cfg(not(target_os = "macos"))]
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sikemux_process::sync::Locked;

/// The longest repository path a client may name.
pub const MAX_REPO_PATH_BYTES: usize = 4 * 1024;
const MAX_WATCHED_REPOS: usize = 64;

/// What a change to a repository's own state, under `.git`, is listed as
/// among the paths that moved.
pub const GIT_STATE: &str = ".git";
/// Stands for the repository's `.git/config`, whose remotes say what the project is known by.
pub const GIT_CONFIG: &str = ".git/config";

/// The one name a repository goes by however it is reached, so symlinks, `.`
/// components and trailing separators never watch or cache it twice.
pub fn repo_key(repo: &str) -> Result<String, String> {
    if repo.is_empty()
        || repo.len() > MAX_REPO_PATH_BYTES
        || repo.contains('\0')
        || !Path::new(repo).is_absolute()
    {
        return Err(format!(
            "repository path must be an absolute path of at most {MAX_REPO_PATH_BYTES} bytes"
        ));
    }
    let canonical = sikemux_process::files::real_path(repo)
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

/// Folders of build output, dependencies and caches, whose churn nobody wants
/// to hear of and which the file palette never lists.
pub fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        // dependency / package managers
        "node_modules" | "bower_components" | "vendor"
        | ".pnpm-store" | ".yarn" | ".npm" | ".pnpm" | ".bun" | ".deno"
        // VCS
        | ".git" | ".hg" | ".svn"
        // build outputs (js / rust / swift / generic)
        | "target" | "dist" | "build" | ".build" | "out" | "_build"
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

/// Each watched repo's latest change number, and for its last few changes the
/// paths each one moved, none when it could not say. A repo with no live
/// watcher is absent: nothing would tell us its tree moved, so nothing may
/// cache a walk of it. Numbers are never reused, even across watchers.
#[derive(Default)]
struct Generations {
    last: u64,
    repos: HashMap<String, RepoChanges>,
}

#[derive(Default)]
struct RepoChanges {
    generation: u64,
    recent: VecDeque<(u64, Option<Arc<[String]>>)>,
    /// The newest change no longer in `recent`.
    forgotten: u64,
}

const REMEMBERED_CHANGES: usize = 32;

fn generations() -> MutexGuard<'static, Generations> {
    static G: OnceLock<Mutex<Generations>> = OnceLock::new();
    G.get_or_init(Mutex::default).locked()
}

pub fn scan_generation(repo_key: &str) -> Option<u64> {
    generations()
        .repos
        .get(repo_key)
        .map(|repo| repo.generation)
}

/// Every path the watcher saw move after change `since`, up to the latest;
/// none when one of those changes could not say what it moved or is no longer
/// remembered.
pub fn changed_paths_since(repo_key: &str, since: u64) -> Option<Vec<String>> {
    let generations = generations();
    let repo = generations.repos.get(repo_key)?;
    if since < repo.forgotten || since > repo.generation {
        return None;
    }
    let mut paths: Vec<String> = Vec::new();
    for (_, changed) in repo.recent.iter().filter(|(at, _)| *at > since) {
        paths.extend(changed.as_ref()?.iter().cloned());
    }
    paths.sort_unstable();
    paths.dedup();
    Some(paths)
}

fn bump(repo_key: &str, paths: Option<&[String]>) {
    let mut generations = generations();
    generations.last += 1;
    let generation = generations.last;
    let repo = generations.repos.entry(repo_key.to_string()).or_default();
    repo.generation = generation;
    if repo.recent.len() == REMEMBERED_CHANGES {
        if let Some((at, _)) = repo.recent.pop_front() {
            repo.forgotten = at;
        }
    }
    repo.recent.push_back((generation, paths.map(Arc::from)));
}

/// A change the core made itself, counted now so the next read sees it
/// rather than waiting for the watcher to report it.
pub fn note_own_change(repo_key: &str, paths: Option<&[String]>) {
    if scan_generation(repo_key).is_some() {
        bump(repo_key, paths);
    }
}

/// Tells the projects at one watched folder, by the names they were followed
/// under, what moved inside it: the paths, `.git` among them for its git
/// state, or none for anything.
pub type Changed = Arc<dyn Fn(&[String], Option<Vec<String>>) + Send + Sync>;

/// The repositories watched, by [`repo_key`].
#[derive(Default)]
pub struct Watches(Mutex<HashMap<String, Watched>>);

struct Watched {
    #[cfg(target_os = "macos")]
    _watcher: fsevent::MacWatcher,
    #[cfg(not(target_os = "macos"))]
    _watcher: RecommendedWatcher,
    projects: Arc<Mutex<Vec<String>>>,
    live: Arc<AtomicBool>,
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

impl Watches {
    /// Watches exactly the folders `projects` names once it holds the watch,
    /// telling `changed` as they move.
    pub fn follow(&self, projects: impl FnOnce() -> Vec<PathBuf>, changed: &Changed) {
        let mut watched = self.0.locked();
        let mut wanted: HashMap<String, BTreeSet<String>> = HashMap::new();
        for project in projects() {
            let Some(path) = project.to_str() else {
                continue;
            };
            if let Ok(key) = repo_key(path) {
                wanted.entry(key).or_default().insert(path.to_owned());
            }
        }
        let dropped: Vec<String> = watched
            .keys()
            .filter(|key| !wanted.contains_key(*key))
            .cloned()
            .collect();
        for key in dropped {
            watched.remove(&key);
            generations().repos.remove(&key);
        }
        for (key, names) in wanted {
            let names: Vec<String> = names.into_iter().collect();
            if let Some(repo) = watched.get(&key) {
                *repo.projects.locked() = names;
                continue;
            }
            if watched.len() >= MAX_WATCHED_REPOS {
                continue;
            }
            match start(&key, names.clone(), changed.clone()) {
                Ok(repo) => {
                    watched.insert(key.clone(), repo);
                    bump(&key, None);
                    changed(&names, None);
                }
                Err(error) => eprintln!("sikemux core: {key} goes unwatched: {error}"),
            }
        }
    }
}

// fsevents on macOS fires bursts for a single save.
const DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_DEBOUNCE: Duration = Duration::from_secs(1);
const EVENT_CHANNEL_CAPACITY: usize = 1_024;
const MAX_BATCH_CHANGES: usize = 512;

enum WatchMessage {
    Changes(Vec<PathBuf>),
    Rescan,
}

fn start(key: &str, projects: Vec<String>, changed: Changed) -> Result<Watched, String> {
    let (tx, rx) = tokio::sync::mpsc::channel::<WatchMessage>(EVENT_CHANNEL_CAPACITY);
    let rescan_requested = Arc::new(AtomicBool::new(false));
    let root = PathBuf::from(key);
    #[cfg(target_os = "macos")]
    let watcher = fsevent::MacWatcher::start(root.clone(), tx, rescan_requested.clone())?;
    #[cfg(not(target_os = "macos"))]
    let watcher = {
        let callback_root = root.clone();
        let callback_rescan = rescan_requested.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let message = match res {
                Ok(event) => changes_for_event(&callback_root, event),
                Err(_) => Some(WatchMessage::Rescan),
            };
            let Some(message) = message else { return };
            if matches!(
                tx.try_send(message),
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
            ) {
                // A dropped path makes the whole batch untrustworthy; the queued
                // wakeup turns this into one complete rescan.
                callback_rescan.store(true, Ordering::Release);
            }
        })
        .map_err(|error| error.to_string())?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| error.to_string())?;
        watcher
    };
    let projects = Arc::new(Mutex::new(projects));
    let live = Arc::new(AtomicBool::new(true));
    tokio::spawn(debounce(
        root,
        rx,
        rescan_requested,
        projects.clone(),
        live.clone(),
        changed,
    ));
    Ok(Watched {
        _watcher: watcher,
        projects,
        live,
    })
}

async fn debounce(
    root: PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<WatchMessage>,
    rescan_requested: Arc<AtomicBool>,
    projects: Arc<Mutex<Vec<String>>>,
    live: Arc<AtomicBool>,
    changed: Changed,
) {
    let key = root.to_string_lossy().into_owned();
    while let Some(first) = rx.recv().await {
        let mut changes = Vec::new();
        let mut rescan = rescan_requested.swap(false, Ordering::AcqRel);
        merge_message(first, &mut changes, &mut rescan);
        let quiet = tokio::time::sleep(DEBOUNCE);
        tokio::pin!(quiet);
        let longest = tokio::time::sleep(MAX_DEBOUNCE);
        tokio::pin!(longest);
        let mut closed = false;
        loop {
            tokio::select! {
                _ = &mut quiet => break,
                _ = &mut longest => break,
                message = rx.recv() => match message {
                    Some(message) => {
                        merge_message(message, &mut changes, &mut rescan);
                        quiet.as_mut().reset(tokio::time::Instant::now() + DEBOUNCE);
                    }
                    None => {
                        closed = true;
                        break;
                    }
                },
            }
        }
        rescan |= rescan_requested.swap(false, Ordering::AcqRel);
        if !live.load(Ordering::Acquire) {
            return;
        }
        let paths = moved_paths(&root, &changes, rescan);
        // Counted before anyone hears of it, so the read it prompts never
        // gets a walk from before the change.
        bump(&key, paths.as_deref());
        let names = projects.locked().clone();
        changed(&names, paths);
        if closed {
            return;
        }
    }
}

fn merge_message(message: WatchMessage, changes: &mut Vec<PathBuf>, rescan: &mut bool) {
    match message {
        WatchMessage::Rescan => {
            *rescan = true;
            changes.clear();
        }
        WatchMessage::Changes(mut incoming) if !*rescan => {
            if changes.len().saturating_add(incoming.len()) > MAX_BATCH_CHANGES {
                *rescan = true;
                changes.clear();
            } else {
                changes.append(&mut incoming);
            }
        }
        WatchMessage::Changes(_) => {}
    }
}

/// The paths below `repo` that moved, with [`GIT_STATE`] standing for any
/// under `.git`; none when the batch cannot say.
fn moved_paths(repo: &Path, changes: &[PathBuf], rescan: bool) -> Option<Vec<String>> {
    if rescan {
        return None;
    }
    let mut paths = Vec::with_capacity(changes.len());
    for path in changes {
        if path.ends_with(".git/config") {
            paths.push(GIT_CONFIG.to_owned());
            continue;
        }
        if is_git_signal(path) {
            paths.push(GIT_STATE.to_owned());
            continue;
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

fn is_git_signal(path: &Path) -> bool {
    let mut after_git = false;
    for component in path.components() {
        // A name that is not UTF-8 cannot be one of the names below.
        let name = component.as_os_str().to_str().unwrap_or_default();
        if !after_git {
            if name == ".git" {
                after_git = true;
            }
            continue;
        }
        return matches!(
            name,
            "HEAD"
                | "config"
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

/// Build output, dependencies and the rest of `.git` would fire thousands of
/// events for every `cargo test` or `npm run build` nobody wants to hear of.
fn should_ignore(repo: &Path, path: &Path) -> bool {
    if is_git_signal(path) {
        return false;
    }
    let Ok(relative) = path.strip_prefix(repo) else {
        // Outside the root: the batch cannot say what moved, so it rescans.
        return false;
    };
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let name = component.as_os_str().to_str().unwrap_or_default();
        if name == ".DS_Store" {
            return true;
        }
        // The denied folder itself still counts: a file named `target` may
        // just have been replaced by one, and the file palette must drop it.
        if components.peek().is_some() && should_skip_dir(name) {
            return true;
        }
    }
    false
}

#[cfg(any(not(target_os = "macos"), test))]
fn changes_for_event(repo: &Path, event: Event) -> Option<WatchMessage> {
    if event.need_rescan() || event.paths.len() > MAX_BATCH_CHANGES {
        return Some(WatchMessage::Rescan);
    }
    match event.kind {
        EventKind::Access(_) => return None,
        EventKind::Any | EventKind::Other => return Some(WatchMessage::Rescan),
        _ => {}
    }
    // An event that names nothing cannot say what moved.
    if event.paths.is_empty() {
        return Some(WatchMessage::Rescan);
    }
    let changes: Vec<PathBuf> = event
        .paths
        .into_iter()
        .filter(|path| !should_ignore(repo, path))
        .collect();
    (!changes.is_empty()).then_some(WatchMessage::Changes(changes))
}

#[cfg(target_os = "macos")]
mod fsevent {
    use std::collections::VecDeque;
    use std::ffi::{CStr, CString, OsStr};
    use std::os::raw::{c_char, c_long, c_uchar, c_uint, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::{should_ignore, should_skip_dir, WatchMessage, MAX_BATCH_CHANGES};

    type CFRef = *mut c_void;
    type CFIndex = c_long;
    type CFTimeInterval = f64;
    type Boolean = c_uchar;
    type FSEventStreamRef = *mut c_void;
    type FSEventStreamEventId = u64;
    type FSEventStreamCreateFlags = c_uint;
    type FSEventStreamEventFlags = c_uint;
    type DispatchQueue = *mut c_void;

    type FSEventStreamCallback = extern "C" fn(
        FSEventStreamRef,
        *mut c_void,
        libc::size_t,
        *mut c_void,
        *const FSEventStreamEventFlags,
        *const FSEventStreamEventId,
    );

    #[repr(C)]
    struct FSEventStreamContext {
        version: CFIndex,
        info: *mut c_void,
        retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
        release: Option<extern "C" fn(*const c_void)>,
        copy_description: Option<extern "C" fn(*const c_void) -> CFRef>,
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW: FSEventStreamEventId = u64::MAX;
    const K_FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER: FSEventStreamCreateFlags = 0x0000_0002;
    const K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS: FSEventStreamCreateFlags = 0x0000_0010;
    const K_FS_EVENT_STREAM_EVENT_FLAG_MUST_SCAN_SUB_DIRS: FSEventStreamEventFlags = 0x0000_0001;
    const K_FS_EVENT_STREAM_EVENT_FLAG_HISTORY_DONE: FSEventStreamEventFlags = 0x0000_0010;
    const K_FS_EVENT_STREAM_EVENT_FLAG_ROOT_CHANGED: FSEventStreamEventFlags = 0x0000_0020;
    const K_FS_EVENT_STREAM_EVENT_FLAG_MOUNT: FSEventStreamEventFlags = 0x0000_0040;
    const K_FS_EVENT_STREAM_EVENT_FLAG_UNMOUNT: FSEventStreamEventFlags = 0x0000_0080;
    const RESCAN_FLAGS: FSEventStreamEventFlags = K_FS_EVENT_STREAM_EVENT_FLAG_MUST_SCAN_SUB_DIRS
        | K_FS_EVENT_STREAM_EVENT_FLAG_ROOT_CHANGED
        | K_FS_EVENT_STREAM_EVENT_FLAG_MOUNT
        | K_FS_EVENT_STREAM_EVENT_FLAG_UNMOUNT;

    // macOS caps FSEventStreamSetExclusionPaths at 8 paths per stream.
    const MAX_EXCLUSION_PATHS: usize = 8;
    const STREAM_LATENCY_SECS: CFTimeInterval = 0.05;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeArrayCallBacks: c_void;
        fn CFRelease(cf: CFRef);
        fn CFStringCreateWithBytes(
            alloc: CFRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: Boolean,
        ) -> CFRef;
        fn CFArrayCreate(
            alloc: CFRef,
            values: *const CFRef,
            num_values: CFIndex,
            call_backs: *const c_void,
        ) -> CFRef;
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn FSEventStreamCreate(
            allocator: CFRef,
            callback: FSEventStreamCallback,
            context: *const FSEventStreamContext,
            paths_to_watch: CFRef,
            since_when: FSEventStreamEventId,
            latency: CFTimeInterval,
            flags: FSEventStreamCreateFlags,
        ) -> FSEventStreamRef;
        fn FSEventStreamSetExclusionPaths(
            stream_ref: FSEventStreamRef,
            paths_to_exclude: CFRef,
        ) -> Boolean;
        fn FSEventStreamSetDispatchQueue(stream_ref: FSEventStreamRef, q: DispatchQueue);
        fn FSEventStreamStart(stream_ref: FSEventStreamRef) -> Boolean;
        fn FSEventStreamStop(stream_ref: FSEventStreamRef);
        fn FSEventStreamInvalidate(stream_ref: FSEventStreamRef);
        fn FSEventStreamRelease(stream_ref: FSEventStreamRef);
    }

    extern "C" {
        fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> DispatchQueue;
        fn dispatch_release(object: DispatchQueue);
        fn dispatch_sync_f(
            queue: DispatchQueue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    struct StreamContext {
        root: PathBuf,
        tx: tokio::sync::mpsc::Sender<WatchMessage>,
        rescan: Arc<AtomicBool>,
    }

    pub struct MacWatcher {
        stream: FSEventStreamRef,
        queue: DispatchQueue,
    }

    // SAFETY: stream and queue pointers are owned exclusively by MacWatcher and only touched on Drop.
    unsafe impl Send for MacWatcher {}
    // SAFETY: MacWatcher exposes no shared interior mutation; only Drop touches its handles.
    unsafe impl Sync for MacWatcher {}

    impl MacWatcher {
        pub fn start(
            root: PathBuf,
            tx: tokio::sync::mpsc::Sender<WatchMessage>,
            rescan: Arc<AtomicBool>,
        ) -> Result<Self, String> {
            let label = CString::new("com.nodelike.sikemux.git-watch")
                .map_err(|error| error.to_string())?;
            // SAFETY: label is a valid NUL-terminated C string; null attr creates a serial queue.
            let queue = unsafe { dispatch_queue_create(label.as_ptr(), ptr::null()) };
            if queue.is_null() {
                return Err("could not create FSEvents dispatch queue".into());
            }
            let watch_paths = cf_path_array(std::slice::from_ref(&root)).ok_or_else(|| {
                // SAFETY: queue was just created and is non-null.
                unsafe { dispatch_release(queue) };
                "could not encode watched path for FSEvents".to_owned()
            })?;
            let context_ptr = Box::into_raw(Box::new(StreamContext {
                root: root.clone(),
                tx,
                rescan,
            }));
            let stream_context = FSEventStreamContext {
                version: 0,
                info: context_ptr.cast(),
                retain: None,
                release: Some(release_context),
                copy_description: None,
            };
            let flags =
                K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS | K_FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER;
            // SAFETY: watch_paths is a valid CFArray of CFStrings and release_context frees context_ptr on release.
            let stream = unsafe {
                FSEventStreamCreate(
                    ptr::null_mut(),
                    callback,
                    &stream_context,
                    watch_paths,
                    K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW,
                    STREAM_LATENCY_SECS,
                    flags,
                )
            };
            // SAFETY: FSEventStreamCreate copied watch_paths; release our reference.
            unsafe { CFRelease(watch_paths) };
            if stream.is_null() {
                // SAFETY: stream creation failed so CoreServices will not invoke release_context.
                unsafe {
                    drop(Box::from_raw(context_ptr));
                    dispatch_release(queue);
                }
                return Err("could not create FSEvents stream".into());
            }
            let excluded = exclusion_paths(&root);
            if let Some(cf_excluded) = cf_path_array(&excluded) {
                // SAFETY: stream and cf_excluded are valid non-null CoreFoundation handles.
                unsafe {
                    FSEventStreamSetExclusionPaths(stream, cf_excluded);
                    CFRelease(cf_excluded);
                }
            }
            // SAFETY: stream and queue are valid; on start failure both are torn down immediately.
            unsafe {
                FSEventStreamSetDispatchQueue(stream, queue);
                if FSEventStreamStart(stream) == 0 {
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                    dispatch_release(queue);
                    return Err("could not start FSEvents stream".into());
                }
            }
            Ok(Self { stream, queue })
        }
    }

    impl Drop for MacWatcher {
        fn drop(&mut self) {
            // SAFETY: stop and invalidate detach callbacks, dispatch_sync_f drains any in-flight callback, then handles are released.
            unsafe {
                FSEventStreamStop(self.stream);
                FSEventStreamInvalidate(self.stream);
                dispatch_sync_f(self.queue, ptr::null_mut(), noop);
                FSEventStreamRelease(self.stream);
                dispatch_release(self.queue);
            }
        }
    }

    extern "C" fn noop(_: *mut c_void) {}

    extern "C" fn release_context(info: *const c_void) {
        if !info.is_null() {
            // SAFETY: info came from Box::into_raw in MacWatcher::start and is released once by CoreServices.
            unsafe {
                drop(Box::from_raw(info as *mut StreamContext));
            }
        }
    }

    extern "C" fn callback(
        _stream_ref: FSEventStreamRef,
        info: *mut c_void,
        num_events: libc::size_t,
        event_paths: *mut c_void,
        event_flags: *const FSEventStreamEventFlags,
        _event_ids: *const FSEventStreamEventId,
    ) {
        if info.is_null() || event_paths.is_null() || event_flags.is_null() {
            return;
        }
        // SAFETY: info points to our live StreamContext for the duration of the stream.
        let ctx = unsafe { &*(info as *const StreamContext) };
        let paths = event_paths as *const *const c_char;
        let mut changes = Vec::new();
        for index in 0..num_events {
            // SAFETY: FSEvents guarantees event_flags and event_paths each hold num_events valid entries.
            let (flag, raw) = unsafe { (*event_flags.add(index), *paths.add(index)) };
            if flag & K_FS_EVENT_STREAM_EVENT_FLAG_HISTORY_DONE != 0 {
                continue;
            }
            if flag & RESCAN_FLAGS != 0 {
                send(ctx, WatchMessage::Rescan);
                return;
            }
            if raw.is_null() {
                continue;
            }
            // SAFETY: each non-null FSEvents path entry is a NUL-terminated C string valid for this callback.
            let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes();
            let path = PathBuf::from(OsStr::from_bytes(bytes));
            if !should_ignore(&ctx.root, &path) {
                changes.push(path);
                if changes.len() > MAX_BATCH_CHANGES {
                    send(ctx, WatchMessage::Rescan);
                    return;
                }
            }
        }
        if !changes.is_empty() {
            send(ctx, WatchMessage::Changes(changes));
        }
    }

    fn send(ctx: &StreamContext, message: WatchMessage) {
        if matches!(
            ctx.tx.try_send(message),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ) {
            ctx.rescan.store(true, Ordering::Release);
        }
    }

    fn cf_path_array(paths: &[PathBuf]) -> Option<CFRef> {
        let mut cf_strings = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = path.as_os_str().as_bytes();
            // SAFETY: bytes is a valid slice of UTF-8 path bytes for the duration of the call.
            let cf = unsafe {
                CFStringCreateWithBytes(
                    ptr::null_mut(),
                    bytes.as_ptr(),
                    bytes.len() as CFIndex,
                    K_CF_STRING_ENCODING_UTF8,
                    0,
                )
            };
            if cf.is_null() {
                for created in cf_strings {
                    // SAFETY: each element in cf_strings is a non-null CFString we created.
                    unsafe { CFRelease(created) };
                }
                return None;
            }
            cf_strings.push(cf);
        }
        // SAFETY: cf_strings holds valid CFStringRefs and kCFTypeArrayCallBacks retains them in the new array.
        let array = unsafe {
            CFArrayCreate(
                ptr::null_mut(),
                cf_strings.as_ptr(),
                cf_strings.len() as CFIndex,
                &raw const kCFTypeArrayCallBacks,
            )
        };
        for created in cf_strings {
            // SAFETY: CFArrayCreate retained the strings (or failed); release our temporary references.
            unsafe { CFRelease(created) };
        }
        (!array.is_null()).then_some(array)
    }

    pub(super) fn exclusion_paths(root: &Path) -> Vec<PathBuf> {
        let mut excluded = vec![
            root.join("target"),
            root.join("node_modules"),
            root.join(".build"),
            root.join(".git/worktrees"),
        ];
        const NESTED_EXCLUDED: [&str; 5] =
            ["node_modules", ".build", "target", "DerivedData", ".gradle"];
        let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
        while let Some((dir, depth)) = queue.pop_front() {
            if excluded.len() >= MAX_EXCLUSION_PATHS || depth > 2 {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<PathBuf> = entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect();
            children.sort();
            for child in children {
                let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if depth > 0 && NESTED_EXCLUDED.contains(&name) {
                    if !excluded.contains(&child) {
                        excluded.push(child);
                        if excluded.len() >= MAX_EXCLUSION_PATHS {
                            break;
                        }
                    }
                } else if depth < 2 && !should_skip_dir(name) && !name.starts_with('.') {
                    queue.push_back((child, depth + 1));
                }
            }
        }
        excluded.truncate(MAX_EXCLUSION_PATHS);
        excluded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, Flag, ModifyKind};

    #[test]
    fn repo_keys_collapse_aliases_and_bound_input() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = repo_key(temp.path().to_string_lossy().as_ref()).unwrap();
        let dotted = temp.path().join(".");
        assert_eq!(
            repo_key(dotted.to_string_lossy().as_ref()).unwrap(),
            canonical
        );
        assert!(repo_key("relative/repo").is_err());
        assert!(repo_key(&format!("/{}", "x".repeat(MAX_REPO_PATH_BYTES))).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let parent = tempfile::tempdir().unwrap();
            let alias = parent.path().join("repo-alias");
            symlink(temp.path(), &alias).unwrap();
            assert_eq!(
                repo_key(alias.to_string_lossy().as_ref()).unwrap(),
                canonical
            );
        }
    }

    #[test]
    fn moved_paths_are_relative_and_git_state_is_one_entry() {
        let repo = Path::new("/repo");
        let changes = vec![
            repo.join("src/old.rs"),
            repo.join("src/new.rs"),
            repo.join("src/new.rs"),
            repo.join(".git/index"),
            repo.join(".git/refs/heads/main"),
        ];
        assert_eq!(
            moved_paths(repo, &changes, false),
            Some(vec![
                ".git".into(),
                "src/new.rs".into(),
                "src/old.rs".into()
            ])
        );
        assert_eq!(moved_paths(repo, &changes, true), None);
        for path in [repo.to_path_buf(), PathBuf::from("/outside/file")] {
            assert_eq!(moved_paths(repo, &[path], false), None);
        }
    }

    #[test]
    fn denied_subtrees_are_ignored_but_git_signals_are_retained() {
        let temp = tempfile::tempdir().unwrap();
        let denied_root =
            Event::new(EventKind::Create(CreateKind::Folder)).add_path(temp.path().join("target"));
        assert!(matches!(
            changes_for_event(temp.path(), denied_root),
            Some(WatchMessage::Changes(_))
        ));

        let denied = Event::new(EventKind::Create(CreateKind::File))
            .add_path(temp.path().join("target/debug/output"));
        assert!(changes_for_event(temp.path(), denied).is_none());

        let git_index = temp.path().join(".git/index");
        let git_event = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(git_index.clone());
        let Some(WatchMessage::Changes(changes)) = changes_for_event(temp.path(), git_event) else {
            panic!("git index should remain a git refresh signal");
        };
        assert_eq!(changes, vec![git_index]);
    }

    #[test]
    fn notify_rescan_flag_forces_a_complete_rescan() {
        let temp = tempfile::tempdir().unwrap();
        let event = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(temp.path().join("named.txt"))
            .set_flag(Flag::Rescan);
        assert!(matches!(
            changes_for_event(temp.path(), event),
            Some(WatchMessage::Rescan)
        ));
    }

    #[test]
    fn oversized_debounce_batch_collapses_to_one_rescan() {
        let incoming = (0..=MAX_BATCH_CHANGES)
            .map(|index| PathBuf::from(format!("file-{index}")))
            .collect();
        let mut changes = Vec::new();
        let mut rescan = false;
        merge_message(WatchMessage::Changes(incoming), &mut changes, &mut rescan);
        assert!(rescan);
        assert!(changes.is_empty());
    }
}
