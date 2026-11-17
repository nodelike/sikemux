use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use tokio::io::AsyncRead;
use tokio::sync::watch;

use super::{exit_words, kill_group, Lines, Process, Reaping};

pub type AgentInput = tokio::process::ChildStdin;
pub type AgentOutput = Box<dyn AsyncRead + Send + Unpin>;

/// The agent itself, whose exit is waited for on a thread of its own.
pub struct Exit(sikemux_process::command::Child);

pub struct Pipes {
    pub stdin: AgentInput,
    pub stdout: AgentOutput,
    pub stderr: AgentOutput,
}

/// A Windows core never hands its agents to a replacement, so nothing is ever released; the
/// type is here for the code the hand-over shares with Mac and Linux.
pub struct Released {
    pub pid: u32,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub pid: u32,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

impl Released {
    pub fn record(&self) -> AgentRecord {
        AgentRecord {
            pid: self.pid,
            stderr_tail: self.stderr_tail.clone(),
            unread: self.unread.clone(),
        }
    }
}

pub fn start(
    command: &mut sikemux_process::command::Command,
) -> std::io::Result<(u32, Pipes, Exit)> {
    let mut child = sikemux_process::spawn_tree(command)?;
    let pid = child.id();
    let pipes = (|| {
        let stdin = tokio::process::ChildStdin::from_std(child.stdin.take()?).ok()?;
        let stdout = tokio::process::ChildStdout::from_std(child.stdout.take()?).ok()?;
        let stderr = tokio::process::ChildStderr::from_std(child.stderr.take()?).ok()?;
        Some(Pipes {
            stdin,
            stdout: Box::new(stdout) as AgentOutput,
            stderr: Box::new(stderr) as AgentOutput,
        })
    })();
    match pipes {
        Some(pipes) => Ok((pid, pipes, Exit(child))),
        None => {
            kill_group(pid);
            let _ = child.kill();
            let _ = child.wait();
            Err(std::io::Error::other("the agent has no input or output"))
        }
    }
}

impl Process {
    pub async fn release(self, _lines: Lines) -> Result<Released, String> {
        Err("A Windows core does not hand its agents over".into())
    }
}

/// Stops an agent; the thread waiting on it collects its exit.
pub fn kill_and_reap(pid: u32) {
    kill_group(pid);
}

/// Waits for the agent to exit and says how it ended.
pub async fn reap(
    _pid: u32,
    exit: Exit,
    reaping: Arc<Mutex<Reaping>>,
    exited: watch::Sender<Option<String>>,
) {
    let Exit(mut child) = exit;
    let words = match tokio::task::spawn_blocking(move || child.wait()).await {
        Ok(Ok(status)) => exit_words(status.code()),
        _ => "The agent stopped".to_owned(),
    };
    if !reaping.locked().released {
        let _ = exited.send(Some(words));
    }
}
