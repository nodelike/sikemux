//! What each device is shown of this host, built from the core's state and
//! sent again whenever anything in it changes.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::ServerCore;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::connection::ClientConn;
use super::{access, display, goals, host_stats, inbox, layout, updates, workspace, Core};
use sikemux_view::calls::{CallState, PersonCall};
use sikemux_view::device::DeviceView;
use sikemux_view::turn::{TurnPhase, TurnState};
use sikemux_view::workspace::{ProjectInfo, ProjectStanding};
use sikemux_wire::protocol::{encode_device_view, AgentGroup};

/// How often what a paired device shows of this host is looked at again
/// while something in it moves by itself, such as an agent mid-turn.
const DEVICE_VIEW_INTERVAL: Duration = Duration::from_millis(400);
/// A burst of changes to the view goes out as one.
const DEVICE_VIEW_GATHER: Duration = Duration::from_millis(50);
/// However much changes, the view is built at most this often.
const DEVICE_VIEW_SPACING: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(crate) struct DeviceViews {
    /// Pokes the device view awake once something may be watching it.
    watched: Notify,
    /// Sends the device view before its next tick, once an agent's state changed.
    pub(super) stale: Notify,
    /// Sends it at once, without waiting for more changes to go with it.
    pub(super) due: Notify,
    /// The view each client was last sent: by the device with what it has in front of its
    /// own, or under none for every other client.
    sent: Mutex<HashMap<Option<String>, Vec<u8>>>,
    /// What the rows showed of this host's load in the view that last went out.
    shown_load: Mutex<Option<sikemux_view::device::HostLoad>>,
}

/// Building and sending the view of this host that devices show.
pub(crate) trait DeviceViewCore {
    fn view_watched(&self);

    /// Tells [`Seen`] the agents each phone shows: the chats and terminals it
    /// has open while its app is in front.
    fn show_phones(&self);

    /// Every session, with what the app calls its agent and whether the
    /// person has seen what the agent last did.
    fn session_infos(&self) -> Vec<sikemux_wire::protocol::SessionInfo>;

    /// Whether what the agent is doing waits on the person, as Needs you lists it.
    fn needs_person(&self, agent_id: &str) -> bool;

    /// The chats under the app's names, most recently active first, with
    /// whether the person has seen what each last did.
    fn chat_infos(&self) -> Vec<sikemux_wire::protocol::ChatInfo>;

    /// The chats and sessions, each agent numbered in the order it wants the
    /// person.
    fn ranked_infos(
        &self,
    ) -> (
        Vec<sikemux_wire::protocol::ChatInfo>,
        Vec<sikemux_wire::protocol::SessionInfo>,
    );

    /// The workspace's agents as clients are sent them, each under its name and in its face.
    fn workspace_agents(&self) -> Vec<sikemux_view::workspace::WorkspaceAgent>;

    /// The workspace as a client asks for it, with how each project stands.
    fn workspace_view_standing(&self) -> sikemux_view::workspace::Workspace;

    /// The workspace as clients are sent it, each agent under its name and
    /// in its face. How each project stands is left out: it costs the turns of every agent.
    fn workspace_view(&self) -> sikemux_view::workspace::Workspace;

    /// The view `device` is sent, or every client without a front of its own, and whether
    /// anything in it moves by itself.
    fn device_view_frame(&self, device: Option<&str>) -> Option<(Vec<u8>, bool)>;

    /// The clients sent the device view: every paired device, and local clients that asked.
    fn view_watchers(&self) -> Vec<Arc<ClientConn>>;

    /// Whether a client that shows this host's load watches the view: the app on this host, or
    /// another host's app joined as a device. Phones show no load.
    fn shows_load(&self) -> bool;

    /// Whether what the rows show of `load` moved since the view last went out.
    fn load_moved(&self, load: &sikemux_view::device::HostLoad) -> bool;

    /// Sends every paired device what it shows of this host, when that changed since they were
    /// last sent it. Answers whether anything in the view moves by itself.
    fn publish_device_view(&self) -> bool;

    /// A device that just connected starts from the whole view.
    fn send_device_view(&self, client: &ClientConn);
}

impl DeviceViewCore for Core {
    fn view_watched(&self) {
        self.device_view().watched.notify_one();
    }

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
                shown.extend(self.chats().followed_by(phone.id));
                shown.extend(
                    agent_sessions
                        .iter()
                        .filter(|(id, _)| phone.is_subscribed(*id))
                        .map(|(_, agent_id)| agent_id.clone()),
                );
            }
            self.seen().show(phone.id, shown, true);
        }
    }

    fn session_infos(&self) -> Vec<sikemux_wire::protocol::SessionInfo> {
        let mut sessions: Vec<_> = self
            .all_sessions()
            .iter()
            .map(|s| s.info())
            .filter(|info| !self.spares().hides(info.pane_id.as_deref()))
            .collect();
        sessions.sort_by_key(|info| info.id);
        self.workspaces().place(&mut sessions);
        for info in &mut sessions {
            if let Some(agent_id) = &info.agent_id {
                info.agent_name = self.names().name(agent_id);
                info.title = self.workspaces().title(agent_id);
                info.unread = self.seen().unread(agent_id);
                info.active_at = self.seen().active_at(agent_id);
                info.next_trigger = self.triggers().next_at(agent_id);
                info.called_at = self.seen().called_at(agent_id);
            }
            let asking = info
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| self.needs_person(agent_id));
            display::show_session(info, asking);
        }
        sessions
    }

    fn needs_person(&self, agent_id: &str) -> bool {
        self.harness().questions.asks_person(agent_id)
            || self.ship().holds(agent_id)
            || self
                .chats()
                .heard(agent_id)
                .is_some_and(|feed| feed.has_guests())
    }

    fn chat_infos(&self) -> Vec<sikemux_wire::protocol::ChatInfo> {
        let mut chats = self.workspaces().listed(self.chats().list());
        for chat in &mut chats {
            chat.name = self.names().name(&chat.agent_id);
            chat.pending_permissions.sort();
            chat.unread = self.seen().unread(&chat.agent_id);
            chat.active_at = self.seen().active_at(&chat.agent_id);
            chat.next_trigger = self.triggers().next_at(&chat.agent_id);
            chat.called_at = self.seen().called_at(&chat.agent_id);
            let asking = self.needs_person(&chat.agent_id);
            display::show_chat(chat, asking);
        }
        chats.sort_by_key(|chat| std::cmp::Reverse(chat.active_at));
        chats
    }

    fn ranked_infos(
        &self,
    ) -> (
        Vec<sikemux_wire::protocol::ChatInfo>,
        Vec<sikemux_wire::protocol::SessionInfo>,
    ) {
        let mut sessions = self.session_infos();
        let mut chats = self.chat_infos();
        let said_at = self.workspaces().said_at();
        display::rank(
            &mut chats,
            &mut sessions,
            |agent_id| self.seen().started_at(agent_id),
            |agent_id| said_at.get(agent_id).copied(),
        );
        (chats, sessions)
    }

    fn workspace_agents(&self) -> Vec<sikemux_view::workspace::WorkspaceAgent> {
        let mut agents = self.workspaces().view().agents;
        self.names().fill(&mut agents);
        self.faces().fill(&mut agents);
        agents
    }

    fn workspace_view_standing(&self) -> sikemux_view::workspace::Workspace {
        let mut workspace = self.workspace_view();
        let turns = super::turn_state::all(self);
        let calls = super::person_calls::shown(self);
        stand_projects(self, &mut workspace.projects, &turns, &calls);
        workspace
    }

    fn workspace_view(&self) -> sikemux_view::workspace::Workspace {
        let preferences = self.preferences().get();
        let mut workspace = self.workspaces().view();
        workspace.palette = self.look().tokens_by_name();
        workspace.backdrop.look = Some(super::backdrop::look(
            &preferences,
            &self.look().palette().tokens,
        ));
        workspace.disabled_plugins = super::plugins::disabled_in(self.config.plugins);
        self.names().fill(&mut workspace.agents);
        self.faces().fill(&mut workspace.agents);
        let accounts = self.workspaces().accounts();
        let preferred = preferences.last_agent_type.as_deref().and_then(|kind| {
            let ids = workspace
                .launchers
                .iter()
                .map(|launcher| (launcher.id.as_str(), launcher.provider.as_str()));
            workspace::launcher_for(ids, accounts.as_ref(), kind)
        });
        for project in &mut workspace.projects {
            let folder = project.path.to_string_lossy();
            project.worktree_default =
                preferences.agent_worktree_defaults.get(&*folder) == Some(&true);
            let fresh = !workspace
                .agents
                .iter()
                .any(|agent| agent.project == project.id);
            if fresh && preferred.is_some() {
                project.launcher.clone_from(&preferred);
            }
            (project.code_host, project.git) = self.git().shown(&folder);
            project.cloning = self.clones().of(&project.id);
        }
        workspace::sort_projects(
            &mut workspace.projects,
            preferences.project_sort,
            &self.workspaces().last_active(),
        );
        self.hosts().copies.fold(self, &mut workspace);
        workspace
    }

    fn device_view_frame(&self, device: Option<&str>) -> Option<(Vec<u8>, bool)> {
        let (chats, sessions) = self.ranked_infos();
        let moving = chats.iter().any(|chat| {
            chat.group == AgentGroup::Working
                || chat.subagents > 0
                || chat.tasks > 0
                || chat.reconnecting.is_some()
        }) || sessions
            .iter()
            .any(|session| session.group == Some(AgentGroup::Working));
        let mut attentions = self.chats().attentions();
        attentions.extend(inbox::foreign_attentions(self));
        attentions.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        let recent = self.workspaces().recent(&chats, &self.triggers().held());
        let inbox = inbox::view(self, &chats, &sessions);
        let mut workers: HashMap<&String, Vec<&String>> = HashMap::new();
        let leads = self.workspaces().agent_leads();
        for (worker, lead) in &leads {
            workers.entry(lead).or_default().push(worker);
        }
        let turns: HashMap<String, TurnState> = (inbox.agents.iter())
            .filter_map(|agent| Some((agent.agent_id.clone(), agent.turn.clone()?)))
            .collect();
        let chiefs = super::chief::seated(self);
        for agent in &inbox.agents {
            if let Some(chat) = self.chats().get(&agent.agent_id) {
                chat.feed.set_turn(agent.turn.clone());
                let hears = (agent.project.as_ref())
                    .and_then(|project| chiefs.get(project))
                    .filter(|chief| **chief != agent.agent_id);
                chat.feed.set_chief_hears(hears.cloned());
                chat.feed.set_shown(
                    self.pictures().turns_of(&agent.agent_id),
                    (workers.get(&agent.agent_id))
                        .map(|workers| super::pictures::results(self, workers, &turns))
                        .unwrap_or_default(),
                );
            }
        }
        goals::view_moved(self, &inbox);
        let calls = super::person_calls::shown(self);
        super::chief::refresh_away(self, &calls);
        let mut workspace = self.workspace_view();
        stand_projects(self, &mut workspace.projects, &turns, &calls);
        let layout = Box::new(layout::view(self, &workspace, &sessions, device));
        let view = DeviceView {
            workspace,
            sessions,
            chats,
            attentions,
            recent,
            inbox,
            calls,
            pins: super::pins::shown(self),
            layout,
            account: self.account().shown().map(Box::new),
            face_style: Box::new(self.faces().style()),
            load: self.shows_load().then(|| {
                let load = host_stats::load(self);
                *self.device_view().shown_load.locked() = Some(host_stats::shown_load(&load));
                load
            }),
            alerts: host_stats::alerts(self),
            updates: updates::host_view(self),
        };
        let frame = encode_device_view(&view).ok()?;
        Some((frame, moving))
    }

    fn view_watchers(&self) -> Vec<Arc<ClientConn>> {
        self.clients()
            .into_iter()
            .filter(|client| !client.peer.is_local() || client.watches_view())
            .collect()
    }

    fn shows_load(&self) -> bool {
        self.view_watchers()
            .iter()
            .any(|client| match &client.peer {
                access::Peer::Device { id } => !self
                    .remote()
                    .device_platform(id)
                    .is_some_and(|platform| matches!(platform.as_str(), "ios" | "android")),
                peer => !matches!(peer, access::Peer::Share { .. }),
            })
    }

    fn load_moved(&self, load: &sikemux_view::device::HostLoad) -> bool {
        *self.device_view().shown_load.locked() != Some(host_stats::shown_load(load))
    }

    fn publish_device_view(&self) -> bool {
        let devices = self.view_watchers();
        if devices.is_empty() {
            return false;
        }
        let fronted = layout::fronted(self);
        let mut last = self.device_view().sent.locked();
        let mut frames: HashMap<Option<String>, Option<Arc<[u8]>>> = HashMap::new();
        let mut moving = false;
        for client in devices {
            let key = client.peer.device_id().filter(|id| fronted.contains(id));
            let frame = frames.entry(key.clone()).or_insert_with(|| {
                let (frame, moves) = self.device_view_frame(key.as_deref())?;
                moving |= moves;
                if last.get(&key) == Some(&frame) {
                    return None;
                }
                let shared: Arc<[u8]> = frame.as_slice().into();
                last.insert(key.clone(), frame);
                Some(shared)
            });
            if let Some(frame) = frame {
                client.send(frame.clone());
            }
        }
        last.retain(|key, _| frames.contains_key(key));
        moving
    }

    fn send_device_view(&self, client: &ClientConn) {
        let _last = self.device_view().sent.locked();
        if let Some((frame, _)) = self.device_view_frame(client.peer.device_id().as_deref()) {
            client.send(frame.into());
        }
    }
}

/// Sends devices the view when anything in it may have changed, and on a
/// tick only while something in it moves by itself.
pub(super) async fn serve(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(DEVICE_VIEW_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut window = None;
    let mut moving = true;
    let mut built = tokio::time::Instant::now();
    loop {
        if core.view_watchers().is_empty() {
            core.device_view().watched.notified().await;
            ticker.reset();
        }
        let changed = if moving {
            tokio::select! {
                _ = ticker.tick() => false,
                () = core.device_view().stale.notified() => true,
            }
        } else {
            core.device_view().stale.notified().await;
            true
        };
        if changed {
            let now = tokio::time::Instant::now();
            let gathered = tokio::time::sleep_until(
                (now + DEVICE_VIEW_GATHER).max(built + DEVICE_VIEW_SPACING),
            );
            tokio::select! {
                () = gathered => {}
                () = core.device_view().due.notified() => {}
            }
        }
        core.show_phones();
        let now = core.window().client_id();
        if now != window {
            window = now;
            core.changed(What::Agents);
        }
        moving = core.publish_device_view();
        built = tokio::time::Instant::now();
    }
}

impl super::part::Part for DeviceViews {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(serve(core.clone()))]
    }
}

/// How each project stands for the person, from its open calls and its agents' turns. An agent
/// the person snoozed does not make its project need them; a settled one that needs them does.
fn stand_projects(
    core: &Core,
    projects: &mut [ProjectInfo],
    turns: &HashMap<String, TurnState>,
    calls: &[PersonCall],
) {
    let now = sikemux_process::clock::epoch_ms();
    let places = core.workspaces().places();
    let chiefs = core.workspaces().chiefs();
    let snoozed = |agent_id: &str| {
        !chiefs.contains(agent_id)
            && (places.get(agent_id))
                .and_then(|(place, _)| super::settling::snoozed_until(place, now))
                .is_some()
    };
    let mut phases: HashMap<String, Vec<TurnPhase>> = HashMap::new();
    for (agent_id, project) in core.workspaces().agent_projects() {
        if let Some(turn) = turns.get(&agent_id) {
            let phase = match turn.state {
                TurnPhase::NeedsYou if snoozed(&agent_id) => continue,
                phase => phase,
            };
            phases.entry(project).or_default().push(phase);
        }
    }
    for project in projects {
        let path = project.path.to_string_lossy();
        project.calls = (calls.iter())
            .filter(|call| call.project == path && call.state == CallState::Open)
            .count() as u32;
        let phases = phases
            .get(&project.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        project.standing = if project.calls > 0 || phases.contains(&TurnPhase::NeedsYou) {
            ProjectStanding::NeedsYou
        } else if (phases.iter())
            .any(|phase| matches!(phase, TurnPhase::Working | TurnPhase::Waiting))
        {
            ProjectStanding::AtWork
        } else {
            ProjectStanding::AtRest
        };
    }
}
