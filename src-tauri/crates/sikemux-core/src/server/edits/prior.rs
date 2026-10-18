//! What each file changed since its last commit held after its latest change,
//! so the next change no edit tool reported can be told line by line. Kept in
//! the project's folder beside its edit record, each text once under its
//! digest, and let go of once the file is committed or matches it again. A
//! file never kept is as its last commit has it.

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Files bigger than this are neither kept nor told line by line.
pub(super) const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = 512;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Changed files that could not be kept, remembered so their next change is
/// not told against their last commit.
const MAX_UNKNOWN: usize = 4096;
const INDEX: &str = "index.json";

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    digest: String,
    len: u64,
    /// When the file was last changed as it was kept, in nanoseconds.
    modified: u128,
}

pub(super) struct Priors {
    folder: Option<PathBuf>,
    kept: HashMap<String, Kept>,
    /// Files changed since their last commit that could not be kept: too big,
    /// not text, or beyond what is kept.
    unknown: HashSet<String>,
    bytes: u64,
    changed: bool,
}

/// What a file changed since its last commit held before its latest change.
pub(super) enum Before {
    Kept(String),
    /// As its last commit has it.
    Committed,
    Unknown,
}

/// A file's text, unless it is too big or not text.
pub(super) fn text_of(path: &Path) -> Option<(String, Metadata)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let text = String::from_utf8(std::fs::read(path).ok()?).ok()?;
    (!text.contains('\0')).then_some((text, meta))
}

fn modified(meta: &Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |at| at.as_nanos())
}

fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Writes `bytes` whole or not at all, without waiting for the disk: losing
/// a copy to a power cut only leaves its file's next change unknown.
fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
    }
    let staging = path.with_extension("new");
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staging)?
        .write_all(bytes)?;
    std::fs::rename(&staging, path)
}

impl Priors {
    /// What was kept in `folder`, less the files that have gone since.
    pub(super) fn load(folder: Option<PathBuf>) -> Self {
        let kept: HashMap<String, Kept> = folder
            .as_ref()
            .and_then(|folder| std::fs::read(folder.join(INDEX)).ok())
            .and_then(|index| serde_json::from_slice(&index).ok())
            .unwrap_or_default();
        let bytes = kept.values().map(|kept| kept.len).sum();
        let mut priors = Self {
            folder,
            kept,
            unknown: HashSet::new(),
            bytes,
            changed: false,
        };
        let gone: Vec<String> = priors
            .kept
            .keys()
            .filter(|path| !Path::new(path).exists())
            .cloned()
            .collect();
        for path in gone {
            priors.forget(&path);
        }
        priors
    }

    pub(super) fn before(&self, path: &str) -> Before {
        if self.unknown.contains(path) {
            return Before::Unknown;
        }
        let Some(kept) = self.kept.get(path) else {
            return Before::Committed;
        };
        self.folder
            .as_ref()
            .and_then(|folder| std::fs::read_to_string(folder.join(&kept.digest)).ok())
            .filter(|text| digest(text) == kept.digest)
            .map_or(Before::Unknown, Before::Kept)
    }

    /// Whether `path` is kept, or known to be changed but not kept.
    pub(super) fn knows(&self, path: &str) -> bool {
        self.kept.contains_key(path) || self.unknown.contains(path)
    }

    /// Whether the file is as it was kept, going by its size and the time it
    /// last changed.
    pub(super) fn fresh(&self, path: &str, meta: &Metadata) -> bool {
        self.kept
            .get(path)
            .is_some_and(|kept| kept.len == meta.len() && kept.modified == modified(meta))
    }

    /// Whether another file can be kept.
    pub(super) fn room(&self) -> bool {
        self.kept.len() < MAX_FILES && self.bytes < MAX_BYTES
    }

    /// Keeps `text` as what `path` now holds, or remembers it as unknown when
    /// it cannot be kept.
    pub(super) fn keep(&mut self, path: &str, text: Option<(&str, &Metadata)>) {
        let Some((text, meta)) = text else {
            self.unknown(path);
            return;
        };
        let digest = digest(text);
        let len = text.len() as u64;
        if let Some(kept) = self.kept.get_mut(path).filter(|kept| kept.digest == digest) {
            kept.modified = modified(meta);
            self.changed = true;
            return;
        }
        let freed = self.kept.get(path).map_or(0, |kept| kept.len);
        let full = !self.kept.contains_key(path) && self.kept.len() >= MAX_FILES;
        let folder = match &self.folder {
            Some(folder) if !full && self.bytes - freed + len <= MAX_BYTES => folder.clone(),
            _ => {
                self.unknown(path);
                return;
            }
        };
        let blob = folder.join(&digest);
        if !blob.exists() && write(&blob, text.as_bytes()).is_err() {
            self.unknown(path);
            return;
        }
        self.forget(path);
        self.bytes += len;
        self.kept.insert(
            path.to_owned(),
            Kept {
                digest,
                len,
                modified: modified(meta),
            },
        );
        self.changed = true;
    }

    fn unknown(&mut self, path: &str) {
        self.forget(path);
        if self.unknown.len() >= MAX_UNKNOWN {
            self.unknown.clear();
        }
        self.unknown.insert(path.to_owned());
    }

    /// Lets go of what was kept of `path`, now as its last commit has it.
    pub(super) fn forget(&mut self, path: &str) {
        self.unknown.remove(path);
        let Some(kept) = self.kept.remove(path) else {
            return;
        };
        self.bytes = self.bytes.saturating_sub(kept.len);
        self.changed = true;
        let shared = self.kept.values().any(|other| other.digest == kept.digest);
        if let (false, Some(folder)) = (shared, &self.folder) {
            let _ = std::fs::remove_file(folder.join(&kept.digest));
        }
    }

    /// The files kept or unknown under `top`.
    pub(super) fn under(&self, top: &Path) -> Vec<String> {
        self.kept
            .keys()
            .chain(&self.unknown)
            .filter(|path| Path::new(path).starts_with(top))
            .cloned()
            .collect()
    }

    pub(super) fn save(&mut self) {
        if !std::mem::take(&mut self.changed) {
            return;
        }
        let Some(folder) = &self.folder else {
            return;
        };
        let written = if self.kept.is_empty() {
            std::fs::remove_dir_all(folder).or_else(|error| match error.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(error),
            })
        } else {
            serde_json::to_vec(&self.kept)
                .map_err(std::io::Error::other)
                .and_then(|index| write(&folder.join(INDEX), &index))
        };
        if let Err(error) = written {
            eprintln!("sikemux core: what changed files held was not saved: {error}");
        }
    }
}
