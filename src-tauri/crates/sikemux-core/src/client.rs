//! The core's client, and finding or starting the core on this machine.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

pub use sikemux_client::client::*;

use crate::protocol::frozen::{FrozenReply, FrozenRequest};
use crate::protocol::{
    encode_frozen, read_frame_sync, BuildIdentity, FrameKind, ServerMessage, PROTOCOL_VERSION,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub enum ProbeError {
    NotRunning(io::Error),
    Rejected { version: u32, pid: u32 },
    Unanswered(String),
}

pub fn probe(socket: &Path, timeout: Duration) -> Result<CoreHello, ProbeError> {
    let mut stream = StdUnixStream::connect(socket).map_err(ProbeError::NotRunning)?;
    let unanswered = |error: &dyn std::fmt::Display| ProbeError::Unanswered(error.to_string());
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| unanswered(&error))?;
    let hello = hello_frame().map_err(|error| unanswered(&error))?;
    stream
        .write_all(&hello)
        .map_err(|error| unanswered(&error))?;
    let frame = read_frame_sync(&mut stream)
        .map_err(|error| unanswered(&error))?
        .ok_or_else(|| ProbeError::Unanswered("closed without answering".into()))?;
    if frame.kind != FrameKind::Control {
        return Err(ProbeError::Unanswered("unexpected frame".into()));
    }
    let message = serde_json::from_slice::<ServerMessage>(&frame.payload)
        .map_err(|error| unanswered(&error))?;
    match hello_reply(message) {
        Ok(hello) => Ok(hello),
        Err(ClientError::VersionMismatch { version, pid, .. }) => {
            Err(ProbeError::Rejected { version, pid })
        }
        Err(error) => Err(unanswered(&error)),
    }
}

/// Sends one of the requests every core answers, whatever protocol it speaks.
/// An upgrade is answered before the core replaces itself.
pub fn frozen_request(
    socket: &Path,
    request: &FrozenRequest,
    timeout: Duration,
) -> Result<FrozenReply, ClientError> {
    let mut stream = StdUnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(&encode_frozen(request)?)?;
    let frame = read_frame_sync(&mut stream)?
        .ok_or_else(|| ClientError::Handshake("the core closed without answering".into()))?;
    if frame.kind != FrameKind::Frozen {
        return Err(ClientError::Handshake(
            "the core answered with another kind of frame".into(),
        ));
    }
    Ok(serde_json::from_slice(&frame.payload)?)
}

/// Waits for the core at `socket`, which accepted an upgrade while it was
/// `pid` running `old`, to answer again from the same process with another
/// build. The same build answering means the upgrade failed and the old core
/// carried on. Without `old`, the old core spoke another protocol, so any
/// answer from the same process is the new build.
pub fn await_upgrade(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
) -> Result<CoreHello, ClientError> {
    wait_for_new_build(socket, pid, old, timeout, false)
}

/// Like [`await_upgrade`] for an upgrade the core deferred: it goes on
/// answering as the old build until its chat turns end.
pub fn await_deferred_upgrade(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
) -> Result<CoreHello, ClientError> {
    wait_for_new_build(socket, pid, old, timeout, true)
}

fn wait_for_new_build(
    socket: &Path,
    pid: u32,
    old: Option<&BuildIdentity>,
    timeout: Duration,
    deferred: bool,
) -> Result<CoreHello, ClientError> {
    let deadline = Instant::now() + timeout;
    let is_old = |hello: &CoreHello| old.is_some_and(|old| hello.build.same_build(old));
    loop {
        let timed_out = Instant::now() >= deadline;
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) if hello.pid != pid => {
                return Err(ClientError::Core(format!(
                    "a new core (pid {}) answered instead of the upgraded one (pid {pid})",
                    hello.pid
                )))
            }
            Ok(hello) if !is_old(&hello) => return Ok(hello),
            Ok(_) if !deferred || timed_out => {
                return Err(ClientError::Core(
                    "the core could not replace itself and carried on as it was".into(),
                ))
            }
            Err(ProbeError::Rejected {
                pid: answered,
                version,
            }) if answered == pid && version != PROTOCOL_VERSION && (!deferred || timed_out) => {
                return Err(ClientError::VersionMismatch {
                    version,
                    pid,
                    message: format!(
                        "the upgraded core speaks protocol version {version}, not {PROTOCOL_VERSION}"
                    ),
                })
            }
            Err(_) if timed_out => {
                return Err(ClientError::Core(format!(
                    "the core did not come back within {timeout:?} of accepting an upgrade"
                )))
            }
            _ => std::thread::sleep(START_POLL),
        }
    }
}

/// Starts `<binary> core --socket <socket> <args>` in its own session,
/// detached from the caller, with its output appended to `log`.
fn start_detached(socket: &Path, binary: &Path, log: &Path, args: &[OsString]) -> io::Result<()> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    let mut command = sikemux_process::user_environment::command(binary);
    command
        .arg("core")
        .arg("--socket")
        .arg(socket)
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: the hook runs in the forked child before exec and calls only
    // fork, setsid and _exit, which are async-signal-safe. The intermediate
    // child exits at once, so the core is never the caller's child and is
    // never left a zombie.
    unsafe {
        command.pre_exec(|| match libc::fork() {
            -1 => Err(io::Error::last_os_error()),
            0 => {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
            _ => libc::_exit(0),
        });
    }
    command.spawn()?.wait()?;
    Ok(())
}

/// Finds the core at `socket`, or starts one with `args` after its socket.
pub fn ensure_running(
    socket: &Path,
    binary: &Path,
    log: &Path,
    args: &[OsString],
) -> Result<CoreHello, ClientError> {
    let reject = |version, pid| {
        ClientError::VersionMismatch {
        version,
        pid,
        message: format!(
            "the Sikemux core at {} (pid {pid}) speaks protocol version {version}, not {PROTOCOL_VERSION}",
            socket.display()
        ),
    }
    };
    match probe(socket, PROBE_TIMEOUT) {
        Ok(hello) => return Ok(hello),
        Err(ProbeError::Rejected { version, pid }) => return Err(reject(version, pid)),
        Err(ProbeError::NotRunning(_) | ProbeError::Unanswered(_)) => {}
    }
    start_detached(socket, binary, log, args)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) => return Ok(hello),
            Err(ProbeError::Rejected { version, pid }) => return Err(reject(version, pid)),
            Err(_) if Instant::now() >= deadline => {
                return Err(ClientError::StartTimeout(START_TIMEOUT))
            }
            Err(_) => std::thread::sleep(START_POLL),
        }
    }
}
