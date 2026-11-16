use crate::server::chat_state::ChatParts;
use crate::server::parts::PartCore;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::serving::ServeParts;
use sikemux_process::command::Stdio;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;
use tokio::io::AsyncWriteExt;

use crate::frozen::{FrozenReply, UpgradeInfo, RESUME_FORMAT, UPGRADE_INFO_ARG};

use super::super::chat::{self, HandOver};
use super::super::connection::{blocking, FrameWriter};
use super::super::{handover, session, Core};
use super::{refused, reply};

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UPGRADE_INFO_BYTES: u64 = 64 * 1024;
const LAUNCH_SETTLE: Duration = Duration::from_secs(5);
const READER_SETTLE: Duration = Duration::from_secs(3);
const CLIENT_FLUSH: Duration = Duration::from_secs(1);
const TOOL_SETTLE: Duration = Duration::from_secs(1);
const HANDOVER_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// Another build, as an update puts in place.
    Update,
    /// The build already running, which a restart starts afresh.
    Restart,
}

pub(super) async fn upgrade(
    core: &Arc<Core>,
    binary: PathBuf,
    force: bool,
    writer: &mut FrameWriter,
) {
    if let Err(error) = begin(core, &binary, Replacement::Update).await {
        eprintln!("sikemux core: not handing over: {error}");
        reply(writer, &refused(error)).await;
        return;
    }
    let later = "the core updates once its chats can hand their agents over";
    hand_over_when_ready(core, binary, force, later, writer).await;
}

/// Restarts this core from its own binary, keeping its terminals and chats,
/// and tells the client, which hears nothing more when it worked.
pub(super) async fn restart(core: &Arc<Core>, writer: &mut FrameWriter) {
    let binary = match own_binary() {
        Ok(binary) => binary,
        Err(error) => {
            reply(
                writer,
                &refused(format!("the core cannot find its own binary: {error}")),
            )
            .await;
            return;
        }
    };
    if let Err(error) = begin(core, &binary, Replacement::Restart).await {
        reply(writer, &refused(error)).await;
        return;
    }
    let later = "the core restarts once its chats can hand their agents over";
    let error = hand_over_when_ready(core, binary, false, later, writer).await;
    reply(writer, &refused(error)).await;
}

/// Hands the core over to `binary` now, or once every chat can hand its agent
/// over, and tells the client which. Returns only if the hand-over failed.
async fn hand_over_when_ready(
    core: &Arc<Core>,
    binary: PathBuf,
    force: bool,
    later: &str,
    writer: &mut FrameWriter,
) -> String {
    if !core.chats().all_handable() {
        reply(
            writer,
            &FrozenReply::Deferred {
                message: later.into(),
            },
        )
        .await;
        drop_writer(writer).await;
        settle_chats(core, force).await;
        core.updating().frozen.send_replace(true);
    } else {
        // Frozen before the answer goes out, so a client that hears it never
        // reaches this core again, only its replacement.
        core.updating().frozen.send_replace(true);
        reply(writer, &FrozenReply::Accepted).await;
    }
    finish(core, binary).await
}

/// Replaces this core with `binary` once every chat can hand its agent over, giving up after
/// `limit`, or, when `force`d, once the chats have had a moment to settle. Returns only if
/// that failed.
pub(crate) async fn replace_with(
    core: &Arc<Core>,
    binary: PathBuf,
    force: bool,
    limit: Duration,
) -> String {
    if let Err(error) = begin(core, &binary, Replacement::Update).await {
        return error;
    }
    if force {
        settle_chats(core, true).await;
    } else {
        let deadline = tokio::time::Instant::now() + limit;
        while !core.chats().all_handable() {
            if tokio::time::Instant::now() >= deadline {
                core.updating().started.store(false, Ordering::Release);
                return "a chat could not hand its agent over".into();
            }
            tokio::time::sleep(HANDOVER_POLL).await;
        }
    }
    core.updating().frozen.send_replace(true);
    finish(core, binary).await
}

async fn begin(core: &Arc<Core>, binary: &Path, replacement: Replacement) -> Result<(), String> {
    if core
        .updating()
        .started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("the core is already updating".into());
    }
    let own = core.config().build.clone();
    let checked = binary.to_owned();
    let preflight =
        blocking(move || check_binary(&checked, &own, replacement).map_err(Into::into)).await;
    if let Err(error) = preflight {
        core.updating().started.store(false, Ordering::Release);
        return Err(error.to_string());
    }
    Ok(())
}

async fn settle_chats(core: &Arc<Core>, force: bool) {
    let wait = match force {
        true => core.config().handover_settle,
        false => core.config().update_patience,
    };
    let deadline = tokio::time::Instant::now() + wait;
    while !core.chats().all_handable() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(HANDOVER_POLL).await;
    }
}

async fn finish(core: &Arc<Core>, binary: PathBuf) -> String {
    let error = hand_over(core, binary).await;
    eprintln!("sikemux core: HAND-OVER FAILED, carrying on as before: {error}");
    thaw(core);
    error
}

/// The client that asked for a deferred update has its answer and need not
/// wait for the update itself.
async fn drop_writer(writer: &mut FrameWriter) {
    let _ = writer.shutdown().await;
}

/// Refuses a binary that is not an executable file, that does not read this
/// core's hand-over, or, for an update, that is an older build or the one already running.
fn check_binary(
    binary: &Path,
    own: &sikemux_wire::protocol::BuildIdentity,
    replacement: Replacement,
) -> Result<(), String> {
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
    if replacement == Replacement::Update && crate::build::older(&info.build(), own) {
        return Err(format!(
            "{} is Sikemux {} {}, older than the {} {} this core runs; a newer Sikemux is already running",
            binary.display(),
            info.version,
            info.commit,
            own.version,
            own.commit,
        ));
    }
    let same_file = own_binary().is_ok_and(|own_binary| own_binary == binary);
    if replacement == Replacement::Update && info.build().same_build(own) && same_file {
        return Err(format!(
            "{} is the build this core already runs",
            binary.display()
        ));
    }
    Ok(())
}

/// The path this core was started from. Once a package manager replaces that file, Linux
/// names it with a " (deleted)" suffix, while the path itself now holds the new build.
fn own_binary() -> std::io::Result<PathBuf> {
    let binary = std::env::current_exe()?;
    Ok(binary
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .unwrap_or(binary))
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
    super::super::spare::discard(core);
    if !core.updating().launching.settle(LAUNCH_SETTLE).await {
        return "a session was still starting".into();
    }
    if !core.updating().pumping.settle(READER_SETTLE).await {
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
        .tools()
        .locked()
        .as_ref()
        .map(|tools| tools.open_calls());
    if let Some(calls) = calls {
        let deadline = Instant::now() + TOOL_SETTLE;
        while calls.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let chats = core.chats().hand_over().await;
    let replacing = core.clone();
    let replaced = blocking(move || {
        let mut chats = chats;
        replacing.flush_parts();
        #[cfg(target_os = "macos")]
        super::super::rollback::watch_the_handover(&replacing);
        let error = handover::replace_process(&replacing, &binary, &mut chats);
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
                    chat::kill_and_reap(pid);
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
    core.updating().frozen.send_replace(false);
    for target in core.all_sessions() {
        session::thaw(core, target);
    }
    core.updating().started.store(false, Ordering::Release);
}
