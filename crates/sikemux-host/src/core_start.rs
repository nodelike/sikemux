//! Starting the core for an app, and keeping it on the app's build: a core of
//! another build is moved to this one in place, keeping every session and
//! chat. Every app that talks to the core starts it here, then connects to it
//! its own way.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sikemux_core::client::{
    await_deferred_upgrade, await_upgrade, frozen_request, probe, ClientError,
};
use sikemux_core::frozen::{FrozenReply, FrozenRequest};
use sikemux_core::protocol::BuildIdentity;

use crate::login_item::{start_core, CoreLaunch};

/// The core first checks that the new binary starts, which can take a while
/// the first time macOS sees it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
const RETURN_TIMEOUT: Duration = Duration::from_secs(20);
/// A core with chat turns running updates once they end, which it gives two
/// minutes.
const DEFERRED_RETURN_TIMEOUT: Duration = Duration::from_secs(150);
/// How long a core that holds its lock gets to answer before it counts as stuck.
const STUCK_PROBE: Duration = Duration::from_secs(3);
/// How long a stuck core gets to exit after it is asked to by signal, before it is forced.
const TERM_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(2);

pub enum Upgrade {
    Done,
    /// The core goes on as it is until its chats can be carried over, then
    /// updates and drops its connections, which come back to the updated core.
    Deferred,
}

pub enum StartError {
    /// No core answered.
    Unreachable(ClientError),
    /// A core of another protocol answers at the socket and could not be
    /// moved to this build, so this app cannot talk to it.
    Incompatible { pid: u32, reason: String },
}

/// Makes sure a core this app can talk to answers at the launch's socket:
/// starts one, through the login item when it is installed, or moves the one
/// running to `build`. A core with chats that cannot be carried over yet is
/// left on its build until they can, unless `force`.
pub async fn start(
    identifier: &str,
    launch: &CoreLaunch,
    build: &BuildIdentity,
    force: bool,
) -> Result<(), StartError> {
    let found = {
        let (identifier, launch) = (identifier.to_owned(), launch.clone());
        tokio::task::spawn_blocking(move || start_core(&identifier, &launch))
            .await
            .map_err(|error| StartError::Unreachable(ClientError::Core(error.to_string())))?
    };
    let binary = launch.binary.clone();
    match found {
        Ok(hello) if hello.build.same_build(build) => Ok(()),
        Ok(hello) => {
            match upgrade(&launch.socket, binary, hello.pid, Some(hello.build), force, false).await {
                Ok(Upgrade::Done) => {}
                Ok(Upgrade::Deferred) => eprintln!(
                    "Sikemux's core updates once its chats can be carried over; using it as it is until then"
                ),
                Err(message) => eprintln!("Sikemux keeps its core as it is: {message}"),
            }
            Ok(())
        }
        Err(ClientError::VersionMismatch { pid, message, .. }) => {
            upgrade(&launch.socket, binary, pid, None, force, true)
                .await
                .map(drop)
                .map_err(|reason| StartError::Incompatible {
                    pid,
                    reason: format!("{message}, and it could not be updated: {reason}"),
                })
        }
        Err(error) => Err(StartError::Unreachable(error)),
    }
}

/// Asks the core at `socket` to replace itself with `binary`, and waits for
/// it to answer from the same process again. Without `old`, the core speaks
/// another protocol and could not say which build it runs. A core that defers
/// is waited for only when `wait_if_deferred`.
pub async fn upgrade(
    socket: &Path,
    binary: PathBuf,
    pid: u32,
    old: Option<BuildIdentity>,
    force: bool,
    wait_if_deferred: bool,
) -> Result<Upgrade, String> {
    let socket = socket.to_owned();
    let upgraded = tokio::task::spawn_blocking(move || {
        let request = FrozenRequest::Upgrade { binary, force };
        match frozen_request(&socket, &request, ANSWER_TIMEOUT) {
            Ok(FrozenReply::Accepted) => await_upgrade(&socket, pid, old.as_ref(), RETURN_TIMEOUT)
                .map(Some)
                .map_err(|error| error.to_string()),
            Ok(FrozenReply::Deferred { .. }) if wait_if_deferred => {
                await_deferred_upgrade(&socket, pid, old.as_ref(), DEFERRED_RETURN_TIMEOUT)
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            Ok(FrozenReply::Deferred { .. }) => Ok(None),
            Ok(FrozenReply::Refused { message }) => Err(message),
            Ok(FrozenReply::UpdateBlockers { .. }) => {
                Err("the core answered another request".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    let Some(upgraded) = upgraded else {
        return Ok(Upgrade::Deferred);
    };
    eprintln!(
        "Sikemux updated its core (pid {pid}) to {} {}",
        upgraded.build.version, upgraded.build.commit
    );
    Ok(Upgrade::Done)
}

/// The core that holds the lock at `socket` but does not answer there, by its process id.
pub fn stuck(socket: &Path) -> Option<u32> {
    let pid = sikemux_core::server::lock_holder(socket)?;
    probe(socket, STUCK_PROBE).is_err().then_some(pid)
}

/// Ends core `pid` while it still holds the lock at `socket`: asks it to by signal, then forces
/// it, so a core that stopped answering cannot keep another from starting. Answers whether
/// it is gone.
pub fn end(socket: &Path, pid: u32) -> bool {
    for (signal, grace) in [(libc::SIGTERM, TERM_GRACE), (libc::SIGKILL, KILL_GRACE)] {
        if sikemux_core::server::lock_holder(socket) != Some(pid) {
            return true;
        }
        let Ok(id) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: kill only takes integers; the pid is a single process above
        // init, so this never signals a group or every process.
        unsafe {
            libc::kill(id, signal);
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if sikemux_core::server::lock_holder(socket) != Some(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    sikemux_core::server::lock_holder(socket) != Some(pid)
}
