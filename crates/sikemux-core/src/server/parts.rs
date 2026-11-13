//! What each feature of the core keeps for as long as the core runs, and how the rest reads it:
//! `core.boards()`, never a field.

use super::connection::ClientId as GoneClient;
use super::part::Part;
use crate::server::serving::ServeParts;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::task::JoinHandle;

use tokio::sync::Notify;

use super::{model_proxy, ports, preferences, streams, themes, ServerConfig};

use super::{
    account, activity_log, board, chat, checks, chief, codehost, collisions, device_view, diagrams,
    drafts, edits, faces, git, goal_summary, goals, harness, hooks, host_stats, hosts, inbox, join,
    landing, layout, lifecycle, line, mcp_apps, memory, mods, names, new_project, notify,
    open_files, outside, person_calls, pictures, pins, places, remote, remote_image, repo_events,
    seen, session_watch, settling, share, ship, spare, stalls, tasks, tools, triggers, updates,
    vault, workspace, worktree, Core,
};

pub(crate) struct Fields {
    pub(super) config: ServerConfig,
    pub(super) manifest_folder: Mutex<Option<PathBuf>>,
    pub(super) model_proxy: model_proxy::ModelProxy,
    pub(super) ports: ports::Ports,
    /// Rings when a window opens, so the day's usage counts it at once.
    pub(super) usage_wake: Notify,
    /// Watches the SSH config once a client has listed its hosts.
    pub(super) ssh_watch: OnceLock<Option<::notify::RecommendedWatcher>>,
    pub(super) preferences: preferences::Prefs,
    /// Answers in parts on the connections that asked for them.
    pub(super) streams: streams::Streams,
    pub(super) look: themes::Look,
    pub(super) harness: harness::Harness,
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
    pub(super) line: line::Line,
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
    pub(super) workspace_above: super::workspace_above::WorkspaceAbove,
}

pub(crate) trait Parts {
    fn config(&self) -> &ServerConfig;
    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>>;
    fn model_proxy(&self) -> &model_proxy::ModelProxy;
    fn ports(&self) -> &ports::Ports;
    fn usage_wake(&self) -> &Notify;
    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>>;
    fn preferences(&self) -> &preferences::Prefs;
    fn streams(&self) -> &streams::Streams;
    fn look(&self) -> &themes::Look;
    fn harness(&self) -> &harness::Harness;
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
    fn line(&self) -> &line::Line;
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
    fn workspace_above(&self) -> &super::workspace_above::WorkspaceAbove;
}

impl Parts for Core {
    fn config(&self) -> &ServerConfig {
        &self.kept::<Fields>().config
    }

    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>> {
        &self.kept::<Fields>().manifest_folder
    }

    fn model_proxy(&self) -> &model_proxy::ModelProxy {
        &self.kept::<Fields>().model_proxy
    }

    fn ports(&self) -> &ports::Ports {
        &self.kept::<Fields>().ports
    }

    fn usage_wake(&self) -> &Notify {
        &self.kept::<Fields>().usage_wake
    }

    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>> {
        &self.kept::<Fields>().ssh_watch
    }

    fn preferences(&self) -> &preferences::Prefs {
        &self.kept::<Fields>().preferences
    }

    fn streams(&self) -> &streams::Streams {
        &self.kept::<Fields>().streams
    }

    fn look(&self) -> &themes::Look {
        &self.kept::<Fields>().look
    }

    fn harness(&self) -> &harness::Harness {
        &self.kept::<Fields>().harness
    }

    fn tools(&self) -> &Mutex<Option<tools::ToolEndpoint>> {
        &self.kept::<Fields>().tools
    }

    fn chats(&self) -> &chat::Chats {
        &self.kept::<Fields>().chats
    }

    fn remote(&self) -> &remote::Remote {
        &self.kept::<Fields>().remote
    }

    fn outside(&self) -> &outside::Outside {
        &self.kept::<Fields>().outside
    }

    fn shares(&self) -> &share::Shares {
        &self.kept::<Fields>().shares
    }

    fn hooks(&self) -> &hooks::Hooks {
        &self.kept::<Fields>().hooks
    }

    fn workspaces(&self) -> &workspace::Workspaces {
        &self.kept::<Fields>().workspaces
    }

    fn seen(&self) -> &seen::Seen {
        &self.kept::<Fields>().seen
    }

    fn settling(&self) -> &settling::Settling {
        &self.kept::<Fields>().settling
    }

    fn stalls(&self) -> &stalls::Stalls {
        &self.kept::<Fields>().stalls
    }

    fn lifecycle(&self) -> &lifecycle::Lifecycle {
        &self.kept::<Fields>().lifecycle
    }

    fn ship(&self) -> &ship::Ships {
        &self.kept::<Fields>().ship
    }

    fn person_calls(&self) -> &person_calls::PersonCalls {
        &self.kept::<Fields>().person_calls
    }

    fn places(&self) -> &places::store::Store {
        &self.kept::<Fields>().places
    }

    fn pins(&self) -> &pins::Pins {
        &self.kept::<Fields>().pins
    }

    fn build_outputs(&self) -> &worktree::build_output::BuildOutputs {
        &self.kept::<Fields>().build_outputs
    }

    fn line(&self) -> &line::Line {
        &self.kept::<Fields>().line
    }

    fn session_watch(&self) -> &session_watch::SessionWatch {
        &self.kept::<Fields>().session_watch
    }

    fn activity_log(&self) -> &Arc<activity_log::ActivityLog> {
        &self.kept::<Fields>().activity_log
    }

    fn device_view(&self) -> &device_view::DeviceViews {
        &self.kept::<Fields>().device_view
    }

    fn host_watch(&self) -> &host_stats::HostWatch {
        &self.kept::<Fields>().host_watch
    }

    fn names(&self) -> &names::Names {
        &self.kept::<Fields>().names
    }

    fn boards(&self) -> &board::Boards {
        &self.kept::<Fields>().boards
    }

    fn goal_summaries(&self) -> &goal_summary::Summaries {
        &self.kept::<Fields>().goal_summaries
    }

    fn goals(&self) -> &goals::Goals {
        &self.kept::<Fields>().goals
    }

    fn repo_events(&self) -> &repo_events::RepoEvents {
        &self.kept::<Fields>().repo_events
    }

    fn memory(&self) -> &memory::Memory {
        &self.kept::<Fields>().memory
    }

    fn triggers(&self) -> &triggers::Triggers {
        &self.kept::<Fields>().triggers
    }

    fn landing(&self) -> &landing::Histories {
        &self.kept::<Fields>().landing
    }

    fn land_queue(&self) -> &edits::queue::LandQueue {
        &self.kept::<Fields>().land_queue
    }

    fn pictures(&self) -> &pictures::Pictures {
        &self.kept::<Fields>().pictures
    }

    fn spares(&self) -> &spare::Spares {
        &self.kept::<Fields>().spares
    }

    fn updates(&self) -> &updates::Updates {
        &self.kept::<Fields>().updates
    }

    fn remote_images(&self) -> &remote_image::RemoteImages {
        &self.kept::<Fields>().remote_images
    }

    fn tasks(&self) -> &tasks::Tasks {
        &self.kept::<Fields>().tasks
    }

    fn git(&self) -> &git::Projects {
        &self.kept::<Fields>().git
    }

    fn code_host_lists(&self) -> &codehost::Lists {
        &self.kept::<Fields>().code_host_lists
    }

    fn open_files(&self) -> &open_files::OpenFiles {
        &self.kept::<Fields>().open_files
    }

    fn layouts(&self) -> &layout::Layouts {
        &self.kept::<Fields>().layouts
    }

    fn account(&self) -> &account::Account {
        &self.kept::<Fields>().account
    }

    fn vault(&self) -> &vault::Vault {
        &self.kept::<Fields>().vault
    }

    fn ci(&self) -> &codehost::ci::Ci {
        &self.kept::<Fields>().ci
    }

    fn edits(&self) -> &edits::Edits {
        &self.kept::<Fields>().edits
    }

    fn collisions(&self) -> &collisions::Collisions {
        &self.kept::<Fields>().collisions
    }

    fn checks(&self) -> &checks::ProjectChecks {
        &self.kept::<Fields>().checks
    }

    fn notify(&self) -> &notify::Notifier {
        &self.kept::<Fields>().notify
    }

    fn inbox(&self) -> &inbox::Turns {
        &self.kept::<Fields>().inbox
    }

    fn joins(&self) -> &join::Joins {
        &self.kept::<Fields>().joins
    }

    fn hosts(&self) -> &hosts::Hosts {
        &self.kept::<Fields>().hosts
    }

    fn faces(&self) -> &faces::Faces {
        &self.kept::<Fields>().faces
    }

    fn drafts(&self) -> &drafts::Drafts {
        &self.kept::<Fields>().drafts
    }

    fn diagrams(&self) -> &diagrams::Diagrams {
        &self.kept::<Fields>().diagrams
    }

    fn mods(&self) -> &mods::Mods {
        &self.kept::<Fields>().mods
    }

    fn mcp_apps(&self) -> &mcp_apps::McpApps {
        &self.kept::<Fields>().mcp_apps
    }

    fn clones(&self) -> &new_project::Clones {
        &self.kept::<Fields>().clones
    }

    fn chiefs(&self) -> &chief::Chiefs {
        &self.kept::<Fields>().chiefs
    }

    fn workspace_above(&self) -> &super::workspace_above::WorkspaceAbove {
        &self.kept::<Fields>().workspace_above
    }
}

/// Walking every part of the core.
pub(crate) trait PartCore {
    fn parts(&self) -> [&dyn Part; 35];

    /// Every feature's tasks, to end when the core stops.
    fn start_parts(self: &Arc<Self>) -> Vec<JoinHandle<()>>;

    /// Waits until everything every feature changed is on disk.
    fn flush_parts(&self);

    fn tell_parts_client_gone(self: &Arc<Self>, client: GoneClient);
}

impl PartCore for Core {
    fn parts(&self) -> [&dyn Part; 35] {
        [
            self.harness(),
            self.outside(),
            self.person_calls(),
            self.layouts(),
            self.boards(),
            self.memory(),
            self.triggers(),
            self.collisions(),
            self.checks(),
            self.workspaces(),
            self.hosts(),
            self.ports(),
            self.spares(),
            self.device_view(),
            self.host_watch(),
            self.line(),
            self.lifecycle(),
            self.updates(),
            self.changes(),
            self.tasks(),
            self.notify(),
            self.window(),
            self.chats(),
            self.seen(),
            self.settling(),
            self.open_files(),
            self.mods(),
            self.diagrams(),
            self.preferences(),
            self.drafts(),
            self.code_host_lists(),
            self.look(),
            self.chiefs(),
            &super::lsp::Lsp,
            &super::fast_forward::FastForward,
        ]
    }

    fn start_parts(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        self.parts()
            .iter()
            .flat_map(|part| part.start(self))
            .collect()
    }

    fn flush_parts(&self) {
        for part in self.parts() {
            part.flush();
        }
    }

    fn tell_parts_client_gone(self: &Arc<Self>, client: GoneClient) {
        for part in self.parts() {
            part.client_gone(self, client);
        }
    }
}
