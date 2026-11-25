//! Cargo never deletes a build once the hashes that name it change, and every Cargo.lock edit or
//! `-p` feature set changes them. This drops the copies no build has used lately while keeping
//! the ones the checkout's current builds need, by the rules `scripts/prune-target.mjs` keeps
//! the main checkout's `target` with, and it shares that script's hourly stamp.

use std::collections::{HashMap, HashSet};
use std::fs::{self, DirEntry};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::warm::{hold_builds, ignored_folders, is_cargo_output};

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
const COPIES_KEEP: usize = 3;
/// macOS records a file's read time at most once a day, so a copy read an hour ago can still
/// look a day unread.
const COPY_IDLE: Duration = DAY.saturating_add(HOUR);
const DEPS_IDLE: Duration = Duration::from_secs(3 * 24 * 3600);
const TRIM_EVERY: Duration = HOUR;
const STAMP: &str = ".pruned";

/// Trims each Cargo output folder in `checkout` that was not trimmed in the last hour and that no
/// build is writing to, and answers the bytes freed.
pub fn trim_hourly(checkout: &Path) -> u64 {
    let Some(ignored) = ignored_folders(checkout) else {
        return 0;
    };
    (ignored.iter())
        .map(|folder| checkout.join(folder))
        .filter(|folder| is_cargo_output(folder) && due(folder))
        .map(|folder| {
            let Some(_held) = hold_builds(&folder) else {
                return 0;
            };
            let _ = fs::write(folder.join(STAMP), "");
            trim(&folder)
        })
        .sum()
}

fn due(target: &Path) -> bool {
    let trimmed = fs::metadata(target.join(STAMP)).and_then(|stamp| stamp.modified());
    trimmed.map_or(true, |at| idle_since(at) >= TRIM_EVERY)
}

/// Drops the copies in the Cargo output folder `target` that no build has used lately, and
/// answers the bytes freed.
pub fn trim(target: &Path) -> u64 {
    let mut freed = 0;
    for profile in profiles(target, 0) {
        freed += trim_incremental(&profile.join("incremental"));
        freed += trim_deps(&profile.join("deps"));
        freed += trim_deps(&profile.join("examples"));
        freed += trim_fingerprints(&profile);
    }
    freed
}

fn entries(dir: &Path) -> Vec<DirEntry> {
    fs::read_dir(dir)
        .map(|read| read.flatten().collect())
        .unwrap_or_default()
}

fn names(dir: &Path) -> Vec<String> {
    (entries(dir).iter())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn idle_since(at: SystemTime) -> Duration {
    SystemTime::now().duration_since(at).unwrap_or_default()
}

/// How long since `path` was last touched, by `when`. A build running alongside can delete files
/// mid-scan; those count as in use.
fn idle(path: &Path, when: fn(&fs::Metadata) -> std::io::Result<SystemTime>) -> Duration {
    fs::symlink_metadata(path)
        .and_then(|meta| when(&meta))
        .map_or(Duration::ZERO, idle_since)
}

fn size_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => (entries(path).iter()).map(|e| size_of(&e.path())).sum(),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

fn remove(path: &Path) -> u64 {
    let size = size_of(path);
    let is_dir = fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    let removed = match is_dir {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    };
    removed.map_or(0, |()| size)
}

/// The folders under `dir` Cargo builds one profile in: `debug`, or `<target triple>/release`.
fn profiles(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let names = names(dir);
    if names.iter().any(|name| name == "deps") && names.iter().any(|name| name == ".fingerprint") {
        return vec![dir.to_path_buf()];
    }
    if depth == 2 {
        return Vec::new();
    }
    (entries(dir).into_iter())
        .filter(|entry| {
            entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .flat_map(|entry| profiles(&entry.path(), depth + 1))
        .collect()
}

/// The crate a `<crate>-<hash>` name is a copy of.
fn crate_of(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |(krate, _)| krate)
}

/// A crate's incremental cache is named `<crate>-<hash>`; each new hash is a new copy.
fn trim_incremental(dir: &Path) -> u64 {
    let mut by_crate: HashMap<String, Vec<(PathBuf, Duration)>> = HashMap::new();
    for name in names(dir) {
        let path = dir.join(&name);
        let idle = idle(&path, fs::Metadata::modified);
        (by_crate.entry(crate_of(&name).to_owned()).or_default()).push((path, idle));
    }
    let mut freed = 0;
    for mut copies in by_crate.into_values() {
        copies.sort_by_key(|(_, idle)| *idle);
        for (path, idle) in copies.iter().skip(COPIES_KEEP) {
            if *idle > COPY_IDLE {
                freed += remove(path);
            }
        }
    }
    freed
}

/// Every compile and link reads the outputs it depends on, which refreshes their read time, so
/// outputs left unread for days belong to no current build. Workspace crates get a new
/// `<crate>-<hash>` copy many times a day, so beyond the few most recently read copies of a
/// crate, a day unread is enough.
fn trim_deps(dir: &Path) -> u64 {
    let mut by_unit: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for name in names(dir) {
        let stem = name.split('.').next().unwrap_or_default();
        let unit = stem.strip_prefix("lib").unwrap_or(stem).to_owned();
        by_unit.entry(unit).or_default().push(dir.join(name));
    }
    let mut by_crate: HashMap<String, Vec<(Vec<PathBuf>, Duration)>> = HashMap::new();
    for (unit, files) in by_unit {
        let unread = (files.iter())
            .map(|file| idle(file, fs::Metadata::accessed))
            .min()
            .unwrap_or_default();
        (by_crate.entry(crate_of(&unit).to_owned()).or_default()).push((files, unread));
    }
    let mut freed = 0;
    for mut copies in by_crate.into_values() {
        copies.sort_by_key(|(_, unread)| *unread);
        for (at, (files, unread)) in copies.iter().enumerate() {
            if *unread > DEPS_IDLE || (at >= COPIES_KEEP && *unread > COPY_IDLE) {
                freed += files.iter().map(|file| remove(file)).sum::<u64>();
            }
        }
    }
    freed
}

/// Cargo trusts a fingerprint without checking its outputs still exist, so a fingerprint left
/// behind by a removed library or example makes the next link fail.
fn trim_fingerprints(profile: &Path) -> u64 {
    let stems = |folder: &str| -> HashSet<String> {
        (names(&profile.join(folder)).into_iter())
            .map(|name| name.split('.').next().unwrap_or_default().to_owned())
            .collect()
    };
    let deps = stems("deps");
    let examples = stems("examples");
    let dir = profile.join(".fingerprint");
    let mut freed = 0;
    for name in names(&dir) {
        let hash = name
            .rsplit_once('-')
            .map_or(name.as_str(), |(_, hash)| hash);
        let files = names(&dir.join(&name));
        if let Some(lib) = files
            .iter()
            .find(|file| file.starts_with("lib-") && !file.contains('.'))
        {
            let output = format!("lib{}-{hash}", lib.strip_prefix("lib-").unwrap_or(lib));
            if !deps.contains(&output) {
                freed += remove(&dir.join(&name));
                continue;
            }
        }
        if let Some(example) = files
            .iter()
            .find(|file| file.starts_with("example-") && !file.contains('.'))
        {
            let output = format!(
                "{}-{hash}",
                example.strip_prefix("example-").unwrap_or(example)
            );
            if !examples.contains(&output) {
                freed += remove(&dir.join(&name));
            }
        }
    }
    freed
}
