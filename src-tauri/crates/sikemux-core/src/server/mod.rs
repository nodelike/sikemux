mod access;
mod activity;
pub(crate) mod activity_log;
mod agent;
mod agent_accounts;
mod agent_sign_in;
mod attachments;
mod board;
mod brief;
mod callers;
mod calls;
mod changed;
mod chat;
mod ci;
mod collisions;
mod connection;
mod custom_command;
mod display;
mod edits;
mod entry;
mod follow;
mod git;
mod guardrails;
mod handover;
mod harness;
mod host;
mod host_key;
mod hosts;
mod inbox;
mod join;
mod kept;
mod launching;
mod lifecycle;
mod load;
mod local_peer;
mod messages;
mod model_providers;
mod names;
mod network_watch;
mod notify;
pub mod plugins;
mod ports;
mod prepare;
mod questions;
mod remote;
mod sandbox;
mod saved;
mod saved_sessions;
mod seen;
mod services;
mod session;
mod session_watch;
mod share;
mod skill_library;
mod spawn;
mod suggestions;
mod tasks;
mod tools;
mod transcript;
mod upgrade;
mod wake;
mod who_wrote;
mod window;
mod workspace;

pub use entry::main;
pub use host::{info as host_info, PLATFORM as HOST_PLATFORM};
pub use remote::trust_remote_file;

use std::collections::{HashMap, HashSet};
use std::fs::{DirBuilder, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use sikemux_process::sync::{Locked, Shared};
use sikemux_pty::agent_detection::{ManifestRegistry, ManifestReloadReport};

use sikemux_pty::error::PtyError;
use sikemux_pty::process::DRAIN_GRACE;
use sikemux_pty::task::{
    task_reclamation_plan, TaskRetentionCandidate, MAX_RETAINED_EXITED_TASK_PTYS,
};
use tokio::sync::{watch, Notify};

use crate::client::{probe, ProbeError};
use crate::protocol::{
    encode_control, AgentGroup, BuildIdentity, DeviceView, Event, ServerMessage, SessionId,
};

use connection::{ClientConn, ClientId};
use session::Session;

pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(5 * 60);
const MAX_ACTIVE_SESSIONS: usize = 256;
const SESSION_POLL: Duration = Duration::from_millis(250);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_TRIM: Duration = Duration::from_secs(10 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How often what a paired device shows of this host is looked at again
/// while something in it moves by itself, such as an agent mid-turn.
const DEVICE_VIEW_INTERVAL: Duration = Duration::from_millis(400);
/// A burst of changes to the view goes out as one.
const DEVICE_VIEW_GATHER: Duration = Duration::from_millis(50);
/// However much changes, the view is built at most this often.
const DEVICE_VIEW_SPACING: Duration = Duration::from_millis(100);

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
    /// Where files devices send for a chat are kept. Without it devices
    /// cannot send files.
    pub attachment_dir: Option<PathBuf>,
    /// Remote access listens on loopback only, with no relay and without
    /// publishing the core's address. For tests.
    pub remote_direct_only: bool,
    /// The accounts server this host keeps a live connection to while it is
    /// signed in. Without it the host learns nothing from its account.
    pub accounts_api: Option<String>,
    /// The host's key and the digest of the devices it trusts live in the
    /// person's secret store. Without it they live in a file beside the
    /// socket, for tests.
    pub secret_store: bool,
}

impl ServerConfig {
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            idle_exit: DEFAULT_IDLE_EXIT,
            build: BuildIdentity::default(),
            cli_endpoint: None,
            data_dir: None,
            attachment_dir: None,
            remote_direct_only: false,
            accounts_api: None,
            secret_store: false,
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

static STARTED: OnceLock<Instant> = OnceLock::new();

fn started() -> Instant {
    *STARTED.get_or_init(Instant::now)
}

/// A core that replaced an earlier one keeps its clock, so the times the
/// earlier one stamped on sessions still mean the same moment.
pub(crate) fn continue_clock(uptime_ms: u64) {
    let start = Instant::now()
        .checked_sub(Duration::from_millis(uptime_ms))
        .unwrap_or_else(Instant::now);
    let _ = STARTED.set(start);
}

pub(crate) fn uptime_ms() -> u64 {
    started().elapsed().as_millis() as u64
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
    me: Weak<Core>,
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
    pub(crate) shares: share::Shares,
    pub(crate) workspaces: workspace::Workspaces,
    pub(crate) seen: seen::Seen,
    pub(crate) lifecycle: lifecycle::Lifecycle,
    pub(crate) session_watch: session_watch::SessionWatch,
    pub(crate) activity_log: Arc<activity_log::ActivityLog>,
    /// Pokes the device view awake once something may be watching it.
    pub(crate) view_watched: Notify,
    /// Sends the device view before its next tick, once an agent's state changed.
    pub(crate) view_stale: Notify,
    pub(crate) names: names::Names,
    pub(crate) boards: board::Boards,
    pub(crate) changes: changed::Changes,
    pub(crate) ports: ports::Ports,
    pub(crate) tasks: tasks::Tasks,
    pub(crate) git_watches: crate::git::watch::Watches,
    pub(crate) ci: ci::Ci,
    pub(crate) edits: edits::Edits,
    pub(crate) collisions: collisions::Collisions,
    pub(crate) notify: notify::Notifier,
    pub(crate) inbox: inbox::Inbox,
    /// Hosts linked with this one, and the agents of teams shared with them.
    pub(crate) hosts: hosts::Hosts,
    /// The view paired devices were last sent, as sent.
    device_view: Mutex<Vec<u8>>,
}

/// Session and window call ids start from the clock, so an id a client still
/// holds from a core that has since restarted never names a new one.
fn first_session_id() -> SessionId {
    sikemux_process::clock::epoch_ms().max(1)
}

impl Core {
    /// The core's own identity for a session it spawns itself, or for a
    /// device on another machine, whose paths mean nothing here.
    pub(crate) fn launch_identity(&self) -> crate::protocol::LaunchIdentity {
        crate::protocol::LaunchIdentity {
            version: self.build.version.clone(),
            cli_executable: std::env::current_exe().ok(),
            cli_endpoint: self
                .listening
                .get()
                .and_then(|listening| listening.config.cli_endpoint.clone()),
        }
    }

    fn new(build: BuildIdentity, data_dir: Option<&Path>) -> Result<Arc<Self>, ServerError> {
        let detection = ManifestRegistry::bundled()
            .map_err(|error| ServerError::Manifests(error.to_string()))?;
        let names = names::Names::new(data_dir);
        let hosts = hosts::Hosts::new(data_dir);
        names.pin(hosts.team.ids());
        let harness = harness::Harness::new(data_dir, |agent_id| names.get(agent_id).is_some());
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
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
            harness,
            window: window::Window::new(first_session_id()),
            frozen: watch::channel(false).0,
            upgrading: std::sync::atomic::AtomicBool::new(false),
            pumping: Gauge::default(),
            launching: Gauge::default(),
            listening: OnceLock::new(),
            tools: Mutex::new(None),
            chats: chat::Chats::new(data_dir),
            remote: remote::Remote::default(),
            shares: share::Shares::default(),
            workspaces: workspace::Workspaces::new(data_dir),
            seen: seen::Seen::default(),
            lifecycle: lifecycle::Lifecycle::default(),
            session_watch: session_watch::SessionWatch::default(),
            activity_log: Arc::new(activity_log::ActivityLog::new(data_dir)),
            view_watched: Notify::new(),
            view_stale: Notify::new(),
            names,
            boards: board::Boards::new(data_dir),
            changes: changed::Changes::default(),
            ports: ports::Ports::default(),
            tasks: tasks::Tasks::new(data_dir),
            git_watches: Default::default(),
            ci: Default::default(),
            edits: edits::Edits::new(data_dir, me.clone()),
            collisions: collisions::Collisions::new(me.clone()),
            notify: notify::Notifier::default(),
            inbox: inbox::Inbox::default(),
            hosts,
            device_view: Mutex::new(Vec::new()),
        }))
    }

    /// This core, for work that outlives the call it starts in.
    pub(crate) fn arc(&self) -> Option<Arc<Core>> {
        self.me.upgrade()
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
        self.manifest_dir.locked().clone()
    }

    fn clients(&self) -> Vec<Arc<ClientConn>> {
        self.clients.locked().values().cloned().collect()
    }

    pub(crate) fn manifest_report(&self) -> ManifestReloadReport {
        self.detection.read_locked().report()
    }

    /// Rebuilds the detection rules from the bundled ones and the person's
    /// directory. Blocks while it reads the directory.
    pub(crate) fn configure_manifests(
        &self,
        directory: Option<PathBuf>,
    ) -> CoreResult<ManifestReloadReport> {
        *self.manifest_dir.locked() = directory;
        self.reload_manifests()
    }

    pub(crate) fn reload_manifests(&self) -> CoreResult<ManifestReloadReport> {
        let directory = self.manifest_dir.locked().clone();
        let manifests = |error: sikemux_pty::agent_detection::ManifestError| {
            CoreError::from(format!("agent detection manifests: {error}"))
        };
        let mut replacement = match directory {
            Some(directory) => ManifestRegistry::with_override_dir(directory).map_err(manifests)?,
            None => ManifestRegistry::bundled().map_err(manifests)?,
        };
        let report = replacement.reload().map_err(manifests)?;
        *self.detection.write_locked() = replacement;
        // The screens may be unchanged while the rules are not, so every agent
        // is read again on the next poll.
        for session in self.all_sessions() {
            if let Some(agent) = session.agent.as_ref() {
                agent.invalidate_detection();
            }
        }
        Ok(report)
    }

    /// Tells [`Seen`] the agents each phone shows: the chats and terminals it
    /// has open while its app is in front.
    fn show_phones(&self) {
        let phones: Vec<_> = self
            .clients()
            .into_iter()
            .filter(|client| !client.peer.is_local())
            .collect();
        if phones.is_empty() {
            return;
        }
        let agent_sessions: Vec<_> = self
            .all_sessions()
            .into_iter()
            .filter_map(|session| Some((session.id, session.agent.as_ref()?.agent_id().to_owned())))
            .collect();
        for phone in phones {
            let mut shown = HashSet::new();
            if phone.in_front() {
                shown.extend(self.chats.followed_by(phone.id));
                shown.extend(
                    agent_sessions
                        .iter()
                        .filter(|(id, _)| phone.is_subscribed(*id))
                        .map(|(_, agent_id)| agent_id.clone()),
                );
            }
            self.seen.show(phone.id, shown, true);
        }
    }

    /// Every chat and agent terminal running here.
    pub(crate) fn running_agents(&self) -> HashSet<String> {
        let mut running: HashSet<String> = self.chats.ids().into_iter().collect();
        running.extend(self.all_sessions().iter().filter_map(|session| {
            session
                .agent
                .as_ref()
                .map(|agent| agent.agent_id().to_owned())
        }));
        running
    }

    /// The agents that keep their names: those running here and those the
    /// workspace holds, asleep or not.
    pub(crate) fn open_agents(&self) -> HashSet<String> {
        let mut open = self.running_agents();
        open.extend(self.workspaces.agent_ids());
        open
    }

    /// The agent's name, given now when it has none. Reads and writes the
    /// disk, so it belongs on a blocking thread.
    pub(crate) fn name_agent(
        &self,
        agent_id: &str,
        cwd: &Path,
        provider: Option<&str>,
        resume_id: Option<&str>,
    ) -> names::AgentName {
        let project = names::project_of(cwd, &self.workspaces.project_paths());
        let (named, changed) = self.names.give(
            names::Starting {
                agent_id,
                project,
                provider,
                resume_id,
            },
            self.open_agents(),
        );
        if changed {
            self.view_changed();
        }
        named
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
        self.sessions.locked().get(&id).cloned()
    }

    fn all_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions.locked().values().cloned().collect()
    }

    fn insert_session(&self, session: Arc<Session>) {
        self.sessions.locked().insert(session.id, session);
    }

    pub(crate) fn remove_session(&self, session: &Arc<Session>) -> bool {
        let removed = {
            let mut sessions = self.sessions.locked();
            let current = sessions
                .get(&session.id)
                .is_some_and(|current| Arc::ptr_eq(current, session));
            if current {
                sessions.remove(&session.id);
            }
            current
        };
        if removed {
            session::detach_all(session);
        }
        removed
    }

    fn take_session_for_kill(&self, id: SessionId) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.locked();
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
            .locked()
            .values()
            .filter(|s| s.is_running())
            .count()
    }

    fn is_idle(&self) -> bool {
        !self.remote.is_enabled()
            && self.clients.locked().is_empty()
            && self.running_sessions() == 0
            && self.chats.count() == 0
            && !board::auto_has_work(self)
    }

    fn register_client(&self, client: Arc<ClientConn>) {
        self.clients.locked().insert(client.id, client);
        self.view_watched.notify_one();
    }

    fn unregister_client(&self, client: &ClientConn) {
        client.close();
        self.window.unregister(client.id);
        self.chats.forget_client(client.id);
        self.seen.client_gone(client.id);
        self.clients.locked().remove(&client.id);
        for id in client.take_subscriptions() {
            if let Some(session) = self.session(id) {
                session::detach(&session, client.id);
            }
        }
        self.view_changed();
    }

    /// Paired devices hear only of terminals ending and changing; the rest
    /// reaches them in the device view.
    pub(crate) fn broadcast_event(&self, event: &Event) {
        match event {
            Event::Attention { attention } => {
                self.notify
                    .send(notify::Signal::Attention(Box::new(attention.clone())));
            }
            Event::AttentionCleared { id, agent_id } => self.notify.send(notify::Signal::Cleared {
                id: id.clone(),
                agent_id: agent_id.clone(),
            }),
            Event::AgentState(state) => self.notify.send(notify::Signal::AgentState {
                agent_id: state.agent_id.clone(),
                state: state.state.clone(),
            }),
            _ => {}
        }
        let devices_hear = matches!(
            event,
            Event::Exited { .. } | Event::ShellMetadata(_) | Event::Changed { .. }
        );
        self.broadcast_to(event, |client| devices_hear || client.peer.is_local());
    }

    /// Every session, with what the app calls its agent and whether the
    /// person has seen what the agent last did.
    pub(crate) fn session_infos(&self) -> Vec<crate::protocol::SessionInfo> {
        let mut sessions: Vec<_> = self.all_sessions().iter().map(|s| s.info()).collect();
        sessions.sort_by_key(|info| info.id);
        for info in &mut sessions {
            if let Some(agent_id) = &info.agent_id {
                info.agent_name = self.names.name(agent_id);
                info.title = self.workspaces.title(agent_id);
                info.unread = self.seen.unread(agent_id);
                info.active_at = self.seen.active_at(agent_id);
            }
            let asking = info
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| self.harness.questions.blocks_on_person(agent_id));
            display::show_session(info, asking);
        }
        sessions
    }

    /// The chats under the app's names, most recently active first, with
    /// whether the person has seen what each last did.
    pub(crate) fn chat_infos(&self) -> Vec<crate::protocol::ChatInfo> {
        let mut chats = self.workspaces.listed(self.chats.list());
        for chat in &mut chats {
            chat.name = self.names.name(&chat.agent_id);
            chat.pending_permissions.sort();
            chat.unread = self.seen.unread(&chat.agent_id);
            chat.active_at = self.seen.active_at(&chat.agent_id);
            let asking = self.harness.questions.blocks_on_person(&chat.agent_id);
            display::show_chat(chat, asking);
        }
        chats.sort_by_key(|chat| std::cmp::Reverse(chat.active_at));
        chats
    }

    /// The chats and sessions, each agent numbered in the order it wants the
    /// person.
    pub(crate) fn ranked_infos(
        &self,
    ) -> (
        Vec<crate::protocol::ChatInfo>,
        Vec<crate::protocol::SessionInfo>,
    ) {
        let mut sessions = self.session_infos();
        let mut chats = self.chat_infos();
        display::rank(&mut chats, &mut sessions, |agent_id| {
            self.seen.started_at(agent_id)
        });
        (chats, sessions)
    }

    /// The view devices are sent, and whether anything in it moves by itself.
    fn device_view_frame(&self) -> Option<(Vec<u8>, bool)> {
        let (chats, sessions) = self.ranked_infos();
        let moving = chats.iter().any(|chat| {
            chat.group == AgentGroup::Working
                || chat.subagents > 0
                || chat.tasks > 0
                || chat.reconnecting.is_some()
        }) || sessions
            .iter()
            .any(|session| session.group == Some(AgentGroup::Working));
        let mut attentions = self.chats.attentions();
        attentions.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        let recent = self.workspaces.recent(&chats);
        let inbox = serde_json::to_value(inbox::view(self, &chats, &sessions)).ok()?;
        let view = DeviceView {
            workspace: self.workspaces.view(),
            sessions,
            chats,
            attentions,
            recent,
            inbox,
        };
        let frame = encode_control(&ServerMessage::Event {
            event: Event::DeviceView { view },
        })
        .ok()?;
        Some((frame, moving))
    }

    /// Sends every paired device what it shows of this host, when that changed
    /// since they were last sent it.
    fn view_watchers(&self) -> Vec<Arc<ClientConn>> {
        self.clients()
            .into_iter()
            .filter(|client| !client.peer.is_local() || client.watches_view())
            .collect()
    }

    /// Whether anything in the view moves by itself.
    fn publish_device_view(&self) -> bool {
        let devices = self.view_watchers();
        if devices.is_empty() {
            return false;
        }
        let mut last = self.device_view.locked();
        let Some((frame, moving)) = self.device_view_frame() else {
            return false;
        };
        if *last != frame {
            let shared: Arc<[u8]> = frame.as_slice().into();
            for client in devices {
                client.send(shared.clone());
            }
            *last = frame;
        }
        moving
    }

    /// A device that just connected starts from the whole view.
    pub(crate) fn send_device_view(&self, client: &ClientConn) {
        let _last = self.device_view.locked();
        if let Some((frame, _)) = self.device_view_frame() {
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
        self.view_stale.notify_one();
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
        let standing = match peer {
            access::Peer::App | access::Peer::Local | access::Peer::Share { .. } => None,
            access::Peer::Device { id } => self.remote.standing_of(id),
        };
        access::permit(peer, standing, needs)
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
            .map(|(index, session)| TaskRetentionCandidate {
                id: index as u32,
                exited_at_ms: session.exited_at_ms.load(Ordering::Acquire),
                has_subscribers: session.has_subscribers(),
            })
            .collect();
        for index in task_reclamation_plan(candidates, now, MAX_RETAINED_EXITED_TASK_PTYS) {
            let Some(session) = sessions.get(index as usize) else {
                continue;
            };
            if session.exited_at_ms.load(Ordering::Acquire) != 0 && !session.has_subscribers() {
                self.remove_session(session);
            }
        }
    }

    fn drain(&self) {
        self.chats.stop_all();
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .locked()
            .drain()
            .map(|(_, session)| session)
            .collect();
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
        let now = uptime_ms();
        for session in core.all_sessions() {
            agent::poll(&core, &session, now);
            if !session.shell_protocol {
                continue;
            }
            let update = session
                .parser
                .locked()
                .callbacks_mut()
                .shell
                .as_mut()
                .and_then(|shell| shell.take_due_event(now));
            if let Some(update) = update {
                core.broadcast_event(&Event::ShellMetadata(update.into_event(session.id)));
            }
        }
    }
}

/// Sends devices the view when anything in it may have changed, and on a
/// tick only while something in it moves by itself.
async fn device_views(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(DEVICE_VIEW_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut window = None;
    let mut moving = true;
    let mut built = tokio::time::Instant::now();
    loop {
        if core.view_watchers().is_empty() {
            core.view_watched.notified().await;
            ticker.reset();
        }
        let changed = if moving {
            tokio::select! {
                _ = ticker.tick() => false,
                () = core.view_stale.notified() => true,
            }
        } else {
            core.view_stale.notified().await;
            true
        };
        if changed {
            let now = tokio::time::Instant::now();
            tokio::time::sleep_until((now + DEVICE_VIEW_GATHER).max(built + DEVICE_VIEW_SPACING))
                .await;
        }
        core.show_phones();
        let now = core.window.client_id();
        if now != window {
            window = now;
            core.agents_changed();
        }
        moving = core.publish_device_view();
        built = tokio::time::Instant::now();
    }
}

async fn sweep(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = uptime_ms();
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
    chat::restore(&core);
    core.chats.clear_history();
    if let Some(path) = config.cli_endpoint.clone() {
        match tools::ToolEndpoint::start(core.clone(), path).await {
            Ok(tools) => {
                *core.tools.locked() = Some(tools);
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
    local_peer::settle();
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    remote::start(&core, &config).await;
    core.chats.resets.arm_saved(&core);
    git::follow_projects(&core);
    if let Some(data_dir) = config.data_dir.as_deref() {
        crate::git_guard::shim::install(data_dir, config.cli_endpoint.as_deref());
        tokio::spawn(workspace::refresh_launchers(core.clone()));
        skill_library::watch_global(&core);
        if crate::usage::Install::Host.reports() {
            let host = host_info();
            let core = core.clone();
            tokio::spawn(crate::usage::report_host(
                data_dir.to_owned(),
                host.version,
                host.channel,
                move || core.remote.owner(),
            ));
        }
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let background = [
        tokio::spawn(poll_sessions(core.clone())),
        tokio::spawn(sweep(core.clone())),
        tokio::spawn(device_views(core.clone())),
        tokio::spawn(lifecycle::serve(core.clone())),
        tokio::spawn(changed::serve(core.clone())),
        tokio::spawn(tasks::publish_runs(core.clone())),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.harness.mailbox.wake,
            messages::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.harness.questions.wake,
            questions::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.boards.wake,
            board::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.collisions.wake,
            collisions::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.workspaces.wake,
            workspace::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.hosts.wake,
            hosts::settle,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.hosts.relink,
            hosts::account::relink,
        )),
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.ports.wake,
            ports::settle,
        )),
        tokio::spawn(workspace::recents(core.clone())),
        tokio::spawn(notify::run(core.clone())),
        tokio::spawn(network_watch::run(core.clone())),
    ];
    let mut shutdown = core.shutdown.subscribe();
    let mut frozen = core.frozen.subscribe();
    crate::waiting::wake(&config.socket);
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
    remote::stop_network(&core);
    remote::stop(&core).await;
    remote::stop_live(&core);
    let tools = core.tools.locked().take();
    if let Some(tools) = tools {
        tools.stop();
    }
    drop(listener);
    let _ = std::fs::remove_file(&config.socket);
    drop(lock);
    crate::waiting::wake(&config.socket);
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
