//! The core's single-instance lock, which sits beside its socket.

use std::fs::File;
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "lock/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "lock/windows.rs"]
mod platform;

pub fn lock_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    PathBuf::from(path)
}

/// Takes the lock on `file` for as long as it stays open; false while another process has it.
pub fn hold(file: &File) -> bool {
    platform::hold(file)
}

/// The process holding the core's single-instance lock at `socket`, so a core that stopped
/// answering can still be found and stopped. None while nothing holds it.
pub fn lock_holder(socket: &Path) -> Option<u32> {
    let mut lock = File::open(lock_path(socket)).ok()?;
    if !platform::held_elsewhere(&lock) {
        return None;
    }
    let mut pid = String::new();
    std::io::Read::read_to_string(&mut lock, &mut pid).ok()?;
    pid.trim().parse().ok().filter(|pid| *pid > 1)
}
