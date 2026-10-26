//! A chat agent's process, talked to one JSON message per line over its
//! standard input and output. Its pipes and pid can be let go of whole, for
//! a core that replaces this one in the same process to take up.

use std::collections::VecDeque;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::git_guard;
use crate::protocol::ChatLaunch;

/// How much of what the agent printed to its error stream is kept, to say why
/// it stopped.
const STDERR_TAIL: usize = 16 * 1024;
/// How often the agent is looked at for having exited, besides whenever a
/// child of this process exits.
const REAP_POLL: Duration = Duration::from_secs(1);
const READ_CHUNK: usize = 64 * 1024;

/// A pipe to an agent, let go of whole when a core hands its agents to the next one. Windows
/// has no such hand-over, so there the pipes are only ever held, never passed on.
#[cfg(unix)]
pub(crate) type Pipe = std::os::fd::OwnedFd;
#[cfg(unix)]
type RawPipe = std::os::fd::RawFd;
#[cfg(windows)]
pub(crate) type Pipe = std::os::windows::io::OwnedHandle;
#[cfg(windows)]
type RawPipe = i32;

#[cfg(unix)]
type PipeSender = tokio::net::unix::pipe::Sender;
#[cfg(unix)]
type PipeReceiver = tokio::net::unix::pipe::Receiver;
#[cfg(windows)]
type PipeSender = tokio::process::ChildStdin;
#[cfg(windows)]
type PipeReceiver = Box<dyn tokio::io::AsyncRead + Send + Unpin>;

enum Input {
    Line(String),
    /// Stop writing once everything before this is written, and give the
    /// pipe back. None when the agent stopped reading.
    Release(oneshot::Sender<Option<PipeSender>>),
}

/// Writes one message per line to the agent. Clones share the same pipe.
#[derive(Clone)]
pub(crate) struct Writer(mpsc::UnboundedSender<Input>);

impl Writer {
    pub fn send(&self, message: &Value) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|error| error.to_string())?;
        self.0
            .send(Input::Line(line))
            .map_err(|_| "The agent is no longer reading its input".to_owned())
    }
}

/// Whether the agent's exit is still this run's to collect.
struct Reaping {
    released: bool,
}

/// An agent let go of by this core: its pid and the pipes still open to it,
/// with what had been read from them but not yet used.
pub(crate) struct Released {
    pub pid: u32,
    pub stdin: Pipe,
    pub stdout: Pipe,
    pub stderr: Pipe,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

/// [`Released`] as the replacement core is told it: the pipes by their
/// descriptor numbers, which stay open across `exec`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRecord {
    pub pid: u32,
    pub stdin: RawPipe,
    pub stdout: RawPipe,
    pub stderr: RawPipe,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

impl Released {
    #[cfg(windows)]
    pub fn record(&self) -> AgentRecord {
        AgentRecord {
            pid: self.pid,
            stdin: -1,
            stdout: -1,
            stderr: -1,
            stderr_tail: self.stderr_tail.clone(),
            unread: self.unread.clone(),
        }
    }

    #[cfg(unix)]
    pub fn record(&self) -> AgentRecord {
        AgentRecord {
            pid: self.pid,
            stdin: self.stdin.as_raw_fd(),
            stdout: self.stdout.as_raw_fd(),
            stderr: self.stderr.as_raw_fd(),
            stderr_tail: self.stderr_tail.clone(),
            unread: self.unread.clone(),
        }
    }

    #[cfg(unix)]
    pub fn fds(&self) -> [RawPipe; 3] {
        [
            self.stdin.as_raw_fd(),
            self.stdout.as_raw_fd(),
            self.stderr.as_raw_fd(),
        ]
    }
}

/// The agent's process. Dropping it kills the agent and everything it
/// started, unless it was released.
pub(crate) struct Process {
    pid: u32,
    exit: watch::Receiver<Option<String>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    reaping: Arc<Mutex<Reaping>>,
    input: mpsc::UnboundedSender<Input>,
    stop_stderr: Option<oneshot::Sender<()>>,
    stderr_task: JoinHandle<Option<PipeReceiver>>,
    released: bool,
}

impl Process {
    /// Starts `launch`'s program with `args` after its own, in the chat's
    /// folder, and answers with the process, its writer and its output lines.
    pub fn spawn(
        launch: &ChatLaunch,
        args: impl IntoIterator<Item = String>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(Self, Writer, Lines), String> {
        let mut command = sikemux_process::user_environment::agent_command(&launch.program);
        command
            .args(git_guard::shim::arguments(&launch.provider))
            .args(&launch.args)
            .args(args)
            .current_dir(&launch.cwd)
            .envs(sikemux_pty::user_shell::login_shell_locale())
            .envs(&launch.env)
            .envs(env)
            .envs(git_guard::shim::environment(
                launch.env.get("PATH").map(String::as_str),
            ))
            .envs(git_guard::homes::environment(
                &launch.provider,
                &launch.agent_id,
                false,
                |key| launch.env.get(key).map(String::as_str),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
            let mut child = command.spawn().map_err(|error| {
                format!("{} could not be started: {error}", launch.program.display())
            })?;
            let pid = child.id();
            let pipes = (|| {
                let stdin = Pipe::from(child.stdin.take()?);
                let stdout = Pipe::from(child.stdout.take()?);
                let stderr = Pipe::from(child.stderr.take()?);
                Some((stdin, stdout, stderr))
            })();
            let Some((stdin, stdout, stderr)) = pipes else {
                kill_and_reap(pid);
                return Err("The agent has no input or output".into());
            };
            crate::server::callers::started(pid, &launch.agent_id);
            Self::take_up(pid, stdin, stdout, stderr, Vec::new(), Vec::new())
                .inspect_err(|_| kill_and_reap(pid))
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
            let mut child = tokio::process::Command::from(command)
                .spawn()
                .map_err(|error| {
                    format!("{} could not be started: {error}", launch.program.display())
                })?;
            let pid = child.id().unwrap_or_default();
            let (Some(stdin), Some(stdout), Some(stderr)) =
                (child.stdin.take(), child.stdout.take(), child.stderr.take())
            else {
                kill_and_reap(pid);
                return Err("The agent has no input or output".into());
            };
            crate::server::callers::started(pid, &launch.agent_id);
            let (exited, exit) = watch::channel(None);
            tokio::spawn(wait_for_exit(child, exited));
            Ok(Self::assemble(
                pid,
                stdin,
                Box::new(stdout),
                Box::new(stderr),
                Vec::new(),
                Vec::new(),
                Arc::new(Mutex::new(Reaping { released: false })),
                exit,
            ))
        }
    }

    /// Takes up an agent an earlier core released, reading on where it
    /// stopped. The agent is a child of this process still, so its exit is
    /// collected as one.
    #[cfg(windows)]
    pub fn adopt(
        _record: &AgentRecord,
        _agent_id: &str,
        _stdin: Pipe,
        _stdout: Pipe,
        _stderr: Pipe,
    ) -> Result<(Self, Writer, Lines), String> {
        Err("Windows keeps an agent with the core that started it".into())
    }

    #[cfg(unix)]
    pub fn adopt(
        record: &AgentRecord,
        agent_id: &str,
        stdin: Pipe,
        stdout: Pipe,
        stderr: Pipe,
    ) -> Result<(Self, Writer, Lines), String> {
        crate::server::callers::started(record.pid, agent_id);
        Self::take_up(
            record.pid,
            stdin,
            stdout,
            stderr,
            record.stderr_tail.clone(),
            record.unread.clone(),
        )
    }

    #[cfg(unix)]
    fn take_up(
        pid: u32,
        stdin: Pipe,
        stdout: Pipe,
        stderr: Pipe,
        stderr_tail: Vec<u8>,
        unread: Vec<u8>,
    ) -> Result<(Self, Writer, Lines), String> {
        let pipe_error =
            |error: std::io::Error| format!("The agent's pipes cannot be read: {error}");
        let stdin = PipeSender::from_owned_fd(stdin).map_err(pipe_error)?;
        let stdout = PipeReceiver::from_owned_fd(stdout).map_err(pipe_error)?;
        let stderr = PipeReceiver::from_owned_fd(stderr).map_err(pipe_error)?;
        let reaping = Arc::new(Mutex::new(Reaping { released: false }));
        let (exited, exit) = watch::channel(None);
        tokio::spawn(reap(pid, reaping.clone(), exited));
        Ok(Self::assemble(
            pid,
            stdin,
            stdout,
            stderr,
            stderr_tail,
            unread,
            reaping,
            exit,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        pid: u32,
        stdin: PipeSender,
        stdout: PipeReceiver,
        stderr: PipeReceiver,
        stderr_tail: Vec<u8>,
        unread: Vec<u8>,
        reaping: Arc<Mutex<Reaping>>,
        exit: watch::Receiver<Option<String>>,
    ) -> (Self, Writer, Lines) {
        let (input, queue) = mpsc::unbounded_channel();
        tokio::spawn(write_lines(stdin, queue));

        let tail = Arc::new(Mutex::new(VecDeque::from(stderr_tail)));
        let (stop_stderr, stopped) = oneshot::channel();
        let stderr_task = tokio::spawn(keep_tail(stderr, tail.clone(), stopped));

        (
            Self {
                pid,
                exit,
                stderr: tail,
                reaping,
                input: input.clone(),
                stop_stderr: Some(stop_stderr),
                stderr_task,
                released: false,
            },
            Writer(input),
            Lines {
                pipe: stdout,
                buffer: unread,
                scanned: 0,
                closed: false,
            },
        )
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Resolves once the process has exited, with how it ended.
    pub async fn exited(&self) -> String {
        let mut exit = self.exit.clone();
        let status = match exit.wait_for(Option::is_some).await {
            Ok(status) => status.clone().unwrap_or_default(),
            Err(_) => "The agent stopped".to_owned(),
        };
        status
    }

    /// Stops the agent and waits until it has exited.
    pub async fn stop(&self) {
        kill_group(self.pid);
        self.exited().await;
    }

    /// The last lines the agent wrote to its error stream.
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr_bytes())
            .trim()
            .to_owned()
    }

    fn stderr_bytes(&self) -> Vec<u8> {
        self.stderr.locked().iter().copied().collect()
    }

    /// `error`, with the last thing the agent wrote to its error stream.
    pub fn start_error(&self, error: String) -> String {
        let stderr = self.stderr();
        match stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            Some(line) => format!("{error}: {line}"),
            None => error,
        }
    }

    /// Why the agent stopped, in words for the chat.
    pub async fn ended(&self) -> String {
        self.start_error(self.exited().await)
    }

    /// Lets go of the agent without stopping it: everything already sent to
    /// it is written first, and from then on nothing here writes to it, reads
    /// from it or collects its exit. Fails, and leaves the agent to be killed
    /// with the process, when the agent has exited or closed a pipe.
    #[cfg(windows)]
    pub async fn release(self, _lines: Lines) -> Result<Released, String> {
        Err("Windows keeps an agent with the core that started it".into())
    }

    #[cfg(unix)]
    pub async fn release(mut self, lines: Lines) -> Result<Released, String> {
        {
            let mut reaping = self.reaping.locked();
            if self.exit.borrow().is_some() {
                return Err("The agent has exited".into());
            }
            reaping.released = true;
        }
        let (reply, stdin) = oneshot::channel();
        let _ = self.input.send(Input::Release(reply));
        let stdin = stdin.await.ok().flatten();
        if let Some(stop) = self.stop_stderr.take() {
            let _ = stop.send(());
        }
        let stderr = (&mut self.stderr_task).await.ok().flatten();
        let (Some(stdin), Some(stderr)) = (stdin, stderr) else {
            self.unrelease();
            return Err("The agent closed its input".into());
        };
        let (stdin, stdout, stderr) = match (
            stdin.into_nonblocking_fd(),
            lines.pipe.into_nonblocking_fd(),
            stderr.into_nonblocking_fd(),
        ) {
            (Ok(stdin), Ok(stdout), Ok(stderr)) => (stdin, stdout, stderr),
            _ => {
                self.unrelease();
                return Err("The agent's pipes could not be let go of".into());
            }
        };
        self.released = true;
        Ok(Released {
            pid: self.pid,
            stdin,
            stdout,
            stderr,
            stderr_tail: self.stderr_bytes(),
            unread: lines.buffer,
        })
    }

    /// A release that failed leaves the agent to be killed, and its exit
    /// collected, as if it had never been tried.
    #[cfg(unix)]
    fn unrelease(&self) {
        self.reaping.locked().released = false;
        let (exited, exit) = watch::channel(None);
        drop(exit);
        tokio::spawn(reap(self.pid, self.reaping.clone(), exited));
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
        if !self.released {
            kill_group(self.pid);
        }
    }
}

/// Kills an agent nothing here collects the exit of any more, and collects
/// it, so it does not linger as a zombie.
#[cfg(unix)]
pub(crate) fn kill_and_reap(pid: u32) {
    kill_group(pid);
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    std::thread::spawn(move || {
        let mut raw = 0;
        // SAFETY: waitpid writes one int status into `raw`; a pid that is not
        // this process's child makes it fail at once.
        while unsafe { libc::waitpid(pid, &mut raw, 0) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {}
    });
}

#[cfg(windows)]
pub(crate) fn kill_and_reap(pid: u32) {
    kill_group(pid);
}

/// The agent leads its own process group, so this reaches the tools and
/// helpers it started as well as the agent itself.
#[cfg(unix)]
pub(crate) fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: killpg only sends a signal; a group that is already gone
        // makes it fail harmlessly.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
pub(crate) fn kill_group(pid: u32) {
    if pid != 0 {
        sikemux_pty::process::terminate_process_tree(pid, true);
    }
}

async fn write_lines(mut pipe: PipeSender, mut queue: mpsc::UnboundedReceiver<Input>) {
    let mut open = true;
    while let Some(input) = queue.recv().await {
        match input {
            Input::Line(mut line) => {
                line.push('\n');
                if open && pipe.write_all(line.as_bytes()).await.is_err() {
                    open = false;
                }
            }
            Input::Release(reply) => {
                let _ = reply.send(open.then_some(pipe));
                return;
            }
        }
    }
}

async fn keep_tail(
    mut pipe: PipeReceiver,
    tail: Arc<Mutex<VecDeque<u8>>>,
    mut stop: oneshot::Receiver<()>,
) -> Option<PipeReceiver> {
    let mut buffer = [0u8; 4096];
    loop {
        tokio::select! {
            stopped = &mut stop => return stopped.ok().map(|()| pipe),
            read = pipe.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut kept = tail.locked();
                    kept.extend(&buffer[..read]);
                    let excess = kept.len().saturating_sub(STDERR_TAIL);
                    kept.drain(..excess);
                }
            },
        }
    }
    stop.await.ok().map(|()| pipe)
}

#[cfg(unix)]
fn exit_words(raw: libc::c_int) -> String {
    words_for(std::process::ExitStatus::from_raw(raw))
}

fn words_for(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "The agent exited".to_owned(),
        Some(code) => format!("The agent exited with status {code}"),
        None => "The agent was stopped by a signal".to_owned(),
    }
}

/// Says how the agent ended, once its exit is collected.
#[cfg(windows)]
async fn wait_for_exit(
    mut child: tokio::process::Child,
    exited: watch::Sender<Option<String>>,
) {
    let words = match child.wait().await {
        Ok(status) => words_for(status),
        Err(_) => "The agent stopped".to_owned(),
    };
    let _ = exited.send(Some(words));
}

/// Collects the agent's exit and says how it ended, until the agent is
/// released to another core.
#[cfg(unix)]
async fn reap(pid: u32, reaping: Arc<Mutex<Reaping>>, exited: watch::Sender<Option<String>>) {
    let mut children = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()).ok();
    loop {
        {
            let reaping = reaping.locked();
            if reaping.released {
                return;
            }
            let mut raw = 0;
            // SAFETY: waitpid writes one int status into `raw`.
            let reaped = unsafe { libc::waitpid(pid as libc::pid_t, &mut raw, libc::WNOHANG) };
            if reaped > 0 {
                let _ = exited.send(Some(exit_words(raw)));
                return;
            }
            if reaped < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                let _ = exited.send(Some("The agent stopped".to_owned()));
                return;
            }
        }
        match children.as_mut() {
            Some(children) => {
                tokio::select! {
                    _ = children.recv() => {}
                    () = tokio::time::sleep(REAP_POLL) => {}
                }
            }
            None => tokio::time::sleep(REAP_POLL).await,
        }
    }
}

/// The agent's output, one JSON message per line. What has been read but not
/// yet made a whole line is kept here, so a read given up part way loses
/// nothing and a release hands it on.
pub(crate) struct Lines {
    pipe: PipeReceiver,
    buffer: Vec<u8>,
    /// How far into `buffer` holds no line break.
    scanned: usize,
    closed: bool,
}

impl Lines {
    fn take_line(&mut self) -> Option<Vec<u8>> {
        let found = self.buffer[self.scanned..]
            .iter()
            .position(|byte| *byte == b'\n');
        match found {
            Some(at) => {
                let end = self.scanned + at;
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                self.scanned = 0;
                Some(line)
            }
            None if self.closed && !self.buffer.is_empty() => {
                self.scanned = 0;
                Some(std::mem::take(&mut self.buffer))
            }
            None => {
                self.scanned = self.buffer.len();
                None
            }
        }
    }

    /// The next message, skipping lines that are not JSON. None once the
    /// agent's output has closed.
    pub async fn next(&mut self) -> Option<Value> {
        loop {
            while let Some(line) = self.take_line() {
                let trimmed = line.trim_ascii();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_slice(trimmed) {
                    Ok(message) => return Some(message),
                    Err(_) => eprintln!(
                        "sikemux core: an agent wrote a line that is not JSON: {}",
                        String::from_utf8_lossy(trimmed)
                            .chars()
                            .take(200)
                            .collect::<String>()
                    ),
                }
            }
            if self.closed {
                return None;
            }
            self.buffer.reserve(READ_CHUNK);
            if let Ok(0) | Err(_) = self.pipe.read_buf(&mut self.buffer).await {
                self.closed = true;
            }
        }
    }
}

/// What reads an agent's output, one message at a time.
pub(crate) trait Reads: Send + 'static {
    /// Something the chat's side hands the reader to act on between messages.
    type Nudge: Send + 'static;

    fn message(&mut self, message: Value);

    fn nudged(&mut self, _nudge: Self::Nudge) {}

    /// The agent's output closed.
    fn closed(&mut self);
}

/// A task reading an agent's output, which can be stopped between messages
/// to have the reader and the output back.
pub(crate) struct Reading<R> {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Option<(R, Lines)>>,
}

impl<R: Reads> Reading<R> {
    pub fn start(mut reader: R, mut lines: Lines) -> (Self, mpsc::UnboundedSender<R::Nudge>) {
        let (nudge, mut nudges) = mpsc::unbounded_channel();
        let (stop, mut stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => return Some((reader, lines)),
                    Some(nudge) = nudges.recv() => reader.nudged(nudge),
                    message = lines.next() => match message {
                        Some(message) => reader.message(message),
                        None => break,
                    },
                }
            }
            reader.closed();
            None
        });
        (
            Self {
                stop: Some(stop),
                task,
            },
            nudge,
        )
    }

    /// Stops reading after the message being read now, answering with the
    /// reader and what is still to be read. None once the output has closed.
    pub async fn stop(mut self) -> Option<(R, Lines)> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        (&mut self.task).await.ok().flatten()
    }
}

impl<R> Drop for Reading<R> {
    fn drop(&mut self) {
        self.task.abort();
    }
}
