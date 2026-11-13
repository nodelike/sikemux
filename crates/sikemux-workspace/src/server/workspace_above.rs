//! What the workspace and the state kept beside it ask of the features built over them. They
//! cannot name those features, so a core is built with these, one function for each.

use std::sync::Arc;

use std::collections::HashSet;
use std::path::Path;

use sikemux_view::workspace::{AgentWorktree, Workspace, WorkspaceAgent};
use sikemux_wire::protocol::ChatInfo;

use sikemux_view::prefs::{PreferenceChange, Preferences};

use super::access::{Needs, Peer};
use super::activity_log::Place;
use super::workspace::Held;

use super::{Core, CoreResult};

pub struct WorkspaceAbove {
    /// The projects the workspace offers changed.
    pub projects_changed: fn(&Core),
    /// The project at this path is no longer offered.
    pub project_removed: fn(&Core, &str),
    /// The person picked which code hosts the project at this path uses.
    pub code_hosts_picked: fn(&Arc<Core>, &str),
    /// This agent is about to leave the workspace.
    pub agent_closing: fn(&Core, &WorkspaceAgent),
    /// The agent with this id left the workspace because it was closed.
    pub agent_closed: fn(&Core, &str),
    /// These agents were closed, and these were their worktrees.
    pub worktrees_closed: fn(&Core, &[String], Vec<AgentWorktree>),
    /// An agent's title changed.
    pub title_moved: fn(&Core),
    /// The agent with this id has this title of its own.
    pub chat_titled: fn(&Core, &str, &str),
    /// These agents are gone from the workspace, however they left.
    pub agents_gone: fn(&Core, &[&String]),
    /// The workspace changed and its names are settled.
    pub settled: fn(&Arc<Core>),
    /// The agents of linked hosts' shared teams.
    pub foreign_agents: fn(&Core) -> HashSet<String>,
    /// What the person has typed to this agent's chat changed.
    pub chat_draft_changed: fn(&Core, &str),
    /// The saved session with this id was deleted.
    pub session_deleted: fn(&Core, &str),
    /// Whether a question still shows the picture at this path.
    pub question_keeps_picture: fn(&Core, &Path) -> bool,
    /// The chats that run now.
    pub open_chats: fn(&Core) -> Vec<ChatInfo>,
    /// The closed chats that triggers keep.
    pub held_chats: fn(&Core) -> Held,
    /// The workspace as clients are sent it.
    pub workspace_view: fn(&Core) -> Workspace,
    /// The agents with a chat running.
    pub chat_agents: fn(&Core) -> Vec<String>,
    /// Whether this peer may do what needs this much.
    pub permit: fn(&Core, &Peer, Needs) -> CoreResult<()>,
    /// A client shows these agents, focused or not, and draws their terminals or not.
    pub agents_shown: fn(&Arc<Core>, &[String], bool, bool),
    /// Where the chat of this agent works and what its CLI writes, when it has a chat.
    pub chat_place: fn(&Core, &str) -> Option<Place>,
    /// The person changed this preference, and it is already kept.
    pub preference_changed: fn(&Core, &PreferenceChange) -> CoreResult<()>,
    /// The preferences as clients are sent them.
    pub shown_preferences: fn(&Core) -> Preferences,
}
