//! A session's end of its pseudo-terminal: reading what the program prints, typing to it, and
//! telling it the size of its window.

pub(crate) enum ReadOutcome {
    Bytes(usize),
    Eof,
}

#[cfg(unix)]
pub(crate) use unix::PtyIo;
#[cfg(windows)]
pub(crate) use windows::PtyIo;

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};

    use portable_pty::MasterPty;
    use tokio::io::unix::AsyncFd;

    use super::ReadOutcome;
    use crate::server::{CoreError, CoreResult};

    pub(crate) struct PtyIo(AsyncFd<File>);

    fn os_error() -> CoreError {
        CoreError::from(std::io::Error::last_os_error())
    }

    impl PtyIo {
        pub(crate) fn open(master: Box<dyn MasterPty + Send>) -> CoreResult<Self> {
            let master_fd = master
                .as_raw_fd()
                .ok_or_else(|| CoreError::from("pty: master pty has no fd"))?;
            // SAFETY: `master` still owns `master_fd`, so the fd is open, and
            // F_GETFL/F_SETFL only read and set its flags without touching memory.
            unsafe {
                let flags = libc::fcntl(master_fd, libc::F_GETFL);
                if flags < 0 {
                    return Err(os_error());
                }
                if libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    return Err(os_error());
                }
            }
            // One fd serves both directions: the dup shares the master's open file
            // description, so dropping portable_pty's master keeps the terminal alive.
            // SAFETY: `master` is dropped only after this line, so `master_fd` is
            // still open; F_DUPFD_CLOEXEC creates a new fd and touches no memory.
            let dup_fd = unsafe { libc::fcntl(master_fd, libc::F_DUPFD_CLOEXEC, 0) };
            if dup_fd < 0 {
                return Err(os_error());
            }
            drop(master);
            // SAFETY: `dup_fd` was just created and checked above and nothing else
            // holds it, so the File is its only owner and closes it exactly once.
            let io_file = unsafe { File::from_raw_fd(dup_fd) };
            // SAFETY: `io_file` owns `dup_fd` and the AsyncFd owns `io_file`, so the fd
            // stays open and the same until the AsyncFd is dropped.
            let io = unsafe { AsyncFd::register(io_file) }.map_err(std::io::Error::from)?;
            Ok(Self(io))
        }

        pub(crate) fn fd(&self) -> RawFd {
            self.0.get_ref().as_raw_fd()
        }

        pub(crate) async fn read(&self, buffer: &mut [u8]) -> ReadOutcome {
            loop {
                let Ok(mut guard) = self.0.readable().await else {
                    return ReadOutcome::Eof;
                };
                match guard.try_io(|inner| inner.get_ref().read(buffer)) {
                    Ok(Ok(0)) | Ok(Err(_)) => return ReadOutcome::Eof,
                    Ok(Ok(read)) => return ReadOutcome::Bytes(read),
                    Err(_would_block) => continue,
                }
            }
        }

        pub(crate) async fn write_all(&self, mut data: &[u8]) -> std::io::Result<()> {
            while !data.is_empty() {
                let mut guard = self.0.writable().await?;
                match guard.try_io(|inner| inner.get_ref().write(data)) {
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

        pub(crate) fn resize(&self, cols: u16, rows: u16) -> CoreResult<()> {
            let size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: the fd belongs to `self`, which stays open for this call,
            // and `size` is a live winsize on the stack, the struct TIOCSWINSZ reads.
            let rc = unsafe {
                libc::ioctl(
                    self.0.get_ref().as_raw_fd(),
                    libc::TIOCSWINSZ,
                    &size as *const libc::winsize,
                )
            };
            if rc != 0 {
                return Err(os_error());
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    use portable_pty::{MasterPty, PtySize};
    use tokio::sync::mpsc;

    use super::ReadOutcome;
    use crate::server::{CoreError, CoreResult};

    const CHUNK_BYTES: usize = 64 * 1024;
    /// How many chunks the reader thread may run ahead of the core, which keeps a program
    /// that prints without end from filling memory.
    const CHUNKS_AHEAD: usize = 16;

    struct Unread {
        chunks: mpsc::Receiver<Vec<u8>>,
        left: Vec<u8>,
        taken: usize,
    }

    /// ConPTY gives blocking pipes, not descriptors to wait on, so a thread reads the
    /// terminal and hands what it reads to the core.
    pub(crate) struct PtyIo {
        unread: tokio::sync::Mutex<Unread>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    }

    impl PtyIo {
        pub(crate) fn open(master: Box<dyn MasterPty + Send>) -> CoreResult<Self> {
            let mut reader = master.try_clone_reader().map_err(CoreError::pty)?;
            let writer = master.take_writer().map_err(CoreError::pty)?;
            let (sender, chunks) = mpsc::channel(CHUNKS_AHEAD);
            std::thread::Builder::new()
                .name("sikemux-pty-read".into())
                .spawn(move || {
                    let mut buffer = vec![0u8; CHUNK_BYTES];
                    loop {
                        match reader.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                let chunk = buffer.get(..read).unwrap_or_default().to_vec();
                                if sender.blocking_send(chunk).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                })
                .map_err(CoreError::from)?;
            Ok(Self {
                unread: tokio::sync::Mutex::new(Unread {
                    chunks,
                    left: Vec::new(),
                    taken: 0,
                }),
                writer: Arc::new(Mutex::new(writer)),
                master: Mutex::new(Some(master)),
            })
        }

        pub(crate) async fn read(&self, buffer: &mut [u8]) -> ReadOutcome {
            let mut unread = self.unread.lock().await;
            if unread.taken == unread.left.len() {
                match unread.chunks.recv().await {
                    Some(chunk) => {
                        unread.left = chunk;
                        unread.taken = 0;
                    }
                    None => return ReadOutcome::Eof,
                }
            }
            let rest = unread.left.get(unread.taken..).unwrap_or_default();
            let count = rest.len().min(buffer.len());
            if let (Some(to), Some(from)) = (buffer.get_mut(..count), rest.get(..count)) {
                to.copy_from_slice(from);
            }
            unread.taken += count;
            ReadOutcome::Bytes(count)
        }

        pub(crate) async fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
            let writer = self.writer.clone();
            let data = data.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut writer = writer
                    .lock()
                    .map_err(|_| std::io::Error::other("pty writer poisoned"))?;
                writer.write_all(&data)?;
                writer.flush()
            })
            .await
            .map_err(std::io::Error::other)?
        }

        pub(crate) fn resize(&self, cols: u16, rows: u16) -> CoreResult<()> {
            let master = self
                .master
                .lock()
                .map_err(|_| CoreError::from("pty: master poisoned"))?;
            let Some(master) = master.as_ref() else {
                return Ok(());
            };
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(CoreError::pty)
        }

        /// ConPTY ends its output only when the pseudoconsole closes, so a program that
        /// has exited is let go of here, which lets the reader see the end.
        pub(crate) fn close(&self) {
            if let Ok(mut master) = self.master.lock() {
                master.take();
            }
        }
    }
}
