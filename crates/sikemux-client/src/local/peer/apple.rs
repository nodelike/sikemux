use std::os::fd::AsRawFd;

use crate::local::Connection;

const SOL_LOCAL: libc::c_int = 0;
const LOCAL_PEERTOKEN: libc::c_int = 6;
const TOKEN_EUID: usize = 1;
const TOKEN_PID: usize = 5;

/// The peer's audit token, which names the process as it was when it connected, so one that
/// has since run another program no longer matches.
pub struct Peer([u32; 8]);

impl Peer {
    pub fn pid(&self) -> u32 {
        self.0[TOKEN_PID]
    }

    pub fn uid(&self) -> u32 {
        self.0[TOKEN_EUID]
    }

    pub fn audit_token(&self) -> &[u32; 8] {
        &self.0
    }
}

pub fn peer(connection: &Connection) -> Option<Peer> {
    let mut token = [0u32; 8];
    let mut length = std::mem::size_of_val(&token) as libc::socklen_t;
    // SAFETY: `token` is a writable buffer of `length` bytes, and the kernel writes at most
    // that many.
    let read = unsafe {
        libc::getsockopt(
            connection.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut length,
        )
    };
    (read == 0 && length as usize == std::mem::size_of_val(&token)).then_some(Peer(token))
}
