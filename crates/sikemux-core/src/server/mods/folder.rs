//! A mod's folder as the account keeps it: every file but dot-files and symbolic links, each named
//! by its path and the SHA-256 of its content. Small text files travel as documents, the rest as
//! sealed files beside them.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{read_manifest, MANIFEST, PAGE};

const MOST_FILES: usize = 512;
pub(super) const MOST_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MOST_FOLDER_BYTES: u64 = 64 * 1024 * 1024;
/// A text file whose content, as JSON, fits in one sealed document of the account.
const MOST_TEXT_JSON: usize = 44 * 1024;
const MOST_PATH: usize = 512;

/// One file as the account lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Entry {
    pub hash: String,
    pub size: u64,
    /// Kept as a document rather than as a sealed file.
    pub text: bool,
}

pub(super) type Listing = BTreeMap<String, Entry>;

/// The account's copy of a mod's folder: its files, and the listing of the copy it was made from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Shared {
    pub files: Listing,
    pub base: Option<String>,
}

/// A folder as it is on this host.
pub(super) enum Local {
    Absent,
    /// There, but not a mod that can show, as while an agent is still writing it.
    Unfit {
        newest: u64,
    },
    /// A mod that shows but cannot follow the account, and why.
    Unsyncable {
        newest: u64,
        problem: String,
    },
    Whole(Scan),
}

pub(super) struct Scan {
    pub files: Listing,
    /// The content of each text file, by hash.
    pub texts: BTreeMap<String, String>,
    /// The newest change to any of its files, in ms since 1970.
    pub newest: u64,
}

/// The size and change time of each file, to tell whether the folder is still being written.
pub(super) type Stats = BTreeMap<String, (u64, u64)>;

pub(super) fn hash_of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What a listing is known by: the hash of its paths and hashes.
pub(super) fn digest(files: &Listing) -> String {
    let mut hasher = Sha256::new();
    for (path, entry) in files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(entry.hash.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// Whether `path` names a file inside a mod's folder: relative, plain names separated by `/`,
/// none starting with a dot.
pub(super) fn fits(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MOST_PATH
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn ms_of(time: std::io::Result<std::time::SystemTime>) -> u64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as u64)
}

/// Every file of `folder` the account keeps, with its size and change time, leaving out
/// dot-files, symbolic links and what is inside them.
fn walk(folder: &Path) -> std::io::Result<Vec<(String, PathBuf, u64, u64)>> {
    let mut found = Vec::new();
    let mut waiting = vec![(String::new(), folder.to_path_buf())];
    while let Some((prefix, dir)) = waiting.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let kind = entry.file_type()?;
            let path = format!("{prefix}{name}");
            if kind.is_dir() {
                waiting.push((format!("{path}/"), entry.path()));
            } else if kind.is_file() {
                let meta = entry.metadata()?;
                found.push((path, entry.path(), meta.len(), ms_of(meta.modified())));
            }
            if found.len() > MOST_FILES {
                return Ok(found);
            }
        }
    }
    Ok(found)
}

/// What `stats` are known by, to tell cheaply whether a folder changed since it was last looked at.
pub(super) fn stats_digest(stats: &Stats) -> String {
    let mut hasher = Sha256::new();
    for (path, (size, ms)) in stats {
        hasher.update(format!("{path}\0{size}\0{ms}\0").as_bytes());
    }
    hex::encode(hasher.finalize())
}

pub(super) fn stats(folder: &Path) -> Stats {
    walk(folder)
        .unwrap_or_default()
        .into_iter()
        .map(|(path, _, size, ms)| (path, (size, ms)))
        .collect()
}

/// Reads the folder as the account would keep it.
pub(super) fn scan(folder: &Path) -> Local {
    if !folder.is_dir() {
        return Local::Absent;
    }
    let Ok(found) = walk(folder) else {
        return Local::Unfit { newest: 0 };
    };
    let newest = found.iter().map(|(_, _, _, ms)| *ms).max().unwrap_or(0);
    let whole = read_manifest(folder).is_ok() && folder.join(PAGE).is_file();
    if !whole {
        return Local::Unfit { newest };
    }
    let unsyncable = |problem: String| Local::Unsyncable { newest, problem };
    if found.len() > MOST_FILES {
        return unsyncable(format!("it holds more than {MOST_FILES} files"));
    }
    if let Some((path, ..)) = found.iter().find(|(path, ..)| !fits(path)) {
        return unsyncable(format!("{path} cannot be named on every host"));
    }
    if let Some((path, ..)) = found.iter().find(|(_, _, size, _)| *size > MOST_FILE_BYTES) {
        return unsyncable(format!("{path} is over 25 MB"));
    }
    if found.iter().map(|(_, _, size, _)| size).sum::<u64>() > MOST_FOLDER_BYTES {
        return unsyncable("its files come to over 64 MB".to_owned());
    }
    let mut files = Listing::new();
    let mut texts = BTreeMap::new();
    for (path, real, _, _) in found {
        let Ok(bytes) = std::fs::read(&real) else {
            return Local::Unfit { newest };
        };
        let hash = hash_of(&bytes);
        let size = bytes.len() as u64;
        let text = String::from_utf8(bytes).ok().filter(|text| {
            serde_json::to_string(text).map_or(0, |json| json.len()) <= MOST_TEXT_JSON
        });
        let is_text = text.is_some();
        if let Some(text) = text {
            texts.insert(hash.clone(), text);
        }
        files.insert(
            path,
            Entry {
                hash,
                size,
                text: is_text,
            },
        );
    }
    Local::Whole(Scan {
        files,
        texts,
        newest,
    })
}

/// Where a mod's folder is built before it takes the place of the one there.
pub(super) fn staging(mods: &Path, id: &str) -> std::io::Result<PathBuf> {
    let suffix =
        hex::encode(super::super::vault::random_key().unwrap_or_default())[..12].to_owned();
    let staged = mods.join(format!(".{id}.incoming-{suffix}"));
    std::fs::create_dir_all(&staged)?;
    Ok(staged)
}

/// Writes one file into a folder being built, refusing a path that would leave it.
pub(super) fn put(staged: &Path, path: &str, bytes: &[u8]) -> std::io::Result<()> {
    if !fits(path) {
        return Err(std::io::Error::other(format!(
            "{path} is not a path inside a mod"
        )));
    }
    let target = staged.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)
}

/// Puts the folder built at `staged` in place of the mod's folder. A copy that lost to the
/// account's goes to the Trash whole; otherwise what the account does not keep, such as dot-files,
/// moves into the new folder and the rest of the old one is deleted, the account holding it.
pub(super) fn swap(mods: &Path, id: &str, staged: &Path, lost: bool) -> Result<(), String> {
    let folder = mods.join(id);
    if !folder.exists() {
        return std::fs::rename(staged, &folder).map_err(|error| error.to_string());
    }
    if lost {
        to_trash(&folder)?;
        return std::fs::rename(staged, &folder).map_err(|error| error.to_string());
    }
    if let Ok(entries) = std::fs::read_dir(&folder) {
        for entry in entries.flatten() {
            let kept = entry.file_name().to_string_lossy().starts_with('.')
                || entry.file_type().is_ok_and(|kind| kind.is_symlink());
            if kept {
                let _ = std::fs::rename(entry.path(), staged.join(entry.file_name()));
            }
        }
    }
    let old = mods.join(format!(".{id}.outgoing"));
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&folder, &old).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(staged, &folder) {
        let _ = std::fs::rename(&old, &folder);
        return Err(error.to_string());
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

pub(super) fn to_trash(folder: &Path) -> Result<(), String> {
    trash::delete(folder).map_err(|error| error.to_string())
}

/// The mod ids of every folder in `mods`.
pub(super) fn ids(mods: &Path) -> Vec<String> {
    std::fs::read_dir(mods)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|id| super::is_mod_id(id))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn required() -> [&'static str; 2] {
    [MANIFEST, PAGE]
}
