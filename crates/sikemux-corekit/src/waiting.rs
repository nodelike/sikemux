//! A process waiting for a core to start, stop or open its window leaves a
//! mark in a folder beside the core's socket: a socket of its own on Unix, a
//! file naming an event on Windows. The core wakes every one of them when that
//! happens, so a waiter checks again then rather than on a timer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[cfg(unix)]
#[path = "waiting/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "waiting/windows.rs"]
mod platform;

fn folder(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".waiting");
    PathBuf::from(name)
}

struct Waiter {
    mark: platform::Mark,
    path: PathBuf,
}

impl Waiter {
    fn beside(socket: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = folder(socket);
        fs::create_dir_all(&folder)?;
        platform::keep_private(&folder)?;
        let path = folder.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mark = platform::Mark::leave(&path)?;
        Ok(Self { mark, path })
    }

    /// False once `deadline` passes without a wake.
    fn wait(&self, deadline: Instant) -> bool {
        let left = deadline.saturating_duration_since(Instant::now());
        !left.is_zero() && self.mark.wait(left)
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
/// away without taking its mark with it is cleared up here.
pub fn wake(socket: &Path) {
    let Ok(waiters) = fs::read_dir(folder(socket)) else {
        return;
    };
    let Some(waker) = platform::Waker::new() else {
        return;
    };
    for waiter in waiters.flatten() {
        let path = waiter.path();
        if !waker.wake(&path) {
            let _ = fs::remove_file(&path);
        }
    }
}
