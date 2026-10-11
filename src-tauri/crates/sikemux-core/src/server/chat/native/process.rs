//! A chat agent's process, talked to one JSON message per line over its
//! standard input and output.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use crate::protocol::ChatLaunch;

/// How much of what the agent printed to its error stream is kept, to say why
/// it stopped.
const STDERR_TAIL: usize = 16 * 1024;

/// Writes one message per line to the agent. Clones share the same pipe.
#[derive(Clone)]
pub(crate) struct Writer(mpsc::UnboundedSender<String>);

impl Writer {
    pub fn send(&self, message: &Value) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|error| error.to_string())?;
        self.0
            .send(line)
            .map_err(|_| "The agent is no longer reading its input".to_owned())
    }
}

/// The agent's process. Dropping it kills the agent and everything it started.
pub(crate) struct Process {
    pid: Option<u32>,
    exit: watch::Receiver<Option<String>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
}

impl Process {
    /// Starts `launch`'s program with `args` after its own, in the chat's
    /// folder, and answers with the process, its writer and its output lines.
    pub fn spawn(
        launch: &ChatLaunch,
        args: impl IntoIterator<Item = String>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(Self, Writer, Lines), String> {
        let mut command =
            Command::from(sikemux_process::user_environment::command(&launch.program));
        command
            .args(&launch.args)
            .args(args)
            .current_dir(&launch.cwd)
            .envs(sikemux_pty::user_shell::login_shell_locale())
            .envs(&launch.env)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|error| {
            format!("{} could not be started: {error}", launch.program.display())
        })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or("The agent has no input")?;
        let stdout = child.stdout.take().ok_or("The agent has no output")?;
        let stderr = child.stderr.take().ok_or("The agent has no error output")?;

        let (writer, mut lines) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = lines.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let kept = tail.clone();
        tokio::spawn(async move {
            let mut stderr = stderr;
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                if let Ok(mut kept) = kept.lock() {
                    kept.extend(&buffer[..read]);
                    let excess = kept.len().saturating_sub(STDERR_TAIL);
                    kept.drain(..excess);
                }
            }
        });

        let (exited, exit) = watch::channel(None);
        tokio::spawn(wait(child, exited));

        Ok((
            Self {
                pid,
                exit,
                stderr: tail,
            },
            Writer(writer),
            Lines(BufReader::new(stdout)),
        ))
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

    /// The last lines the agent wrote to its error stream.
    pub fn stderr(&self) -> String {
        let tail = self
            .stderr
            .lock()
            .map(|tail| tail.iter().copied().collect::<Vec<u8>>())
            .unwrap_or_default();
        String::from_utf8_lossy(&tail).trim().to_owned()
    }

    /// Why the agent stopped, in words for the chat.
    pub async fn ended(&self) -> String {
        let status = self.exited().await;
        let stderr = self.stderr();
        let last = stderr.lines().rev().find(|line| !line.trim().is_empty());
        match last {
            Some(line) => format!("{status}: {}", line.trim()),
            None => status,
        }
    }
}

async fn wait(mut child: Child, exited: watch::Sender<Option<String>>) {
    let status = match child.wait().await {
        Ok(status) => match status.code() {
            Some(0) => "The agent exited".to_owned(),
            Some(code) => format!("The agent exited with status {code}"),
            None => "The agent was stopped by a signal".to_owned(),
        },
        Err(error) => format!("The agent could not be watched: {error}"),
    };
    let _ = exited.send(Some(status));
}

impl Drop for Process {
    fn drop(&mut self) {
        // The agent leads its own process group, so this reaches the tools
        // and helpers it started as well as the agent itself.
        if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: killpg only sends a signal; a group that is already gone
            // makes it fail harmlessly.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
}

/// The agent's output, one JSON message per line.
pub(crate) struct Lines(BufReader<ChildStdout>);

impl Lines {
    /// The next message, skipping lines that are not JSON. None once the
    /// agent's output has closed.
    pub async fn next(&mut self) -> Option<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.0.read_line(&mut line).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str(trimmed) {
                Ok(message) => return Some(message),
                Err(_) => eprintln!(
                    "sikemux core: an agent wrote a line that is not JSON: {}",
                    trimmed.chars().take(200).collect::<String>()
                ),
            }
        }
    }
}
