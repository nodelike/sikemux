//! Reaching a core on this machine. On Windows the core listens on a named pipe
//! that only this user can open, since Windows has no sockets in the file system.

#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
pub use std::os::unix::net::UnixStream as LocalStream;

#[cfg(windows)]
mod windows {
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::runtime::{Builder, Runtime};

    const ERROR_PIPE_BUSY: i32 = 231;
    const BUSY_RETRY: Duration = Duration::from_millis(20);

    /// The pipe a core with this socket path listens on. The path only names the
    /// core, so the same configuration finds the same pipe on every platform.
    pub fn pipe_name(socket: &Path) -> String {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in socket.to_string_lossy().to_lowercase().bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        format!(r"\\.\pipe\sikemux-core-{hash:016x}")
    }

    /// Opens the pipe of the core at `socket`, waiting while every instance of it is busy.
    pub async fn open(socket: &Path) -> io::Result<NamedPipeClient> {
        let name = pipe_name(socket);
        loop {
            match ClientOptions::new().open(&name) {
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(BUSY_RETRY).await;
                }
                Ok(pipe) => return served_by_this_user(pipe),
                other => return other,
            }
        }
    }

    /// Anyone may create a pipe of this name before the core does, so a pipe is only
    /// trusted when the process serving it runs as the same user as this one.
    fn served_by_this_user(pipe: NamedPipeClient) -> io::Result<NamedPipeClient> {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Security::{
            EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
        };
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        fn user_of(process: HANDLE) -> io::Result<Vec<u8>> {
            let mut token: HANDLE = std::ptr::null_mut();
            // SAFETY: `token` is a live local that receives the handle, closed below.
            if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut size = 0;
            // SAFETY: a null buffer of size 0 only asks how many bytes are needed.
            unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
            let mut buffer = vec![0u8; size as usize];
            // SAFETY: `buffer` has the `size` bytes the call asked for.
            let read = unsafe {
                GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size)
            };
            let error = io::Error::last_os_error();
            // SAFETY: `token` was opened above and nothing else holds it.
            unsafe { CloseHandle(token) };
            if read == 0 {
                return Err(error);
            }
            Ok(buffer)
        }

        let refuse = || io::Error::new(io::ErrorKind::PermissionDenied, "another user's process serves this pipe");
        let mut server_id = 0;
        // SAFETY: the handle belongs to `pipe`, open for this call, and `server_id` is a live local.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_id) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the handle is closed before returning.
        let server = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server_id) };
        if server.is_null() {
            return Err(refuse());
        }
        let theirs = user_of(server);
        // SAFETY: `server` was opened above and nothing else holds it.
        unsafe { CloseHandle(server) };
        // SAFETY: the pseudo handle for this process needs no closing.
        let ours = user_of(unsafe { GetCurrentProcess() })?;
        let (theirs, ours) = (theirs?, ours);
        // SAFETY: each buffer starts with a TOKEN_USER whose sid points into the same buffer,
        // which stays alive through the call.
        let same = unsafe {
            EqualSid(
                (*theirs.as_ptr().cast::<TOKEN_USER>()).User.Sid,
                (*ours.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            )
        } != 0;
        if same {
            Ok(pipe)
        } else {
            Err(refuse())
        }
    }

    /// A blocking connection to a core, with the calls of `std`'s Unix stream.
    pub struct LocalStream {
        runtime: Runtime,
        pipe: NamedPipeClient,
        read_timeout: Option<Duration>,
        write_timeout: Option<Duration>,
    }

    impl LocalStream {
        pub fn connect(socket: &Path) -> io::Result<Self> {
            let runtime = Builder::new_current_thread().enable_all().build()?;
            let pipe = runtime.block_on(open(socket))?;
            Ok(Self {
                runtime,
                pipe,
                read_timeout: None,
                write_timeout: None,
            })
        }

        pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.read_timeout = check(timeout)?;
            Ok(())
        }

        pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.write_timeout = check(timeout)?;
            Ok(())
        }
    }

    fn check(timeout: Option<Duration>) -> io::Result<Option<Duration>> {
        match timeout {
            Some(Duration::ZERO) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot set a zero duration timeout",
            )),
            other => Ok(other),
        }
    }

    fn within<T>(
        runtime: &Runtime,
        timeout: Option<Duration>,
        work: impl std::future::Future<Output = io::Result<T>>,
    ) -> io::Result<T> {
        runtime.block_on(async {
            match timeout {
                Some(timeout) => tokio::time::timeout(timeout, work)
                    .await
                    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?,
                None => work.await,
            }
        })
    }

    impl Read for LocalStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            within(&self.runtime, self.read_timeout, self.pipe.read(buffer))
        }
    }

    impl Write for LocalStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            within(&self.runtime, self.write_timeout, self.pipe.write(buffer))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
