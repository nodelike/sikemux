//! A small record the core keeps in one file of the data folder, written
//! whole each time it changes, so it outlives a crash and a restart.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;

/// How long [`Kept::save_soon`] waits for more changes before writing.
const SETTLE: Duration = Duration::from_millis(250);
/// How many times a write that failed is tried again, a settle apart.
const RETRIES: usize = 5;

pub(crate) struct Kept {
    path: Option<PathBuf>,
    queue: Arc<Queue>,
}

#[derive(Default)]
struct Queue {
    state: Mutex<Pending>,
    changed: Condvar,
}

#[derive(Default)]
struct Pending {
    /// What was last handed to be saved.
    last: Vec<u8>,
    next: Option<Vec<u8>>,
    writing: bool,
    hurry: bool,
}

impl Kept {
    /// Without a data folder nothing is kept.
    pub(crate) fn new(data_dir: Option<&Path>, file: &str) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join(file)),
            queue: Arc::default(),
        }
    }

    /// What the file holds, or the default when there is none yet. A file
    /// that cannot be read is moved aside first, so the next save does not
    /// overwrite what it held.
    pub(crate) fn load<T: DeserializeOwned + Default>(&self) -> T {
        let Some(path) = &self.path else {
            return T::default();
        };
        let read = match std::fs::read(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return T::default(),
            read => read.map_err(|error| error.to_string()),
        };
        read.and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
            .unwrap_or_else(|error| {
                self.set_aside(&error);
                T::default()
            })
    }

    /// Moves a file that cannot be used to `<file>.unreadable-<when>`.
    pub(crate) fn set_aside(&self, why: &str) {
        let Some(path) = &self.path else {
            return;
        };
        let mut aside = path.clone().into_os_string();
        aside.push(format!(
            ".unreadable-{}",
            sikemux_process::clock::epoch_ms()
        ));
        let moved = std::fs::rename(path, &aside);
        eprintln!(
            "sikemux core: {} could not be read ({why}); {}",
            path.display(),
            match moved {
                Ok(()) => format!("it was kept as {}", Path::new(&aside).display()),
                Err(error) => format!("it could not be moved aside either: {error}"),
            }
        );
    }

    pub(crate) fn save<T: Serialize>(&self, value: &T) {
        let Some(path) = &self.path else {
            return;
        };
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                write(path, &bytes);
            }
            Err(error) => eprintln!("sikemux core: {} was not saved: {error}", path.display()),
        }
    }

    /// Saves on a thread of its own, so whoever changed the value never waits
    /// on the disk. Changes made while one is being written are saved together
    /// after it, only the latest of them. Answers false when `value` is what
    /// was last saved, which is then not written again.
    pub(crate) fn save_soon<T: Serialize>(&self, value: &T) -> bool {
        let Some(path) = &self.path else {
            return true;
        };
        let bytes = match serde_json::to_vec(value) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("sikemux core: {} was not saved: {error}", path.display());
                return true;
            }
        };
        let mut pending = self.queue.state.locked();
        if pending.last == bytes {
            return false;
        }
        pending.last.clone_from(&bytes);
        pending.next = Some(bytes);
        if pending.writing {
            return true;
        }
        pending.writing = true;
        drop(pending);
        let (target, queue) = (path.clone(), self.queue.clone());
        let spawned = std::thread::Builder::new()
            .name("sikemux-save".into())
            .spawn(move || queue.write_all(&target));
        if spawned.is_err() {
            self.queue.write_all(path);
        }
        true
    }

    /// Waits until every [`Kept::save_soon`] so far is on disk.
    pub(crate) fn flush(&self) {
        let mut pending = self.queue.state.locked();
        if !pending.writing {
            return;
        }
        pending.hurry = true;
        self.queue.changed.notify_all();
        drop(
            self.queue
                .changed
                .wait_while(pending, |pending| pending.writing)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

impl Queue {
    fn write_all(&self, path: &Path) {
        let mut pending = self.state.locked();
        let mut failed = 0;
        loop {
            if !pending.hurry {
                pending = self
                    .changed
                    .wait_timeout_while(pending, SETTLE, |pending| !pending.hurry)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            let Some(bytes) = pending.next.take() else {
                break;
            };
            drop(pending);
            let written = write(path, &bytes);
            pending = self.state.locked();
            if written {
                failed = 0;
            } else if pending.next.is_none() && failed < RETRIES {
                failed += 1;
                pending.next = Some(bytes);
            } else {
                pending.last.clear();
            }
        }
        pending.writing = false;
        pending.hurry = false;
        self.changed.notify_all();
    }
}

/// The file a record keyed by `key`, such as a project's path, is kept in
/// among others of its kind in one folder.
pub(crate) fn file_for(key: &str) -> String {
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    format!("{}.json", digest.get(..16).unwrap_or(&digest))
}

/// Every record kept in `folder`, one file each; a file that cannot be read
/// is moved aside.
pub(crate) fn load_all<T: DeserializeOwned>(folder: Option<&Path>) -> Vec<T> {
    let Some(entries) = folder.and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|entry| {
            Kept::new(folder, &entry.file_name().to_string_lossy()).load::<Option<T>>()
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> bool {
    let written = sikemux_process::files::write_atomic(path, bytes, true);
    if let Err(error) = &written {
        eprintln!("sikemux core: {} was not saved: {error}", path.display());
    }
    written.is_ok()
}
