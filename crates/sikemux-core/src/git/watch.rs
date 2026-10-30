//! Watches each project's tree, so every client hears when its files or its
//! git state move, and the walks [`super::status`] and [`super::log`] keep of
//! it stay good until then.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sikemux_process::sync::Locked;

/// The longest repository path a client may name.
pub const MAX_REPO_PATH_BYTES: usize = 4 * 1024;
const MAX_WATCHED_REPOS: usize = 64;

/// What a change to a repository's own state, under `.git`, is listed as
/// among the paths that moved.
pub const GIT_STATE: &str = ".git";

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

fn start(key: &str, projects: Vec<String>, changed: Changed) -> notify::Result<Watched> {
    let (tx, rx) = tokio::sync::mpsc::channel::<WatchMessage>(EVENT_CHANNEL_CAPACITY);
    let rescan_requested = Arc::new(AtomicBool::new(false));
    let root = PathBuf::from(key);
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
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
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
        let event = Event::new(EventKind::Any).set_flag(Flag::Rescan);
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
