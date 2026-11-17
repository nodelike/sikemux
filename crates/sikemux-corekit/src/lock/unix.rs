use std::fs::File;
use std::os::fd::AsRawFd;

pub fn hold(file: &File) -> bool {
    // SAFETY: flock only reads the integer fd, which `file` keeps open.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

pub fn held_elsewhere(file: &File) -> bool {
    // SAFETY: flock only reads the integer fd, which `file` keeps open.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}
