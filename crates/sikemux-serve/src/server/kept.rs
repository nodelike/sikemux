//! A small record the core keeps in one file of the data folder, written
//! whole each time it changes, so it outlives a crash and a restart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sikemux_process::files;
use sikemux_process::sync::Locked;

/// How long [`Kept::save_soon`] waits for more changes before writing.
const SETTLE: Duration = Duration::from_millis(250);
/// How many times a write that failed is tried again, a settle apart.
const RETRIES: usize = 5;

pub struct Kept {
    path: Option<PathBuf>,
    queue: Arc<Queue>,
    sync: Arc<SyncWrite>,
}

#[derive(Default)]
struct SyncWrite {
    next_seq: AtomicU64,
    io: Mutex<u64>,
}

/// Serialized bytes captured under a caller's state lock, ready to be written durably after
/// releasing that lock.
pub struct PreparedSave {
    path: PathBuf,
    bytes: Vec<u8>,
    seq: u64,
    sync: Arc<SyncWrite>,
}

impl PreparedSave {
    pub fn write(self) {
        let mut written = self.sync.io.locked();
        if self.seq <= *written {
            return;
        }
        if write(&self.path, &self.bytes).is_ok() {
            *written = self.seq;
        }
    }

    pub async fn write_async(self) {
        let _ = tokio::task::spawn_blocking(move || self.write()).await;
    }

    pub fn write_blocking(self) {
        let multi_thread = tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        if multi_thread {
            tokio::task::block_in_place(|| self.write());
        } else {
            self.write();
        }
    }
}

#[derive(Default)]
struct Queue {
    state: Mutex<Pending>,
    changed: Condvar,
}

#[derive(Default)]
struct Pending {
    /// Sha256 digest of what was last handed to be saved.
    last: Option<[u8; 32]>,
    next: Option<Vec<u8>>,
    writing: bool,
    hurry: bool,
    failed: usize,
    /// Why the latest change handed to be saved did not reach the disk.
    unsaved: Option<String>,
}

struct Job {
    due: Instant,
    retry: bool,
    path: PathBuf,
    queue: Arc<Queue>,
}

struct Saver {
    jobs: Mutex<Vec<Job>>,
    wake: Condvar,
}

impl Saver {
    fn schedule(&self, path: PathBuf, queue: Arc<Queue>, due: Instant, retry: bool) {
        self.jobs.locked().push(Job {
            due,
            retry,
            path,
            queue,
        });
        self.wake.notify_one();
    }

    fn hurry(&self, queue: &Arc<Queue>) {
        let now = Instant::now();
        let mut jobs = self.jobs.locked();
        let mut moved = false;
        for job in jobs.iter_mut() {
            if !job.retry && Arc::ptr_eq(&job.queue, queue) {
                job.due = now;
                moved = true;
            }
        }
        if moved {
            self.wake.notify_one();
        }
    }

    fn run(&self) {
        loop {
            let job = {
                let mut jobs = self.jobs.locked();
                loop {
                    let Some((index, earliest)) = jobs
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, job)| job.due)
                        .map(|(index, job)| (index, job.due))
                    else {
                        jobs = self.wake.wait(jobs).unwrap_or_else(PoisonError::into_inner);
                        continue;
                    };
                    let now = Instant::now();
                    if earliest <= now {
                        break jobs.swap_remove(index);
                    }
                    let wait = earliest.saturating_duration_since(now);
                    jobs = self
                        .wake
                        .wait_timeout(jobs, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            };
            let bytes = {
                let mut pending = job.queue.state.locked();
                match pending.next.take() {
                    Some(bytes) => bytes,
                    None => {
                        pending.writing = false;
                        pending.hurry = false;
                        pending.failed = 0;
                        job.queue.changed.notify_all();
                        continue;
                    }
                }
            };
            let written = write(&job.path, &bytes);
            let mut pending = job.queue.state.locked();
            match written {
                Ok(()) => {
                    pending.failed = 0;
                    pending.unsaved = None;
                }
                Err(_) if pending.next.is_none() && pending.failed < RETRIES => {
                    pending.failed += 1;
                    pending.next = Some(bytes);
                    drop(pending);
                    self.schedule(job.path, job.queue, Instant::now() + SETTLE, true);
                    continue;
                }
                Err(error) => {
                    pending.failed = 0;
                    pending.last = None;
                    pending.unsaved = Some(error);
                }
            }
            if pending.next.is_some() {
                let due = if pending.hurry {
                    Instant::now()
                } else {
                    Instant::now() + SETTLE
                };
                drop(pending);
                self.schedule(job.path, job.queue, due, false);
            } else {
                pending.writing = false;
                pending.hurry = false;
                job.queue.changed.notify_all();
            }
        }
    }
}

fn saver() -> &'static Arc<Saver> {
    static SAVER: OnceLock<Arc<Saver>> = OnceLock::new();
    SAVER.get_or_init(|| {
        let saver = Arc::new(Saver {
            jobs: Mutex::default(),
            wake: Condvar::new(),
        });
        let worker = saver.clone();
        let _ = std::thread::Builder::new()
            .name("sikemux-save".into())
            .spawn(move || worker.run());
        saver
    })
}

impl Kept {
    /// Without a data folder nothing is kept.
    pub fn new(data_dir: Option<&Path>, file: &str) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join(file)),
            queue: Arc::default(),
            sync: Arc::default(),
        }
    }

    /// What the file holds, or the default when there is none yet. A file
    /// that cannot be read is moved aside first, so the next save does not
    /// overwrite what it held.
    pub fn load<T: DeserializeOwned + Default>(&self) -> T {
        (self.path.as_deref())
            .and_then(|path| files::read_or_set_aside(path, |bytes| serde_json::from_slice(bytes)))
            .unwrap_or_default()
    }

    /// Moves a file that cannot be used to `<file>.unreadable-<when>`.
    pub fn set_aside(&self, why: &str) {
        if let Some(path) = &self.path {
            files::set_aside(path, &format!("{} cannot be read: {why}", path.display()));
        }
    }

    /// Copies the file as it is to `<file>.<suffix>`, unless that copy is already there.
    pub fn copy_once(&self, suffix: &str) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut copy = path.clone().into_os_string();
        copy.push(format!(".{suffix}"));
        if Path::new(&copy).exists() {
            return Ok(());
        }
        std::fs::copy(path, &copy).map(|_| ())
    }

    /// Serializes `value` under the caller's lock and returns a write to run after dropping it.
    pub fn prepare<T: Serialize>(&self, value: &T) -> Option<PreparedSave> {
        let path = self.path.clone()?;
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                let seq = self.sync.next_seq.fetch_add(1, Ordering::AcqRel) + 1;
                Some(PreparedSave {
                    path,
                    bytes,
                    seq,
                    sync: self.sync.clone(),
                })
            }
            Err(error) => {
                eprintln!("sikemux core: {} was not saved: {error}", path.display());
                None
            }
        }
    }

    pub fn save<T: Serialize>(&self, value: &T) {
        if let Some(prepared) = self.prepare(value) {
            prepared.write();
        }
    }

    /// Saves on a shared background thread, so whoever changed the value never waits
    /// on the disk. Changes made while one is being written are saved together
    /// after it, only the latest of them. Answers false when `value` is what
    /// was last saved, which is then not written again.
    pub fn save_soon<T: Serialize>(&self, value: &T) -> bool {
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
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let mut pending = self.queue.state.locked();
        if pending.last == Some(digest) {
            return false;
        }
        pending.last = Some(digest);
        pending.next = Some(bytes);
        if pending.writing {
            return true;
        }
        pending.writing = true;
        drop(pending);
        saver().schedule(
            path.clone(),
            self.queue.clone(),
            Instant::now() + SETTLE,
            false,
        );
        true
    }

    /// Waits until every [`Kept::save_soon`] so far is on disk.
    pub fn flush(&self) {
        {
            let mut pending = self.queue.state.locked();
            if !pending.writing {
                return;
            }
            pending.hurry = true;
        }
        saver().hurry(&self.queue);
        let pending = self.queue.state.locked();
        drop(
            self.queue
                .changed
                .wait_while(pending, |pending| pending.writing)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// Why the latest change handed to [`Kept::save_soon`] is not on disk,
    /// once its writes, tries again included, are over.
    pub fn unsaved(&self) -> Option<String> {
        self.queue.state.locked().unsaved.clone()
    }
}

/// The file a record keyed by `key`, such as a project's path, is kept in
/// among others of its kind in one folder.
pub fn file_for(key: &str) -> String {
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    format!("{}.json", digest.get(..16).unwrap_or(&digest))
}

/// Every record kept in `folder`, one file each; a file that cannot be read
/// is moved aside.
pub fn load_all<T: DeserializeOwned>(folder: Option<&Path>) -> Vec<T> {
    load_all_upgraded(folder, |value| value)
}

/// Every record kept in `folder`, each carried over by `upgrade` from how an
/// earlier core wrote it before it is read; a file that cannot be read is
/// moved aside.
pub fn load_all_upgraded<T: DeserializeOwned>(
    folder: Option<&Path>,
    upgrade: impl Fn(serde_json::Value) -> serde_json::Value,
) -> Vec<T> {
    let Some(entries) = folder.and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|entry| {
            let kept = Kept::new(folder, &entry.file_name().to_string_lossy());
            let value = kept.load::<Option<serde_json::Value>>()?;
            serde_json::from_value::<T>(upgrade(value))
                .map_err(|error| kept.set_aside(&error.to_string()))
                .ok()
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    sikemux_process::files::write_atomic(path, bytes, true).map_err(|error| {
        let error = format!("{} was not saved: {error}", path.display());
        eprintln!("sikemux core: {error}");
        error
    })
}
