//! The core's protocol types as the phone app sees them. Each is built from
//! the core's own type, so a change to the protocol fails to compile here
//! instead of failing on the phone.

use std::collections::HashMap;

use sikemux_core::protocol as core;

#[derive(uniffi::Record)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
}

impl From<core::ProjectInfo> for ProjectInfo {
    fn from(project: core::ProjectInfo) -> Self {
        Self {
            id: project.id,
            name: project.name,
            path: project.path.display().to_string(),
        }
    }
}

#[derive(uniffi::Record)]
pub struct LauncherInfo {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub permission_mode: String,
}

impl From<core::LauncherInfo> for LauncherInfo {
    fn from(launcher: core::LauncherInfo) -> Self {
        Self {
            id: launcher.id,
            provider: launcher.provider,
            label: launcher.label,
            permission_mode: launcher.permission_mode,
        }
    }
}

/// What the Mac draws behind its panes: the moving grain, or the picture
/// [`crate::Connection::save_backdrop`] fetches by `image`.
#[derive(uniffi::Record)]
pub struct Backdrop {
    pub texture: bool,
    pub image: Option<String>,
}

#[derive(uniffi::Record)]
pub struct Workspace {
    pub projects: Vec<ProjectInfo>,
    pub launchers: Vec<LauncherInfo>,
    /// The Mac's theme colours by name; empty until its app publishes them.
    pub palette: HashMap<String, String>,
    pub backdrop: Backdrop,
}

impl From<core::Workspace> for Workspace {
    fn from(workspace: core::Workspace) -> Self {
        Self {
            projects: workspace.projects.into_iter().map(Into::into).collect(),
            launchers: workspace.launchers.into_iter().map(Into::into).collect(),
            palette: workspace.palette.into_iter().collect(),
            backdrop: Backdrop {
                texture: workspace.backdrop.texture,
                image: workspace.backdrop.image,
            },
        }
    }
}

#[derive(uniffi::Enum)]
pub enum SessionKind {
    Terminal,
    Task,
}

#[derive(uniffi::Record)]
pub struct TaskInfo {
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub project: String,
}

#[derive(uniffi::Record)]
pub struct SessionExit {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

#[derive(uniffi::Record)]
pub struct SessionInfo {
    pub id: u64,
    pub kind: SessionKind,
    pub running: bool,
    pub cols: u16,
    pub rows: u16,
    pub project: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_state: Option<String>,
    pub task: Option<TaskInfo>,
    pub exit: Option<SessionExit>,
    pub killed: bool,
    pub started_by: Option<String>,
}

impl From<core::SessionInfo> for SessionInfo {
    fn from(session: core::SessionInfo) -> Self {
        Self {
            id: session.id,
            kind: match session.kind {
                core::SessionKind::Terminal => SessionKind::Terminal,
                core::SessionKind::Task => SessionKind::Task,
            },
            running: session.running,
            cols: session.cols,
            rows: session.rows,
            project: session.project,
            agent_id: session.agent_id,
            agent_type: session.agent_type,
            agent_state: session.agent_state,
            task: session.task.map(|task| TaskInfo {
                label: task.label,
                command: task.command,
                cwd: task.cwd,
                project: task.project,
            }),
            exit: session.exit.map(|exit| SessionExit {
                code: exit.code,
                signal: exit.signal,
            }),
            killed: session.killed,
            started_by: session.started_by,
        }
    }
}

#[derive(uniffi::Enum)]
pub enum ChatState {
    Starting,
    Ready,
    Stopped,
}

#[derive(uniffi::Record)]
pub struct ChatInfo {
    pub agent_id: String,
    pub provider: String,
    pub title: Option<String>,
    pub cwd: String,
    pub state: ChatState,
    pub running: bool,
    pub pending_permissions: Vec<String>,
    pub started_by: Option<String>,
    pub launcher: Option<String>,
    pub permission_mode: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub asleep: bool,
}

impl From<core::ChatInfo> for ChatInfo {
    fn from(chat: core::ChatInfo) -> Self {
        Self {
            agent_id: chat.agent_id,
            provider: chat.provider,
            title: chat.title,
            cwd: chat.cwd.display().to_string(),
            state: match chat.state {
                core::ChatState::Starting => ChatState::Starting,
                core::ChatState::Ready => ChatState::Ready,
                core::ChatState::Stopped => ChatState::Stopped,
            },
            running: chat.running,
            pending_permissions: chat.pending_permissions,
            started_by: chat.started_by,
            launcher: chat.launcher,
            permission_mode: chat.permission_mode,
            model: chat.model,
            effort: chat.effort,
            asleep: chat.asleep,
        }
    }
}

/// An agent waiting on a person to answer a permission request.
#[derive(uniffi::Record)]
pub struct Attention {
    pub id: String,
    pub agent_id: String,
    pub provider: String,
    pub cwd: String,
    /// The agent's own request, as JSON.
    pub request_json: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

impl From<core::Attention> for Attention {
    fn from(attention: core::Attention) -> Self {
        Self {
            id: attention.id,
            agent_id: attention.agent_id,
            provider: attention.provider,
            cwd: attention.cwd.display().to_string(),
            request_json: attention.request.to_string(),
            at: attention.at,
        }
    }
}

/// Everything the phone shows of one Mac.
#[derive(uniffi::Record)]
pub struct DeviceView {
    pub workspace: Workspace,
    pub sessions: Vec<SessionInfo>,
    pub chats: Vec<ChatInfo>,
    pub attentions: Vec<Attention>,
}

impl From<core::DeviceView> for DeviceView {
    fn from(view: core::DeviceView) -> Self {
        Self {
            workspace: view.workspace.into(),
            sessions: view.sessions.into_iter().map(Into::into).collect(),
            chats: view.chats.into_iter().map(Into::into).collect(),
            attentions: view.attentions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(uniffi::Enum)]
pub enum BuildChannel {
    Dev,
    Nightly,
    Stable,
}

#[derive(uniffi::Record)]
pub struct HostInfo {
    pub name: String,
    pub model: String,
    pub version: String,
    pub channel: BuildChannel,
}

impl From<core::HostInfo> for HostInfo {
    fn from(host: core::HostInfo) -> Self {
        Self {
            name: host.name,
            model: host.model,
            version: host.version,
            channel: match host.channel {
                core::BuildChannel::Dev => BuildChannel::Dev,
                core::BuildChannel::Nightly => BuildChannel::Nightly,
                core::BuildChannel::Stable => BuildChannel::Stable,
            },
        }
    }
}

/// The last of a chat's events the phone heard, to pick up from after a
/// reconnect.
#[derive(uniffi::Record, Clone)]
pub struct ChatMark {
    pub feed: String,
    pub seq: u64,
}

impl From<core::ChatMark> for ChatMark {
    fn from(mark: core::ChatMark) -> Self {
        Self {
            feed: mark.feed,
            seq: mark.seq,
        }
    }
}

impl From<ChatMark> for core::ChatMark {
    fn from(mark: ChatMark) -> Self {
        Self {
            feed: mark.feed,
            seq: mark.seq,
        }
    }
}

/// A chat's events cross as JSON, `{ kind, payload }` each: the chat screen
/// reads them with the Mac app's own chat code.
#[derive(uniffi::Enum)]
pub enum ChatAttachment {
    /// Live events follow, numbered from `mark.seq + 1`.
    Live {
        session_id: String,
        capabilities_json: String,
        setup_json: String,
        permission_mode: String,
        running: bool,
        turned: bool,
        replay_json: String,
        mark: ChatMark,
    },
    /// What the phone missed since the mark it attached with.
    Resumed {
        events_json: String,
        mark: ChatMark,
    },
    Missing,
    /// The chat said more than the Mac keeps; the Mac app has to open it.
    Restart,
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

impl From<core::ChatAttachment> for ChatAttachment {
    fn from(attachment: core::ChatAttachment) -> Self {
        match attachment {
            core::ChatAttachment::Live {
                start,
                permission_mode,
                running,
                turned,
                replay,
                mark,
            } => Self::Live {
                session_id: start.session_id,
                capabilities_json: json(&start.capabilities),
                setup_json: json(&start.setup),
                permission_mode,
                running,
                turned,
                replay_json: json(&replay),
                mark: mark.into(),
            },
            core::ChatAttachment::Resumed { events, mark } => Self::Resumed {
                events_json: json(&events),
                mark: mark.into(),
            },
            core::ChatAttachment::Missing => Self::Missing,
            core::ChatAttachment::Restart => Self::Restart,
        }
    }
}

/// What the Mac sends without being asked.
#[derive(uniffi::Enum)]
pub enum CoreEvent {
    /// One of a chat's events, as `{ kind, payload }` JSON.
    Chat {
        agent_id: String,
        seq: u64,
        event_json: String,
    },
    View {
        view: DeviceView,
    },
    Exited {
        session: u64,
        code: Option<u32>,
        signal: Option<String>,
        killed: bool,
    },
}

impl CoreEvent {
    pub(crate) fn from_core(event: core::Event) -> Option<Self> {
        Some(match event {
            core::Event::Chat {
                agent_id,
                seq,
                event,
            } => Self::Chat {
                agent_id,
                seq,
                event_json: json(&event),
            },
            core::Event::DeviceView { view } => Self::View { view: view.into() },
            core::Event::Exited {
                id,
                code,
                signal,
                killed,
            } => Self::Exited {
                session: id,
                code,
                signal,
                killed,
            },
            _ => return None,
        })
    }
}
