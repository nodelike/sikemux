mod about;
mod access;
mod account;
mod activity;
pub(crate) mod activity_log;
mod agent;
mod agent_accounts;
mod agent_sign_in;
mod agent_view;
mod ansi;
mod approvals;
mod attachments;
mod backdrop;
mod board;
mod callers;
mod calls;
mod catalog;
mod changed;
pub mod chat;
mod cli_install;
pub(crate) mod codehost;
mod collisions;
mod connection;
mod custom_command;
mod data_format;
mod device_view;
mod diagrams;
mod display;
mod drafts;
mod edits;
mod entry;
mod faces;
mod follow;
mod from_app;
mod gestures;
mod git;
mod handover;
mod harness;
mod hooks;
mod host;
mod host_key;
mod hosts;
mod inbox;
mod job_graph;
mod join;
mod kept;
mod launching;
mod layout;
mod lifecycle;
mod load;
mod local_peer;
mod lsp;
mod messages;
mod model_providers;
mod move_agent;
mod names;
mod network_watch;
mod notch;
mod notify;
mod open_files;
mod plugin_views;
pub mod plugins;
mod ports;
mod preferences;
mod prepare;
mod project_files;
mod project_roots;
mod prompt_draft;
mod questions;
mod remote;
mod remote_image;
mod saved;
mod saved_sessions;
pub mod search;
mod seen;
mod session;
mod session_watch;
mod share;
mod shown_cache;
mod skill_library;
mod spare;
mod spawn;
mod ssh;
mod stash;
mod streams;
mod tasks;
pub mod themes;
mod tools;
pub(crate) mod transcript;
mod triggers;
mod updates;
mod upgrade;
mod wait_for;
mod wake;
mod wallpaper;
mod who_wrote;
mod window;
mod workspace;
mod worktree;

pub use calls::{call, NoParams};
pub use entry::main;
pub use host::{info as host_info, PLATFORM as HOST_PLATFORM};
pub use project_roots::ProjectEntry;
pub use remote::{trust_remote_file, ApprovalSetting};
pub use ssh::{Reach, SshHost};
pub use tasks::{ConfigStatus, JavaView, RunView, Source, TaskList, TaskView, TrustQuestion};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{DirBuilder, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
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
use crate::protocol::{encode_control, BuildIdentity, Event, ServerMessage, SessionId};

use connection::{ClientConn, ClientId};
use session::Session;

pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(5 * 60);
/// For a core a service runs, which nothing would start again once it left.
pub const NEVER_IDLE_EXIT: Duration = Duration::from_millis(u64::MAX);
const MAX_ACTIVE_SESSIONS: usize = 256;
const SESSION_POLL: Duration = Duration::from_millis(250);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_TRIM: Duration = Duration::from_secs(10 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long one step of stopping may take before the core gives up on it and stops anyway.
const STOP_STEP_LIMIT: Duration = Duration::from_secs(5);

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
    /// The built-in plugins, which the binary that runs the core provides.
    /// Without them every plugin call says the plugin is missing.
    pub plugins: Option<plugins::Provided>,
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
            plugins: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("a Sikemux core is already running at {}{}", path.display(), pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default())]
    AlreadyRunning { path: PathBuf, pid: Option<u32> },
    #[error("the Sikemux core at {}{} holds its lock but does not answer", path.display(), pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default())]
    Unanswering { path: PathBuf, pid: Option<u32> },
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

/// What this core listens on and was started with, which a hand-over passes
/// to its replacement.
pub(crate) struct Listening {
    pub config: ServerConfig,
    pub listener_fd: std::os::fd::RawFd,
    pub lock_fd: std::os::fd::RawFd,
}

pub(crate) struct Core {
    me: Weak<Core>,
    /// When this core's clock reads zero, which session times count from.
    started: Instant,
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
    pub(crate) updating: upgrade::Updating,
    pub(crate) listening: OnceLock<Listening>,
    pub(crate) tools: Mutex<Option<tools::ToolEndpoint>>,
    pub(crate) chats: chat::Chats,
    pub(crate) remote: remote::Remote,
    pub(crate) shares: share::Shares,
    pub(crate) hooks: hooks::Hooks,
    pub(crate) workspaces: workspace::Workspaces,
    pub(crate) seen: seen::Seen,
    pub(crate) lifecycle: lifecycle::Lifecycle,
    pub(crate) session_watch: session_watch::SessionWatch,
    pub(crate) activity_log: Arc<activity_log::ActivityLog>,
    pub(crate) device_view: device_view::DeviceViews,
    pub(crate) names: names::Names,
    pub(crate) boards: board::Boards,
    pub(crate) triggers: triggers::Triggers,
    pub(crate) changes: changed::Changes,
    pub(crate) ports: ports::Ports,
    pub(crate) spares: spare::Spares,
    pub(crate) updates: updates::Updates,
    pub(crate) remote_images: remote_image::RemoteImages,
    /// Rings when a window opens, so the day's usage counts it at once.
    pub(crate) usage_wake: Notify,
    pub(crate) tasks: tasks::Tasks,
    pub(crate) git: git::Projects,
    /// The code-host lists clients look at, kept and read again while they do.
    pub(crate) plugins: Option<plugins::Provided>,
    pub(crate) code_host_lists: codehost::Lists,
    /// The plugin views panes look at, kept and read again while they do.
    pub(crate) plugin_views: plugin_views::Views,
    /// The files clients have open, watched wherever they are.
    pub(crate) open_files: open_files::OpenFiles,
    pub(crate) layouts: layout::Layouts,
    /// Who this host is signed in to Sikemux as.
    pub(crate) account: account::Account,
    /// The rail numbers running loose terminals have.
    rail_numbers: Mutex<BTreeSet<u32>>,
    /// Watches the SSH config once a client has listed its hosts.
    pub(crate) ssh_watch: OnceLock<Option<::notify::RecommendedWatcher>>,
    pub(crate) ci: codehost::ci::Ci,
    pub(crate) edits: edits::Edits,
    pub(crate) collisions: collisions::Collisions,
    pub(crate) notify: notify::Notifier,
    pub(crate) inbox: inbox::Turns,
    /// Hosts linked with this one, and the agents of teams shared with them.
    pub(crate) hosts: hosts::Hosts,
    /// The faces agents wear, and the style the person picked for them.
    pub(crate) faces: faces::Faces,
    /// How the person wants Sikemux to behave.
    pub(crate) preferences: preferences::Prefs,
    /// What the person has typed and not sent yet, by what it is for.
    pub(crate) drafts: drafts::Drafts,
    /// Where the person moved each diagram's tables, by what it is a diagram of.
    pub(crate) diagrams: diagrams::Diagrams,
    /// Answers in parts on the connections that asked for them.
    pub(crate) streams: streams::Streams,
    pub(crate) look: themes::Look,
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

    #[cfg(test)]
    fn new(build: BuildIdentity, data_dir: Option<&Path>) -> Result<Arc<Self>, ServerError> {
        Self::continuing(build, data_dir, 0, None)
    }

    /// A core whose clock reads `uptime_ms` already: a core that replaced an earlier one keeps
    /// its clock, so the times the earlier one stamped on sessions still mean the same moment.
    fn continuing(
        build: BuildIdentity,
        data_dir: Option<&Path>,
        uptime_ms: u64,
        plugins: Option<plugins::Provided>,
    ) -> Result<Arc<Self>, ServerError> {
        let started = Instant::now()
            .checked_sub(Duration::from_millis(uptime_ms))
            .unwrap_or_else(Instant::now);
        if let Some(dir) = data_dir {
            crate::sessions::index::open(dir);
        }
        let detection = ManifestRegistry::bundled()
            .map_err(|error| ServerError::Manifests(error.to_string()))?;
        let names = names::Names::new(data_dir);
        let hosts = hosts::Hosts::new(data_dir);
        names.pin(hosts.team.ids());
        let harness = harness::Harness::new(data_dir, |agent_id| names.get(agent_id).is_some());
        let core = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            started,
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
            updating: upgrade::Updating::default(),
            listening: OnceLock::new(),
            tools: Mutex::new(None),
            chats: chat::Chats::new(data_dir),
            remote: remote::Remote::default(),
            shares: share::Shares::default(),
            hooks: hooks::Hooks::default(),
            workspaces: workspace::Workspaces::new(data_dir).with_plugins(plugins),
            seen: seen::Seen::default(),
            lifecycle: lifecycle::Lifecycle::default(),
            session_watch: session_watch::SessionWatch::default(),
            activity_log: Arc::new(activity_log::ActivityLog::new(data_dir)),
            device_view: device_view::DeviceViews::default(),
            names,
            boards: board::Boards::new(data_dir),
            triggers: triggers::Triggers::new(data_dir),
            changes: changed::Changes::default(),
            ports: ports::Ports::default(),
            spares: spare::Spares::default(),
            updates: updates::Updates::default(),
            remote_images: remote_image::RemoteImages::default(),
            usage_wake: Notify::new(),
            tasks: tasks::Tasks::new(data_dir),
            git: git::Projects::default(),
            plugins,
            code_host_lists: Default::default(),
            plugin_views: Default::default(),
            open_files: Default::default(),
            layouts: layout::Layouts::new(data_dir),
            account: Default::default(),
            rail_numbers: Mutex::default(),
            ssh_watch: OnceLock::new(),
            ci: Default::default(),
            edits: edits::Edits::new(data_dir, me.clone()),
            collisions: collisions::Collisions::new(me.clone()),
            notify: notify::Notifier::default(),
            inbox: inbox::Turns::default(),
            hosts,
            faces: faces::Faces::new(data_dir),
            preferences: preferences::Prefs::new(data_dir),
            drafts: drafts::Drafts::new(data_dir),
            diagrams: diagrams::Diagrams::new(data_dir),
            streams: streams::Streams::default(),
            look: themes::Look::default(),
        });
        from_app::carry(&core, data_dir);
        core.theme_changed();
        Ok(core)
    }

    /// This core, for work that outlives the call it starts in.
    pub(crate) fn arc(&self) -> Option<Arc<Core>> {
        self.me.upgrade()
    }

    /// Milliseconds on this core's clock, which session times are stamped in.
    pub(crate) fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Resolves the person's theme again, for apps, devices and programs' colour queries.
    pub(crate) fn theme_changed(&self) {
        self.look.set(themes::palette(&self.preferences.get()));
        self.view_changed();
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

    /// The agent's name, given now when it has none. An agent taking up a closed agent's
    /// chat again takes back the name the person gave that one and the buddy it wore, which
    /// it answers too. Reads and writes the disk, so it belongs on a blocking thread.
    pub(crate) fn name_agent(
        &self,
        agent_id: &str,
        cwd: &Path,
        provider: Option<&str>,
        resume_id: Option<&str>,
    ) -> (
        names::AgentName,
        Option<Option<crate::protocol::AgentBuddy>>,
    ) {
        let starting = names::Starting {
            agent_id,
            project: self.workspaces.project_of(cwd),
            provider,
            resume_id,
        };
        let reopened = self.names.reopen(&starting, self.open_agents());
        if let Some(Some(buddy)) = &reopened {
            let _ = self
                .workspaces
                .edit_agent(agent_id, |agent| agent.buddy = Some(buddy.clone()));
        }
        let (named, changed) = self.names.give(starting, self.open_agents());
        if changed || reopened.is_some() {
            self.view_changed();
        }
        (named, reopened)
    }

    /// Each ssh terminal's config host and how it reaches it.
    pub(crate) fn ssh_links(&self) -> Vec<(String, Option<sikemux_pty::shell_protocol::SshLink>)> {
        self.all_sessions()
            .iter()
            .filter(|session| session.is_running())
            .filter_map(|session| session.ssh_link())
            .collect()
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

    /// The lowest rail number no running loose terminal has, taken now.
    fn take_rail_number(&self) -> u32 {
        let mut taken = self.rail_numbers.locked();
        let number = (1..).find(|number| !taken.contains(number)).unwrap_or(0);
        taken.insert(number);
        number
    }

    fn insert_session(&self, session: Arc<Session>) {
        if let Some(number) = session.rail_number() {
            self.rail_numbers.locked().insert(number);
        }
        let ssh = session.ssh_link().is_some();
        self.sessions.locked().insert(session.id, session);
        if ssh {
            ssh::moved(self);
        }
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
            .filter(|s| s.is_running() && !self.spares.is_spare(s.id))
            .count()
    }

    fn is_idle(&self) -> bool {
        !self.remote.is_enabled()
            && self.clients.locked().is_empty()
            && self.running_sessions() == 0
            && self.chats.count() == 0
            && !board::auto::has_work(self)
    }

    pub(crate) fn client_conn(&self, id: ClientId) -> Option<Arc<ClientConn>> {
        self.clients.locked().get(&id).cloned()
    }

    fn register_client(&self, client: Arc<ClientConn>) {
        self.clients.locked().insert(client.id, client);
        self.view_watched();
    }

    fn unregister_client(&self, client: &ClientConn) {
        client.close();
        self.window.unregister(client.id);
        self.chats.forget_client(client.id);
        self.seen.client_gone(client.id);
        lsp::orphan(client.id);
        self.clients.locked().remove(&client.id);
        for (stream, id) in client.take_streams() {
            if let Some(session) = self.session(id) {
                session::detach(&session, stream);
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
                state: state.state,
            }),
            _ => {}
        }
        let devices_hear = matches!(
            event,
            Event::Exited { .. } | Event::ShellMetadata(_) | Event::Changed { .. }
        );
        self.broadcast_to(event, |client| devices_hear || client.peer.is_local());
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
        if device_view::shows(event) {
            self.view_changed();
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

    /// Stops every chat and session. Only reaping a killed process can hang, so each session is
    /// reaped on a thread of its own within `STOP_STEP_LIMIT`, and one still unreaped then is
    /// reported gone without its status, so every exit is still recorded.
    fn drain(self: &Arc<Self>) {
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
        let (reaped, reaping) = std::sync::mpsc::channel();
        for session in &sessions {
            let (core, session, reaped) = (self.clone(), session.clone(), reaped.clone());
            std::thread::spawn(move || {
                session::finish_drain(&core, &session);
                let _ = reaped.send(());
            });
        }
        drop(reaped);
        let deadline = Instant::now() + STOP_STEP_LIMIT;
        let finished = (0..sessions.len())
            .take_while(|_| {
                reaping
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_ok()
            })
            .count();
        if finished < sessions.len() {
            eprintln!(
                "sikemux core: {} sessions were not reaped within {}s; stopping without them",
                sessions.len() - finished,
                STOP_STEP_LIMIT.as_secs()
            );
        }
        for session in &sessions {
            session::report_exit(self, session, None);
            session::detach_all(session);
        }
    }

    fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Stops every chat and session, off the async threads.
    async fn stop_everything(self: &Arc<Self>) {
        let draining = self.clone();
        let _ = connection::blocking(move || {
            draining.drain();
            Ok(())
        })
        .await;
    }
}

/// Waits for `step` of stopping the core, or gives up on it after `STOP_STEP_LIMIT`, saying
/// which step it was, so one that hangs cannot keep the core from exiting.
async fn within_stop_limit<T>(step: &str, stopping: impl std::future::Future<Output = T>) {
    if tokio::time::timeout(STOP_STEP_LIMIT, stopping)
        .await
        .is_err()
    {
        eprintln!(
            "sikemux core: stopping {step} took over {}s; stopping without it",
            STOP_STEP_LIMIT.as_secs()
        );
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
        let path = socket.to_path_buf();
        return Err(match probe(socket, PROBE_TIMEOUT) {
            Ok(hello) => ServerError::AlreadyRunning {
                path,
                pid: Some(hello.pid),
            },
            Err(_) => ServerError::Unanswering {
                path,
                pid: lock_holder(socket),
            },
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
    lock.set_len(0)?;
    lock.write_all_at(std::process::id().to_string().as_bytes(), 0)?;
    Ok((listener, lock))
}

/// The process holding the core's single-instance lock at `socket`, so a core that stopped
/// answering can still be found and stopped. None while nothing holds it.
pub fn lock_holder(socket: &Path) -> Option<u32> {
    let mut lock = File::open(lock_path(socket)).ok()?;
    // SAFETY: flock only reads the integer fd, which `lock` keeps open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return None;
    }
    let mut pid = String::new();
    std::io::Read::read_to_string(&mut lock, &mut pid).ok()?;
    pid.trim().parse().ok().filter(|pid| *pid > 1)
}

/// Flushes shell metadata a quiet prompt left coalesced, and reads settled
/// agent screens.
async fn poll_sessions(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(SESSION_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let now = core.uptime_ms();
        for session in core.all_sessions() {
            agent::poll(&core, &session, now);
            if !session.shell_protocol {
                continue;
            }
            let update = session
                .screen
                .locked()
                .shell
                .as_mut()
                .and_then(|shell| shell.take_due_event(now));
            if let Some(update) = update {
                core.broadcast_event(&Event::ShellMetadata(update.into_event(session.id)));
            }
        }
    }
}

async fn sweep(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = core.uptime_ms();
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
    data_format::upgrade(&config);
    let core = Core::continuing(
        config.build.clone(),
        config.data_dir.as_deref(),
        0,
        config.plugins,
    )?;
    chat::restore(&core);
    workspace::restore(&core);
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
    backdrop::follow(&core);
    if let Some(data_dir) = config.data_dir.as_deref() {
        crate::agent_accounts::usage::keep_readings_in(data_dir.join("plan-limits.json"));
        crate::git_guard::shim::install(data_dir, config.cli_endpoint.as_deref());
        tokio::spawn(workspace::refresh_launchers(core.clone()));
        skill_library::watch_global(&core);
        account::start(&core);
        tokio::spawn(crate::usage::serve(core.clone(), data_dir.to_owned()));
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let background = [
        tokio::spawn(poll_sessions(core.clone())),
        tokio::spawn(sweep(core.clone())),
        tokio::spawn(device_view::serve(core.clone())),
        tokio::spawn(load::watch(core.clone())),
        tokio::spawn(lifecycle::serve(core.clone())),
        tokio::spawn(updates::serve(core.clone())),
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
            |core| &core.triggers.wake,
            triggers::settle,
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
        tokio::spawn(wake::serve(
            core.clone(),
            |core| &core.spares.wake,
            spare::settle,
        )),
        tokio::spawn(workspace::recents(core.clone())),
        tokio::spawn(notify::run(core.clone())),
        tokio::spawn(network_watch::run(core.clone())),
    ];
    let mut shutdown = core.shutdown.subscribe();
    let mut frozen = core.updating.watch();
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
            _ = terminate.recv() => {
                eprintln!("sikemux core: stopping, as something sent it SIGTERM");
                break;
            }
            _ = interrupt.recv() => {
                eprintln!("sikemux core: stopping, as something sent it SIGINT");
                break;
            }
            _ = ticker.tick() => {
                if core.is_idle() && !core.updating.is_frozen() {
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= config.idle_exit {
                        eprintln!(
                            "sikemux core: stopping, as nothing used it for {}s",
                            config.idle_exit.as_secs()
                        );
                        break;
                    }
                } else {
                    idle_since = None;
                }
            }
        }
    }
    say_stopping(&core).await;
    for task in background {
        task.abort();
    }
    core.workspaces.flush();
    remote::stop_network(&core);
    within_stop_limit("remote access", remote::stop(&core)).await;
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

/// How long the app and the notch get to hear that the core is stopping.
const STOPPING_FLUSH: Duration = Duration::from_millis(200);

/// Tells the app and the notch the core is stopping on purpose, before its socket closes, so
/// neither starts it again. A core that dies without a word is started again.
async fn say_stopping(core: &Core) {
    core.broadcast_local(&Event::Changed {
        topic: changed::Topic::Stopping,
        project: None,
        paths: None,
    });
    let deadline = Instant::now() + STOPPING_FLUSH;
    for client in core.clients() {
        client
            .wait_flushed(deadline.saturating_duration_since(Instant::now()))
            .await;
    }
}

pub fn run(config: ServerConfig) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sikemux-core")
        .build()?;
    let result = runtime.block_on(serve(config));
    lsp::drain_all();
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
