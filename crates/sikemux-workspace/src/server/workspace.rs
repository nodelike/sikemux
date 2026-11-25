//! The workspace on this host: its projects and spaces, and its agents with
//! how each one runs, kept by the core across restarts. Every client reads it
//! in the device view and changes it through the core's calls, so a phone
//! sees what the app sees with or without the app. The core offers each agent
//! on each of the person's accounts and finds its program itself; a launch
//! builds its environment afresh, keys and all.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::workspace::AddProjectParams;
pub use sikemux_calls::workspace::{AgentPatch, UpdateAgentParams};
pub use sikemux_calls::workspace::{
    CloseAgentsParams, DefaultsParams, MoveProjectParams, RemoveProjectParams, SpacesParams,
    UpdateProjectParams,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_accounts::{by_hand, Book};
use crate::agent_catalog::{AgentKind, AGENTS};
use crate::cli::protocol::is_tool_agent_id;
use crate::launch::providers::{Helper, Providers, Wiring};
use crate::launch::{skills, AgentSpec, PermissionMode};
use crate::sessions::recent::{self as saved_chats, RecentProvider, RecentRequest};
use sikemux_view::device::RecentInfo;
use sikemux_view::prefs::{PreferenceChange, ProjectSort};
use sikemux_view::workspace::{
    Backdrop, LauncherInfo, PinnedChat, ProjectInfo, RecentModel, SpaceInfo, Workspace,
    WorkspaceAgent,
};
use sikemux_wire::protocol::{ChatAccount, ChatInfo, ChatLaunch, ChatState, SessionInfo};

use super::changed::Topic;
use super::connection::blocking;
use super::followed_spaces::Reaches;
use super::kept;
use super::model_providers::checks::Checks;
use super::wake::Wake;
use super::{Core, CoreError, CoreResult};
use crate::acp::pickers;
pub use place::{ListPlace, Settled, Snooze};

mod agents;
mod launchers;
mod migrate;
mod place;
mod policy;
mod projects;
mod recent;
#[cfg(test)]
mod tests;

use agents::*;
pub use agents::{
    close, close_agent, close_agents, close_gone, create_agent, new_agent, restore, retitle,
    update_agent, AgentPatchExt, CreateAgentParams,
};
use projects::*;
pub use projects::{
    add_project, move_project, project_of, remove_project, set_defaults, set_spaces, sort_projects,
    update_project, AddProjectParamsExt,
};

pub use launchers::*;
pub use policy::*;
pub use recent::*;

/// Bumped by a change to what [`Saved`] holds that an earlier file needs
/// carried over to.
const VERSION: u32 = 7;
const MAX_PROJECTS: usize = 512;
const MAX_SPACES: usize = 64;
const MAX_PINNED: usize = 256;
const MAX_LAUNCHERS: usize = 64;
const MAX_AGENTS: usize = 1024;
const MAX_TITLE_CHARS: usize = 200;
/// As long as a provider lists a session's name, so a prompt standing in for one reads back whole.
const STAND_IN_TITLE_CHARS: usize = 72;
const MAX_FIELD_CHARS: usize = 512;
/// Room for a worktree or a buddy, which are a handful of short fields.
const MAX_DETAIL_BYTES: usize = 4096;
/// A phone-sized JPEG is a few hundred kilobytes; this leaves room without letting one fill a frame.
const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
/// Every buddy pack has at least this many colours, so an agent's colour
/// number means the same slot whichever pack the person picks.
const BUDDY_COLOURS: usize = 8;
/// Colour numbers clients read as a buddy's own.
const MAX_BUDDY_COLOUR: u32 = 63;
/// How many of the newest saved chats devices can take up again.
const RECENT_LIMIT: usize = 30;
/// How many chats a device paged or searched to the core remembers to take up again.
const MAX_FOUND: usize = 200;
/// How many chats a device is sent a page of when it asks for none in particular.
const RECENT_PAGE: usize = 12;
/// Saved chats change with every turn, so many ending at once look once.
const RECENT_SETTLE: Duration = Duration::from_secs(2);

/// One agent the core offers, on one of the person's accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launcher {
    pub id: String,
    pub label: String,
    pub agent: AgentSpec,
    pub permission_mode: PermissionMode,
    /// Whether the agent is ready, signed out or cannot tell, as the core last checked.
    pub status: Option<AgentStatus>,
    /// The program the core last found for the agent.
    pub program: Option<PathBuf>,
}

impl Launcher {
    /// Why it cannot be started now, as the core last checked.
    fn refusal(&self) -> Option<String> {
        self.status.as_ref()?.refusal(&self.label)
    }
}

/// An agent as the core keeps it: what every client sees, and the accounts
/// its chat signs in as, which no device sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    #[serde(flatten)]
    pub agent: WorkspaceAgent,
    pub account: Option<ChatAccount>,
    #[serde(default)]
    pub fallbacks: Vec<ChatAccount>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accounts: BTreeMap<String, String>,
    /// The program a client started the chat with itself, rather than one of the launchers,
    /// which it wakes with again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    /// It was put to sleep as the person stopped everything, so only the
    /// person opening or messaging it wakes it, never mail waiting for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// It moved to another account, which its agent has yet to open the chat on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
    #[serde(default, skip_serializing_if = "ListPlace::is_empty")]
    pub place: ListPlace,
}

/// What survives a restart of the core.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    projects: Vec<ProjectInfo>,
    #[serde(default)]
    spaces: Vec<SpaceInfo>,
    #[serde(default)]
    agents: Vec<Kept>,
    #[serde(default)]
    pinned: Vec<PinnedChat>,
    /// The person's agent accounts, absent until the app first says which they are.
    accounts: Option<Book>,
    /// The mode the person starts agents in, absent until they first choose.
    permission_mode: Option<PermissionMode>,
    /// The `configOptions` each launcher's last session offered, by launcher, so a chat that
    /// has not started yet offers the same models and efforts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    launcher_options: BTreeMap<String, Value>,
    /// The model and effort the person last picked for each kind of agent, which its new
    /// chats start on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_picks: BTreeMap<String, ModelPick>,
    /// The models new chats of each kind of agent started on lately, newest first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    recent_models: BTreeMap<String, Vec<RecentModel>>,
    /// The agents closed lately as they stood when they closed, newest first, so reopening one
    /// puts back what its session does not keep: its lead, checkout branch, mode and labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    closed: Vec<WorkspaceAgent>,
}

/// How the last ask of a launcher's agent for what it offers went.
#[derive(Clone, Debug)]
pub struct OfferRead {
    /// The agent's program and sign-in as they stood when it was asked, so a later look can
    /// tell whether either changed since.
    pub asked_of: String,
    /// Why it could not say, as a chat failure; None when it said.
    pub unread: Option<Value>,
}

/// How many closed agents' records the workspace keeps for reopening them.
const MOST_CLOSED: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPick {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ModelPick {
    /// Marks the picked model and effort as current in a session's `configOptions`, where
    /// they are among its choices.
    fn mark(&self, options: &mut Value) {
        if let Some(model) = &self.model {
            pickers::pick(options, |row| row["id"] == "model", model);
        }
        pickers::fit_effort(options);
        if let Some(effort) = &self.effort {
            pickers::pick(options, sikemux_fold::pickers::is_effort, effort);
        }
    }
}

impl Saved {
    fn kept(&self, agent_id: &str) -> Option<&Kept> {
        self.agents.iter().find(|kept| kept.agent.id == agent_id)
    }

    fn kept_mut(&mut self, agent_id: &str) -> Option<&mut Kept> {
        self.agents
            .iter_mut()
            .find(|kept| kept.agent.id == agent_id)
    }

    fn project(&self, id: &str) -> Option<&ProjectInfo> {
        self.projects.iter().find(|project| project.id == id)
    }
}

/// A saved chat devices may take up again, with the launcher that does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    launcher: String,
    provider: String,
    session_id: String,
    title: String,
    project: PathBuf,
    /// The folder it ran in: the project, or a worktree of it.
    cwd: PathBuf,
    active_at: u64,
}

impl Published {
    /// The launchers of chat agents, then of those that run only in their own terminal.
    fn every_launcher(&self) -> impl Iterator<Item = &Launcher> {
        self.launchers.iter().chain(&self.terminals)
    }
}

#[derive(Default)]
pub struct Published {
    saved: Saved,
    launchers: Vec<Launcher>,
    /// The agents found that run only in their own terminal.
    terminals: Vec<Launcher>,
    recent: Vec<Recent>,
    /// Every saved chat in each project, by its id.
    saved_chats: HashMap<PathBuf, Vec<String>>,
    /// Older chats a device paged or searched to, newest looked up first, which it may take up
    /// again as it does a recent one.
    found: Vec<Recent>,
    /// The project each pinned chat's folder belongs to, when that folder is a worktree.
    pin_homes: HashMap<PathBuf, PathBuf>,
    texture: bool,
    image: Option<BackdropImage>,
}

/// A picture shown behind the panes, shrunk to suit a phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackdropImage {
    /// Changes whenever the picture does, so a device fetches it only then.
    pub id: String,
    pub data_url: String,
}

/// How an agent is launched, as the workspace keeps it.
#[derive(Clone, Debug)]
pub struct AgentRun {
    pub launcher: String,
    pub permission_mode: PermissionMode,
}

fn too_long(text: &str, chars: usize) -> bool {
    text.chars().count() > chars
}

/// `value` with `patch`'s fields set over its own, a null field cleared, and
/// any of `fixed` refused.
fn patched<T: Serialize + for<'de> Deserialize<'de>>(
    value: &T,
    patch: Map<String, Value>,
    fixed: &[&str],
) -> CoreResult<T> {
    let mut whole = serde_json::to_value(value)?;
    let Some(object) = whole.as_object_mut() else {
        return Err("only a record can be changed".into());
    };
    for (key, field) in patch {
        if fixed.contains(&key.as_str()) {
            return Err(format!("{key} cannot be changed").into());
        }
        if field.is_null() {
            object.remove(&key);
        } else {
            object.insert(key, field);
        }
    }
    serde_json::from_value(whole)
        .map_err(|error| CoreError::from(format!("the change does not fit: {error}")))
}

pub struct Workspaces {
    published: Mutex<Published>,
    /// Where [`Saved`] is kept; nowhere for a core without a data folder.
    file: kept::Kept,
    providers: Option<Providers>,
    /// Whether models on each provider call tools the way agents need.
    pub model_checks: Checks,
    /// How far the person's project folders carry a space.
    pub reaches: Reaches,
    /// The data folder, which holds the skills library.
    data_dir: Option<PathBuf>,
    /// Counts the looks over the agents, so one that took longer never replaces a newer one.
    asked: AtomicU64,
    /// The newest look whose result is in use.
    applied: AtomicU64,
    /// Why each agent the last look missed was missed, so the log says it once.
    missed: Mutex<BTreeMap<String, String>>,
    /// Tells the core the workspace, or the launchers it offers, changed.
    pub wake: Wake,
    /// The saved chats may have changed.
    recent_stale: Notify,
    /// The launchers whose agents are being asked what they offer.
    offers_asking: Mutex<HashSet<String>>,
    offers_read: Mutex<HashMap<String, OfferRead>>,
}

impl Workspaces {
    /// The workspace kept in `data_dir`, taking over what the earlier app last saved, `app`,
    /// the first time.
    pub fn new(data_dir: Option<&Path>, app: Option<Value>) -> Self {
        let file = kept::Kept::new(data_dir, "workspace.json");
        let read: Value = file.load();
        let mut saved = if read.is_null() {
            Saved::default()
        } else {
            Saved::deserialize(&read).unwrap_or_else(|error| {
                file.set_aside(&error.to_string());
                Saved::default()
            })
        };
        let upgraded = saved.version < VERSION;
        if saved.version < 1 {
            migrate::carry_over(&mut saved, &read, app);
        }
        if saved.version < 2 {
            migrate::dress_agents(&mut saved);
        }
        if saved.version < 3 {
            migrate::mark_terminals(&mut saved);
        }
        if saved.version < 4 {
            migrate::name_accounts(&mut saved, |agent| agent.accounts);
        }
        if saved.version < 5 {
            migrate::untitle_chiefs(&mut saved);
        }
        if saved.version < 6 {
            migrate::unsettle_by_core(&mut saved, &read);
        }
        if saved.version < 7 {
            migrate::name_accounts(&mut saved, |agent| agent.accounts_by_hand);
        }
        saved.version = VERSION;
        let workspaces = Self {
            published: Mutex::new(Published {
                saved,
                ..Published::default()
            }),
            file,
            providers: data_dir.map(Providers::new),
            model_checks: Checks::new(data_dir),
            reaches: Reaches::default(),
            data_dir: data_dir.map(Path::to_path_buf),
            asked: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            missed: Mutex::default(),
            wake: Wake::default(),
            recent_stale: Notify::new(),
            offers_asking: Mutex::default(),
            offers_read: Mutex::default(),
        };
        if upgraded {
            workspaces.file.save(&workspaces.published.locked().saved);
        }
        workspaces
    }

    /// Changes what is saved, keeps it, and tells the core when anything
    /// changed.
    fn change<T>(&self, change: impl FnOnce(&mut Saved) -> CoreResult<T>) -> CoreResult<T> {
        let mut published = self.published.locked();
        let changed = change(&mut published.saved)?;
        let saved = self.file.save_soon(&published.saved);
        drop(published);
        if saved {
            self.wake.now();
        }
        Ok(changed)
    }

    pub fn publish_backdrop(&self, texture: bool, image: Option<BackdropImage>) -> CoreResult<()> {
        if let Some(image) = &image {
            if image.data_url.len() > MAX_IMAGE_BYTES {
                return Err("the backdrop picture is larger than the core keeps".into());
            }
        }
        let mut published = self.published.locked();
        published.texture = texture;
        published.image = image;
        Ok(())
    }

    pub fn backdrop_image_id(&self) -> Option<String> {
        self.published
            .locked()
            .image
            .as_ref()
            .map(|image| image.id.clone())
    }

    pub fn backdrop_image(&self) -> Option<String> {
        self.published
            .locked()
            .image
            .as_ref()
            .map(|image| image.data_url.clone())
    }

    pub fn view(&self) -> Workspace {
        let published = self.published.locked();
        let saved = &published.saved;
        let offered = |id: &String| {
            published
                .launchers
                .iter()
                .any(|launcher| &launcher.id == id)
        };
        let launcher = |project: &ProjectInfo| {
            saved
                .agents
                .iter()
                .filter(|kept| kept.agent.project == project.id)
                .max_by_key(|kept| kept.agent.created_at)
                .map(|kept| launcher_of(&kept.agent))
                .filter(offered)
                .or_else(|| {
                    let kind = &published.launchers.first()?.agent.provider;
                    let ids = published
                        .launchers
                        .iter()
                        .map(|launcher| (launcher.id.as_str(), launcher.agent.provider.as_str()));
                    launcher_for(ids, saved.accounts.as_ref(), kind)
                })
        };
        Workspace {
            projects: saved
                .projects
                .iter()
                .map(|project| ProjectInfo {
                    launcher: launcher(project),
                    saved_chats: unheld_chats(&published, &project.path),
                    agents: (saved.agents.iter())
                        .filter(|kept| kept.agent.project == project.id && !kept.agent.asleep)
                        .count() as u32,
                    ..project.clone()
                })
                .collect(),
            elsewhere: Vec::new(),
            spaces: saved.spaces.clone(),
            agents: (saved.agents.iter())
                .map(|kept| {
                    let mut agent = kept.agent.clone();
                    agent.titled = agent.renamed || !untitled(saved, &agent);
                    agent
                })
                .collect(),
            pinned: saved.pinned.clone(),
            permission_mode: saved.permission_mode.unwrap_or(PermissionMode::Bypass),
            disabled_plugins: Default::default(),
            launchers: published
                .launchers
                .iter()
                .map(|launcher| launcher_info(saved, launcher))
                .collect(),
            terminal_launchers: published
                .terminals
                .iter()
                .map(|launcher| launcher_info(saved, launcher))
                .collect(),
            palette: BTreeMap::new(),
            backdrop: Backdrop {
                texture: published.texture,
                image: published.image.as_ref().map(|image| image.id.clone()),
                look: None,
            },
        }
    }
}

/// Runs whenever the workspace changes: agents in it keep their names while
/// nothing of them runs.
pub fn settle(core: &Arc<Core>) {
    settle_names(core);
    (core.workspace_above().settled)(core);
    core.changed(What::Agents);
}

impl super::part::Part for Workspaces {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            super::part::on_wake(core, |core| &core.workspaces().wake, settle),
            tokio::spawn(recents(core.clone())),
        ]
    }

    fn flush(&self) {
        self.file.flush();
    }
}
