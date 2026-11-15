use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};

use super::Split;

pub type Stream = UnixStream;
pub type Connection = UnixStream;
pub type SyncStream = std::os::unix::net::UnixStream;

pub async fn connect(path: &Path) -> io::Result<Stream> {
    UnixStream::connect(path).await
}

pub fn connect_sync(path: &Path, timeout: Duration) -> io::Result<SyncStream> {
    let stream = SyncStream::connect(path)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

impl Split for UnixStream {
    type Read = OwnedReadHalf;
    type Write = OwnedWriteHalf;

    fn split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        self.into_split()
    }
}

pub struct Listener(UnixListener);

impl Listener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        UnixListener::bind(path).map(Self)
    }

    /// Takes up a socket already bound, such as one a core handed to the binary replacing it.
    pub fn from_std(listener: std::os::unix::net::UnixListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        UnixListener::from_std(listener).map(Self)
    }

    pub async fn accept(&mut self) -> io::Result<Connection> {
        self.0.accept().await.map(|(stream, _)| stream)
    }
}
