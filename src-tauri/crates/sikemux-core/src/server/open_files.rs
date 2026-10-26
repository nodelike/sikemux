//! The files clients have open, which the core watches on their own, in build
//! and dependency folders as much as anywhere, and names in a `files` change
//! when one moves on disk, so an editor reads it again.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use sikemux_process::sync::Locked;

use super::changed::Topic;
use super::connection::ClientId;
use super::{Core, CoreError, CoreResult};

/// Files one connection may have open at once.
const OPEN_AT_MOST: usize = 256;
/// A save fires a burst of events; they are told as one.
const SETTLE: Duration = Duration::from_millis(100);
const EVENTS_QUEUED: usize = 1_024;

#[derive(Default)]
pub(crate) struct OpenFiles {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Each connection's open files, by the path the disk reports, to the
    /// path the client named.
    by_client: HashMap<ClientId, HashMap<PathBuf, String>>,
    /// The folders watched, which hold the open files.
    folders: HashSet<PathBuf>,
    watcher: Option<RecommendedWatcher>,
}

/// The files a connection has open now, by absolute path; none when it shows none.
#[derive(Deserialize)]
pub(crate) struct ShowFiles {
    paths: Vec<String>,
}

/// The path the disk reports for `path`: its folder with links resolved.
fn on_disk(path: &Path) -> PathBuf {
    match (
        path.parent()
            .and_then(|folder| sikemux_process::os::canonicalize(folder).ok()),
        path.file_name(),
    ) {
        (Some(folder), Some(name)) => folder.join(name),
        _ => path.to_path_buf(),
    }
}

pub(crate) fn show(core: &Arc<Core>, client: ClientId, shown: ShowFiles) -> CoreResult<()> {
    if shown.paths.len() > OPEN_AT_MOST {
        return Err(CoreError::from(format!(
            "a client may show at most {OPEN_AT_MOST} files"
        )));
    }
    let mut open = HashMap::new();
    for path in shown.paths {
        if !Path::new(&path).is_absolute() {
            return Err(CoreError::from(format!("{path} is not an absolute path")));
        }
        open.insert(on_disk(Path::new(&path)), path);
    }
    let connected: HashSet<ClientId> = core.clients().iter().map(|client| client.id).collect();
    let mut state = core.open_files.state.locked();
    state.by_client.retain(|id, _| connected.contains(id));
    if open.is_empty() {
        state.by_client.remove(&client);
    } else {
        state.by_client.insert(client, open);
    }
    rewatch(core, &mut state)
}

/// Watches exactly the folders that hold an open file.
fn rewatch(core: &Arc<Core>, state: &mut State) -> CoreResult<()> {
    let wanted: HashSet<PathBuf> = state
        .by_client
        .values()
        .flat_map(HashMap::keys)
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    if state.watcher.is_none() {
        if wanted.is_empty() {
            return Ok(());
        }
        state.watcher = Some(start(Arc::downgrade(core))?);
    }
    let Some(watcher) = state.watcher.as_mut() else {
        return Ok(());
    };
    for gone in state.folders.difference(&wanted) {
        let _ = watcher.unwatch(gone);
    }
    let mut folders = HashSet::new();
    for folder in wanted {
        if state.folders.contains(&folder)
            || watcher.watch(&folder, RecursiveMode::NonRecursive).is_ok()
        {
            folders.insert(folder);
        }
    }
    state.folders = folders;
    Ok(())
}

fn start(core: Weak<Core>) -> CoreResult<RecommendedWatcher> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<PathBuf>>(EVENTS_QUEUED);
    let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let _ = tx.try_send(event.paths);
    })
    .map_err(|error| CoreError::from(format!("the core cannot watch open files: {error}")))?;
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut moved: HashSet<PathBuf> = first.into_iter().collect();
            let settle = tokio::time::sleep(SETTLE);
            tokio::pin!(settle);
            loop {
                tokio::select! {
                    () = &mut settle => break,
                    more = rx.recv() => match more {
                        Some(more) => moved.extend(more),
                        None => break,
                    },
                }
            }
            let Some(core) = core.upgrade() else { return };
            let named: HashSet<String> = {
                let state = core.open_files.state.locked();
                state
                    .by_client
                    .values()
                    .flat_map(|open| open.iter())
                    .filter(|(path, _)| moved.contains(*path))
                    .map(|(_, named)| named.clone())
                    .collect()
            };
            if !named.is_empty() {
                core.changes
                    .changed(Topic::Files, None, Some(named.into_iter().collect()));
            }
        }
    });
    Ok(watcher)
}
