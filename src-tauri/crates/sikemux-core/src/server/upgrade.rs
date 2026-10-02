//! Answers the requests in [`crate::protocol::frozen`]: replacing this core
//! with a newer binary in the same process, and stopping everything.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use crate::protocol::encode_frozen;
use crate::protocol::frozen::{
    FrozenReply, FrozenRequest, UpgradeInfo, RESUME_FORMAT, UPGRADE_INFO_ARG,
};

use super::chat::{self, ChatRecord};
use super::connection::{blocking, FrameWriter};
use super::{handover, session, Core};

/// What anything that asks the core for work hears while it hands itself over.
pub(crate) const UPDATING: &str =
    "Sikemux is updating its background process; try again in a moment";
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UPGRADE_INFO_BYTES: u64 = 64 * 1024;
const LAUNCH_SETTLE: Duration = Duration::from_secs(5);
const READER_SETTLE: Duration = Duration::from_secs(3);
const CLIENT_FLUSH: Duration = Duration::from_secs(1);
const TOOL_SETTLE: Duration = Duration::from_secs(1);
/// How long an update waits for chat turns to end. A turn still running
/// after that ends with the update, and its chat starts again on its session.
const TURN_SETTLE: Duration = Duration::from_secs(120);
const TURN_POLL: Duration = Duration::from_millis(250);

async fn reply(writer: &mut FrameWriter, answer: &FrozenReply) {
    if let Ok(frame) = encode_frozen(answer) {
        let _ = writer.write_all(&frame).await;
        let _ = writer.flush().await;
    }
}

fn refused(message: impl Into<String>) -> FrozenReply {
    FrozenReply::Refused {
        message: message.into(),
    }
}

/// Answers one frozen request on a connection that sent it instead of a hello.
pub(crate) async fn answer(core: &Arc<Core>, payload: &[u8], writer: &mut FrameWriter) {
    match serde_json::from_slice::<FrozenRequest>(payload) {
        Ok(FrozenRequest::Upgrade { binary }) => upgrade(core, binary, writer).await,
        Ok(FrozenRequest::StopEverything) => {
            reply(writer, &FrozenReply::Accepted).await;
            let draining = core.clone();
            let _ = blocking(move || {
                draining.drain();
                Ok(())
            })
            .await;
            core.begin_shutdown();
        }
        Err(error) => reply(writer, &refused(format!("unknown request: {error}"))).await,
    }
}

async fn upgrade(core: &Arc<Core>, binary: PathBuf, writer: &mut FrameWriter) {
    if core
        .upgrading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        reply(writer, &refused("the core is already updating")).await;
        return;
    }
    let own = core.build.clone();
    let checked = binary.clone();
    let preflight = blocking(move || check_binary(&checked, &own).map_err(Into::into)).await;
    if let Err(error) = preflight {
        core.upgrading.store(false, Ordering::Release);
        reply(writer, &refused(error.to_string())).await;
        return;
    }
    if core.chats.any_turn_running() {
        reply(
            writer,
            &FrozenReply::Deferred {
                message: "the core updates once its chat turns end".into(),
            },
        )
        .await;
        drop_writer(writer).await;
        let deadline = tokio::time::Instant::now() + TURN_SETTLE;
        while core.chats.any_turn_running() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(TURN_POLL).await;
        }
        core.frozen.send_replace(true);
    } else {
        // Frozen before the answer goes out, so a client that hears it never
        // reaches this core again, only its replacement.
        core.frozen.send_replace(true);
        reply(writer, &FrozenReply::Accepted).await;
    }
    let error = hand_over(core, binary).await;
    eprintln!("sikemux core: UPDATE FAILED, carrying on as before: {error}");
    thaw(core);
}

/// The client that asked for a deferred update has its answer and need not
/// wait for the update itself.
async fn drop_writer(writer: &mut FrameWriter) {
    let _ = writer.shutdown().await;
}

/// Refuses a binary that is not an executable file, that does not read this
/// core's hand-over, or that is the build already running.
fn check_binary(binary: &Path, own: &crate::protocol::BuildIdentity) -> Result<(), String> {
    if !binary.is_absolute() {
        return Err(format!("{} is not an absolute path", binary.display()));
    }
    let metadata = std::fs::metadata(binary)
        .map_err(|error| format!("{} cannot be read: {error}", binary.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("{} is not an executable file", binary.display()));
    }
    let info = upgrade_info(binary)?;
    if !info.reads(RESUME_FORMAT) {
        return Err(format!(
            "{} does not read hand-over format {RESUME_FORMAT}, which this core writes",
            binary.display(),
        ));
    }
    if info.build().same_build(own) {
        return Err(format!(
            "{} is the build this core already runs",
            binary.display()
        ));
    }
    Ok(())
}

/// Runs `<binary> core --upgrade-info`, which also proves the binary starts.
fn upgrade_info(binary: &Path) -> Result<UpgradeInfo, String> {
    let mut child = sikemux_process::user_environment::command(binary)
        .args(["core", UPGRADE_INFO_ARG])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{} does not start: {error}", binary.display()))?;
    let deadline = Instant::now() + PREFLIGHT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} did not say which build it is within {PREFLIGHT_TIMEOUT:?}",
                    binary.display()
                ));
            }
        }
    };
    let mut output = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_UPGRADE_INFO_BYTES).read_to_end(&mut output);
    }
    if !status.success() {
        return Err(format!(
            "{} cannot replace a core ({status})",
            binary.display()
        ));
    }
    serde_json::from_slice(&output).map_err(|error| {
        format!(
            "{} said something unreadable about its build: {error}",
            binary.display()
        )
    })
}

/// Stops taking work, lets what is in flight land, and replaces the process.
/// Returns only if that failed.
async fn hand_over(core: &Arc<Core>, binary: PathBuf) -> String {
    if !core.launching.settle(LAUNCH_SETTLE).await {
        return "a session was still starting".into();
    }
    if !core.pumping.settle(READER_SETTLE).await {
        return "a terminal reader did not stop".into();
    }
    let flushing: Vec<_> = core
        .clients()
        .into_iter()
        .map(|client| tokio::spawn(async move { client.wait_flushed(CLIENT_FLUSH).await }))
        .collect();
    for flush in flushing {
        let _ = flush.await;
    }
    let calls = core
        .tools
        .lock()
        .ok()
        .and_then(|tools| tools.as_ref().map(|tools| tools.open_calls()));
    if let Some(calls) = calls {
        let deadline = Instant::now() + TOOL_SETTLE;
        while calls.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let chats = core.chats.hand_over().await;
    let replacing = core.clone();
    let handed = chats.clone();
    let replaced = blocking(move || {
        Err::<std::convert::Infallible, _>(handover::replace_process(&replacing, &binary, &handed))
    })
    .await;
    restart_chats(core, chats);
    match replaced {
        Ok(never) => match never {},
        Err(error) => error.to_string(),
    }
}

/// The chats an update stopped start again on their sessions when it fails.
fn restart_chats(core: &Arc<Core>, chats: Vec<ChatRecord>) {
    for record in chats {
        chat::resume(core, record);
    }
}

fn thaw(core: &Arc<Core>) {
    core.frozen.send_replace(false);
    for target in core.all_sessions() {
        session::thaw(core, target);
    }
    core.upgrading.store(false, Ordering::Release);
}
