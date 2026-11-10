//! What each file of one checkout changed since its last commit held after its
//! latest change, so the next change no edit tool reported can be told line by
//! line. Kept in the checkout's folder beside its edit record, each text once
//! under its digest, and let go of once the file is committed or matches it
//! again. A file never kept is as its last commit has it, unless more changed
//! files went unkept than are remembered.

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::files::write_atomic_unsynced;

/// Files bigger than this are neither kept nor told line by line.
pub(super) const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = 512;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Changed files that could not be kept, remembered so their next change is
/// not told against their last commit.
const MAX_UNKNOWN: usize = 4096;
const INDEX: &str = "index.json";
const UNKNOWN: &str = "unknown.json";

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    digest: String,
    len: u64,
    /// When the file was last changed as it was kept, in nanoseconds.
    modified: u128,
}

/// The changed files that could not be kept.
#[derive(Default, Serialize, Deserialize)]
struct Unknown {
    paths: HashSet<String>,
    /// More could not be kept than `paths` holds, so any file not kept may
    /// have changed since its last commit.
    overflowed: bool,
}

pub(super) struct Priors {
    folder: Option<PathBuf>,
    kept: HashMap<String, Kept>,
    /// Files changed since their last commit that could not be kept: too big,
    /// not text, or beyond what is kept.
    unknown: Unknown,
    bytes: u64,
    changed: bool,
    unknown_changed: bool,
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

impl Priors {
    /// What was kept in `folder` of the checkout at `top`, less the files that
    /// have gone since.
    pub(super) fn load(folder: Option<PathBuf>, top: &Path) -> Self {
        let read = |name: &str| {
            folder
                .as_ref()
                .and_then(|folder| std::fs::read(folder.join(name)).ok())
        };
        let kept: HashMap<String, Kept> = read(INDEX)
            .and_then(|index| serde_json::from_slice(&index).ok())
            .unwrap_or_default();
        let mut unknown: Unknown = read(UNKNOWN)
            .and_then(|unknown| serde_json::from_slice(&unknown).ok())
            .unwrap_or_default();
        unknown
            .paths
            .retain(|path| Path::new(path).starts_with(top));
        let bytes = kept.values().map(|kept| kept.len).sum();
        let mut priors = Self {
            folder,
            kept,
            unknown,
            bytes,
            changed: false,
            unknown_changed: false,
        };
        let gone: Vec<String> = priors
            .kept
            .keys()
            .filter(|path| !Path::new(path).starts_with(top) || !Path::new(path).exists())
            .cloned()
            .collect();
        for path in gone {
            priors.forget(&path);
        }
        priors
    }

    pub(super) fn before(&self, path: &str) -> Before {
        if self.unknown.paths.contains(path) {
            return Before::Unknown;
        }
        let Some(kept) = self.kept.get(path) else {
            return match self.unknown.overflowed {
                true => Before::Unknown,
                false => Before::Committed,
            };
        };
        self.folder
            .as_ref()
            .and_then(|folder| std::fs::read_to_string(folder.join(&kept.digest)).ok())
            .filter(|text| digest(text) == kept.digest)
            .map_or(Before::Unknown, Before::Kept)
    }

    /// Whether `path` is kept, or known to be changed but not kept.
    pub(super) fn knows(&self, path: &str) -> bool {
        self.kept.contains_key(path) || self.unknown.paths.contains(path)
    }

    /// Starts telling again which files could not be kept, as every changed
    /// file is about to be looked at. Whether too many were before.
    pub(super) fn recount(&mut self) -> bool {
        self.unknown_changed |= self.unknown.overflowed;
        std::mem::take(&mut self.unknown.overflowed)
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
        if !blob.exists() && write_atomic_unsynced(&blob, text.as_bytes(), true).is_err() {
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
        self.unknown_changed = true;
        if self.unknown.paths.len() >= MAX_UNKNOWN {
            self.unknown.overflowed = true;
        } else {
            self.unknown.paths.insert(path.to_owned());
        }
    }

    /// Lets go of what was kept of `path`, now as its last commit has it.
    pub(super) fn forget(&mut self, path: &str) {
        self.unknown_changed |= self.unknown.paths.remove(path);
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
            .chain(&self.unknown.paths)
            .filter(|path| Path::new(path).starts_with(top))
            .cloned()
            .collect()
    }

    pub(super) fn save(&mut self) {
        let kept_changed = std::mem::take(&mut self.changed);
        let unknown_changed = std::mem::take(&mut self.unknown_changed);
        let Some(folder) = &self.folder else {
            return;
        };
        let nothing = self.kept.is_empty() && self.unknown.paths.is_empty();
        let written = if nothing && !self.unknown.overflowed {
            if !kept_changed && !unknown_changed {
                return;
            }
            std::fs::remove_dir_all(folder).or_else(|error| match error.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(error),
            })
        } else {
            let write = |name: &str, json: serde_json::Result<Vec<u8>>| {
                json.map_err(std::io::Error::other)
                    .and_then(|json| write_atomic_unsynced(&folder.join(name), &json, true))
            };
            let kept = match kept_changed {
                true => write(INDEX, serde_json::to_vec(&self.kept)),
                false => Ok(()),
            };
            kept.and_then(|()| match unknown_changed {
                true => write(UNKNOWN, serde_json::to_vec(&self.unknown)),
                false => Ok(()),
            })
        };
        if let Err(error) = written {
            eprintln!("sikemux core: what changed files held was not saved: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On 2026-11-10 a checkout holding 35,000 untracked build files filled what was
    /// kept, and forgetting every unkept file at once made the next change to the
    /// shared tools.json count all its uncommitted lines as one agent's.
    #[test]
    fn a_file_that_could_not_be_kept_is_never_told_against_its_commit() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("checkout");
        let folder = Some(dir.path().join("kept"));
        let shared = top.join("tools.json").to_string_lossy().into_owned();
        let mut priors = Priors::load(folder.clone(), &top);
        priors.keep(&shared, None);
        for n in 0..MAX_UNKNOWN + 10 {
            priors.keep(&top.join(format!("target/{n}")).to_string_lossy(), None);
        }
        priors.save();
        assert!(matches!(priors.before(&shared), Before::Unknown));
        let reloaded = Priors::load(folder, &top);
        assert!(matches!(reloaded.before(&shared), Before::Unknown));
    }
}
