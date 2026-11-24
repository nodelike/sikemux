//! Local sockets only their owner can reach.

use std::io;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;

use socket2::{Domain, SockAddr, Socket, Type};

const BACKLOG: i32 = 128;

/// Listens at `path`. Nobody can connect before `listen`, and by then only the owner may.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    socket.bind(&SockAddr::unix(path)?)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    socket.listen(BACKLOG)?;
    Ok(socket.into())
}

/// Whether the other end of `stream` runs as this user, or as root, which no check here stops.
pub fn is_owner(stream: &impl AsFd) -> bool {
    // SAFETY: geteuid takes nothing and cannot fail.
    let own = unsafe { libc::geteuid() };
    peer_uid(stream).is_ok_and(|uid| uid == own || uid == 0)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &impl AsFd) -> io::Result<libc::uid_t> {
    let mut peer = libc::ucred {
        pid: 0,
        uid: libc::uid_t::MAX,
        gid: libc::gid_t::MAX,
    };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the pointers are to a ucred and its size, both alive through the call.
    let read = unsafe {
        libc::getsockopt(
            stream.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut peer).cast(),
            &mut size,
        )
    };
    match read {
        0 => Ok(peer.uid),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &impl AsFd) -> io::Result<libc::uid_t> {
    let (mut uid, mut gid) = (libc::uid_t::MAX, libc::gid_t::MAX);
    // SAFETY: the pointers are to two ids alive through the call.
    match unsafe { libc::getpeereid(stream.as_fd().as_raw_fd(), &mut uid, &mut gid) } {
        0 => Ok(uid),
        _ => Err(io::Error::last_os_error()),
    }
}
