//! What each device is shown of this host, built from the core's state and
//! sent again whenever anything in it changes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::connection::ClientConn;
use super::{access, display, inbox, layout, load, updates, workspace, Core};
use crate::protocol::{encode_control, AgentGroup, DeviceView, Event, ServerMessage};

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
    stale: Notify,
    /// Sends it at once, without waiting for more changes to go with it.
    due: Notify,
    /// The view each client was last sent: by the device with what it has in front of its
    /// own, or under none for every other client.
    sent: Mutex<HashMap<Option<String>, Vec<u8>>>,
    /// What the rows showed of this host's load in the view that last went out.
    shown_load: Mutex<Option<(u32, Option<u64>)>>,
}

impl Core {
    /// Something may watch the device view now, so it wakes to send it.
    pub(crate) fn view_watched(&self) {
        self.device_view.watched.notify_one();
    }

    /// Something a device's view of this host shows changed; the view goes
    /// out again shortly, once with any changes that follow it.
    pub(crate) fn view_changed(&self) {
        self.device_view.stale.notify_one();
    }

    /// Something the person did changed the view, such as the tab in front, so it goes out
    /// at once: a window shows its tabs as the view says.
    pub(crate) fn view_changed_now(&self) {
        self.device_view.due.notify_one();
        self.view_changed();
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
    /// Every session, with what the app calls its agent and whether the
    /// person has seen what the agent last did.
    pub(crate) fn session_infos(&self) -> Vec<crate::protocol::SessionInfo> {
        let mut sessions: Vec<_> = self
            .all_sessions()
            .iter()
            .map(|s| s.info())
            .filter(|info| !self.spares.hides(info.pane_id.as_deref()))
            .collect();
        sessions.sort_by_key(|info| info.id);
        self.workspaces.place(&mut sessions);
        for info in &mut sessions {
            if let Some(agent_id) = &info.agent_id {
                info.agent_name = self.names.name(agent_id);
                info.title = self.workspaces.title(agent_id);
                info.unread = self.seen.unread(agent_id);
                info.active_at = self.seen.active_at(agent_id);
                info.next_trigger = self.triggers.next_at(agent_id);
                info.called_at = self.seen.called_at(agent_id);
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
            chat.next_trigger = self.triggers.next_at(&chat.agent_id);
            chat.called_at = self.seen.called_at(&chat.agent_id);
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

    /// The face each agent wears, by agent id; one showing its logo has none.
    pub(crate) fn agent_faces(&self) -> HashMap<String, crate::protocol::AgentFace> {
        let mut agents = self.workspaces.view().agents;
        self.names.fill(&mut agents);
        self.faces.fill(&mut agents);
        agents
            .into_iter()
            .filter_map(|agent| Some((agent.id, agent.face?)))
            .collect()
    }

    /// The workspace as clients are sent it, each agent under its name and
    /// in its face.
    pub(crate) fn workspace_view(&self) -> crate::protocol::Workspace {
        let preferences = self.preferences.get();
        let mut workspace = self.workspaces.view();
        workspace.palette = self.look.tokens_by_name();
        self.names.fill(&mut workspace.agents);
        self.faces.fill(&mut workspace.agents);
        let accounts = self.workspaces.accounts();
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
            (project.code_host, project.git) = self.git.shown(&folder);
        }
        workspace
    }

    /// The view `device` is sent, or every client without a front of its own, and whether
    /// anything in it moves by itself.
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
        let mut attentions = self.chats.attentions();
        attentions.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        let recent = self.workspaces.recent(&chats);
        let inbox = inbox::view(self, &chats, &sessions);
        let workspace = self.workspace_view();
        let layout = Box::new(layout::view(self, &workspace, &sessions, device));
        let view = DeviceView {
            workspace,
            sessions,
            chats,
            attentions,
            recent,
            inbox,
            layout,
            account: self.account.shown().map(Box::new),
            face_style: Box::new(self.faces.style()),
            load: self.shows_load().then(|| {
                let load = load::now();
                *self.device_view.shown_load.locked() = Some(load::shown(&load));
                load
            }),
            updates: updates::host_view(self),
        };
        let frame = encode_control(&ServerMessage::Event {
            event: Event::DeviceView {
                view: Box::new(view),
            },
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

    /// Whether a client that shows this host's load watches the view: the app on this host, or
    /// another host's app joined as a device. Phones show no load.
    pub(crate) fn shows_load(&self) -> bool {
        self.view_watchers()
            .iter()
            .any(|client| match &client.peer {
                access::Peer::Device { id } => !self
                    .remote
                    .device_platform(id)
                    .is_some_and(|platform| matches!(platform.as_str(), "ios" | "android")),
                peer => !matches!(peer, access::Peer::Share { .. }),
            })
    }

    /// Whether what the rows show of `load` moved since the view last went out.
    pub(crate) fn load_moved(&self, load: &crate::protocol::HostLoad) -> bool {
        *self.device_view.shown_load.locked() != Some(load::shown(load))
    }

    /// Whether anything in the view moves by itself.
    fn publish_device_view(&self) -> bool {
        let devices = self.view_watchers();
        if devices.is_empty() {
            return false;
        }
        let fronted = layout::fronted(self);
        let mut last = self.device_view.sent.locked();
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

    /// A device that just connected starts from the whole view.
    pub(crate) fn send_device_view(&self, client: &ClientConn) {
        let _last = self.device_view.sent.locked();
        if let Some((frame, _)) = self.device_view_frame(client.peer.device_id().as_deref()) {
            client.send(frame.into());
        }
    }
}

/// Whether the device view shows what `event` tells of. What it shows that
/// no event tells of reaches it through [`Core::view_changed`].
pub(super) fn shows(event: &Event) -> bool {
    match event {
        Event::Exited { .. }
        | Event::SshLink { .. }
        | Event::AgentState(_)
        | Event::Attention { .. }
        | Event::AttentionCleared { .. }
        | Event::ChatBegun { .. }
        | Event::TerminalTitle { .. } => true,
        Event::ShellMetadata(_)
        | Event::TaskOutput { .. }
        | Event::Chat { .. }
        | Event::DeviceView { .. }
        | Event::Remote { .. }
        | Event::Network { .. }
        | Event::FocusAgent { .. }
        | Event::Notice { .. }
        | Event::AccountDevice { .. }
        | Event::Changed { .. }
        | Event::Stream { .. } => false,
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
            core.device_view.watched.notified().await;
            ticker.reset();
        }
        let changed = if moving {
            tokio::select! {
                _ = ticker.tick() => false,
                () = core.device_view.stale.notified() => true,
            }
        } else {
            core.device_view.stale.notified().await;
            true
        };
        if changed {
            let now = tokio::time::Instant::now();
            let gathered = tokio::time::sleep_until(
                (now + DEVICE_VIEW_GATHER).max(built + DEVICE_VIEW_SPACING),
            );
            tokio::select! {
                () = gathered => {}
                () = core.device_view.due.notified() => {}
            }
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
