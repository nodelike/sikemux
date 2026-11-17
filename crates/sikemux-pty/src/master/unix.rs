use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use portable_pty::MasterPty;
use tokio::io::unix::AsyncFd;

use super::Read;
use crate::process::Terminal;

/// A new terminal.
pub fn open_pty(size: portable_pty::PtySize) -> std::io::Result<portable_pty::PtyPair> {
    #[allow(clippy::disallowed_types)]
    let system = portable_pty::NativePtySystem::default();
    portable_pty::PtySystem::openpty(&system, size).map_err(std::io::Error::other)
}

/// One descriptor serves both directions, read and written as soon as it is ready.
pub struct Master {
    io: AsyncFd<File>,
}

impl Master {
    /// Takes over the terminal from `master`, which is dropped: the descriptor kept shares
    /// its open file, so the program's terminal stays open.
    pub fn open(master: Box<dyn MasterPty + Send>, _program: Option<u32>) -> std::io::Result<Self> {
        let fd = master
            .as_raw_fd()
            .ok_or_else(|| std::io::Error::other("pty: master pty has no fd"))?;
        // SAFETY: `master` still owns `fd`, so it is open; F_DUPFD_CLOEXEC makes a new
        // descriptor and touches no memory.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if copy < 0 {
            return Err(std::io::Error::last_os_error());
        }
        drop(master);
        // SAFETY: `copy` was just made and nothing else holds it.
        Self::adopt(unsafe { File::from_raw_fd(copy) })
    }

    /// The terminal whose master side `file` is, as one core hands it to the next.
    pub fn adopt(file: File) -> std::io::Result<Self> {
        let fd = file.as_raw_fd();
        // SAFETY: F_GETFL/F_SETFL only read and set the flags of the open descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: `file` owns the descriptor and the AsyncFd owns `file`, so it stays open
        // and the same until the AsyncFd is dropped.
        let io = unsafe { AsyncFd::register(file) }.map_err(std::io::Error::from)?;
        Ok(Self { io })
    }

    pub fn fd(&self) -> RawFd {
        self.io.get_ref().as_raw_fd()
    }

    pub fn terminal(&self) -> Terminal {
        Terminal::from_fd(self.fd())
    }

    /// The next output there is; cancelling it loses none.
    pub async fn read(&self, buffer: &mut [u8]) -> Read {
        loop {
            let Ok(mut ready) = self.io.readable().await else {
                return Read::Eof;
            };
            match ready.try_io(|inner| inner.get_ref().read(buffer)) {
                Ok(Ok(0)) | Ok(Err(_)) => return Read::Eof,
                Ok(Ok(read)) => return Read::Bytes(read),
                Err(_would_block) => continue,
            }
        }
    }

    pub async fn write_all(&self, mut data: &[u8]) -> std::io::Result<()> {
        while !data.is_empty() {
            let mut ready = self.io.writable().await?;
            match ready.try_io(|inner| inner.get_ref().write(data)) {
                Ok(Ok(0)) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "pty write returned 0",
                    ));
                }
                Ok(Ok(written)) => data = data.get(written..).unwrap_or_default(),
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => continue,
            }
        }
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the descriptor stays open for this call, and `size` is the live winsize
        // TIOCSWINSZ reads.
        let rc = unsafe { libc::ioctl(self.fd(), libc::TIOCSWINSZ, &size as *const libc::winsize) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // `Master::open` rests on this: after the master is copied and portable_pty's
    // `MasterPty` dropped, the copy keeps the child's terminal open. The child sleeps
    // before printing, so a hangup on drop would end the read before the marker arrives.
    #[test]
    fn lone_master_dup_keeps_child_alive_after_masterpty_drop() {
        use portable_pty::{CommandBuilder, PtySize};
        use std::io::Read;
        use std::os::fd::FromRawFd;

        let pair = super::open_pty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("sleep 0.2; printf MARKER");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        let master_fd = pair.master.as_raw_fd().expect("master fd");
        // SAFETY: `pair.master` still owns `master_fd`, so it is open; dup only
        // creates a new fd and touches no memory.
        let dup_fd = unsafe { libc::dup(master_fd) };
        assert!(dup_fd >= 0, "dup failed");
        drop(pair.master);

        // SAFETY: `dup_fd` is a fresh fd, checked above, that nothing else owns,
        // so the File is its only owner and closes it exactly once.
        let mut file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
        let mut got = String::new();
        let mut buf = [0u8; 256];
        loop {
            match file.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if got.contains("MARKER") {
                        break;
                    }
                }
            }
        }
        let _ = child.wait();
        assert!(
            got.contains("MARKER"),
            "the child lost its terminal; got {got:?}"
        );
    }
}
