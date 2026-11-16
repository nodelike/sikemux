//! The one way the core starts a program. On the Mac and Linux it wraps std's and tokio's
//! `Command`. On Windows it starts the program itself, so it can hand the child only the handles
//! it was given and start it on a token other than the core's own.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Output};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub use std::process::{ChildStderr, ChildStdin, ChildStdout};

/// Where a child's standard input or output goes.
pub struct Stdio(platform::Stdio);

impl Stdio {
    pub fn null() -> Self {
        Self(platform::Stdio::null())
    }

    pub fn piped() -> Self {
        Self(platform::Stdio::piped())
    }

    pub fn inherit() -> Self {
        Self(platform::Stdio::inherit())
    }
}

macro_rules! stdio_from {
    ($($from:ty),* $(,)?) => {
        $(impl From<$from> for Stdio {
            fn from(from: $from) -> Self {
                Self(platform::Stdio::from(from))
            }
        })*
    };
}

stdio_from!(
    std::fs::File,
    std::io::PipeReader,
    std::io::PipeWriter,
    ChildStdin,
    ChildStdout,
    ChildStderr,
);

#[cfg(unix)]
stdio_from!(std::os::fd::OwnedFd);
#[cfg(windows)]
stdio_from!(std::os::windows::io::OwnedHandle);

macro_rules! setters {
    () => {
        pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
            self.inner.arg(arg);
            self
        }

        pub fn args<I, S>(&mut self, args: I) -> &mut Self
        where
            I: IntoIterator<Item = S>,
            S: AsRef<OsStr>,
        {
            self.inner.args(args);
            self
        }

        pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
            self.inner.env(key, value);
            self
        }

        pub fn envs<I, K, V>(&mut self, variables: I) -> &mut Self
        where
            I: IntoIterator<Item = (K, V)>,
            K: AsRef<OsStr>,
            V: AsRef<OsStr>,
        {
            self.inner.envs(variables);
            self
        }

        pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
            self.inner.env_remove(key);
            self
        }

        pub fn env_clear(&mut self) -> &mut Self {
            self.inner.env_clear();
            self
        }

        pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
            self.inner.current_dir(dir);
            self
        }

        pub fn stdin(&mut self, stdio: impl Into<Stdio>) -> &mut Self {
            self.inner.stdin(stdio.into().0);
            self
        }

        pub fn stdout(&mut self, stdio: impl Into<Stdio>) -> &mut Self {
            self.inner.stdout(stdio.into().0);
            self
        }

        pub fn stderr(&mut self, stdio: impl Into<Stdio>) -> &mut Self {
            self.inner.stderr(stdio.into().0);
            self
        }

        /// Windows' process creation flags, added to the `CREATE_NO_WINDOW` every child gets.
        #[cfg(windows)]
        pub fn creation_flags(&mut self, flags: u32) -> &mut Self {
            platform::command(&mut self.inner).creation_flags(flags);
            self
        }

        /// An argument passed to the child exactly as written, unquoted.
        #[cfg(windows)]
        pub fn raw_arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
            platform::command(&mut self.inner).raw_arg(arg);
            self
        }

        #[cfg(unix)]
        pub fn process_group(&mut self, group: i32) -> &mut Self {
            std::os::unix::process::CommandExt::process_group(
                platform::command(&mut self.inner),
                group,
            );
            self
        }

        /// Runs `hook` in the child after fork and before exec.
        ///
        /// # Safety
        ///
        /// As for std's `CommandExt::pre_exec`: the hook may call only async-signal-safe
        /// functions.
        #[cfg(unix)]
        pub unsafe fn pre_exec(
            &mut self,
            hook: impl FnMut() -> io::Result<()> + Send + Sync + 'static,
        ) -> &mut Self {
            // SAFETY: the caller holds up pre_exec's contract.
            unsafe {
                std::os::unix::process::CommandExt::pre_exec(
                    platform::command(&mut self.inner),
                    hook,
                );
            }
            self
        }

        pub fn get_program(&self) -> &OsStr {
            platform::command_ref(&self.inner).get_program()
        }

        pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
            platform::command_ref(&self.inner).get_args()
        }

        /// The variables this command sets, or removes with `None`, on top of what it inherits.
        pub fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
            platform::command_ref(&self.inner).get_envs()
        }

        pub fn get_current_dir(&self) -> Option<&Path> {
            platform::command_ref(&self.inner).get_current_dir()
        }
    };
}

/// A program to start, built like std's `Command`.
pub struct Command {
    inner: platform::Command,
}

impl Command {
    /// A command without the user's environment, for what reads that environment.
    #[allow(clippy::disallowed_methods)]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            inner: platform::Command::new(program.as_ref()),
        }
    }

    setters!();

    /// Starts the program on the pseudoconsole `console`, which stands in for its standard handles.
    #[cfg(windows)]
    pub fn pseudoconsole(
        &mut self,
        console: windows_sys::Win32::System::Console::HPCON,
    ) -> &mut Self {
        self.inner.pseudoconsole(console);
        self
    }

    /// Replaces this process with the program, returning only if that fails.
    #[cfg(unix)]
    pub fn exec(&mut self) -> io::Error {
        std::os::unix::process::CommandExt::exec(&mut self.inner)
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        platform::spawn(&mut self.inner).map(Child::new)
    }

    /// Runs the program to its end and collects its output. Stdin is empty unless set.
    pub fn output(&mut self) -> io::Result<Output> {
        platform::output(&mut self.inner)
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        platform::status(&mut self.inner)
    }

    /// The same command, to start from async code.
    pub fn into_async(self) -> AsyncCommand {
        AsyncCommand {
            inner: platform::into_async(self.inner),
        }
    }
}

/// A started program.
pub struct Child {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    inner: platform::Child,
}

impl Child {
    fn new(
        (inner, stdin, stdout, stderr): (
            platform::Child,
            Option<ChildStdin>,
            Option<ChildStdout>,
            Option<ChildStderr>,
        ),
    ) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
            inner,
        }
    }

    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        self.inner.wait()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    /// Stops the program by force. A program that has already exited is not an error.
    pub fn kill(&mut self) -> io::Result<()> {
        self.inner.kill()
    }

    pub fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        platform::wait_with_output(self.inner, self.stdout, self.stderr)
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for Child {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for Child {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: the process handle stays open for as long as the child.
        unsafe {
            std::os::windows::io::BorrowedHandle::borrow_raw(
                std::os::windows::io::AsRawHandle::as_raw_handle(self),
            )
        }
    }
}

/// A [`Command`] started from async code, built like tokio's `Command`.
pub struct AsyncCommand {
    inner: platform::AsyncCommand,
}

impl AsyncCommand {
    setters!();

    /// Stops the program by force when its [`AsyncChild`] is dropped before it exits.
    pub fn kill_on_drop(&mut self, kill: bool) -> &mut Self {
        self.inner.kill_on_drop(kill);
        self
    }

    pub fn spawn(&mut self) -> io::Result<AsyncChild> {
        platform::spawn_async(&mut self.inner).map(AsyncChild::new)
    }

    /// Runs the program to its end and collects its output. Stdin is empty unless set.
    pub async fn output(&mut self) -> io::Result<Output> {
        platform::output_async(&mut self.inner).await
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        platform::status_async(&mut self.inner).await
    }
}

/// A program started from async code.
pub struct AsyncChild {
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout: Option<tokio::process::ChildStdout>,
    pub stderr: Option<tokio::process::ChildStderr>,
    inner: platform::AsyncChild,
}

impl AsyncChild {
    fn new(
        (inner, stdin, stdout, stderr): (
            platform::AsyncChild,
            Option<tokio::process::ChildStdin>,
            Option<tokio::process::ChildStdout>,
            Option<tokio::process::ChildStderr>,
        ),
    ) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
            inner,
        }
    }

    /// The program's pid, until it has been waited for.
    pub fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        self.inner.wait().await
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    /// Starts stopping the program by force, without waiting for it to exit.
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.inner.start_kill()
    }

    /// Stops the program by force and waits for it to exit.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.inner.start_kill()?;
        self.wait().await.map(drop)
    }

    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        platform::wait_with_output_async(self.inner, self.stdout, self.stderr).await
    }
}
