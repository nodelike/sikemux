//! Watches the folders of the files agents share, so a change no agent's
//! edit tool reported, by the person, a command or an agent in a terminal,
//! is seen as it happens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Weak};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::server::Core;

#[derive(Default)]
pub(super) struct Watch(Mutex<Option<Folders>>);

struct Folders {
    watcher: RecommendedWatcher,
    /// How many followed files each watched folder holds.
    files: HashMap<PathBuf, usize>,
}

impl Watch {
    fn lock(&self) -> MutexGuard<'_, Option<Folders>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Watches the folder `path` is in, for as long as it is followed.
    pub(super) fn follow(&self, core: &Weak<Core>, path: &Path) {
        let Some(folder) = path.parent() else {
            return;
        };
        let mut folders = self.lock();
        if folders.is_none() {
            let core = core.clone();
            let started =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let Ok(event) = event else { return };
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }
                    let Some(core) = core.upgrade() else { return };
                    for path in &event.paths {
                        core.collisions.disk_changed(path);
                    }
                });
            match started {
                Ok(watcher) => {
                    *folders = Some(Folders {
                        watcher,
                        files: HashMap::new(),
                    });
                }
                Err(error) => {
                    eprintln!("sikemux core: changes outside agents' edits go unseen: {error}");
                    return;
                }
            }
        }
        let Some(folders) = folders.as_mut() else {
            return;
        };
        let count = folders.files.entry(folder.to_path_buf()).or_default();
        if *count == 0
            && folders
                .watcher
                .watch(folder, RecursiveMode::NonRecursive)
                .is_err()
        {
            folders.files.remove(folder);
            return;
        }
        *count += 1;
    }

    pub(super) fn unfollow(&self, path: &Path) {
        let Some(folder) = path.parent() else {
            return;
        };
        let mut folders = self.lock();
        let Some(folders) = folders.as_mut() else {
            return;
        };
        let Some(count) = folders.files.get_mut(folder) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            folders.files.remove(folder);
            let _ = folders.watcher.unwatch(folder);
        }
    }
}
