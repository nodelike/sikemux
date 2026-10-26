//! A process waiting for a core to start, stop or open its window leaves a
//! socket in a folder beside the core's socket. The core wakes every one of
//! them when that happens, so a waiter checks again then rather than on a
//! timer.

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn folder(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".waiting");
    PathBuf::from(name)
}

struct Waiter {
    #[cfg(unix)]
    socket: UnixDatagram,
    path: PathBuf,
}

/// How long a waiter on Windows goes between looks, as it has no way to be woken.
#[cfg(windows)]
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

impl Waiter {
    fn beside(socket: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = folder(socket);
        fs::create_dir_all(&folder)?;
        sikemux_process::os::set_mode(&folder, 0o700)?;
        let path = folder.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        #[cfg(unix)]
        let socket = UnixDatagram::bind(&path)?;
        Ok(Self {
            #[cfg(unix)]
            socket,
            path,
        })
    }

    #[cfg(windows)]
    fn wait(&self, deadline: Instant) -> bool {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(POLL));
        true
    }

    /// False once `deadline` passes without a wake.
    #[cfg(unix)]
    fn wait(&self, deadline: Instant) -> bool {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || self.socket.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        let mut signal = [0u8; 1];
        self.socket.recv(&mut signal).is_ok()
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Answers what `check` finds, checking again each time the core at `socket`
/// starts, stops or opens its window, until `deadline`.
pub fn wait_until<T>(
    socket: &Path,
    deadline: Instant,
    mut check: impl FnMut() -> Option<T>,
) -> io::Result<Option<T>> {
    let waiter = Waiter::beside(socket)?;
    loop {
        if let Some(found) = check() {
            return Ok(Some(found));
        }
        if !waiter.wait(deadline) {
            return Ok(check());
        }
    }
}

/// Wakes every process waiting on the core at `socket`. A waiter that went
/// away without taking its socket with it is cleared up here.
#[cfg(windows)]
pub fn wake(_socket: &Path) {}

#[cfg(unix)]
pub fn wake(socket: &Path) {
    let Ok(waiters) = fs::read_dir(folder(socket)) else {
        return;
    };
    let Ok(sender) = UnixDatagram::unbound() else {
        return;
    };
    if sender.set_nonblocking(true).is_err() {
        return;
    }
    for waiter in waiters.flatten() {
        let path = waiter.path();
        if let Err(error) = sender.send_to(&[1], &path) {
            if error.kind() == io::ErrorKind::ConnectionRefused {
                let _ = fs::remove_file(&path);
            }
        }
    }
}
