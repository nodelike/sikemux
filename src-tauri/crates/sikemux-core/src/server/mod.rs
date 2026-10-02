mod access;
mod agent;
mod bonjour;
mod chat;
mod connection;
mod entry;
mod handover;
mod harness;
mod host;
mod pairing;
mod prepare;
mod remote;
mod session;
mod tools;
mod upgrade;
mod window;
mod workspace;

pub use entry::main;

use std::collections::HashMap;
use std::fs::{DirBuilder, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sikemux_pty::agent_detection::{ManifestRegistry, ManifestReloadReport};

use sikemux_pty::error::PtyError;
use sikemux_pty::process::DRAIN_GRACE;
use sikemux_pty::task::{
    task_reclamation_plan, TaskRetentionCandidate, MAX_RETAINED_EXITED_TASK_PTYS,
};
use tokio::sync::{watch, Notify};

use crate::client::{probe, ProbeError};
use crate::protocol::{encode_control, BuildIdentity, DeviceView, Event, ServerMessage, SessionId};

use connection::{ClientConn, ClientId};
use session::Session;

pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(5 * 60);
const MAX_ACTIVE_SESSIONS: usize = 256;
const SESSION_POLL: Duration = Duration::from_millis(250);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_TRIM: Duration = Duration::from_secs(10 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How soon a paired device sees a change to what it shows of this Mac.
const DEVICE_VIEW_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub socket: PathBuf,
    pub idle_exit: Duration,
    pub build: BuildIdentity,
    /// Where to publish the agents' tool endpoint. Without it the core serves
    /// none.
    pub cli_endpoint: Option<PathBuf>,
    /// The app's data directory, for the harness journal and tool tally.
    /// Without it they are kept in memory, or not at all.
    pub data_dir: Option<PathBuf>,
    /// Remote access listens on loopback only, with no relay and without
    /// publishing the core's address. For tests.
    pub remote_direct_only: bool,
}

impl ServerConfig {
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            idle_exit: DEFAULT_IDLE_EXIT,
            build: BuildIdentity::default(),
            cli_endpoint: None,
            data_dir: None,
            remote_direct_only: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("a Sikemux core is already running at {}{}", path.display(), pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default())]
    AlreadyRunning { path: PathBuf, pid: Option<u32> },
    #[error("{} is in use by a process that is not a Sikemux core", .0.display())]
    SocketInUse(PathBuf),
    #[error("the bundled agent detection rules do not load: {0}")]
    Manifests(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub(crate) struct CoreError(String);

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl CoreError {
    fn pty(error: impl std::fmt::Display) -> Self {
        Self(format!("pty: {error}"))
    }

    fn poisoned<T>(_: T) -> Self {
        Self("pty: lock poisoned".into())
    }
}

impl From<&str> for CoreError {
    fn from(message: &str) -> Self {
        Self(message.into())
    }
}

impl From<String> for CoreError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<PtyError> for CoreError {
    fn from(error: PtyError) -> Self {
        Self(error.to_string())
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self(format!("io: {error}"))
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("json: {error}"))
    }
}

pub(crate) type CoreResult<T> = Result<T, CoreError>;

static EPOCH: OnceLock<Instant> = OnceLock::new();

fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

/// A core that replaced an earlier one keeps its clock, so the times the
/// earlier one stamped on sessions still mean the same moment.
pub(crate) fn continue_clock(uptime_ms: u64) {
    let start = Instant::now()
        .checked_sub(Duration::from_millis(uptime_ms))
        .unwrap_or_else(Instant::now);
    let _ = EPOCH.set(start);
}

pub(crate) fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// Counts launching, live and retained sessions, so the PTY budget holds even
/// while a session is between being spawned and being published.
pub(crate) struct Capacity {
    active: AtomicUsize,
    limit: usize,
}

pub(crate) struct CapacityPermit(Arc<Capacity>);

impl Capacity {
    fn try_acquire(self: &Arc<Self>) -> CoreResult<CapacityPermit> {
        self.active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limit).then_some(active + 1)
            })
            .map(|_| CapacityPermit(self.clone()))
            .map_err(|_| CoreError::from("pty: PTY capacity reached"))
    }
}

impl Drop for CapacityPermit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
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

/// What this core listens on and was started with, which a hand-over passes
/// to its replacement.
pub(crate) struct Listening {
    pub config: ServerConfig,
    pub listener_fd: std::os::fd::RawFd,
    pub lock_fd: std::os::fd::RawFd,
}

pub(crate) struct Core {
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    next_session_id: AtomicU64,
    capacity: Arc<Capacity>,
    clients: Mutex<HashMap<ClientId, Arc<ClientConn>>>,
    next_client_id: AtomicU64,
    shutdown: watch::Sender<bool>,
    pub(crate) build: BuildIdentity,
    pub(crate) detection: RwLock<ManifestRegistry>,
    manifest_dir: Mutex<Option<PathBuf>>,
    pub(crate) harness: harness::Harness,
    pub(crate) window: window::Window,
    /// True while the core hands itself over to a newer binary: it takes no
    /// new work and its readers stop at the next whole chunk.
    pub(crate) frozen: watch::Sender<bool>,
    pub(crate) upgrading: std::sync::atomic::AtomicBool,
    /// Readers that hold output not yet fed to their screen.
    pub(crate) pumping: Gauge,
    /// Sessions being spawned that are not in the table yet.
    pub(crate) launching: Gauge,
    pub(crate) listening: OnceLock<Listening>,
    pub(crate) tools: Mutex<Option<tools::ToolEndpoint>>,
    pub(crate) chats: chat::Chats,
    pub(crate) remote: remote::Remote,
    pub(crate) workspaces: workspace::Workspaces,
    /// The view paired devices were last sent, as sent.
    device_view: Mutex<Vec<u8>>,
}

/// Session and window call ids start from the clock, so an id a client still
/// holds from a core that has since restarted never names a new one.
fn first_session_id() -> SessionId {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
        .max(1)
}

impl Core {
    fn new(build: BuildIdentity, data_dir: Option<&Path>) -> Result<Arc<Self>, ServerError> {
        let detection = ManifestRegistry::bundled()
            .map_err(|error| ServerError::Manifests(error.to_string()))?;
        Ok(Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            next_session_id: AtomicU64::new(first_session_id()),
            capacity: Arc::new(Capacity {
                active: AtomicUsize::new(0),
                limit: MAX_ACTIVE_SESSIONS,
            }),
            clients: Mutex::new(HashMap::new()),
            next_client_id: AtomicU64::new(1),
            shutdown: watch::channel(false).0,
            build,
            detection: RwLock::new(detection),
            manifest_dir: Mutex::new(None),
            harness: harness::Harness::new(data_dir),
            window: window::Window::new(first_session_id()),
            frozen: watch::channel(false).0,
            upgrading: std::sync::atomic::AtomicBool::new(false),
            pumping: Gauge::default(),
            launching: Gauge::default(),
            listening: OnceLock::new(),
            tools: Mutex::new(None),
            chats: chat::Chats::default(),
            remote: remote::Remote::default(),
            workspaces: workspace::Workspaces::default(),
            device_view: Mutex::new(Vec::new()),
        }))
    }

    pub(crate) fn is_frozen(&self) -> bool {
        *self.frozen.borrow()
    }

    /// Resolves once the core starts handing itself over.
    pub(crate) async fn until_frozen(&self) {
        let mut frozen = self.frozen.subscribe();
        let _ = frozen.wait_for(|frozen| *frozen).await;
    }

    pub(crate) fn manifest_dir(&self) -> Option<PathBuf> {
        self.manifest_dir.lock().ok().and_then(|dir| dir.clone())
    }

    pub(crate) fn has_local_client(&self) -> bool {
        self.clients().iter().any(|client| client.peer.is_local())
    }

    fn clients(&self) -> Vec<Arc<ClientConn>> {
        self.clients
            .lock()
            .map(|clients| clients.values().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn manifest_report(&self) -> CoreResult<ManifestReloadReport> {
        self.detection
            .read()
            .map(|registry| registry.report())
            .map_err(|_| "agent detection registry lock poisoned".into())
    }

    /// Rebuilds the detection rules from the bundled ones and the person's
    /// directory. Blocks while it reads the directory.
    pub(crate) fn configure_manifests(
        &self,
        directory: Option<PathBuf>,
    ) -> CoreResult<ManifestReloadReport> {
        *self.manifest_dir.lock().map_err(CoreError::poisoned)? = directory;
        self.reload_manifests()
    }

    pub(crate) fn reload_manifests(&self) -> CoreResult<ManifestReloadReport> {
        let directory = self
            .manifest_dir
            .lock()
            .map_err(CoreError::poisoned)?
            .clone();
        let manifests = |error: sikemux_pty::agent_detection::ManifestError| {
            CoreError::from(format!("agent detection manifests: {error}"))
        };
        let mut replacement = match directory {
            Some(directory) => ManifestRegistry::with_override_dir(directory).map_err(manifests)?,
            None => ManifestRegistry::bundled().map_err(manifests)?,
        };
        let report = replacement.reload().map_err(manifests)?;
        *self.detection.write().map_err(CoreError::poisoned)? = replacement;
        // The screens may be unchanged while the rules are not, so every agent
        // is read again on the next poll.
        for session in self.all_sessions() {
            if let Some(agent) = session.agent.as_ref() {
                agent.invalidate_detection();
            }
        }
        Ok(report)
    }

    pub(crate) fn agent_session(&self, agent_id: &str) -> Option<Arc<Session>> {
        self.all_sessions().into_iter().find(|session| {
            session
                .agent
                .as_ref()
                .is_some_and(|agent| agent.agent_id() == agent_id)
        })
    }

    pub(crate) fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions.lock().ok()?.get(&id).cloned()
    }

    fn all_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .map(|sessions| sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    fn insert_session(&self, session: Arc<Session>) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session.id, session);
        }
    }

    pub(crate) fn remove_session(&self, session: &Arc<Session>) -> bool {
        let removed = self.sessions.lock().is_ok_and(|mut sessions| {
            if sessions
                .get(&session.id)
                .is_some_and(|current| Arc::ptr_eq(current, session))
            {
                sessions.remove(&session.id);
                true
            } else {
                false
            }
        });
        if removed {
            session::detach_all(session);
        }
        removed
    }

    fn take_session_for_kill(&self, id: SessionId) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.lock().ok()?;
        let session = sessions.get(&id)?.clone();
        // A killed task keeps its snapshot and output log for later reads.
        if !session.is_task() {
            sessions.remove(&id);
            drop(sessions);
            session::detach_all(&session);
        }
        Some(session)
    }

    fn running_sessions(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.values().filter(|s| s.is_running()).count())
            .unwrap_or(0)
    }

    fn is_idle(&self) -> bool {
        !self.remote.is_enabled()
            && self.clients.lock().is_ok_and(|clients| clients.is_empty())
            && self.running_sessions() == 0
            && self.chats.count() == 0
    }

    fn register_client(&self, client: Arc<ClientConn>) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.insert(client.id, client);
        }
    }

    fn unregister_client(&self, client: &ClientConn) {
        client.close();
        self.window.unregister(client.id);
        self.chats.forget_client(client.id);
        if let Ok(mut clients) = self.clients.lock() {
            clients.remove(&client.id);
        }
        for id in client.take_subscriptions() {
            if let Some(session) = self.session(id) {
                session::detach(&session, client.id);
            }
        }
    }

    /// Paired devices hear only of terminals ending and changing; the rest
    /// reaches them through [`Core::publish_device_view`].
    pub(crate) fn broadcast_event(&self, event: &Event) {
        let devices_hear = matches!(event, Event::Exited { .. } | Event::ShellMetadata(_));
        self.broadcast_to(event, |client| devices_hear || client.peer.is_local());
    }

    fn device_view_frame(&self) -> Option<Vec<u8>> {
        let mut sessions: Vec<_> = self.all_sessions().iter().map(|s| s.info()).collect();
        sessions.sort_by_key(|info| info.id);
        let mut chats = self.workspaces.listed(self.chats.list());
        for chat in &mut chats {
            chat.pending_permissions.sort();
        }
        let mut attentions = self.chats.attentions();
        attentions.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        let view = DeviceView {
            workspace: self.workspaces.view(),
            sessions,
            chats,
            attentions,
        };
        encode_control(&ServerMessage::Event {
            event: Event::DeviceView { view },
        })
        .ok()
    }

    /// Sends every paired device what it shows of this Mac, when that changed
    /// since they were last sent it.
    pub(crate) fn publish_device_view(&self) {
        let devices: Vec<_> = self
            .clients()
            .into_iter()
            .filter(|client| !client.peer.is_local())
            .collect();
        if devices.is_empty() {
            return;
        }
        let Ok(mut last) = self.device_view.lock() else {
            return;
        };
        let Some(frame) = self.device_view_frame() else {
            return;
        };
        if *last == frame {
            return;
        }
        let shared: Arc<[u8]> = frame.as_slice().into();
        for client in devices {
            client.send(shared.clone());
        }
        *last = frame;
    }

    /// A device that just connected starts from the whole view.
    pub(crate) fn send_device_view(&self, client: &ClientConn) {
        let Ok(_last) = self.device_view.lock() else {
            return;
        };
        if let Some(frame) = self.device_view_frame() {
            client.send(frame.into());
        }
    }

    pub(crate) fn broadcast_local(&self, event: &Event) {
        self.broadcast_to(event, |client| client.peer.is_local());
    }

    fn broadcast_to(&self, event: &Event, to: impl Fn(&ClientConn) -> bool) {
        let Ok(frame) = encode_control(&ServerMessage::Event {
            event: event.clone(),
        }) else {
            return;
        };
        let frame: Arc<[u8]> = frame.into();
        for client in self.clients() {
            if to(&client) {
                client.send(frame.clone());
            }
        }
    }

    /// Ends the connections of one paired device, or of every one.
    pub(crate) fn close_device_clients(&self, id: Option<&str>) {
        for client in self.clients() {
            let closing = match id {
                Some(id) => client.peer.is_device(id),
                None => !client.peer.is_local(),
            };
            if closing {
                client.close();
            }
        }
    }

    pub(crate) fn permit(&self, peer: &access::Peer, needs: access::Needs) -> CoreResult<()> {
        let device_access = match peer {
            access::Peer::Local => None,
            access::Peer::Device { id } => self.remote.access_of(id),
        };
        access::permit(peer, device_access, needs)
    }

    pub(crate) fn schedule_task_output_notice(self: &Arc<Self>, id: SessionId, delay: Duration) {
        let core = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(session) = core.session(id) {
                session::mark_task_output_noticed(&session);
            }
            core.broadcast_event(&Event::TaskOutput { id });
            harness::note_output(&core, id);
        });
    }

    /// Drops the oldest exited sessions nobody is watching once there are too
    /// many or they have been kept long enough.
    pub(crate) fn reclaim_exited_sessions(&self, now: u64) {
        let sessions = self.all_sessions();
        let candidates = sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                Some(TaskRetentionCandidate {
                    id: index as u32,
                    exited_at_ms: session.exited_at_ms.load(Ordering::Acquire),
                    has_subscribers: session.has_subscribers()?,
                })
            })
            .collect();
        for index in task_reclamation_plan(candidates, now, MAX_RETAINED_EXITED_TASK_PTYS) {
            let Some(session) = sessions.get(index as usize) else {
                continue;
            };
            if session.exited_at_ms.load(Ordering::Acquire) != 0
                && session.has_subscribers() == Some(false)
            {
                self.remove_session(session);
            }
        }
    }

    fn drain(&self) {
        self.chats.stop_all();
        let sessions: Vec<Arc<Session>> = match self.sessions.lock() {
            Ok(mut sessions) => sessions.drain().map(|(_, session)| session).collect(),
            Err(_) => return,
        };
        if sessions.is_empty() {
            return;
        }
        for session in &sessions {
            session::signal_for_drain(session);
        }
        std::thread::sleep(DRAIN_GRACE);
        for session in &sessions {
            session::finish_drain(self, session);
            session::detach_all(session);
        }
    }

    fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }
}

fn lock_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    PathBuf::from(path)
}

/// Takes the single-instance lock and binds the socket. The lock is held for
/// the life of the core and released by the kernel when it exits.
fn claim_socket(socket: &Path) -> Result<(std::os::unix::net::UnixListener, File), ServerError> {
    if let Some(parent) = socket
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if !parent.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path(socket))?;
    // SAFETY: flock only reads the integer fd, which `lock` keeps open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ServerError::AlreadyRunning {
            path: socket.to_path_buf(),
            pid: probe(socket, PROBE_TIMEOUT).ok().map(|hello| hello.pid),
        });
    }
    match probe(socket, PROBE_TIMEOUT) {
        Ok(hello) => {
            return Err(ServerError::AlreadyRunning {
                path: socket.to_path_buf(),
                pid: Some(hello.pid),
            })
        }
        Err(ProbeError::Rejected { pid, .. }) => {
            return Err(ServerError::AlreadyRunning {
                path: socket.to_path_buf(),
                pid: Some(pid),
            })
        }
        Err(ProbeError::Unanswered(_)) => {
            return Err(ServerError::SocketInUse(socket.to_path_buf()))
        }
        Err(ProbeError::NotRunning(_)) => {}
    }
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok((listener, lock))
}

/// Flushes shell metadata a quiet prompt left coalesced, and reads settled
/// agent screens.
async fn poll_sessions(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(SESSION_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let now = now_ms();
        for session in core.all_sessions() {
            agent::poll(&core, &session, now);
            if !session.shell_protocol {
                continue;
            }
            let update = session.parser.lock().ok().and_then(|mut parser| {
                parser
                    .callbacks_mut()
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.take_due_event(now))
            });
            if let Some(update) = update {
                core.broadcast_event(&Event::ShellMetadata(
                    sikemux_pty::shell_protocol::PtyShellMetadataEvent::from_update(
                        session.id, update,
                    ),
                ));
            }
        }
    }
}

async fn device_views(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(DEVICE_VIEW_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        core.publish_device_view();
    }
}

async fn sweep(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = now_ms();
        core.reclaim_exited_sessions(now);
        for session in core.all_sessions() {
            session::trim_if_idle(&session, now, IDLE_TRIM);
        }
    }
}

/// Serves until a client asks it to shut down or the core has been idle for
/// `idle_exit`.
pub async fn serve(config: ServerConfig) -> Result<(), ServerError> {
    let socket = config.socket.clone();
    let (listener, lock) = tokio::task::spawn_blocking(move || claim_socket(&socket))
        .await
        .map_err(std::io::Error::other)??;
    let core = Core::new(config.build.clone(), config.data_dir.as_deref())?;
    if let Some(path) = config.cli_endpoint.clone() {
        match tools::ToolEndpoint::start(core.clone(), path).await {
            Ok(tools) => {
                if let Ok(mut slot) = core.tools.lock() {
                    *slot = Some(tools);
                }
            }
            Err(error) => eprintln!("sikemux core: agents' tools are unavailable: {error}"),
        }
    }
    run_core(core, listener, lock, config).await
}

/// The accept loop of a core, whether it started fresh or took over from an
/// earlier one.
pub(crate) async fn run_core(
    core: Arc<Core>,
    listener: std::os::unix::net::UnixListener,
    lock: File,
    config: ServerConfig,
) -> Result<(), ServerError> {
    let _ = core.listening.set(Listening {
        config: config.clone(),
        listener_fd: listener.as_raw_fd(),
        lock_fd: lock.as_raw_fd(),
    });
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    remote::start(&core, &config.socket, config.remote_direct_only).await;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let background = [
        tokio::spawn(poll_sessions(core.clone())),
        tokio::spawn(sweep(core.clone())),
        tokio::spawn(device_views(core.clone())),
    ];
    let mut shutdown = core.shutdown.subscribe();
    let mut frozen = core.frozen.subscribe();
    let mut ticker = tokio::time::interval(
        (config.idle_exit / 10).clamp(Duration::from_millis(10), Duration::from_secs(1)),
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut idle_since: Option<Instant> = None;
    loop {
        // While the core hands itself over, connections wait in the socket's
        // backlog for its replacement.
        let accepting = !*frozen.borrow_and_update();
        tokio::select! {
            accepted = listener.accept(), if accepting => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(connection::serve_local(core.clone(), stream));
                }
                Err(error) => {
                    eprintln!("sikemux core: accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            _ = frozen.changed() => {}
            _ = shutdown.changed() => break,
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            _ = ticker.tick() => {
                if core.is_idle() && !core.is_frozen() {
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= config.idle_exit {
                        break;
                    }
                } else {
                    idle_since = None;
                }
            }
        }
    }
    for task in background {
        task.abort();
    }
    remote::stop(&core).await;
    let tools = core.tools.lock().ok().and_then(|mut tools| tools.take());
    if let Some(tools) = tools {
        tools.stop();
    }
    drop(listener);
    let _ = std::fs::remove_file(&config.socket);
    drop(lock);
    Ok(())
}

pub fn run(config: ServerConfig) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sikemux-core")
        .build()?;
    let result = runtime.block_on(serve(config));
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::Capacity;

    #[test]
    fn session_capacity_is_hard_under_concurrent_admission() {
        const LIMIT: usize = 7;
        const CONTENDERS: usize = 64;
        let capacity = Arc::new(Capacity {
            active: AtomicUsize::new(0),
            limit: LIMIT,
        });
        let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
        let results = std::thread::scope(|scope| {
            let handles = (0..CONTENDERS)
                .map(|_| {
                    let capacity = capacity.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        capacity.try_acquire().ok()
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("capacity contender"))
                .collect::<Vec<_>>()
        });
        let mut permits = results.into_iter().flatten().collect::<Vec<_>>();

        assert_eq!(permits.len(), LIMIT);
        assert!(capacity.try_acquire().is_err());
        permits.pop();
        let replacement = capacity.try_acquire().expect("released slot is reusable");
        assert_eq!(capacity.active.load(Ordering::Acquire), LIMIT);
        drop(replacement);
        drop(permits);
        assert_eq!(capacity.active.load(Ordering::Acquire), 0);
    }
}
