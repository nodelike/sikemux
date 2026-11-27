//! The files clients have open, which the core watches on their own, in build
//! and dependency folders as much as anywhere, and names in a `files` change
//! when one moves on disk, so an editor reads it again.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::serving::ServeParts;
use crate::server::watch_hub::WatchHandle;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::open_files::ShowFiles;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::RecursiveMode;
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
pub struct OpenFiles {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Each connection's open files, by the path the disk reports, to the
    /// path the client named.
    by_client: HashMap<ClientId, HashMap<PathBuf, String>>,
    /// The folders watched, which hold the open files.
    folders: HashSet<PathBuf>,
    watcher: Option<WatchHandle>,
}

/// The path the disk reports for `path`: its folder with links resolved.
fn on_disk(path: &Path) -> PathBuf {
    match (
        path.parent()
            .and_then(|folder| sikemux_process::files::real_path(folder).ok()),
        path.file_name(),
    ) {
        (Some(folder), Some(name)) => folder.join(name),
        _ => path.to_path_buf(),
    }
}

pub fn show(core: &Arc<Core>, client: ClientId, shown: ShowFiles) -> CoreResult<()> {
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
    let mut state = core.open_files().state.locked();
    if open.is_empty() {
        state.by_client.remove(&client);
    } else {
        state.by_client.insert(client, open);
    }
    rewatch(core, &mut state)
}

fn rewatch(core: &Arc<Core>, state: &mut State) -> CoreResult<()> {
    let wanted: HashSet<PathBuf> = state
        .by_client
        .values()
        .flat_map(HashMap::keys)
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    if wanted.is_empty() {
        state.watcher = None;
        state.folders.clear();
        return Ok(());
    }
    if state.watcher.is_none() {
        state.watcher = Some(start(core));
    }
    if state.folders == wanted {
        return Ok(());
    }
    let Some(watcher) = state.watcher.as_ref() else {
        return Ok(());
    };
    let watched = watcher.set_paths(
        wanted
            .into_iter()
            .map(|folder| (folder, RecursiveMode::NonRecursive)),
    );
    state.folders = watched.into_keys().collect();
    Ok(())
}

fn start(core: &Arc<Core>) -> WatchHandle {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<PathBuf>>(EVENTS_QUEUED);
    let watcher = core.watch_hub().subscribe(move |event| {
        let _ = tx.try_send(event.paths.clone());
    });
    let core = Arc::downgrade(core);
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
                let state = core.open_files().state.locked();
                state
                    .by_client
                    .values()
                    .flat_map(|open| open.iter())
                    .filter(|(path, _)| moved.contains(*path))
                    .map(|(_, named)| named.clone())
                    .collect()
            };
            if !named.is_empty() {
                core.changed(What::ReadIn(
                    Topic::Files,
                    None,
                    Some(named.into_iter().collect()),
                ));
            }
        }
    });
    watcher
}

impl super::part::Part for OpenFiles {
    fn client_gone(&self, core: &Arc<Core>, client: ClientId) {
        let mut state = self.state.locked();
        if state.by_client.remove(&client).is_some() {
            if let Err(error) = rewatch(core, &mut state) {
                eprintln!(
                    "sikemux core: the folders of open files were not watched again: {error}"
                );
            }
        }
    }
}
