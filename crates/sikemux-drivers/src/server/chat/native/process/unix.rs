use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use tokio::net::unix::pipe;
use tokio::sync::{oneshot, watch};

use super::{exit_words, kill_group, Input, Lines, Process, Reaping, Writer};

/// How often the agent is looked at for having exited, besides whenever a
/// child of this process exits.
const REAP_POLL: std::time::Duration = std::time::Duration::from_secs(1);

pub type AgentInput = pipe::Sender;
pub type AgentOutput = pipe::Receiver;

/// The agent's exit is collected by its pid, so an adopted agent's is too.
pub struct Exit;

pub struct Pipes {
    pub stdin: AgentInput,
    pub stdout: AgentOutput,
    pub stderr: AgentOutput,
}

/// An agent let go of by this core: its pid and the pipes still open to it,
/// with what had been read from them but not yet used.
pub struct Released {
    pub pid: u32,
    pub stdin: OwnedFd,
    pub stdout: OwnedFd,
    pub stderr: OwnedFd,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

/// [`Released`] as the replacement core is told it: the pipes by their
/// descriptor numbers, which stay open across `exec`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub pid: u32,
    pub stdin: RawFd,
    pub stdout: RawFd,
    pub stderr: RawFd,
    pub stderr_tail: Vec<u8>,
    pub unread: Vec<u8>,
}

impl Released {
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

    pub fn fds(&self) -> [RawFd; 3] {
        [
            self.stdin.as_raw_fd(),
            self.stdout.as_raw_fd(),
            self.stderr.as_raw_fd(),
        ]
    }
}

pub fn start(command: &mut std::process::Command) -> std::io::Result<(u32, Pipes, Exit)> {
    let mut child = sikemux_process::spawn_tree(command)?;
    let pid = child.id();
    let pipes = (|| {
        let stdin = OwnedFd::from(child.stdin.take()?);
        let stdout = OwnedFd::from(child.stdout.take()?);
        let stderr = OwnedFd::from(child.stderr.take()?);
        Some((stdin, stdout, stderr))
    })();
    let Some((stdin, stdout, stderr)) = pipes else {
        kill_and_reap(pid);
        return Err(std::io::Error::other("the agent has no input or output"));
    };
    open(stdin, stdout, stderr)
        .map(|pipes| (pid, pipes, Exit))
        .inspect_err(|_| kill_and_reap(pid))
}

fn open(stdin: OwnedFd, stdout: OwnedFd, stderr: OwnedFd) -> std::io::Result<Pipes> {
    Ok(Pipes {
        stdin: pipe::Sender::from_owned_fd(stdin)?,
        stdout: pipe::Receiver::from_owned_fd(stdout)?,
        stderr: pipe::Receiver::from_owned_fd(stderr)?,
    })
}

impl Process {
    /// Takes up an agent an earlier core released, reading on where it
    /// stopped. The agent is a child of this process still, so its exit is
    /// collected as one.
    pub fn adopt(
        record: &AgentRecord,
        agent_id: &str,
        stdin: OwnedFd,
        stdout: OwnedFd,
        stderr: OwnedFd,
    ) -> Result<(Self, Writer, Lines), String> {
        crate::server::callers::started(record.pid, agent_id);
        let pipes = open(stdin, stdout, stderr)
            .map_err(|error| format!("The agent's pipes cannot be read: {error}"))?;
        Ok(Self::take_up(
            record.pid,
            pipes,
            Exit,
            record.stderr_tail.clone(),
            record.unread.clone(),
        ))
    }

    /// Lets go of the agent without stopping it: everything already sent to
    /// it is written first, and from then on nothing here writes to it, reads
    /// from it or collects its exit. Fails, and leaves the agent to be killed
    /// with the process, when the agent has exited or closed a pipe.
    pub async fn release(mut self, lines: Lines) -> Result<Released, String> {
        {
            let mut reaping = self.hand_over.reaping.locked();
            if self.exit.borrow().is_some() {
                return Err("The agent has exited".into());
            }
            reaping.released = true;
        }
        let (reply, stdin) = oneshot::channel();
        let _ = self.hand_over.input.send(Input::Release(reply));
        let stdin = stdin.await.ok().flatten();
        if let Some(stop) = self.hand_over.stop_stderr.take() {
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
    fn unrelease(&self) {
        self.hand_over.reaping.locked().released = false;
        let (exited, exit) = watch::channel(None);
        drop(exit);
        tokio::spawn(reap(self.pid, Exit, self.hand_over.reaping.clone(), exited));
    }
}

/// Kills an agent nothing here collects the exit of any more, and collects
/// it, so it does not linger as a zombie.
pub fn kill_and_reap(pid: u32) {
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

/// Collects the agent's exit and says how it ended, until the agent is
/// released to another core.
pub async fn reap(
    pid: u32,
    _exit: Exit,
    reaping: Arc<Mutex<Reaping>>,
    exited: watch::Sender<Option<String>>,
) {
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
                let status = std::process::ExitStatus::from_raw(raw);
                let _ = exited.send(Some(exit_words(status.code())));
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
