//! Answers the requests in [`crate::frozen`]: replacing this core
//! with a newer binary in the same process, and stopping everything.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;
use tokio::io::AsyncWriteExt;

use crate::frozen::{
    FrozenReply, FrozenRequest, UpdateBlocker, UpgradeInfo, RESUME_FORMAT, UPGRADE_INFO_ARG,
};
use crate::protocol::encode_frozen;

use super::access::{Peer, APP_ONLY};
use super::chat::{self, HandOver};
use super::connection::{blocking, FrameWriter};
#[cfg(unix)]
use super::handover;
use super::{session, Core};

/// What anything that asks the core for work hears while it hands itself over.
pub(crate) const UPDATING: &str =
    "Sikemux is updating its background process; try again in a moment";
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UPGRADE_INFO_BYTES: u64 = 64 * 1024;
const LAUNCH_SETTLE: Duration = Duration::from_secs(5);
const READER_SETTLE: Duration = Duration::from_secs(3);
const CLIENT_FLUSH: Duration = Duration::from_secs(1);
const TOOL_SETTLE: Duration = Duration::from_secs(1);
/// How long a forced update waits for chats still starting, or in the middle
/// of a request, to be able to hand their agents over. One that still cannot
/// after that is stopped and starts again on its session. An update that is
/// not forced waits for them however long they take.
const HANDOVER_SETTLE: Duration = Duration::from_secs(10);
const HANDOVER_POLL: Duration = Duration::from_millis(100);

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
/// Only the app replaces the core: the binary it names takes over everything.
pub(crate) async fn answer(
    core: &Arc<Core>,
    peer: &Peer,
    payload: &[u8],
    writer: &mut FrameWriter,
) {
    match serde_json::from_slice::<FrozenRequest>(payload) {
        Ok(FrozenRequest::Upgrade { .. }) if *peer != Peer::App => {
            reply(writer, &refused(APP_ONLY)).await;
        }
        Ok(FrozenRequest::Upgrade { binary, force }) => upgrade(core, binary, force, writer).await,
        Ok(FrozenRequest::UpdateBlockers) => {
            let chats = core
                .chats
                .blockers()
                .into_iter()
                .map(|blocker| UpdateBlocker {
                    agent_id: blocker.agent_id,
                    provider: blocker.provider,
                    title: blocker.title,
                    reason: blocker.reason,
                })
                .collect();
            reply(writer, &FrozenReply::UpdateBlockers { chats }).await;
        }
        Ok(FrozenRequest::StopEverything) => {
            reply(writer, &FrozenReply::Accepted).await;
            core.stop_everything().await;
            core.begin_shutdown();
        }
        Err(error) => reply(writer, &refused(format!("unknown request: {error}"))).await,
    }
}

async fn upgrade(core: &Arc<Core>, binary: PathBuf, force: bool, writer: &mut FrameWriter) {
    if let Err(error) = begin(core, &binary).await {
        reply(writer, &refused(error)).await;
        return;
    }
    if !core.chats.all_handable() {
        reply(
            writer,
            &FrozenReply::Deferred {
                message: "the core updates once its chats can hand their agents over".into(),
            },
        )
        .await;
        drop_writer(writer).await;
        settle_chats(core, force).await;
        core.frozen.send_replace(true);
    } else {
        // Frozen before the answer goes out, so a client that hears it never
        // reaches this core again, only its replacement.
        core.frozen.send_replace(true);
        reply(writer, &FrozenReply::Accepted).await;
    }
    finish(core, binary).await;
}

/// Replaces this core with `binary` once every chat can hand its agent over,
/// as an update the app asks for does. Returns only if that failed.
pub(crate) async fn replace_with(core: &Arc<Core>, binary: PathBuf) -> String {
    if let Err(error) = begin(core, &binary).await {
        return error;
    }
    settle_chats(core, false).await;
    core.frozen.send_replace(true);
    finish(core, binary).await
}

async fn begin(core: &Arc<Core>, binary: &Path) -> Result<(), String> {
    if core
        .upgrading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("the core is already updating".into());
    }
    let own = core.build.clone();
    let checked = binary.to_owned();
    let preflight = blocking(move || check_binary(&checked, &own).map_err(Into::into)).await;
    if let Err(error) = preflight {
        core.upgrading.store(false, Ordering::Release);
        return Err(error.to_string());
    }
    Ok(())
}

async fn settle_chats(core: &Arc<Core>, force: bool) {
    let deadline = tokio::time::Instant::now() + HANDOVER_SETTLE;
    while !core.chats.all_handable() && (!force || tokio::time::Instant::now() < deadline) {
        tokio::time::sleep(HANDOVER_POLL).await;
    }
}

async fn finish(core: &Arc<Core>, binary: PathBuf) -> String {
    let error = hand_over(core, binary).await;
    eprintln!("sikemux core: UPDATE FAILED, carrying on as before: {error}");
    thaw(core);
    error
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
    if !metadata.is_file() || !sikemux_process::os::is_executable(binary) {
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
    super::spare::discard(core);
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
    let calls = core.tools.locked().as_ref().map(|tools| tools.open_calls());
    if let Some(calls) = calls {
        let deadline = Instant::now() + TOOL_SETTLE;
        while calls.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let chats = core.chats.hand_over().await;
    let replacing = core.clone();
    let replaced = blocking(move || {
        let mut chats = chats;
        replacing.workspaces.flush();
        #[cfg(unix)]
        let error = handover::replace_process(&replacing, &binary, &mut chats);
        #[cfg(windows)]
        let error = {
            let _ = &binary;
            std::io::Error::other("Windows replaces a core by stopping it and starting the new one")
        };
        Ok((error, chats))
    })
    .await;
    match replaced {
        Ok((error, chats)) => {
            restart_chats(core, chats);
            error.to_string()
        }
        Err(error) => error.to_string(),
    }
}

/// When an update fails, the chats it let go of are taken up again here,
/// and the ones it stopped start again on their sessions.
fn restart_chats(core: &Arc<Core>, chats: Vec<HandOver>) {
    for handed in chats {
        match handed {
            HandOver::Live(handed) => {
                let chat::HandedChat {
                    live,
                    released,
                    subscribers,
                } = *handed;
                let record = live.record.clone();
                let pid = released.pid;
                let pipes = [released.stdin, released.stdout, released.stderr];
                if let Err(error) = chat::adopt(core, live, pipes, &subscribers) {
                    eprintln!("sikemux core: a chat's agent is started again after the failed update: {error}");
                    chat::kill_agent(pid);
                    chat::resume(
                        core,
                        chat::ChatRecord {
                            notice: Some(error),
                            ..record
                        },
                    );
                }
            }
            HandOver::Resume(record) => chat::resume(core, *record),
        }
    }
}

fn thaw(core: &Arc<Core>) {
    core.frozen.send_replace(false);
    for target in core.all_sessions() {
        session::thaw(core, target);
    }
    core.upgrading.store(false, Ordering::Release);
}
