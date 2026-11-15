use std::os::fd::AsRawFd;
use std::path::PathBuf;

use crate::local::Connection;

pub struct Peer {
    pid: u32,
    uid: u32,
}

impl Peer {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// What the peer runs now, which may not be what it ran when it connected.
    pub fn program(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/exe", self.pid)).ok()
    }
}

pub fn peer(connection: &Connection) -> Option<Peer> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: `credentials` is a writable `ucred` of `length` bytes, and the kernel writes at
    // most that many.
    let read = unsafe {
        libc::getsockopt(
            connection.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut credentials).cast(),
            &mut length,
        )
    };
    (read == 0 && credentials.pid > 0).then(|| Peer {
        pid: credentials.pid.unsigned_abs(),
        uid: credentials.uid,
    })
}
