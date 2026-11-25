//! Cargo never deletes a build once the hashes that name it change, and every Cargo.lock edit or
//! `-p` feature set changes them. This drops the copies no build has used lately while keeping
//! the ones the checkout's current builds need.

use std::collections::{HashMap, HashSet};
use std::fs::{self, DirEntry};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::warm::{hold_builds, ignored_folders, is_cargo_output};

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
const COPIES_KEEP: usize = 3;
const BUILD_COPIES_KEEP: usize = 2;
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
        .map(|folder| prune_target(&folder, true, false).unwrap_or(0))
        .sum()
}

/// Prunes one Cargo output folder while holding its `.cargo-lock`s, skipping when throttled or
/// when a build is active.
pub fn prune_target(target: &Path, throttled: bool, dry_run: bool) -> Option<u64> {
    if !target.exists() {
        return None;
    }
    if throttled && !due(target) {
        return None;
    }
    let _held = hold_builds(target)?;
    if throttled && !dry_run {
        let _ = fs::write(target.join(STAMP), "");
    }
    Some(trim_with(target, dry_run))
}

fn due(target: &Path) -> bool {
    let trimmed = fs::metadata(target.join(STAMP)).and_then(|stamp| stamp.modified());
    trimmed.map_or(true, |at| idle_since(at) >= TRIM_EVERY)
}

/// Drops the copies in the Cargo output folder `target` that no build has used lately, and
/// answers the bytes freed.
pub fn trim(target: &Path) -> u64 {
    trim_with(target, false)
}

fn trim_with(target: &Path, dry_run: bool) -> u64 {
    let mut freed = 0;
    for profile in profiles(target, 0) {
        freed += trim_incremental(&profile.join("incremental"), dry_run);
        freed += trim_build(&profile.join("build"), dry_run);
        freed += trim_deps(&profile.join("deps"), dry_run);
        freed += trim_deps(&profile.join("examples"), dry_run);
        freed += trim_fingerprints(&profile, dry_run);
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

fn remove(path: &Path, dry_run: bool) -> u64 {
    let size = size_of(path);
    if dry_run {
        return size;
    }
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
    name.rsplit_once('-')
        .filter(|(krate, _)| !krate.is_empty())
        .map_or(name, |(krate, _)| krate)
}

/// A crate's incremental cache is named `<crate>-<hash>`; each new hash is a new copy.
fn trim_incremental(dir: &Path, dry_run: bool) -> u64 {
    let mut by_crate: HashMap<String, Vec<(PathBuf, Duration)>> = HashMap::new();
    for name in names(dir) {
        let Some((krate, _)) = name.rsplit_once('-').filter(|(k, _)| !k.is_empty()) else {
            continue;
        };
        let path = dir.join(&name);
        let idle = idle(&path, fs::Metadata::modified);
        (by_crate.entry(krate.to_owned()).or_default()).push((path, idle));
    }
    let mut freed = 0;
    for mut copies in by_crate.into_values() {
        copies.sort_by_key(|(_, idle)| *idle);
        for (path, idle) in copies.iter().skip(COPIES_KEEP) {
            if *idle > COPY_IDLE {
                freed += remove(path, dry_run);
            }
        }
    }
    freed
}

/// Build script outputs live in `build/<crate>-<hash>/`, with one folder for the compiled script
/// and another for its `out/` directory; keep the newest two of each kind per crate.
fn trim_build(dir: &Path, dry_run: bool) -> u64 {
    let mut by_group: HashMap<(String, bool), Vec<(PathBuf, Duration)>> = HashMap::new();
    for name in names(dir) {
        let Some((krate, _)) = name.rsplit_once('-').filter(|(k, _)| !k.is_empty()) else {
            continue;
        };
        let path = dir.join(&name);
        if !path.is_dir() {
            continue;
        }
        let is_run = path.join("out").is_dir() || path.join("output").is_file();
        let idle = idle(&path, fs::Metadata::modified);
        (by_group.entry((krate.to_owned(), is_run)).or_default()).push((path, idle));
    }
    let mut freed = 0;
    for mut copies in by_group.into_values() {
        copies.sort_by_key(|(_, idle)| *idle);
        for (path, idle) in copies.iter().skip(BUILD_COPIES_KEEP) {
            if *idle > COPY_IDLE {
                freed += remove(path, dry_run);
            }
        }
    }
    freed
}

/// Every compile and link reads the outputs it depends on, which refreshes their read time, so
/// outputs left unread for days belong to no current build. Workspace crates get a new
/// `<crate>-<hash>` copy many times a day, so beyond the few most recently read copies of a
/// crate, a day unread is enough.
fn trim_deps(dir: &Path, dry_run: bool) -> u64 {
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
                freed += files.iter().map(|file| remove(file, dry_run)).sum::<u64>();
            }
        }
    }
    freed
}

/// Cargo trusts a fingerprint without checking its outputs still exist, so a fingerprint left
/// behind by a removed library, example or build script makes the next build fail.
fn trim_fingerprints(profile: &Path, dry_run: bool) -> u64 {
    let stems = |folder: &str| -> HashSet<String> {
        (names(&profile.join(folder)).into_iter())
            .map(|name| name.split('.').next().unwrap_or_default().to_owned())
            .collect()
    };
    let deps = stems("deps");
    let examples = stems("examples");
    let builds: HashSet<String> = names(&profile.join("build")).into_iter().collect();
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
                freed += remove(&dir.join(&name), dry_run);
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
                freed += remove(&dir.join(&name), dry_run);
                continue;
            }
        }
        if files.iter().any(|file| {
            !file.contains('.')
                && (file.starts_with("run-build-script-") || file.starts_with("build-script-"))
        }) && !builds.contains(&name)
        {
            freed += remove(&dir.join(&name), dry_run);
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn set_age(path: &Path, age: Duration) {
        let when = SystemTime::now().checked_sub(age).unwrap();
        let times = fs::FileTimes::new().set_modified(when).set_accessed(when);
        File::open(path).unwrap().set_times(times).unwrap();
    }

    #[test]
    fn trim_build_keeps_newest_two_and_drops_stale_fingerprints() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let debug = target.join("debug");
        fs::create_dir_all(debug.join("deps")).unwrap();
        fs::create_dir_all(debug.join("build")).unwrap();
        fs::create_dir_all(debug.join(".fingerprint")).unwrap();
        fs::write(target.join(".rustc_info.json"), "{}").unwrap();

        let copies = [
            ("serde-aaaa", Duration::from_secs(3600)),
            ("serde-bbbb", Duration::from_secs(2 * 3600)),
            ("serde-cccc", Duration::from_secs(30 * 3600)),
        ];
        for (name, age) in copies {
            let dir = debug.join("build").join(name);
            fs::create_dir_all(dir.join("out")).unwrap();
            fs::write(dir.join("out/generated.rs"), vec![b'x'; 256]).unwrap();
            set_age(&dir, age);

            let fp = debug.join(".fingerprint").join(name);
            fs::create_dir_all(&fp).unwrap();
            fs::write(fp.join("run-build-script-build-script-build"), "fp").unwrap();
        }

        let would_free = prune_target(&target, false, true).unwrap();
        assert!(would_free >= 256);
        assert!(debug.join("build/serde-cccc").exists());

        let freed = prune_target(&target, false, false).unwrap();
        assert!(freed >= 256);
        assert!(debug.join("build/serde-aaaa").exists());
        assert!(debug.join("build/serde-bbbb").exists());
        assert!(!debug.join("build/serde-cccc").exists());
        assert!(debug.join(".fingerprint/serde-aaaa").exists());
        assert!(debug.join(".fingerprint/serde-bbbb").exists());
        assert!(!debug.join(".fingerprint/serde-cccc").exists());
    }

    #[test]
    fn prune_target_skips_when_cargo_lock_is_held() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let debug = target.join("debug");
        fs::create_dir_all(debug.join("deps")).unwrap();
        fs::create_dir_all(debug.join(".fingerprint")).unwrap();
        fs::write(target.join(".rustc_info.json"), "{}").unwrap();
        let lock = File::create(debug.join(".cargo-lock")).unwrap();
        lock.lock().unwrap();

        assert_eq!(prune_target(&target, false, false), None);
        drop(lock);
        assert_eq!(prune_target(&target, false, false), Some(0));
    }
}
