pub mod about;
pub(crate) use sikemux_serve::server::above;
pub(crate) use sikemux_serve::server::access;
pub mod account;
mod activity;
pub(crate) use sikemux_serve::server::agent;
pub use sikemux_workspace::server::activity_log;
pub mod agent_accounts;
mod agent_now;
pub mod agent_sign_in;
pub mod agent_updates;
pub mod agent_view;
mod approvals;
pub use sikemux_workspace::server::attachments;
mod backdrop;
mod base;
pub mod board;
pub(crate) use sikemux_workspace::server::caller;
pub mod calls;
pub(crate) use sikemux_serve::server::catalog;
pub(crate) use sikemux_serve::server::changed;
pub mod chat;
mod chat_above;
mod checks;
pub mod chief;
pub use sikemux_workspace::server::cli_install;
pub mod codehost;
mod collisions;
pub(crate) use sikemux_serve::server::connection;
pub mod custom_command;
mod data_format;
mod device_view;
pub mod diagnosis;
pub use sikemux_workspace::server::diagrams;
mod display;
pub use sikemux_workspace::server::drafts;
pub mod edits;
mod entry;
pub use sikemux_workspace::server::faces;
mod fast_forward;
mod follow;
mod from_app;
pub(crate) use sikemux_workspace::server::gestures;
pub mod git;
mod goal_summary;
mod goals;
mod handover;
pub mod harness;
mod hooks;
pub mod host;
pub(crate) use sikemux_serve::server::host_key;
pub mod host_stats;
pub mod hosts;
pub mod inbox;
mod join;
pub(crate) use sikemux_serve::server::kept;
pub mod landing;
pub mod launching;
pub mod layout;
mod lifecycle;
mod line;
pub(crate) use sikemux_serve::server::local_peer;
pub(crate) use sikemux_serve::server::log_times;
pub mod lsp;
pub mod mcp_apps;
pub mod memory;
mod merge_queue;
pub mod messages;
pub use sikemux_workspace::server::model_providers;
mod model_proxy;
pub mod mods;
pub mod move_agent;
pub use sikemux_workspace::server::names;
mod network_watch;
pub use sikemux_workspace::server::new_project;
mod notch;
mod notify;
pub use sikemux_workspace::server::open_files;
pub mod outside;
mod overlap;
pub(crate) use sikemux_serve::server::part;
mod parts;
pub(crate) mod person_calls;
pub mod pictures;
mod pins;
pub mod places;
pub mod playbook;
pub mod plugins;
pub mod ports;
pub(crate) use sikemux_serve::server::prepare;
pub use sikemux_workspace::server::preferences;
pub use sikemux_workspace::server::project_files;
pub use sikemux_workspace::server::project_roots;
pub mod questions;
pub(crate) use sikemux_serve::server::reaper;
pub mod remote;
pub mod remote_image;
mod repo_events;
mod requests;
mod roster;
mod runtime_file;
pub mod saved;
pub(crate) use sikemux_serve::server::serving;
pub use sikemux_serve::server::session;
pub use sikemux_workspace::server::saved_sessions;
pub use sikemux_workspace::server::search;
pub(crate) use sikemux_workspace::server::seen;
mod session_watch;
pub mod settling;
pub mod setup_check;
pub mod share;
pub mod ship;
mod shown_cache;
pub mod skill_library;
pub mod spare;
mod spawn;
pub mod ssh;
mod stalls;
pub mod stash;
pub mod streams;
pub mod tasks;
mod team_told;
pub mod themes;
mod tools;
pub mod triage;
pub mod triggers;
pub mod turn_state;
pub mod updates;
pub(crate) use sikemux_serve::server::updating;
mod upgrade;
pub mod usage;
pub mod vault;
mod wait_for;
pub(crate) use sikemux_serve::server::waits;
pub(crate) use sikemux_serve::server::wake;
pub mod wallpaper;
mod who_wrote;
mod why;
pub(crate) use sikemux_serve::server::window;
pub use sikemux_workspace::server::workspace;
pub(crate) use sikemux_workspace::server::workspace_above;
pub(crate) use sikemux_workspace::server::workspace_state;
pub mod worktree;

pub use calls::call;
pub use entry::main;
pub use turn_state::TakeStepParams;

use crate::server::changed::ChangedCore;
use crate::server::parts::PartCore;
use crate::server::serving::ServeCore;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_corekit::lock::lock_holder;
use sikemux_corekit::lock::lock_path;
use std::collections::HashMap;
use std::fs::{DirBuilder, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use sikemux_process::sync::{Locked, Shared};
use sikemux_pty::agent_detection::ManifestRegistry;
pub use sikemux_pty::agent_detection::{ManifestReloadReport, ManifestSourceKind};

use sikemux_pty::process::DRAIN_GRACE;
use tokio::sync::Notify;

use crate::client::{probe, ProbeError};
use sikemux_wire::protocol::{BuildIdentity, Event, SessionId};

pub(crate) use sikemux_agents::callers;
pub(crate) use sikemux_kernel::{Core, CoreError, CoreResult, Listening};
pub(crate) use sikemux_serve::server::Capacity;

use changed::What;
use parts::Parts;
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
    /// The first wait before an agent that stopped by itself is started again;
    /// it doubles with each restart. Tests shorten it.
    pub restart_wait: Duration,
    /// How long a forced update waits for chats still starting before it stops
    /// them. Tests shorten it.
    pub handover_settle: Duration,
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
            restart_wait: chat::RESTART_WAIT,
            handover_settle: upgrade::HANDOVER_SETTLE,
        }
    }

    /// What a released core runs with, however it came to be running.
    pub(crate) fn production(
        socket: PathBuf,
        idle_exit: Duration,
        cli_endpoint: Option<PathBuf>,
        data_dir: Option<PathBuf>,
        build: BuildIdentity,
        plugins: Option<plugins::Provided>,
    ) -> Self {
        Self {
            socket,
            idle_exit,
            build,
            cli_endpoint,
            attachment_dir: data_dir.as_deref().and_then(attachments::pasted_dir),
            data_dir,
            remote_direct_only: false,
            accounts_api: Some(crate::accounts::api_base()),
            secret_store: true,
            plugins,
            restart_wait: chat::RESTART_WAIT,
            handover_settle: upgrade::HANDOVER_SETTLE,
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

/// Session and window call ids start from the clock, so an id a client still
/// holds from a core that has since restarted never names a new one.
fn first_session_id() -> SessionId {
    sikemux_process::clock::epoch_ms().max(1)
}

/// What a running core does with its sessions, its clients and its own stopping.
pub(crate) trait ServerCore {
    /// The core's own identity for a session it spawns itself, or for a
    /// device on another machine, whose paths mean nothing here.
    fn launch_identity(&self) -> sikemux_wire::protocol::LaunchIdentity;

    #[cfg(test)]
    fn new(build: BuildIdentity, data_dir: Option<&Path>) -> Result<Arc<Self>, ServerError>;

    /// Brings the data folder up to date and puts what agents find first on their PATH in
    /// place, then builds the core, so nothing it starts, the chats taken up from an earlier
    /// core included, runs without its checks.
    fn open(
        config: ServerConfig,
        listening: Listening,
        uptime_ms: u64,
    ) -> Result<Arc<Self>, ServerError>;

    /// A core whose clock reads `uptime_ms` already: a core that replaced an earlier one keeps
    /// its clock, so the times the earlier one stamped on sessions still mean the same moment.
    fn built(
        config: ServerConfig,
        listening: Listening,
        uptime_ms: u64,
    ) -> Result<Arc<Self>, ServerError>;

    /// The preferences as devices draw them, with Omarchy's background as the pane image
    /// while it is followed. The picture the person chose stays saved underneath.
    fn shown_preferences(&self) -> sikemux_view::prefs::Preferences;

    /// Resolves the person's theme again, for apps, devices and programs' colour queries.
    fn theme_changed(&self);

    fn manifest_dir(&self) -> Option<PathBuf>;

    fn manifest_report(&self) -> ManifestReloadReport;

    /// Rebuilds the detection rules from the bundled ones and the person's
    /// directory. Blocks while it reads the directory.
    fn configure_manifests(&self, directory: Option<PathBuf>) -> CoreResult<ManifestReloadReport>;

    fn reload_manifests(&self) -> CoreResult<ManifestReloadReport>;

    /// Each ssh terminal's config host and how it reaches it.
    fn ssh_links(&self) -> Vec<(String, Option<sikemux_pty::shell_protocol::SshLink>)>;

    fn running_sessions(&self) -> usize;

    fn is_idle(&self) -> bool;

    fn permit(&self, peer: &access::Peer, needs: access::Needs) -> CoreResult<()>;

    /// Puts every chat to sleep and stops every session. Only reaping a killed process can
    /// hang, so each session is reaped on a thread of its own within `STOP_STEP_LIMIT`, and one
    /// still unreaped then is reported gone without its status, so every exit is still recorded.
    fn drain(self: &Arc<Self>);

    fn begin_shutdown(&self);

    /// Puts every chat to sleep and stops every session, off the async threads.
    async fn stop_everything(self: &Arc<Self>);
}

impl ServerCore for Core {
    fn launch_identity(&self) -> sikemux_wire::protocol::LaunchIdentity {
        sikemux_wire::protocol::LaunchIdentity {
            version: self.config().build.version.clone(),
            cli_executable: std::env::current_exe().ok(),
            cli_endpoint: self.config().cli_endpoint.clone(),
        }
    }

    #[cfg(test)]
    fn new(build: BuildIdentity, data_dir: Option<&Path>) -> Result<Arc<Self>, ServerError> {
        sikemux_process::user_environment::search_path_only_from_now();
        let config = ServerConfig {
            build,
            data_dir: data_dir.map(Path::to_owned),
            ..ServerConfig::new(PathBuf::new())
        };
        let listening = Listening {
            listener_fd: -1,
            lock_fd: -1,
        };
        Self::built(config, listening, 0)
    }

    fn open(
        config: ServerConfig,
        listening: Listening,
        uptime_ms: u64,
    ) -> Result<Arc<Self>, ServerError> {
        data_format::upgrade(&config);
        if let Some(data_dir) = config.data_dir.as_deref() {
            crate::git_guard::shim::install(data_dir, config.cli_endpoint.as_deref());
        }
        Self::built(config, listening, uptime_ms)
    }

    fn built(
        config: ServerConfig,
        listening: Listening,
        uptime_ms: u64,
    ) -> Result<Arc<Self>, ServerError> {
        let data_dir = config.data_dir.clone();
        let data_dir = data_dir.as_deref();
        let started = Instant::now()
            .checked_sub(Duration::from_millis(uptime_ms))
            .unwrap_or_else(Instant::now);
        if let Some(dir) = data_dir {
            crate::sessions::index::open(dir);
            crate::sessions::said::open(dir);
        }
        let detection = ManifestRegistry::bundled()
            .map_err(|error| ServerError::Manifests(error.to_string()))?;
        let names = names::Names::new(data_dir);
        let hosts = hosts::Hosts::new(data_dir);
        names.pin(hosts.team.ids());
        let harness = harness::Harness::new(data_dir, |agent_id| names.get(agent_id).is_some());
        let activity_log = Arc::new(activity_log::ActivityLog::new(data_dir));
        let person_calls = person_calls::PersonCalls::new(data_dir, activity_log.clone(), |id| {
            harness.questions.is_open(id)
        });
        let core = Core::build(started, listening, |me| {
            vec![
                Box::new(serving::ServeFields {
                    sessions: Mutex::new(HashMap::new()),
                    next_session_id: AtomicU64::new(first_session_id()),
                    capacity: Arc::new(Capacity {
                        active: AtomicUsize::new(0),
                        limit: MAX_ACTIVE_SESSIONS,
                    }),
                    connections: Mutex::new(HashMap::new()),
                    next_client_id: AtomicU64::new(1),
                    detection: RwLock::new(detection),
                    window: window::Window::new(first_session_id()),
                    waits: waits::Waits::new(data_dir),
                    changes: changed::Changes::default(),
                    updating: upgrade::Updating::default(),
                    rail_numbers: Mutex::default(),
                    above: base::above(),
                    build: config.build.clone(),
                }) as sikemux_kernel::Parts,
                Box::new(workspace_state::WorkspaceFields {
                    workspaces: workspace::Workspaces::new(data_dir, from_app::app_state(data_dir)),
                    names,
                    clones: new_project::Clones::default(),
                    preferences: preferences::Prefs::new(data_dir, themes::on_omarchy()),
                    activity_log,
                    open_files: Default::default(),
                    drafts: drafts::Drafts::new(data_dir),
                    diagrams: diagrams::Diagrams::new(data_dir),
                    seen: seen::Seen::default(),
                    faces: faces::Faces::new(data_dir),
                    workspace_above: base::workspace_above(),
                    attachment_dir: config.attachment_dir.clone(),
                    data_dir: config.data_dir.clone(),
                }) as sikemux_kernel::Parts,
                Box::new(parts::Fields {
                    config,
                    manifest_folder: Mutex::new(None),
                    model_proxy: model_proxy::ModelProxy::default(),
                    ports: ports::Ports::new(data_dir),
                    usage_wake: Notify::new(),
                    ssh_watch: OnceLock::new(),
                    streams: streams::Streams::default(),
                    look: themes::Look::default(),
                    harness,
                    tools: Mutex::new(None),
                    chats: chat::Chats::new(data_dir),
                    remote: remote::Remote::default(),
                    outside: outside::Outside::new(data_dir),
                    shares: share::Shares::default(),
                    hooks: hooks::Hooks::default(),
                    settling: settling::Settling::default(),
                    stalls: stalls::Stalls::default(),
                    lifecycle: lifecycle::Lifecycle::default(),
                    ship: ship::Ships::default(),
                    person_calls,
                    pins: pins::Pins::new(data_dir),
                    build_outputs: Default::default(),
                    line: line::Line::new(data_dir),
                    session_watch: session_watch::SessionWatch::default(),
                    device_view: device_view::DeviceViews::default(),
                    host_watch: host_stats::HostWatch::default(),
                    boards: board::Boards::new(data_dir),
                    places: places::store::Store::new(data_dir),
                    goal_summaries: goal_summary::Summaries::new(data_dir),
                    goals: goals::Goals::default(),
                    repo_events: repo_events::RepoEvents::default(),
                    landing: landing::Histories::default(),
                    land_queue: Default::default(),
                    pictures: pictures::Pictures::new(data_dir),
                    memory: memory::Memory::new(data_dir),
                    triggers: triggers::Triggers::new(data_dir),
                    spares: spare::Spares::default(),
                    updates: updates::Updates::default(),
                    remote_images: remote_image::RemoteImages::default(),
                    tasks: tasks::Tasks::new(data_dir),
                    git: git::Projects::default(),
                    code_host_lists: Default::default(),
                    layouts: layout::Layouts::new(data_dir),
                    account: Default::default(),
                    vault: vault::Vault::default(),
                    ci: Default::default(),
                    edits: edits::Edits::new(data_dir, me.clone()),
                    collisions: collisions::Collisions::new(me.clone()),
                    checks: Default::default(),
                    notify: notify::Notifier::default(),
                    inbox: inbox::Turns::new(data_dir),
                    joins: Default::default(),
                    hosts,
                    mods: mods::Mods::new(data_dir),
                    mcp_apps: mcp_apps::McpApps::new(data_dir),
                    chiefs: chief::Chiefs::new(data_dir),
                    chat_above: base::chat_above(),
                }) as sikemux_kernel::Parts,
            ]
        });
        from_app::carry(&core, data_dir);
        pictures::tidy(&core, false);
        core.theme_changed();
        Ok(core)
    }

    fn shown_preferences(&self) -> sikemux_view::prefs::Preferences {
        let mut preferences = self.preferences().get();
        if preferences.pane_image_follows_omarchy {
            if let Some(background) = self.look().omarchy_background() {
                preferences.pane_image = Some(background);
            }
        }
        preferences
    }

    fn theme_changed(&self) {
        let omarchy = self.look().omarchy();
        self.look()
            .set(themes::palette(&self.preferences().get(), omarchy.as_ref()));
        self.changed(What::View);
    }

    fn manifest_dir(&self) -> Option<PathBuf> {
        self.manifest_folder().locked().clone()
    }

    fn manifest_report(&self) -> ManifestReloadReport {
        self.detection().read_locked().report()
    }

    fn configure_manifests(&self, directory: Option<PathBuf>) -> CoreResult<ManifestReloadReport> {
        *self.manifest_folder().locked() = directory;
        self.reload_manifests()
    }

    fn reload_manifests(&self) -> CoreResult<ManifestReloadReport> {
        let directory = self.manifest_folder().locked().clone();
        let manifests = |error: sikemux_pty::agent_detection::ManifestError| {
            CoreError::from(format!("agent detection manifests: {error}"))
        };
        let mut replacement = match directory {
            Some(directory) => ManifestRegistry::with_override_dir(directory).map_err(manifests)?,
            None => ManifestRegistry::bundled().map_err(manifests)?,
        };
        let report = replacement.reload().map_err(manifests)?;
        *self.detection().write_locked() = replacement;
        // The screens may be unchanged while the rules are not, so every agent
        // is read again on the next poll.
        for session in self.all_sessions() {
            if let Some(agent) = session.agent.as_ref() {
                agent.invalidate_detection();
            }
        }
        Ok(report)
    }

    fn ssh_links(&self) -> Vec<(String, Option<sikemux_pty::shell_protocol::SshLink>)> {
        self.all_sessions()
            .iter()
            .filter(|session| session.is_running())
            .filter_map(|session| session.ssh_link())
            .collect()
    }

    fn running_sessions(&self) -> usize {
        self.sessions()
            .locked()
            .values()
            .filter(|s| s.is_running() && !self.spares().is_spare(s.id))
            .count()
    }

    fn is_idle(&self) -> bool {
        !self.remote().is_enabled()
            && self.connections().locked().is_empty()
            && self.running_sessions() == 0
            && self.chats().count() == 0
            && !board::auto::has_work(self.boards())
    }

    fn permit(&self, peer: &access::Peer, needs: access::Needs) -> CoreResult<()> {
        let standing = match peer {
            access::Peer::App | access::Peer::Local | access::Peer::Share { .. } => None,
            access::Peer::Device { id } => self.remote().standing_of(id),
        };
        access::permit(peer, standing, needs)
    }

    fn drain(self: &Arc<Self>) {
        lifecycle::sleep_all(self);
        let sessions: Vec<Arc<Session>> = self
            .sessions()
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
        self.shutdown().send_replace(true);
    }

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
                core.changed(What::Event(&Event::ShellMetadata(
                    update.into_event(session.id),
                )));
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
    let listening = Listening {
        listener_fd: listener.as_raw_fd(),
        lock_fd: lock.as_raw_fd(),
    };
    let core = Core::open(config, listening, 0)?;
    *core.tools().locked() = start_tools(&core).await;
    chat::restore(&core);
    // Only a core started afresh: one that took over keeps the tabs that were open.
    workspace::restore(&core);
    run_core(core, listener, lock).await
}

pub(crate) async fn start_tools(core: &Arc<Core>) -> Option<tools::ToolEndpoint> {
    let path = core.config().cli_endpoint.clone()?;
    tools::ToolEndpoint::start(core.clone(), path)
        .await
        .inspect_err(|error| eprintln!("sikemux core: agents' tools are unavailable: {error}"))
        .ok()
}

/// The accept loop of a core, whether it started fresh or took over from an
/// earlier one.
pub(crate) async fn run_core(
    core: Arc<Core>,
    listener: std::os::unix::net::UnixListener,
    lock: File,
) -> Result<(), ServerError> {
    let config = core.config().clone();
    core.chats().clear_history();
    local_peer::settle();
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    remote::start(&core, &config).await;
    runtime_file::write(
        &remote::file_path(&config.socket),
        core.remote().owner(),
        core.remote().core_id(),
    );
    if config.plugins.is_some() {
        vault::start(&core);
    }
    git::follow_projects(&core);
    backdrop::follow(&core);
    goal_summary::name_unnamed(&core);
    if let Some(data_dir) = config.data_dir.as_deref() {
        crate::agent_accounts::usage::keep_readings_in(data_dir.join("plan-limits.json"));
        tokio::spawn(workspace::refresh_launchers(core.clone()));
        if let Err(error) = crate::skills::place_built_in(data_dir) {
            eprintln!(
                "sikemux core: the skills Sikemux ships were not put in the library: {error}"
            );
        }
        skill_library::watch_global(&core);
        account::start(&core);
        tokio::spawn(usage::serve(core.clone(), data_dir.to_owned()));
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut background = core.start_parts();
    background.extend([
        tokio::spawn(poll_sessions(core.clone())),
        tokio::spawn(sweep(core.clone())),
        tokio::spawn(network_watch::run(core.clone())),
    ]);
    let mut shutdown = core.shutdown().subscribe();
    let mut frozen = core.updating().watch();
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
                if core.is_idle() && !core.updating().is_frozen() {
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
    core.flush_parts();
    remote::stop_network(&core);
    within_stop_limit("remote access", remote::stop(&core)).await;
    remote::stop_live(&core);
    let tools = core.tools().locked().take();
    if let Some(tools) = tools {
        tools.stop();
    }
    drop(listener);
    let _ = std::fs::remove_file(&config.socket);
    runtime_file::remove(&remote::file_path(&config.socket));
    drop(lock);
    crate::waiting::wake(&config.socket);
    Ok(())
}

/// How long the app and the notch get to hear that the core is stopping.
const STOPPING_FLUSH: Duration = Duration::from_millis(200);

/// Tells the app and the notch the core is stopping on purpose, before its socket closes, so
/// neither starts it again. A core that dies without a word is started again.
async fn say_stopping(core: &Core) {
    core.changed(What::Event(&Event::Changed {
        topic: changed::Topic::Stopping.to_json(),
        project: None,
        paths: None,
    }));
    let deadline = Instant::now() + STOPPING_FLUSH;
    for client in core.clients() {
        client
            .wait_flushed(deadline.saturating_duration_since(Instant::now()))
            .await;
    }
}

pub fn run(config: ServerConfig) -> Result<(), ServerError> {
    on_runtime(serve(config))
}

/// Runs a core on a runtime of its own until it stops, then ends what outlives its tasks.
pub(crate) fn on_runtime(
    core: impl std::future::Future<Output = Result<(), ServerError>>,
) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sikemux-core")
        .build()?;
    let result = runtime.block_on(core);
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
