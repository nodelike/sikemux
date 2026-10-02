//! Terminals live in the background core (`sikemux core`); these commands
//! forward to it over its socket. Output comes back on the connection and is
//! fanned out to the webview channels that show each terminal, with the same
//! ack-based flow control the in-process terminals had.

pub(crate) mod commands;
mod sink;
mod streams;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sikemux_core::client::{
    await_deferred_upgrade, await_upgrade, ensure_running, frozen_request, Attached, ClientError,
    CoreClient, Reply,
};
use sikemux_core::protocol::frozen::{FrozenReply, FrozenRequest};
use sikemux_core::protocol::{BuildIdentity, Request, SessionId, SessionInfo, SessionKind};
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

use sink::AppSink;
use streams::StreamTable;

/// Browser tabs are child webviews of the same app. Every terminal event
/// belongs to the workbench, so address it by label instead of broadcasting.
const MAIN_WEBVIEW: &str = "main";
/// Long enough for the core's shared kill grace plus reaping a few hundred
/// sessions.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// The core first checks that the sidecar starts, which can take a while the
/// first time macOS sees a new binary.
const UPGRADE_ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
const UPGRADE_RETURN_TIMEOUT: Duration = Duration::from_secs(20);
/// A core with chat turns running updates once they end, which it gives two
/// minutes.
const DEFERRED_RETURN_TIMEOUT: Duration = Duration::from_secs(150);
/// `ESC c`, which clears a pane's screen and history before its replay.
const FULL_RESET: &[u8] = b"\x1bc";

struct CoreSettings {
    app: AppHandle,
    socket: PathBuf,
    binary: Option<PathBuf>,
    log: PathBuf,
    manifest_dir: Option<PathBuf>,
    /// Where a core started from here publishes agents' tool endpoint and
    /// keeps the harness journal.
    core_args: Vec<OsString>,
}

#[derive(Default)]
pub struct PtyManager {
    settings: OnceLock<CoreSettings>,
    client: Mutex<Option<Arc<CoreClient>>>,
    connecting: tokio::sync::Mutex<()>,
    streams: Arc<StreamTable>,
    /// Set once the app is leaving, so a core that goes away is not started
    /// again.
    closing: AtomicBool,
    /// The process of the core last connected to. A core that comes back in
    /// the same process after an update still has every session.
    core_pid: AtomicU32,
    /// A core this app could neither update nor talk to, already reported.
    noticed: AtomicU32,
}

pub(crate) fn core_error(error: ClientError) -> AppError {
    match error {
        ClientError::Core(message) => {
            if let Some(reason) = message.strip_prefix("invalid argument: ") {
                AppError::BadArgText(reason.to_string())
            } else if let Some(reason) = message.strip_prefix("pty: ") {
                AppError::Pty(reason.to_string())
            } else {
                AppError::Other(message)
            }
        }
        other => AppError::Pty(other.to_string()),
    }
}

/// What a PTY was opened for, so a process found under it can be traced back
/// to a terminal pane, an agent or a task.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyOwner {
    pub project: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    pub task_execution_id: Option<String>,
}

pub(crate) struct PtyProcess {
    pub pid: u32,
    pub pty_id: SessionId,
    pub owner: PtyOwner,
}

#[derive(serde::Serialize)]
pub struct PtyDiagnostics {
    pub core_pid: Option<u32>,
    pub ptys: usize,
    pub subscribers: usize,
    pub output_frames: u64,
    pub output_bytes: u64,
    pub working_agents: usize,
    pub blocked_agents: usize,
    pub idle_agents: usize,
    pub unknown_agents: usize,
}

impl PtyManager {
    /// Remembers where the core lives and starts connecting to it on a worker
    /// thread. Call once the process environment is final, because a core
    /// started from here inherits it.
    pub fn start(&self, app: &AppHandle) {
        let Some(socket) = sikemux_core::default_socket_path() else {
            eprintln!("Sikemux terminals are unavailable: HOME is not set");
            return;
        };
        let log = app
            .path()
            .app_log_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("core.log");
        let manifest_dir = app
            .path()
            .app_config_dir()
            .ok()
            .map(|directory| directory.join("agent-detection"));
        let mut core_args = Vec::new();
        if let Some(endpoint) = crate::cli_paths::cli_endpoint_path() {
            core_args.extend([OsString::from("--cli-endpoint"), endpoint.into()]);
        }
        if let Ok(data_dir) = app.path().app_data_dir() {
            core_args.extend([OsString::from("--data-dir"), data_dir.into()]);
        }
        let _ = self.settings.set(CoreSettings {
            app: app.clone(),
            socket,
            binary: crate::cli_paths::cli_executable_path(),
            log,
            manifest_dir,
            core_args,
        });
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(manager) = app.try_state::<PtyManager>() {
                if let Err(error) = manager.client().await {
                    eprintln!("Sikemux could not reach its terminal core: {error}");
                }
            }
        });
    }

    /// The command a core is started with from here, which the login item repeats.
    pub(crate) fn core_launch(&self) -> Option<crate::login_item::CoreLaunch> {
        let settings = self.settings.get()?;
        Some(crate::login_item::CoreLaunch {
            binary: settings.binary.clone()?,
            socket: settings.socket.clone(),
            log: settings.log.clone(),
            args: settings.core_args.clone(),
        })
    }

    pub(crate) fn current_client(&self) -> Option<Arc<CoreClient>> {
        self.current()
    }

    fn current(&self) -> Option<Arc<CoreClient>> {
        self.client
            .lock()
            .ok()?
            .clone()
            .filter(|client| client.is_connected())
    }

    /// The live connection to the core, starting the core first if needed.
    pub(crate) async fn client(&self) -> AppResult<Arc<CoreClient>> {
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let _connecting = self.connecting.lock().await;
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let settings = self
            .settings
            .get()
            .ok_or_else(|| AppError::Pty("the terminal core is not configured yet".into()))?;
        let binary = settings.binary.clone().ok_or_else(|| {
            AppError::Pty("the sikemux-editor sidecar that runs terminals is missing".into())
        })?;
        let socket = settings.socket.clone();
        let log = settings.log.clone();
        let core_args = settings.core_args.clone();
        if let Some(directory) = log.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let starting = binary.clone();
        let found = tauri::async_runtime::spawn_blocking(move || {
            ensure_running(&socket, &starting, &log, &core_args)
        })
        .await
        .map_err(|error| AppError::Pty(format!("core start join: {error}")))?;
        match found {
            Ok(hello) if hello.build.same_build(&crate::build_identity()) => {}
            Ok(hello) => {
                match upgrade_core(settings, binary, hello.pid, Some(hello.build), false).await {
                    Ok(Upgrade::Done) => {}
                    Ok(Upgrade::Deferred) => eprintln!(
                        "Sikemux's terminal core updates once its chat turns end; using it as it is until then"
                    ),
                    Err(message) => {
                        eprintln!("Sikemux keeps its terminal core as it is: {message}")
                    }
                }
            }
            Err(ClientError::VersionMismatch { pid, message, .. }) => {
                if let Err(reason) = upgrade_core(settings, binary, pid, None, true).await {
                    self.notice_incompatible(settings, pid);
                    return Err(AppError::Pty(format!(
                        "{message}, and it could not be updated: {reason}"
                    )));
                }
            }
            Err(error) => return Err(core_error(error)),
        }
        let sink = Arc::new(AppSink::new(settings.app.clone(), self.streams.clone()));
        let client = Arc::new(
            CoreClient::connect_with(&settings.socket, sink)
                .await
                .map_err(core_error)?,
        );
        client
            .configure(settings.manifest_dir.clone())
            .await
            .map_err(core_error)?;
        client.register_window().await.map_err(core_error)?;
        crate::remote::connected(&settings.app, self.core_launch().as_ref(), &client).await;
        self.core_pid.store(client.core_pid(), Ordering::Release);
        if let Ok(mut current) = self.client.lock() {
            *current = Some(client.clone());
        }
        Ok(client)
    }

    /// The core went away. One that answers again from the same process was
    /// updated in place and still has every session, so the panes take theirs
    /// back; otherwise every session the app knew is gone with it.
    fn disconnected(&self) {
        let settings = self.settings.get();
        if self.closing.load(Ordering::Acquire) || settings.is_none() {
            sink::report_all_exited(&self.streams);
            return;
        }
        let Some(settings) = settings else {
            return;
        };
        let previous = self.core_pid.load(Ordering::Acquire);
        let app = settings.app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(manager) = app.try_state::<PtyManager>() else {
                return;
            };
            match manager.client().await {
                Ok(client) if client.core_pid() == previous => {
                    manager.reattach(&client).await;
                    crate::acp::reconnected(&app, Some(&client)).await;
                }
                Ok(_) => {
                    sink::report_all_exited(&manager.streams);
                    crate::acp::reconnected(&app, None).await;
                }
                Err(error) => {
                    sink::report_all_exited(&manager.streams);
                    crate::acp::reconnected(&app, None).await;
                    eprintln!("Sikemux could not restart its terminal core: {error}");
                }
            }
        });
    }

    /// Sends every pane its screen again from the updated core, followed by
    /// the live output, and tells tasks that ended meanwhile.
    async fn reattach(&self, client: &Arc<CoreClient>) {
        for id in self.streams.core_subscribed() {
            let streams = self.streams.clone();
            let submitted = client.submit(Request::Attach { id }, move |reply| match reply {
                Ok(Reply::Attached(Attached {
                    alternate_screen,
                    exited,
                    replay,
                    ..
                })) => {
                    streams.restart(id);
                    // An alternate screen repaints itself in full and leaves
                    // the pane's own normal-screen history alone.
                    let mut screen = Vec::with_capacity(replay.len() + FULL_RESET.len());
                    if !alternate_screen {
                        screen.extend_from_slice(FULL_RESET);
                    }
                    screen.extend_from_slice(&replay);
                    sink::deliver(&streams, id, &screen);
                    if exited {
                        streams.channels(id).send(&[]);
                    }
                }
                // A dropped connection is not the session ending; the next
                // reconnect deals with it.
                Err(ClientError::Disconnected) => {}
                _ => sink::report_exited(&streams, id, sink::task_exit(None, None)),
            });
            if let Ok(replied) = submitted {
                let _ = replied.await;
            }
        }
        let watched = self.streams.watched_tasks();
        if watched.is_empty() {
            return;
        }
        let Ok(sessions) = client.list().await else {
            return;
        };
        for id in watched {
            let session = sessions.iter().find(|session| session.id == id);
            if session.is_some_and(|session| session.running) {
                continue;
            }
            let exit = session.and_then(|session| session.exit.clone());
            let exit_channel = self
                .streams
                .lock()
                .ok()
                .and_then(|mut guard| guard.take_task_exit(id));
            if let Some(channel) = exit_channel {
                let (code, signal) = exit.map_or((None, None), |exit| (exit.code, exit.signal));
                let _ = channel.send(sink::task_exit(code, signal));
            }
        }
    }

    /// A page load is a quiet moment to move the core to the sidecar's build,
    /// for a sidecar rebuilt while the app runs.
    pub fn update_core_if_stale(&self) {
        let Some(settings) = self.settings.get() else {
            return;
        };
        let app = settings.app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(manager) = app.try_state::<PtyManager>() else {
                return;
            };
            let Some(settings) = manager.settings.get() else {
                return;
            };
            let _connecting = manager.connecting.lock().await;
            let Some(client) = manager.current() else {
                return;
            };
            let build = client.hello().build.clone();
            let (Some(binary), false) = (
                settings.binary.clone(),
                build.same_build(&crate::build_identity()),
            ) else {
                return;
            };
            match upgrade_core(settings, binary, client.core_pid(), Some(build), false).await {
                Ok(Upgrade::Done) => {}
                Ok(Upgrade::Deferred) => {
                    eprintln!("Sikemux's terminal core updates once its chat turns end")
                }
                Err(message) => eprintln!("Sikemux keeps its terminal core as it is: {message}"),
            }
        });
    }

    /// Says once per core that this app cannot use it, and offers to end it.
    fn notice_incompatible(&self, settings: &CoreSettings, pid: u32) {
        if self.noticed.swap(pid, Ordering::AcqRel) == pid {
            return;
        }
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let app = settings.app.clone();
        let socket = settings.socket.clone();
        settings
            .app
            .dialog()
            .message(format!(
                "Sikemux's background process (pid {pid}) belongs to another version of Sikemux and could not be updated, so terminals, agents and tasks cannot open in this window.\n\nQuit and Stop Everything ends that process and everything running in it. Sikemux starts a new one when you open it again."
            ))
            .title("Terminals are unavailable")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Quit and Stop Everything".into(),
                "Not Now".into(),
            ))
            .show(move |stop| {
                if !stop {
                    return;
                }
                std::thread::spawn(move || {
                    stop_core_process(&socket, pid);
                    app.exit(0);
                });
            });
    }

    fn block_on_core<F>(&self, work: impl FnOnce(Arc<CoreClient>) -> F)
    where
        F: std::future::Future<Output = Result<(), ClientError>>,
    {
        let Some(client) = self.current() else {
            return;
        };
        let work = work(client);
        let result = tauri::async_runtime::block_on(async move {
            tokio::time::timeout(STOP_TIMEOUT, work).await
        });
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("Sikemux terminal core: {error}"),
            Err(_) => eprintln!("Sikemux terminal core did not answer within {STOP_TIMEOUT:?}"),
        }
        drop(self.streams.take_all());
    }

    /// The page went away with every channel it held. Terminals and tasks keep
    /// running in the core, which stops sending what nobody will show.
    pub fn detach_all(&self) {
        let subscribed = self.streams.release_all();
        let Some(client) = self.current() else {
            return;
        };
        for id in subscribed {
            let _ = client.submit(Request::Detach { id }, |_| ());
        }
    }

    /// The app is leaving and its sessions stay with the core.
    pub fn release(&self) {
        self.closing.store(true, Ordering::Release);
        self.detach_all();
    }

    /// Stops every session and lets the core exit with the app.
    pub fn stop_everything(&self) {
        self.closing.store(true, Ordering::Release);
        self.block_on_core(|client| async move { client.shutdown(true).await });
    }

    pub(crate) fn core_pid(&self) -> Option<u32> {
        self.current().map(|client| client.core_pid())
    }

    pub(crate) async fn sessions(&self) -> AppResult<Vec<SessionInfo>> {
        self.client().await?.list().await.map_err(core_error)
    }

    pub(crate) async fn live_processes(&self) -> AppResult<Vec<PtyProcess>> {
        Ok(self
            .sessions()
            .await?
            .into_iter()
            .filter(|session| session.running)
            .filter_map(|session| {
                Some(PtyProcess {
                    pid: session.pid?,
                    pty_id: session.id,
                    owner: PtyOwner {
                        project: session.project,
                        pane_id: session.pane_id,
                        agent_id: session.agent_id,
                        task_execution_id: session.task_execution_id,
                    },
                })
            })
            .collect())
    }

    pub async fn diagnostics(&self) -> PtyDiagnostics {
        let sessions = match self.current() {
            Some(client) => client.list().await.unwrap_or_default(),
            None => Vec::new(),
        };
        let agents = |state: &str| {
            sessions
                .iter()
                .filter(|session| {
                    session.kind == SessionKind::Terminal
                        && session.agent_state.as_deref() == Some(state)
                })
                .count()
        };
        let (output_frames, output_bytes) = sink::output_totals();
        PtyDiagnostics {
            core_pid: self.core_pid(),
            ptys: sessions.len(),
            subscribers: self.streams.subscriber_count(),
            output_frames,
            output_bytes,
            working_agents: agents("working"),
            blocked_agents: agents("blocked"),
            idle_agents: agents("idle"),
            unknown_agents: agents("unknown"),
        }
    }
}

enum Upgrade {
    Done,
    /// The core goes on as it is until its chat turns end, then updates and
    /// drops its connections, which come back to the updated core.
    Deferred,
}

/// Asks the core at `socket` to replace itself with `binary`, and waits for
/// it to answer from the same process again. Without `old`, the core speaks
/// another protocol and could not say which build it runs. A core that
/// defers is waited for only when `wait_if_deferred`.
async fn upgrade_core(
    settings: &CoreSettings,
    binary: PathBuf,
    pid: u32,
    old: Option<BuildIdentity>,
    wait_if_deferred: bool,
) -> Result<Upgrade, String> {
    let socket = settings.socket.clone();
    let upgraded = tauri::async_runtime::spawn_blocking(move || {
        let request = FrozenRequest::Upgrade { binary };
        match frozen_request(&socket, &request, UPGRADE_ANSWER_TIMEOUT) {
            Ok(FrozenReply::Accepted) => {
                await_upgrade(&socket, pid, old.as_ref(), UPGRADE_RETURN_TIMEOUT)
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            Ok(FrozenReply::Deferred { .. }) if wait_if_deferred => {
                await_deferred_upgrade(&socket, pid, old.as_ref(), DEFERRED_RETURN_TIMEOUT)
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            Ok(FrozenReply::Deferred { .. }) => Ok(None),
            Ok(FrozenReply::Refused { message }) => Err(message),
            Err(error) => Err(error.to_string()),
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    let Some(upgraded) = upgraded else {
        return Ok(Upgrade::Deferred);
    };
    eprintln!(
        "Sikemux updated its terminal core (pid {pid}) to {} {}",
        upgraded.build.version, upgraded.build.commit
    );
    Ok(Upgrade::Done)
}

/// Ends a core this app cannot talk to, with everything running in it.
fn stop_core_process(socket: &std::path::Path, pid: u32) {
    let stopped = frozen_request(socket, &FrozenRequest::StopEverything, STOP_TIMEOUT);
    if !matches!(stopped, Ok(FrozenReply::Accepted)) && pid > 1 {
        // SAFETY: kill only takes integers; the pid is a single process above
        // init, so this never signals a group or every process.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// "Quit and Stop Everything": every terminal, agent and task in the core
/// ends with the app.
pub(crate) fn quit_and_stop_everything(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(manager) = app.try_state::<PtyManager>() {
            manager.stop_everything();
        }
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    use super::{core_error, PtyManager};
    use sikemux_core::client::ClientError;

    fn recorded() -> (tauri::ipc::Channel<tauri::ipc::Response>, Arc<Mutex<usize>>) {
        let messages = Arc::new(Mutex::new(0));
        let counter = messages.clone();
        let channel = tauri::ipc::Channel::new(move |_| {
            *counter.lock().expect("count") += 1;
            Ok(())
        });
        (channel, messages)
    }

    #[test]
    fn leaving_detaches_without_telling_any_pane_its_process_ended() {
        let manager = PtyManager::default();
        let (channel, messages) = recorded();
        {
            let mut guard = manager.streams.lock().expect("lock");
            guard.begin_attach(3).expect("begin");
            guard.finish_attach(3, channel).expect("attach");
        }
        manager.detach_all();
        assert_eq!(manager.streams.subscriber_count(), 0);
        assert_eq!(*messages.lock().expect("count"), 0);
        assert!(!manager.closing.load(Ordering::Acquire));

        manager.release();
        assert!(manager.closing.load(Ordering::Acquire));
    }

    #[test]
    fn stopping_everything_without_a_core_still_marks_the_app_as_leaving() {
        let manager = PtyManager::default();
        manager.stop_everything();
        assert!(manager.closing.load(Ordering::Acquire));
    }

    #[test]
    fn core_errors_keep_the_categories_and_messages_the_frontend_reads() {
        let missing = core_error(ClientError::Core("invalid argument: pty not found".into()));
        assert_eq!(missing.category(), "bad-arg");
        assert_eq!(missing.to_string(), "invalid argument: pty not found");

        let capacity = core_error(ClientError::Core("pty: PTY capacity reached".into()));
        assert_eq!(capacity.category(), "pty");
        assert_eq!(capacity.to_string(), "pty: PTY capacity reached");

        let gone = core_error(ClientError::Disconnected);
        assert_eq!(gone.category(), "pty");
    }
}
