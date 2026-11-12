//! What each feature of the core keeps for as long as the core runs, and how the rest reads it:
//! `core.boards()`, never a field.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use sikemux_pty::agent_detection::ManifestRegistry;
use sikemux_wire::protocol::SessionId;
use tokio::sync::Notify;

use super::connection::{ClientConn, ClientId};
use super::session::Session;
use super::{changed, model_proxy, ports, preferences, streams, themes, waits, window, Capacity};

use super::{
    account, activity_log, board, builds, chat, checks, chief, codehost, collisions, device_view,
    diagrams, drafts, edits, faces, git, goal_summary, goals, harness, hooks, host_stats, hosts,
    inbox, join, landing, layout, lifecycle, mcp_apps, memory, mods, names, new_project, notify,
    open_files, outside, person_calls, pictures, pins, places, remote, remote_image, repo_events,
    seen, session_watch, settling, share, ship, spare, stalls, tasks, tools, triggers, updates,
    upgrade, vault, workspace, worktree, Core,
};

pub(crate) struct Fields {
    pub(super) sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    pub(super) next_session_id: AtomicU64,
    pub(super) capacity: Arc<Capacity>,
    pub(super) connections: Mutex<HashMap<ClientId, Arc<ClientConn>>>,
    pub(super) next_client_id: AtomicU64,
    pub(super) detection: RwLock<ManifestRegistry>,
    pub(super) manifest_folder: Mutex<Option<PathBuf>>,
    pub(super) window: window::Window,
    pub(super) model_proxy: model_proxy::ModelProxy,
    pub(super) waits: waits::Waits,
    pub(super) changes: changed::Changes,
    pub(super) ports: ports::Ports,
    /// Rings when a window opens, so the day's usage counts it at once.
    pub(super) usage_wake: Notify,
    /// The rail numbers running loose terminals have.
    pub(super) rail_numbers: Mutex<BTreeSet<u32>>,
    /// Watches the SSH config once a client has listed its hosts.
    pub(super) ssh_watch: OnceLock<Option<::notify::RecommendedWatcher>>,
    pub(super) preferences: preferences::Prefs,
    /// Answers in parts on the connections that asked for them.
    pub(super) streams: streams::Streams,
    pub(super) look: themes::Look,
    pub(super) harness: harness::Harness,
    pub(super) updating: upgrade::Updating,
    pub(super) tools: Mutex<Option<tools::ToolEndpoint>>,
    pub(super) chats: chat::Chats,
    pub(super) remote: remote::Remote,
    /// Programs Sikemux did not start that the person lets drive its chats.
    pub(super) outside: outside::Outside,
    pub(super) shares: share::Shares,
    pub(super) hooks: hooks::Hooks,
    pub(super) workspaces: workspace::Workspaces,
    pub(super) seen: seen::Seen,
    pub(super) settling: settling::Settling,
    pub(super) stalls: stalls::Stalls,
    pub(super) lifecycle: lifecycle::Lifecycle,
    /// Agents' actions that leave this machine, held for the person.
    pub(super) ship: ship::Ships,
    /// The decisions put to the person.
    pub(super) person_calls: person_calls::PersonCalls,
    pub(super) places: places::store::Store,
    pub(super) pins: pins::Pins,
    pub(super) build_outputs: worktree::build_output::BuildOutputs,
    /// Builds agents start, taking turns.
    pub(super) builds: builds::Builds,
    pub(super) session_watch: session_watch::SessionWatch,
    pub(super) activity_log: Arc<activity_log::ActivityLog>,
    pub(super) device_view: device_view::DeviceViews,
    pub(super) host_watch: host_stats::HostWatch,
    pub(super) names: names::Names,
    pub(super) boards: board::Boards,
    pub(super) goal_summaries: goal_summary::Summaries,
    pub(super) goals: goals::Goals,
    pub(super) repo_events: repo_events::RepoEvents,
    pub(super) memory: memory::Memory,
    pub(super) triggers: triggers::Triggers,
    pub(super) landing: landing::Histories,
    pub(super) land_queue: edits::queue::LandQueue,
    pub(super) pictures: pictures::Pictures,
    pub(super) spares: spare::Spares,
    pub(super) updates: updates::Updates,
    pub(super) remote_images: remote_image::RemoteImages,
    pub(super) tasks: tasks::Tasks,
    pub(super) git: git::Projects,
    /// The code-host lists clients look at, kept and read again while they do.
    pub(super) code_host_lists: codehost::Lists,
    /// The files clients have open, watched wherever they are.
    pub(super) open_files: open_files::OpenFiles,
    pub(super) layouts: layout::Layouts,
    /// Who this host is signed in to Sikemux as.
    pub(super) account: account::Account,
    /// The account's vault of sign-ins, as this host holds it.
    pub(super) vault: vault::Vault,
    pub(super) ci: codehost::ci::Ci,
    pub(super) edits: edits::Edits,
    pub(super) collisions: collisions::Collisions,
    pub(super) checks: checks::ProjectChecks,
    pub(super) notify: notify::Notifier,
    pub(super) inbox: inbox::Turns,
    pub(super) joins: join::Joins,
    /// Hosts linked with this one, and the agents of teams shared with them.
    pub(super) hosts: hosts::Hosts,
    /// The faces agents wear, and the style the person picked for them.
    pub(super) faces: faces::Faces,
    /// What the person has typed and not sent yet, by what it is for.
    pub(super) drafts: drafts::Drafts,
    /// Where the person moved each diagram's tables, by what it is a diagram of.
    pub(super) diagrams: diagrams::Diagrams,
    pub(super) mods: mods::Mods,
    pub(super) mcp_apps: mcp_apps::McpApps,
    /// The repositories coming down into new projects.
    pub(super) clones: new_project::Clones,
    pub(super) chiefs: chief::Chiefs,
}

pub(crate) trait Parts {
    fn sessions(&self) -> &Mutex<HashMap<SessionId, Arc<Session>>>;
    fn next_session_id(&self) -> &AtomicU64;
    fn capacity(&self) -> &Arc<Capacity>;
    fn connections(&self) -> &Mutex<HashMap<ClientId, Arc<ClientConn>>>;
    fn next_client_id(&self) -> &AtomicU64;
    fn detection(&self) -> &RwLock<ManifestRegistry>;
    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>>;
    fn window(&self) -> &window::Window;
    fn model_proxy(&self) -> &model_proxy::ModelProxy;
    fn waits(&self) -> &waits::Waits;
    fn changes(&self) -> &changed::Changes;
    fn ports(&self) -> &ports::Ports;
    fn usage_wake(&self) -> &Notify;
    fn rail_numbers(&self) -> &Mutex<BTreeSet<u32>>;
    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>>;
    fn preferences(&self) -> &preferences::Prefs;
    fn streams(&self) -> &streams::Streams;
    fn look(&self) -> &themes::Look;
    fn harness(&self) -> &harness::Harness;
    fn updating(&self) -> &upgrade::Updating;
    fn tools(&self) -> &Mutex<Option<tools::ToolEndpoint>>;
    fn chats(&self) -> &chat::Chats;
    fn remote(&self) -> &remote::Remote;
    fn outside(&self) -> &outside::Outside;
    fn shares(&self) -> &share::Shares;
    fn hooks(&self) -> &hooks::Hooks;
    fn workspaces(&self) -> &workspace::Workspaces;
    fn seen(&self) -> &seen::Seen;
    fn settling(&self) -> &settling::Settling;
    fn stalls(&self) -> &stalls::Stalls;
    fn lifecycle(&self) -> &lifecycle::Lifecycle;
    fn ship(&self) -> &ship::Ships;
    fn person_calls(&self) -> &person_calls::PersonCalls;
    fn places(&self) -> &places::store::Store;
    fn pins(&self) -> &pins::Pins;
    fn build_outputs(&self) -> &worktree::build_output::BuildOutputs;
    fn builds(&self) -> &builds::Builds;
    fn session_watch(&self) -> &session_watch::SessionWatch;
    fn activity_log(&self) -> &Arc<activity_log::ActivityLog>;
    fn device_view(&self) -> &device_view::DeviceViews;
    fn host_watch(&self) -> &host_stats::HostWatch;
    fn names(&self) -> &names::Names;
    fn boards(&self) -> &board::Boards;
    fn goal_summaries(&self) -> &goal_summary::Summaries;
    fn goals(&self) -> &goals::Goals;
    fn repo_events(&self) -> &repo_events::RepoEvents;
    fn memory(&self) -> &memory::Memory;
    fn triggers(&self) -> &triggers::Triggers;
    fn landing(&self) -> &landing::Histories;
    fn land_queue(&self) -> &edits::queue::LandQueue;
    fn pictures(&self) -> &pictures::Pictures;
    fn spares(&self) -> &spare::Spares;
    fn updates(&self) -> &updates::Updates;
    fn remote_images(&self) -> &remote_image::RemoteImages;
    fn tasks(&self) -> &tasks::Tasks;
    fn git(&self) -> &git::Projects;
    fn code_host_lists(&self) -> &codehost::Lists;
    fn open_files(&self) -> &open_files::OpenFiles;
    fn layouts(&self) -> &layout::Layouts;
    fn account(&self) -> &account::Account;
    fn vault(&self) -> &vault::Vault;
    fn ci(&self) -> &codehost::ci::Ci;
    fn edits(&self) -> &edits::Edits;
    fn collisions(&self) -> &collisions::Collisions;
    fn checks(&self) -> &checks::ProjectChecks;
    fn notify(&self) -> &notify::Notifier;
    fn inbox(&self) -> &inbox::Turns;
    fn joins(&self) -> &join::Joins;
    fn hosts(&self) -> &hosts::Hosts;
    fn faces(&self) -> &faces::Faces;
    fn drafts(&self) -> &drafts::Drafts;
    fn diagrams(&self) -> &diagrams::Diagrams;
    fn mods(&self) -> &mods::Mods;
    fn mcp_apps(&self) -> &mcp_apps::McpApps;
    fn clones(&self) -> &new_project::Clones;
    fn chiefs(&self) -> &chief::Chiefs;
}

impl Parts for Core {
    fn sessions(&self) -> &Mutex<HashMap<SessionId, Arc<Session>>> {
        &self.parts.sessions
    }

    fn next_session_id(&self) -> &AtomicU64 {
        &self.parts.next_session_id
    }

    fn capacity(&self) -> &Arc<Capacity> {
        &self.parts.capacity
    }

    fn connections(&self) -> &Mutex<HashMap<ClientId, Arc<ClientConn>>> {
        &self.parts.connections
    }

    fn next_client_id(&self) -> &AtomicU64 {
        &self.parts.next_client_id
    }

    fn detection(&self) -> &RwLock<ManifestRegistry> {
        &self.parts.detection
    }

    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>> {
        &self.parts.manifest_folder
    }

    fn window(&self) -> &window::Window {
        &self.parts.window
    }

    fn model_proxy(&self) -> &model_proxy::ModelProxy {
        &self.parts.model_proxy
    }

    fn waits(&self) -> &waits::Waits {
        &self.parts.waits
    }

    fn changes(&self) -> &changed::Changes {
        &self.parts.changes
    }

    fn ports(&self) -> &ports::Ports {
        &self.parts.ports
    }

    fn usage_wake(&self) -> &Notify {
        &self.parts.usage_wake
    }

    fn rail_numbers(&self) -> &Mutex<BTreeSet<u32>> {
        &self.parts.rail_numbers
    }

    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>> {
        &self.parts.ssh_watch
    }

    fn preferences(&self) -> &preferences::Prefs {
        &self.parts.preferences
    }

    fn streams(&self) -> &streams::Streams {
        &self.parts.streams
    }

    fn look(&self) -> &themes::Look {
        &self.parts.look
    }

    fn harness(&self) -> &harness::Harness {
        &self.parts.harness
    }

    fn updating(&self) -> &upgrade::Updating {
        &self.parts.updating
    }

    fn tools(&self) -> &Mutex<Option<tools::ToolEndpoint>> {
        &self.parts.tools
    }

    fn chats(&self) -> &chat::Chats {
        &self.parts.chats
    }

    fn remote(&self) -> &remote::Remote {
        &self.parts.remote
    }

    fn outside(&self) -> &outside::Outside {
        &self.parts.outside
    }

    fn shares(&self) -> &share::Shares {
        &self.parts.shares
    }

    fn hooks(&self) -> &hooks::Hooks {
        &self.parts.hooks
    }

    fn workspaces(&self) -> &workspace::Workspaces {
        &self.parts.workspaces
    }

    fn seen(&self) -> &seen::Seen {
        &self.parts.seen
    }

    fn settling(&self) -> &settling::Settling {
        &self.parts.settling
    }

    fn stalls(&self) -> &stalls::Stalls {
        &self.parts.stalls
    }

    fn lifecycle(&self) -> &lifecycle::Lifecycle {
        &self.parts.lifecycle
    }

    fn ship(&self) -> &ship::Ships {
        &self.parts.ship
    }

    fn person_calls(&self) -> &person_calls::PersonCalls {
        &self.parts.person_calls
    }

    fn places(&self) -> &places::store::Store {
        &self.parts.places
    }

    fn pins(&self) -> &pins::Pins {
        &self.parts.pins
    }

    fn build_outputs(&self) -> &worktree::build_output::BuildOutputs {
        &self.parts.build_outputs
    }

    fn builds(&self) -> &builds::Builds {
        &self.parts.builds
    }

    fn session_watch(&self) -> &session_watch::SessionWatch {
        &self.parts.session_watch
    }

    fn activity_log(&self) -> &Arc<activity_log::ActivityLog> {
        &self.parts.activity_log
    }

    fn device_view(&self) -> &device_view::DeviceViews {
        &self.parts.device_view
    }

    fn host_watch(&self) -> &host_stats::HostWatch {
        &self.parts.host_watch
    }

    fn names(&self) -> &names::Names {
        &self.parts.names
    }

    fn boards(&self) -> &board::Boards {
        &self.parts.boards
    }

    fn goal_summaries(&self) -> &goal_summary::Summaries {
        &self.parts.goal_summaries
    }

    fn goals(&self) -> &goals::Goals {
        &self.parts.goals
    }

    fn repo_events(&self) -> &repo_events::RepoEvents {
        &self.parts.repo_events
    }

    fn memory(&self) -> &memory::Memory {
        &self.parts.memory
    }

    fn triggers(&self) -> &triggers::Triggers {
        &self.parts.triggers
    }

    fn landing(&self) -> &landing::Histories {
        &self.parts.landing
    }

    fn land_queue(&self) -> &edits::queue::LandQueue {
        &self.parts.land_queue
    }

    fn pictures(&self) -> &pictures::Pictures {
        &self.parts.pictures
    }

    fn spares(&self) -> &spare::Spares {
        &self.parts.spares
    }

    fn updates(&self) -> &updates::Updates {
        &self.parts.updates
    }

    fn remote_images(&self) -> &remote_image::RemoteImages {
        &self.parts.remote_images
    }

    fn tasks(&self) -> &tasks::Tasks {
        &self.parts.tasks
    }

    fn git(&self) -> &git::Projects {
        &self.parts.git
    }

    fn code_host_lists(&self) -> &codehost::Lists {
        &self.parts.code_host_lists
    }

    fn open_files(&self) -> &open_files::OpenFiles {
        &self.parts.open_files
    }

    fn layouts(&self) -> &layout::Layouts {
        &self.parts.layouts
    }

    fn account(&self) -> &account::Account {
        &self.parts.account
    }

    fn vault(&self) -> &vault::Vault {
        &self.parts.vault
    }

    fn ci(&self) -> &codehost::ci::Ci {
        &self.parts.ci
    }

    fn edits(&self) -> &edits::Edits {
        &self.parts.edits
    }

    fn collisions(&self) -> &collisions::Collisions {
        &self.parts.collisions
    }

    fn checks(&self) -> &checks::ProjectChecks {
        &self.parts.checks
    }

    fn notify(&self) -> &notify::Notifier {
        &self.parts.notify
    }

    fn inbox(&self) -> &inbox::Turns {
        &self.parts.inbox
    }

    fn joins(&self) -> &join::Joins {
        &self.parts.joins
    }

    fn hosts(&self) -> &hosts::Hosts {
        &self.parts.hosts
    }

    fn faces(&self) -> &faces::Faces {
        &self.parts.faces
    }

    fn drafts(&self) -> &drafts::Drafts {
        &self.parts.drafts
    }

    fn diagrams(&self) -> &diagrams::Diagrams {
        &self.parts.diagrams
    }

    fn mods(&self) -> &mods::Mods {
        &self.parts.mods
    }

    fn mcp_apps(&self) -> &mcp_apps::McpApps {
        &self.parts.mcp_apps
    }

    fn clones(&self) -> &new_project::Clones {
        &self.parts.clones
    }

    fn chiefs(&self) -> &chief::Chiefs {
        &self.parts.chiefs
    }
}
