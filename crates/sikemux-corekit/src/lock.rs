//! The core's single-instance lock, which sits beside its socket.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub fn lock_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    PathBuf::from(path)
}

/// The process holding the core's single-instance lock at `socket`, so a core that stopped
/// answering can still be found and stopped. None while nothing holds it.
pub fn lock_holder(socket: &Path) -> Option<u32> {
    let mut lock = File::open(lock_path(socket)).ok()?;
    // SAFETY: flock only reads the integer fd, which `lock` keeps open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return None;
    }
    let mut pid = String::new();
    std::io::Read::read_to_string(&mut lock, &mut pid).ok()?;
    pid.trim().parse().ok().filter(|pid| *pid > 1)
}
