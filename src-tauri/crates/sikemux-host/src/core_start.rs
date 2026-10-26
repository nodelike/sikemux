//! Starting the core for an app, and keeping it on the app's build: a core of
//! another build is moved to this one in place, keeping every session and
//! chat. Every app that talks to the core starts it here, then connects to it
//! its own way.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sikemux_core::client::{await_deferred_upgrade, await_upgrade, frozen_request, ClientError};
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
/// Long enough for the core's shared kill grace plus reaping a few hundred
/// sessions.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Ends a core this app cannot talk to, with everything running in it.
pub fn stop(socket: &Path, pid: u32) {
    let stopped = frozen_request(socket, &FrozenRequest::StopEverything, STOP_TIMEOUT);
    if !matches!(stopped, Ok(FrozenReply::Accepted)) && pid > 1 {
        let _ = sikemux_process::os::terminate(pid);
    }
}
