use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use sikemux_client::local::{Connection, Listener};
use tokio::signal::unix::{signal, Signal, SignalKind};

use super::super::Listening;

pub(crate) type Bound = std::os::unix::net::UnixListener;

pub(crate) fn private_folder(path: &Path) -> std::io::Result<()> {
    sikemux_process::files::private_folder(path)
}

pub(crate) fn private_to_owner(options: &mut OpenOptions) -> &mut OpenOptions {
    options.mode(0o600)
}

pub(crate) fn bind(socket: &Path) -> std::io::Result<Bound> {
    sikemux_process::private_socket::bind(socket)
}

/// Whether what connected may use the core: the core's own user, or root.
pub(crate) fn admits(connection: &Connection) -> bool {
    sikemux_process::private_socket::is_owner(connection)
}

pub(crate) fn accepting(bound: Bound) -> std::io::Result<Listener> {
    Listener::from_std(bound)
}

/// What a replacing core is handed: the socket and the lock.
pub(crate) fn listening(bound: &Bound, lock: &File) -> Listening {
    Listening {
        listener_fd: bound.as_raw_fd(),
        lock_fd: lock.as_raw_fd(),
    }
}

#[cfg(test)]
pub(crate) fn not_listening() -> Listening {
    Listening {
        listener_fd: -1,
        lock_fd: -1,
    }
}

pub(crate) struct Stopping {
    terminate: Signal,
    interrupt: Signal,
}

impl Stopping {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    /// The signal that asked the core to stop, once one has.
    pub(crate) async fn asked(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
        }
    }
}
