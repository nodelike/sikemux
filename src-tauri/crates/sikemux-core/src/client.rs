//! The core's client, and finding or starting the core on this machine.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::process::CommandExt;
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;
use std::time::{Duration, Instant};

pub use sikemux_client::client::*;
use sikemux_client::local::LocalStream;

use crate::frozen::{FrozenReply, FrozenRequest};
use crate::protocol::{
    encode_frozen, read_frame_sync, BuildIdentity, FrameKind, ServerMessage, PROTOCOL_VERSION,
};
use crate::waiting;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum ProbeError {
    NotRunning(io::Error),
    Rejected { version: u32, pid: u32 },
    Unanswered(String),
}

pub fn probe(socket: &Path, timeout: Duration) -> Result<CoreHello, ProbeError> {
    let mut stream = LocalStream::connect(socket).map_err(ProbeError::NotRunning)?;
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
    let mut stream = LocalStream::connect(socket)?;
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
/// answering as the old build until its chats can be carried over.
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
    let settled = waiting::wait_until(socket, deadline, || {
        let timed_out = Instant::now() >= deadline;
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) if hello.pid != pid => Some(Err(ClientError::Core(format!(
                "a new core (pid {}) answered instead of the upgraded one (pid {pid})",
                hello.pid
            )))),
            Ok(hello) if !is_old(&hello) => Some(Ok(hello)),
            Ok(_) if !deferred || timed_out => Some(Err(ClientError::Core(
                "the core could not replace itself and carried on as it was".into(),
            ))),
            Err(ProbeError::Rejected {
                pid: answered,
                version,
            }) if answered == pid && version != PROTOCOL_VERSION && (!deferred || timed_out) => {
                Some(Err(ClientError::VersionMismatch {
                    version,
                    pid,
                    message: format!(
                        "the upgraded core speaks protocol version {version}, not {PROTOCOL_VERSION}"
                    ),
                }))
            }
            _ => None,
        }
    })?;
    settled.unwrap_or_else(|| {
        Err(ClientError::Core(format!(
            "the core did not come back within {timeout:?} of accepting an upgrade"
        )))
    })
}

/// Starts `<binary> core --socket <socket> <args>` in its own session,
/// detached from the caller, with its output appended to `log`.
fn start_detached(
    socket: &Path,
    binary: &Path,
    log_path: &Path,
    args: &[OsString],
) -> io::Result<()> {
    if let Some(folder) = log_path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut log = OpenOptions::new();
    log.create(true).append(true);
    #[cfg(unix)]
    log.mode(0o600);
    let log = log.open(log_path)?;
    let mut command = sikemux_process::user_environment::command(binary);
    command.arg("core").arg("--socket").arg(socket).args(args);
    #[cfg(target_os = "macos")]
    {
        spawn_responsible(&command, &log)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        spawn_double_forked(command, log)
    }
    #[cfg(windows)]
    {
        spawn_detached_process(command, log)
    }
}

#[cfg(windows)]
fn spawn_detached_process(
    mut command: std::process::Command,
    log: std::fs::File,
) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command.spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_double_forked(mut command: std::process::Command, log: std::fs::File) -> io::Result<()> {
    command
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

#[cfg(target_os = "macos")]
extern "C" {
    fn responsibility_spawnattrs_setdisclaim(
        attrs: *mut libc::posix_spawnattr_t,
        disclaim: libc::c_int,
    ) -> libc::c_int;
}

/// macOS charges a permission such as Screen Recording to the process
/// responsible for the one asking. Without this, every shell and agent the
/// core starts answers for itself, and the person's grant to Sikemux never
/// reaches them.
#[cfg(target_os = "macos")]
fn spawn_responsible(command: &std::process::Command, log: &std::fs::File) -> io::Result<()> {
    use std::collections::BTreeMap;
    use std::ffi::{CString, OsStr};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    const POSIX_SPAWN_SETSID: libc::c_int = 0x0400;

    let c_string = |value: &OsStr| CString::new(value.as_bytes()).map_err(io::Error::other);
    let program = c_string(command.get_program())?;
    let args = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(c_string)
        .collect::<io::Result<Vec<_>>>()?;
    let mut variables: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => variables.insert(key.to_owned(), value.to_owned()),
            None => variables.remove(key),
        };
    }
    let environment = variables
        .into_iter()
        .map(|(mut entry, value)| {
            entry.push("=");
            entry.push(value);
            c_string(&entry)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let pointers = |strings: &[CString]| {
        strings
            .iter()
            .map(|string| string.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect::<Vec<_>>()
    };
    let (argv, envp) = (pointers(&args), pointers(&environment));
    let check = |code: libc::c_int| match code {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(code)),
    };

    let mut pid: libc::pid_t = 0;
    // SAFETY: the attributes and file actions are initialised before use and
    // destroyed once; every pointer handed to posix_spawn points into the
    // vectors above, which outlive the call, and both lists end in null.
    let spawned = unsafe {
        let mut attrs: libc::posix_spawnattr_t = std::ptr::null_mut();
        let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
        check(libc::posix_spawnattr_init(&mut attrs))?;
        check(libc::posix_spawn_file_actions_init(&mut actions))?;
        let spawned = (|| {
            check(libc::posix_spawnattr_setflags(
                &mut attrs,
                (POSIX_SPAWN_SETSID | libc::POSIX_SPAWN_CLOEXEC_DEFAULT) as libc::c_short,
            ))?;
            check(responsibility_spawnattrs_setdisclaim(&mut attrs, 1))?;
            check(libc::posix_spawn_file_actions_addopen(
                &mut actions,
                0,
                c"/dev/null".as_ptr(),
                libc::O_RDONLY,
                0,
            ))?;
            for fd in [1, 2] {
                check(libc::posix_spawn_file_actions_adddup2(
                    &mut actions,
                    log.as_raw_fd(),
                    fd,
                ))?;
            }
            check(libc::posix_spawn(
                &mut pid,
                program.as_ptr(),
                &actions,
                &attrs,
                argv.as_ptr(),
                envp.as_ptr(),
            ))
        })();
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attrs);
        spawned
    };
    spawned?;
    // The core outlives the caller as a rule; while the caller runs, this
    // collects its exit so it never lingers as a zombie.
    std::thread::spawn(move || {
        let mut status = 0;
        // SAFETY: waitpid writes one int status into `status`.
        while unsafe { libc::waitpid(pid, &mut status, 0) } < 0
            && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {}
    });
    Ok(())
}

fn rejected(socket: &Path, version: u32, pid: u32) -> ClientError {
    ClientError::VersionMismatch {
        version,
        pid,
        message: format!(
            "the Sikemux core at {} (pid {pid}) speaks protocol version {version}, not {PROTOCOL_VERSION}",
            socket.display()
        ),
    }
}

/// The core at `socket` once it answers, waiting up to `timeout` for one
/// that is starting.
pub fn wait_for_core(socket: &Path, timeout: Duration) -> Result<CoreHello, ClientError> {
    let found = waiting::wait_until(socket, Instant::now() + timeout, || {
        match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) => Some(Ok(hello)),
            Err(ProbeError::Rejected { version, pid }) => Some(Err(rejected(socket, version, pid))),
            Err(ProbeError::NotRunning(_) | ProbeError::Unanswered(_)) => None,
        }
    })?;
    found.unwrap_or(Err(ClientError::StartTimeout(timeout)))
}

/// Finds the core at `socket`, or starts one with `args` after its socket.
pub fn ensure_running(
    socket: &Path,
    binary: &Path,
    log: &Path,
    args: &[OsString],
) -> Result<CoreHello, ClientError> {
    match probe(socket, PROBE_TIMEOUT) {
        Ok(hello) => return Ok(hello),
        Err(ProbeError::Rejected { version, pid }) => return Err(rejected(socket, version, pid)),
        Err(ProbeError::NotRunning(_) | ProbeError::Unanswered(_)) => {}
    }
    start_detached(socket, binary, log, args)?;
    wait_for_core(socket, START_TIMEOUT)
}
