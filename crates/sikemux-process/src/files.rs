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

/// The top of the git checkout `dir` is in: the nearest folder at or above it that holds
/// a `.git`, whether that is a folder or a linked worktree's file.
pub fn repo_root(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
}

/// `path`, taken from `project` with links followed, or `None` when that leads
/// outside the project.
pub fn confine_to_project(project: &str, path: &str) -> std::io::Result<Option<PathBuf>> {
    let root = fs::canonicalize(project)?;
    let target = fs::canonicalize(root.join(path))?;
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
