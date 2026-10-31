//! Answers the requests in [`crate::frozen`]: replacing this core
//! with a newer binary in the same process, restarting it from its own, and
//! stopping everything.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Notify};

use crate::frozen::{
    FrozenReply, FrozenRequest, UpdateBlocker, UpgradeInfo, RESUME_FORMAT, UPGRADE_INFO_ARG,
};
use crate::protocol::encode_frozen;

use super::access::{Peer, APP_ONLY};
use super::chat::{self, HandOver};
use super::connection::{blocking, FrameWriter};
use super::{handover, session, Core};

/// Whether this core hands itself over to a newer binary, and the work in
/// progress a hand-over waits for.
pub(crate) struct Updating {
    /// True while the core hands itself over: it takes no new work and its
    /// readers stop at the next whole chunk.
    frozen: watch::Sender<bool>,
    started: AtomicBool,
    /// Readers that hold output not yet fed to their screen.
    pub(crate) pumping: Gauge,
    /// Sessions being spawned that are not in the table yet.
    pub(crate) launching: Gauge,
}

impl Default for Updating {
    fn default() -> Self {
        Self {
            frozen: watch::channel(false).0,
            started: AtomicBool::new(false),
            pumping: Gauge::default(),
            launching: Gauge::default(),
        }
    }
}

impl Updating {
    pub(crate) fn is_frozen(&self) -> bool {
        *self.frozen.borrow()
    }

    /// Resolves once the core starts handing itself over.
    pub(crate) async fn until_frozen(&self) {
        let mut frozen = self.frozen.subscribe();
        let _ = frozen.wait_for(|frozen| *frozen).await;
    }

    /// Hears each time the core freezes or thaws.
    pub(crate) fn watch(&self) -> watch::Receiver<bool> {
        self.frozen.subscribe()
    }
}

/// Counts work in progress that a hand-over waits for, and wakes it when the
/// count drops.
#[derive(Default)]
pub(crate) struct Gauge {
    count: AtomicUsize,
    idle: Notify,
}

pub(crate) struct GaugeGuard<'a>(&'a Gauge);

impl Gauge {
    pub(crate) fn enter(&self) -> GaugeGuard<'_> {
        self.count.fetch_add(1, Ordering::AcqRel);
        GaugeGuard(self)
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    /// False when the work did not finish in time.
    pub(crate) async fn settle(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.is_idle() {
                return true;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return self.is_idle();
            }
        }
    }
}

impl Drop for GaugeGuard<'_> {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
        self.0.idle.notify_waiters();
    }
}

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
/// Any program here may restart it, which runs only the binary already running.
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
        Ok(FrozenRequest::Restart) => restart(core, writer).await,
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

/// What a hand-over replaces the core with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// Another build, as an update puts in place.
    Update,
    /// The build already running, which a restart starts afresh.
    Restart,
}

async fn upgrade(core: &Arc<Core>, binary: PathBuf, force: bool, writer: &mut FrameWriter) {
    if let Err(error) = begin(core, &binary, Replacement::Update).await {
        reply(writer, &refused(error)).await;
        return;
    }
    let later = "the core updates once its chats can hand their agents over";
    hand_over_when_ready(core, binary, force, later, writer).await;
}

/// Restarts this core from its own binary, keeping its terminals and chats,
/// and tells the client, which hears nothing more when it worked.
async fn restart(core: &Arc<Core>, writer: &mut FrameWriter) {
    let binary = match std::env::current_exe() {
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
    if !core.chats.all_handable() {
        reply(
            writer,
            &FrozenReply::Deferred {
                message: later.into(),
            },
        )
        .await;
        drop_writer(writer).await;
        settle_chats(core, force).await;
        core.updating.frozen.send_replace(true);
    } else {
        // Frozen before the answer goes out, so a client that hears it never
        // reaches this core again, only its replacement.
        core.updating.frozen.send_replace(true);
        reply(writer, &FrozenReply::Accepted).await;
    }
    finish(core, binary).await
}

/// Replaces this core with `binary` once every chat can hand its agent over,
/// as an update the app asks for does. Returns only if that failed.
pub(crate) async fn replace_with(core: &Arc<Core>, binary: PathBuf) -> String {
    if let Err(error) = begin(core, &binary, Replacement::Update).await {
        return error;
    }
    settle_chats(core, false).await;
    core.updating.frozen.send_replace(true);
    finish(core, binary).await
}

async fn begin(core: &Arc<Core>, binary: &Path, replacement: Replacement) -> Result<(), String> {
    if core
        .updating
        .started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("the core is already updating".into());
    }
    let own = core.build.clone();
    let checked = binary.to_owned();
    let preflight =
        blocking(move || check_binary(&checked, &own, replacement).map_err(Into::into)).await;
    if let Err(error) = preflight {
        core.updating.started.store(false, Ordering::Release);
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
/// core's hand-over, or, for an update, that is the build already running.
fn check_binary(
    binary: &Path,
    own: &crate::protocol::BuildIdentity,
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
    if replacement == Replacement::Update && info.build().same_build(own) {
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
    if !core.updating.launching.settle(LAUNCH_SETTLE).await {
        return "a session was still starting".into();
    }
    if !core.updating.pumping.settle(READER_SETTLE).await {
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
    core.updating.frozen.send_replace(false);
    for target in core.all_sessions() {
        session::thaw(core, target);
    }
    core.updating.started.store(false, Ordering::Release);
}
