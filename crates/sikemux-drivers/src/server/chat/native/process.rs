//! A chat agent's process, talked to one JSON message per line over its
//! standard input and output. Its pipes and pid can be let go of whole, for
//! a core that replaces this one in the same process to take up.

use sikemux_process::command::Stdio;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::git_guard;
use sikemux_wire::protocol::ChatLaunch;

#[cfg(unix)]
#[path = "process/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod platform;

pub use platform::{kill_and_reap, AgentRecord, Released};
use platform::{AgentInput, AgentOutput, Exit, Pipes};

/// How much of what the agent printed to its error stream is kept, to say why
/// it stopped.
const STDERR_TAIL: usize = 16 * 1024;
const READ_CHUNK: usize = 64 * 1024;

#[cfg_attr(windows, allow(dead_code))]
enum Input {
    Line(String),
    /// Stop writing once everything before this is written, and give the
    /// pipe back. None when the agent stopped reading.
    Release(oneshot::Sender<Option<AgentInput>>),
}

/// Writes one message per line to the agent. Clones share the same pipe.
#[derive(Clone)]
pub struct Writer(mpsc::UnboundedSender<Input>);

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

/// What letting go of the agent needs, which only Mac and Linux do.
#[cfg_attr(windows, allow(dead_code))]
struct HandOver {
    reaping: Arc<Mutex<Reaping>>,
    input: mpsc::UnboundedSender<Input>,
    stop_stderr: Option<oneshot::Sender<()>>,
}

/// The agent's process. Dropping it kills the agent and everything it
/// started, unless it was released.
pub struct Process {
    pid: u32,
    exit: watch::Receiver<Option<String>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    #[cfg_attr(windows, allow(dead_code))]
    hand_over: HandOver,
    stderr_task: JoinHandle<Option<AgentOutput>>,
    released: bool,
    /// How long the agent gets to shut down by itself before it is killed.
    grace: Option<Duration>,
}

impl Process {
    /// Starts `launch`'s program with `args` after its own, in the chat's
    /// folder, and answers with the process, its writer and its output lines.
    pub fn spawn(
        launch: &ChatLaunch,
        args: impl IntoIterator<Item = String>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(Self, Writer, Lines), String> {
        git_guard::shim::stand_in_for_heavy(&launch.cwd);
        let mut command =
            sikemux_process::user_environment::agent_command_in_own_cgroup(&launch.program);
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
        if git_guard::identity::note(&launch.cwd) {
            for name in sikemux_process::vars::PERSON_GITHUB_TOKENS {
                command.env_remove(name);
            }
            command.env(sikemux_process::vars::GITHUB_TOKENS_WITHHELD, "1");
        }
        let (pid, pipes, exit) =
            platform::start(&mut command).map_err(|error| match launch.cwd.is_dir() {
                true => format!("{} could not be started: {error}", launch.program.display()),
                false => format!(
                    "the agent's folder {} is gone, so it could not be started there",
                    launch.cwd.display()
                ),
            })?;
        crate::server::callers::started(pid, &launch.agent_id);
        Ok(Self::take_up(pid, pipes, exit, Vec::new(), Vec::new()))
    }

    fn take_up(
        pid: u32,
        pipes: Pipes,
        agent: Exit,
        stderr_tail: Vec<u8>,
        unread: Vec<u8>,
    ) -> (Self, Writer, Lines) {
        let Pipes {
            stdin,
            stdout,
            stderr,
        } = pipes;
        let (input, queue) = mpsc::unbounded_channel();
        tokio::spawn(write_lines(stdin, queue));

        let tail = Arc::new(Mutex::new(VecDeque::from(stderr_tail)));
        let (stop_stderr, stopped) = oneshot::channel();
        let stderr_task = tokio::spawn(keep_tail(stderr, tail.clone(), stopped));

        let reaping = Arc::new(Mutex::new(Reaping { released: false }));
        let (exited, exit) = watch::channel(None);
        tokio::spawn(platform::reap(pid, agent, reaping.clone(), exited));

        (
            Self {
                pid,
                exit,
                stderr: tail,
                hand_over: HandOver {
                    reaping,
                    input: input.clone(),
                    stop_stderr: Some(stop_stderr),
                },
                stderr_task,
                released: false,
                grace: None,
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

    /// When stopped, the agent is asked to quit and only killed if it has not
    /// after `grace`, for an agent that writes what it must keep as it quits.
    pub fn stop_gently(&mut self, grace: Duration) {
        self.grace = Some(grace);
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
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
        if self.released {
            return;
        }
        let waits = self.grace.zip(tokio::runtime::Handle::try_current().ok());
        let Some((grace, runtime)) = waits else {
            kill_group(self.pid);
            return;
        };
        sikemux_process::terminate_tree(self.pid, false);
        let (pid, mut exit) = (self.pid, self.exit.clone());
        runtime.spawn(async move {
            if tokio::time::timeout(grace, exit.wait_for(Option::is_some))
                .await
                .is_err()
            {
                kill_group(pid);
            }
        });
    }
}

/// The agent leads its own process group, so this reaches the tools and
/// helpers it started as well as the agent itself.
pub fn kill_group(pid: u32) {
    sikemux_process::terminate_tree(pid, true);
}

async fn write_lines(mut pipe: AgentInput, mut queue: mpsc::UnboundedReceiver<Input>) {
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
    mut pipe: AgentOutput,
    tail: Arc<Mutex<VecDeque<u8>>>,
    mut stop: oneshot::Receiver<()>,
) -> Option<AgentOutput> {
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

fn exit_words(code: Option<i32>) -> String {
    match code {
        Some(0) => "The agent exited".to_owned(),
        Some(code) => format!("The agent exited with status {code}"),
        None => "The agent was stopped by a signal".to_owned(),
    }
}

/// The agent's output, one JSON message per line. What has been read but not
/// yet made a whole line is kept here, so a read given up part way loses
/// nothing and a release hands it on.
pub struct Lines {
    pipe: AgentOutput,
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

    /// The next message, skipping lines that are not one, with what is wrong
    /// with them. None once the agent's output has closed.
    pub async fn next<M: DeserializeOwned>(&mut self) -> Option<M> {
        loop {
            while let Some(line) = self.take_line() {
                let trimmed = line.trim_ascii();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_slice(trimmed) {
                    Ok(message) => return Some(message),
                    Err(error) => eprintln!(
                        "sikemux core: an agent wrote a line Sikemux cannot read ({error}): {}",
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
pub trait Reads: Send + 'static {
    /// Something the chat's side hands the reader to act on between messages.
    type Nudge: Send + 'static;
    /// What each line of the agent's output is read as.
    type Message: DeserializeOwned;

    fn message(&mut self, message: Self::Message);

    fn nudged(&mut self, _nudge: Self::Nudge) {}

    /// The agent's output closed.
    fn closed(&mut self);
}

/// A task reading an agent's output, which can be stopped between messages
/// to have the reader and the output back.
pub struct Reading<R> {
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

/// Lets go of an agent without stopping it: stops reading it, turns what the
/// reader leaves into `state`, and hands the process over. On failure the agent
/// is stopped. `name` says who stopped, should its output already have closed.
pub async fn hand_over<R: Reads, State: Serialize>(
    process: Process,
    reading: Reading<R>,
    name: &str,
    state: impl FnOnce(R) -> State + Send,
) -> Result<(Value, Released), String> {
    let (reader, lines) = reading
        .stop()
        .await
        .ok_or_else(|| format!("{name} stopped"))?;
    let state = serde_json::to_value(state(reader)).map_err(|error| error.to_string())?;
    let released = process.release(lines).await?;
    Ok((state, released))
}
