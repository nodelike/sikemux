use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path` through a rename, so a reader sees the old file or the
/// whole new one, never half of it. A `private` file is readable only by this user,
/// as is any folder this call creates for it.
pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    write_whole(path, bytes, private, true)
}

/// [`write_atomic`] without waiting for the disk, for a file whose loss to a power cut
/// costs little: it is still whole or not there at all.
pub fn write_atomic_unsynced(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    write_whole(path, bytes, private, false)
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn write_whole(path: &Path, bytes: &[u8], private: bool, synced: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        if private {
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        }
        builder.create(parent)?;
    }
    let staging = staging_path(parent, path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    let written = options.open(&staging).and_then(|mut file| {
        file.write_all(bytes)?;
        if synced {
            file.sync_all()?;
        }
        Ok(())
    });
    let result = written.and_then(|()| fs::rename(&staging, path));
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

/// What `path` holds, by `parse`: `Ok(None)` when there is no file, and an error when there
/// is one that cannot be read or parsed, so the caller never writes over it as if empty.
pub fn read_parsed<T, E: std::fmt::Display>(
    path: &Path,
    parse: impl FnOnce(&[u8]) -> Result<T, E>,
) -> Result<Option<T>, String> {
    let unreadable =
        |error: &dyn std::fmt::Display| format!("{} cannot be read: {error}", path.display());
    match fs::read(path) {
        Ok(bytes) => parse(&bytes).map(Some).map_err(|error| unreadable(&error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(&error)),
    }
}

/// What a file only Sikemux writes holds, by `parse`, or `None` when there is none yet. One
/// that cannot be read is first moved aside to `<file>.unreadable-<when>`, so the next write
/// does not destroy what it held.
pub fn read_or_set_aside<T, E: std::fmt::Display>(
    path: &Path,
    parse: impl FnOnce(&[u8]) -> Result<T, E>,
) -> Option<T> {
    read_parsed(path, parse).unwrap_or_else(|why| {
        set_aside(path, &why);
        None
    })
}

/// Moves a file that cannot be used to `<file>.unreadable-<when>`, and says so.
pub fn set_aside(path: &Path, why: &str) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".unreadable-{}", crate::clock::epoch_ms()));
    let aside = PathBuf::from(aside);
    match fs::rename(path, &aside) {
        Ok(()) => eprintln!("sikemux: {why}; it was kept as {}", aside.display()),
        Err(error) => eprintln!("sikemux: {why}; it could not be moved aside either: {error}"),
    }
}

/// The top of the git checkout `dir` is in: the nearest folder at or above it that holds
/// a `.git`, whether that is a folder or a linked worktree's file.
pub fn repo_root(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
}

/// `path` made absolute with links followed. On Windows this leaves off the `\\?\` prefix
/// std adds, which cmd.exe, git and node refuse in a working folder.
#[allow(clippy::disallowed_methods)]
pub fn real_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    #[cfg(windows)]
    return dunce::canonicalize(path);
    #[cfg(not(windows))]
    fs::canonicalize(path)
}

/// `path`, taken from `project` with links followed, or `None` when that leads
/// outside the project.
pub fn confine_to_project(project: &str, path: &str) -> std::io::Result<Option<PathBuf>> {
    let root = real_path(project)?;
    let target = real_path(root.join(path))?;
    Ok(target.starts_with(&root).then_some(target))
}

fn staging_path(parent: &Path, path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
