//! What each feature of the core keeps for as long as the core runs, and how the rest reads it:
//! `core.boards()`, never a field.

use super::connection::ClientId as GoneClient;
use super::part::Part;
use crate::server::chat_state::ChatParts;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::task::JoinHandle;

use tokio::sync::Notify;

use super::{streams, themes, ServerConfig};

use super::{
    board, chief, device_view, git, goal_summary, goals, harness, hooks, host_stats, hosts, inbox,
    layout, lifecycle, line, memory, mods, outside, pictures, pins, remote_image, session_watch,
    settling, spare, stalls, tasks, tools, triggers, updates, Core,
};

pub(crate) struct Fields {
    pub(super) config: ServerConfig,
    pub(super) manifest_folder: Mutex<Option<PathBuf>>,
    /// Rings when a window opens, so the day's usage counts it at once.
    pub(super) usage_wake: Notify,
    /// Watches the SSH config once a client has listed its hosts.
    pub(super) ssh_watch: OnceLock<Option<::notify::RecommendedWatcher>>,
    /// Answers in parts on the connections that asked for them.
    pub(super) streams: streams::Streams,
    pub(super) look: themes::Look,
    pub(super) harness: harness::Harness,
    pub(super) tools: Mutex<Option<tools::ToolEndpoint>>,
    /// Programs Sikemux did not start that the person lets drive its chats.
    pub(super) outside: outside::Outside,
    pub(super) hooks: hooks::Hooks,
    pub(super) settling: settling::Settling,
    pub(super) stalls: stalls::Stalls,
    pub(super) lifecycle: lifecycle::Lifecycle,
    pub(super) pins: pins::Pins,
    /// Builds agents start, taking turns.
    pub(super) line: line::Line,
    pub(super) session_watch: session_watch::SessionWatch,
    pub(super) device_view: device_view::DeviceViews,
    pub(super) host_watch: host_stats::HostWatch,
    pub(super) boards: board::Boards,
    pub(super) goal_summaries: goal_summary::Summaries,
    pub(super) goals: goals::Goals,
    pub(super) memory: memory::Memory,
    pub(super) triggers: triggers::Triggers,
    pub(super) pictures: pictures::Pictures,
    pub(super) spares: spare::Spares,
    pub(super) updates: updates::Updates,
    pub(super) remote_images: remote_image::RemoteImages,
    pub(super) tasks: tasks::Tasks,
    pub(super) git: git::Projects,
    pub(super) layouts: layout::Layouts,
    pub(super) inbox: inbox::Turns,
    /// Hosts linked with this one, and the agents of teams shared with them.
    pub(super) hosts: hosts::Hosts,
    pub(super) mods: mods::Mods,
    pub(super) chiefs: chief::Chiefs,
}

pub(crate) trait Parts {
    fn config(&self) -> &ServerConfig;
    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>>;
    fn usage_wake(&self) -> &Notify;
    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>>;
    fn streams(&self) -> &streams::Streams;
    fn look(&self) -> &themes::Look;
    fn harness(&self) -> &harness::Harness;
    fn tools(&self) -> &Mutex<Option<tools::ToolEndpoint>>;
    fn outside(&self) -> &outside::Outside;
    fn hooks(&self) -> &hooks::Hooks;
    fn settling(&self) -> &settling::Settling;
    fn stalls(&self) -> &stalls::Stalls;
    fn lifecycle(&self) -> &lifecycle::Lifecycle;
    fn pins(&self) -> &pins::Pins;
    fn line(&self) -> &line::Line;
    fn session_watch(&self) -> &session_watch::SessionWatch;
    fn device_view(&self) -> &device_view::DeviceViews;
    fn host_watch(&self) -> &host_stats::HostWatch;
    fn boards(&self) -> &board::Boards;
    fn goal_summaries(&self) -> &goal_summary::Summaries;
    fn goals(&self) -> &goals::Goals;
    fn memory(&self) -> &memory::Memory;
    fn triggers(&self) -> &triggers::Triggers;
    fn pictures(&self) -> &pictures::Pictures;
    fn spares(&self) -> &spare::Spares;
    fn updates(&self) -> &updates::Updates;
    fn remote_images(&self) -> &remote_image::RemoteImages;
    fn tasks(&self) -> &tasks::Tasks;
    fn git(&self) -> &git::Projects;
    fn layouts(&self) -> &layout::Layouts;
    fn inbox(&self) -> &inbox::Turns;
    fn hosts(&self) -> &hosts::Hosts;
    fn mods(&self) -> &mods::Mods;
    fn chiefs(&self) -> &chief::Chiefs;
}

impl Parts for Core {
    fn config(&self) -> &ServerConfig {
        &self.kept::<Fields>().config
    }

    fn manifest_folder(&self) -> &Mutex<Option<PathBuf>> {
        &self.kept::<Fields>().manifest_folder
    }

    fn usage_wake(&self) -> &Notify {
        &self.kept::<Fields>().usage_wake
    }

    fn ssh_watch(&self) -> &OnceLock<Option<::notify::RecommendedWatcher>> {
        &self.kept::<Fields>().ssh_watch
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

    fn outside(&self) -> &outside::Outside {
        &self.kept::<Fields>().outside
    }

    fn hooks(&self) -> &hooks::Hooks {
        &self.kept::<Fields>().hooks
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

    fn pins(&self) -> &pins::Pins {
        &self.kept::<Fields>().pins
    }

    fn line(&self) -> &line::Line {
        &self.kept::<Fields>().line
    }

    fn session_watch(&self) -> &session_watch::SessionWatch {
        &self.kept::<Fields>().session_watch
    }

    fn device_view(&self) -> &device_view::DeviceViews {
        &self.kept::<Fields>().device_view
    }

    fn host_watch(&self) -> &host_stats::HostWatch {
        &self.kept::<Fields>().host_watch
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

    fn memory(&self) -> &memory::Memory {
        &self.kept::<Fields>().memory
    }

    fn triggers(&self) -> &triggers::Triggers {
        &self.kept::<Fields>().triggers
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

    fn layouts(&self) -> &layout::Layouts {
        &self.kept::<Fields>().layouts
    }

    fn inbox(&self) -> &inbox::Turns {
        &self.kept::<Fields>().inbox
    }

    fn hosts(&self) -> &hosts::Hosts {
        &self.kept::<Fields>().hosts
    }

    fn mods(&self) -> &mods::Mods {
        &self.kept::<Fields>().mods
    }

    fn chiefs(&self) -> &chief::Chiefs {
        &self.kept::<Fields>().chiefs
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
