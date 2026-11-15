//! The connection between programs on this machine and its core: a Unix socket on Unix and a
//! named pipe on Windows, reached by the same path. Only the signed-in user can connect.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

/// What the core holds of a client's connection.
pub use platform::Connection;
pub use platform::Listener;
/// What a client holds of its connection to the core.
pub use platform::Stream;
/// A blocking [`Stream`] whose every read and write gives up after the timeout it was opened
/// with.
pub use platform::SyncStream;

pub async fn connect(path: &Path) -> io::Result<Stream> {
    platform::connect(path).await
}

pub fn connect_sync(path: &Path, timeout: Duration) -> io::Result<SyncStream> {
    platform::connect_sync(path, timeout)
}

/// A connection's two directions, to read and write from separate tasks.
pub trait Split {
    type Read: AsyncRead + Send + Unpin + 'static;
    type Write: AsyncWrite + Send + Unpin + 'static;

    fn split(self) -> (Self::Read, Self::Write);
}
