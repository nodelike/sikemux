use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::time::Duration;

pub fn keep_private(folder: &Path) -> io::Result<()> {
    fs::set_permissions(folder, fs::Permissions::from_mode(0o700))
}

pub struct Mark(UnixDatagram);

impl Mark {
    pub fn leave(path: &Path) -> io::Result<Self> {
        let _ = fs::remove_file(path);
        UnixDatagram::bind(path).map(Self)
    }

    pub fn wait(&self, left: Duration) -> bool {
        if self.0.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        let mut signal = [0u8; 1];
        self.0.recv(&mut signal).is_ok()
    }
}

pub struct Waker(UnixDatagram);

impl Waker {
    pub fn new() -> Option<Self> {
        let sender = UnixDatagram::unbound().ok()?;
        sender.set_nonblocking(true).ok()?;
        Some(Self(sender))
    }

    /// False when nobody waits at `path` any more.
    pub fn wake(&self, path: &Path) -> bool {
        !matches!(
            self.0.send_to(&[1], path),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused
        )
    }
}
