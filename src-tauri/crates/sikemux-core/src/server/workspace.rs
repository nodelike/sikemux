//! The workspace on this host: its projects and spaces, and its agents with
//! how each one runs, kept by the core across restarts. Every client reads it
//! in the device view and changes it through the core's calls, so a phone
//! sees what the app sees with or without the app. The core offers each agent
//! on each of the person's accounts and finds its program itself; a launch
//! builds its environment afresh, keys and all.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Notify;

use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_accounts::Book;
use crate::agent_catalog::{AgentKind, AGENTS};
use crate::cli::protocol::is_tool_agent_id;
use crate::launch::providers::{Helper, Providers, Wiring, ROUTED_ENV};
use crate::launch::{skills, AgentSpec};
use crate::protocol::{
    Backdrop, BackdropImage, ChatAccount, ChatInfo, ChatLaunch, ChatState, LauncherInfo,
    PinnedChat, ProjectInfo, RecentInfo, SpaceInfo, Workspace, WorkspaceAgent,
};
use crate::sessions::recent::{self as saved_chats, RecentProvider, RecentRef, RecentRequest};

use super::chat;
use super::connection::blocking;
use super::model_providers::checks::Checks;
use super::names::project_of;
use super::wake::Wake;
use super::{Core, CoreError, CoreResult};
use crate::acp::pickers;

mod migrate;

/// Bumped by a change to what [`Saved`] holds that an earlier file needs
/// carried over to.
const VERSION: u32 = 1;
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
const MAX_COLOURS: usize = 64;
const MAX_FOLDER_VARIABLES: usize = 256;
const MAX_DONE_TASKS: usize = 32;
const MAX_NOTES_CHARS: usize = 4000;
const MAX_VARIABLE_CHARS: usize = 8192;
/// A phone-sized JPEG is a few hundred kilobytes; this leaves room without letting one fill a frame.
const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
const MAX_COLOUR_CHARS: usize = 64;
const MAX_BUDDY_AGENTS: usize = 4096;
const MAX_BUDDY_BOXES: usize = 512;
/// How many of the newest saved chats devices can take up again.
const RECENT_LIMIT: usize = 30;
/// Saved chats change with every turn, so many ending at once look once.
const RECENT_SETTLE: Duration = Duration::from_secs(2);

/// One agent the core offers, on one of the person's accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Launcher {
    pub id: String,
    pub label: String,
    pub agent: AgentSpec,
    pub permission_mode: String,
    /// Whether the agent is ready, signed out or cannot tell, as the core last checked.
    pub status: Option<String>,
    /// The program the core last found for the agent.
    pub program: Option<PathBuf>,
}

/// An agent as the core keeps it: what every client sees, and the accounts
/// its chat signs in as, which no device sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Kept {
    #[serde(flatten)]
    pub agent: WorkspaceAgent,
    #[serde(default)]
    pub account: Option<ChatAccount>,
    #[serde(default)]
    pub fallbacks: Vec<ChatAccount>,
}

/// What survives a restart of the core.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
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
    permission_mode: Option<String>,
    /// The environment of each folder of a project, by the project's folder and then the
    /// folder's, `.` for its root. Never sent to devices, since it can hold secrets.
    #[serde(default)]
    folder_settings: BTreeMap<String, BTreeMap<String, FolderSettings>>,
    /// What every agent in a project is told, by the project's folder.
    #[serde(default)]
    project_rules: BTreeMap<String, ProjectRules>,
}

/// The environment processes started in one folder of a project get.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderSettings {
    /// Whose environment files are read: absent reads none, empty only the ones every
    /// environment shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Set by the person, over what the files say.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The tasks an agent's work in the folder must pass before it is done, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<String>,
}

/// What the person tells every agent working in a project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRules {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Agents run every folder's checks, not only those of the folders they changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check_every_folder: bool,
    /// An agent whose check fails stops and says so, instead of fixing it first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_on_failure: bool,
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
struct Recent {
    launcher: String,
    provider: String,
    session_id: String,
    title: String,
    cwd: PathBuf,
    active_at: u64,
}

#[derive(Default)]
struct Published {
    saved: Saved,
    launchers: Vec<Launcher>,
    recent: Vec<Recent>,
    /// Each provider's `configOptions` from its last session, by provider.
    config_options: BTreeMap<String, Value>,
    palette: BTreeMap<String, String>,
    texture: bool,
    image: Option<BackdropImage>,
    buddies: Option<Buddies>,
}

/// The characters the app draws for its agents while buddies are on, so the
/// notch and phones draw the same ones.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Buddies {
    pack: String,
    shape: String,
    motion: bool,
    /// Each agent's buddy as the app draws it, by agent id.
    agents: BTreeMap<String, BuddyLook>,
    /// The part of its 24-unit square each shape fills, by `pack/shape`, as a
    /// viewBox; only the app can measure it.
    boxes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuddyLook {
    pack: String,
    shape: String,
    color: String,
}

impl Buddies {
    fn oversized(&self) -> bool {
        let long = |text: &String| text.len() > MAX_COLOUR_CHARS;
        long(&self.pack)
            || long(&self.shape)
            || self.agents.len() > MAX_BUDDY_AGENTS
            || self.boxes.len() > MAX_BUDDY_BOXES
            || self
                .boxes
                .iter()
                .any(|(key, view_box)| long(key) || long(view_box))
            || self.agents.iter().any(|(id, look)| {
                long(id) || long(&look.pack) || long(&look.shape) || long(&look.color)
            })
    }
}

/// How an agent is launched, as the workspace keeps it.
#[derive(Clone, Debug)]
pub(crate) struct AgentRun {
    pub launcher: String,
    pub permission_mode: String,
}

/// The launcher that starts `agent`: its CLI on the account it runs on.
pub(crate) fn launcher_of(agent: &WorkspaceAgent) -> String {
    match &agent.profile_id {
        Some(profile) => format!("{}:{profile}", agent.provider),
        None => agent.provider.clone(),
    }
}

/// The agent's title when it has one beyond its CLI's name.
fn named(agent: &WorkspaceAgent) -> Option<String> {
    Some(agent.title.clone()).filter(|title| !title.trim().is_empty() && title != &agent.provider)
}

fn is_chat_provider(provider: &str) -> bool {
    AGENTS
        .iter()
        .any(|def| def.kind == provider && def.kind != "pi")
}

fn now_ms() -> u64 {
    super::remote::unix_ms()
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

fn check_agent(agent: &WorkspaceAgent, saved: &Saved) -> CoreResult<()> {
    let long = |text: &Option<String>| {
        text.as_deref()
            .is_some_and(|text| too_long(text, MAX_FIELD_CHARS))
    };
    let detail = |value: &Option<Value>| {
        value
            .as_ref()
            .is_some_and(|value| value.to_string().len() > MAX_DETAIL_BYTES)
    };
    if !is_tool_agent_id(&agent.id) {
        return Err("that is not an agent id".into());
    }
    if too_long(&agent.title, MAX_TITLE_CHARS)
        || too_long(&agent.provider, 64)
        || too_long(&agent.permission_mode, 64)
        || [
            &agent.lead,
            &agent.resume_id,
            &agent.profile_id,
            &agent.model,
            &agent.model_provider,
            &agent.effort,
        ]
        .into_iter()
        .any(long)
        || detail(&agent.worktree)
        || detail(&agent.buddy)
    {
        return Err("the agent has a field longer than the core keeps".into());
    }
    if !agent.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    if saved.project(&agent.project).is_none() {
        return Err("that project is not on this host".into());
    }
    Ok(())
}

fn check_project(project: &ProjectInfo, saved: &Saved) -> CoreResult<()> {
    if too_long(&project.name, MAX_TITLE_CHARS) || too_long(&project.id, MAX_FIELD_CHARS) {
        return Err("the project has a field longer than the core keeps".into());
    }
    if !project.path.is_absolute() {
        return Err(format!("project {} has no absolute path", project.name).into());
    }
    if let Some(space) = &project.space {
        if !saved.spaces.iter().any(|known| &known.id == space) {
            return Err("that space is not on this host".into());
        }
    }
    Ok(())
}

pub(crate) struct Workspaces {
    published: Mutex<Published>,
    /// Where [`Saved`] is kept; nowhere for a core without a data folder.
    path: Option<PathBuf>,
    providers: Option<Providers>,
    /// Whether models on each provider call tools the way agents need.
    pub(crate) model_checks: Checks,
    /// The data folder, which holds the skills library.
    data_dir: Option<PathBuf>,
    /// Counts the looks over the agents, so one that took longer never replaces a newer one.
    asked: AtomicU64,
    /// Tells the core the workspace, or the launchers it offers, changed.
    pub(crate) wake: Wake,
    /// The saved chats may have changed.
    recent_stale: Notify,
}

pub(crate) struct ChatChoice {
    pub launcher: String,
    pub project: String,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The provider's session to load instead of opening a new one.
    pub resume_id: Option<String>,
}

/// What a device asked of a recent chat it takes up again.
pub(crate) struct ResumeChoice {
    pub recent: String,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn recent_id(chat: &Recent) -> String {
    format!("{}:{}", chat.provider, chat.session_id)
}

fn running_now(chat: &Recent, open: &[ChatInfo]) -> bool {
    open.iter().any(|live| {
        live.provider == chat.provider
            && live.session_id.as_deref() == Some(chat.session_id.as_str())
    })
}

impl Workspaces {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join("workspace.json"));
        let read: Value = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        let mut saved: Saved = serde_json::from_value(read.clone()).unwrap_or_default();
        let upgraded = saved.version < VERSION;
        if upgraded {
            migrate::carry_over(&mut saved, &read, data_dir);
            saved.version = VERSION;
        }
        let workspaces = Self {
            published: Mutex::new(Published {
                saved,
                ..Published::default()
            }),
            path,
            providers: data_dir.map(Providers::new),
            model_checks: Checks::new(data_dir),
            data_dir: data_dir.map(Path::to_path_buf),
            asked: AtomicU64::new(0),
            wake: Wake::default(),
            recent_stale: Notify::new(),
        };
        if upgraded {
            workspaces.save(&workspaces.lock().saved);
        }
        workspaces
    }

    /// The core's data folder, which holds the skills library.
    pub(crate) fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// The model providers the host's agents run on, and their keys.
    pub(crate) fn providers(&self) -> Option<&Providers> {
        self.providers.as_ref()
    }

    fn lock(&self) -> MutexGuard<'_, Published> {
        self.published
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, saved: &Saved) {
        let Some(path) = &self.path else {
            return;
        };
        let written = serde_json::to_vec(saved)
            .map_err(std::io::Error::from)
            .and_then(|bytes| sikemux_process::files::write_atomic(path, &bytes, true));
        if let Err(error) = written {
            eprintln!("sikemux core: the workspace was not saved: {error}");
        }
    }

    /// Changes what is saved, keeps it, and tells the core.
    fn change<T>(&self, change: impl FnOnce(&mut Saved) -> CoreResult<T>) -> CoreResult<T> {
        let mut published = self.lock();
        let changed = change(&mut published.saved)?;
        self.save(&published.saved);
        drop(published);
        self.wake.now();
        Ok(changed)
    }

    /// The saved chats may have changed, so the recent ones are looked for again.
    pub(crate) fn recent_stale(&self) {
        self.recent_stale.notify_one();
    }

    /// Adds a folder on this host as a project, or answers the project already
    /// there. A client may name the new project's id.
    fn add_project(
        &self,
        path: PathBuf,
        name: Option<String>,
        id: Option<String>,
    ) -> CoreResult<ProjectInfo> {
        let project = self.change(|saved| {
            if let Some(project) = saved.projects.iter().find(|project| project.path == path) {
                return Ok(project.clone());
            }
            if saved.projects.len() >= MAX_PROJECTS {
                return Err("this host already keeps as many projects as it can".into());
            }
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            if saved.project(&id).is_some() {
                return Err("a project with that id is already on this host".into());
            }
            let name = name
                .map(|name| {
                    name.trim()
                        .chars()
                        .take(MAX_TITLE_CHARS)
                        .collect::<String>()
                })
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| path.display().to_string());
            let project = ProjectInfo {
                id,
                name,
                path,
                pinned: false,
                space: None,
                code_hosts: Default::default(),
            };
            check_project(&project, saved)?;
            saved.projects.push(project.clone());
            Ok(project)
        })?;
        self.recent_stale();
        Ok(project)
    }

    /// The project `cwd` is in, added when it is in none, for an agent that
    /// started there.
    fn project_for(&self, cwd: &Path) -> CoreResult<String> {
        let path = PathBuf::from(project_of(cwd, &self.project_paths()));
        Ok(self.add_project(path, None, None)?.id)
    }

    /// Stops offering a project, and forgets the agents in it.
    fn remove_project(&self, path: &Path) -> CoreResult<()> {
        self.change(|saved| {
            let id = saved
                .projects
                .iter()
                .find(|project| project.path == path)
                .map(|project| project.id.clone())
                .ok_or_else(|| CoreError::from("that folder is not a project on this host"))?;
            saved.projects.retain(|project| project.id != id);
            saved.agents.retain(|kept| kept.agent.project != id);
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    fn update_project(&self, id: &str, patch: Map<String, Value>) -> CoreResult<ProjectInfo> {
        self.change(|saved| {
            let index = saved
                .projects
                .iter()
                .position(|project| project.id == id)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            let project = patched(&saved.projects[index], patch, &["id", "path"])?;
            check_project(&project, saved)?;
            saved.projects[index] = project.clone();
            Ok(project)
        })
    }

    fn move_project(&self, id: &str, index: usize) -> CoreResult<()> {
        self.change(|saved| {
            let from = saved
                .projects
                .iter()
                .position(|project| project.id == id)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            let project = saved.projects.remove(from);
            let index = index.min(saved.projects.len());
            saved.projects.insert(index, project);
            Ok(())
        })
    }

    /// Replaces the spaces; a project in one that is gone is shown in every space.
    fn set_spaces(&self, spaces: Vec<SpaceInfo>) -> CoreResult<()> {
        let oversized = spaces.iter().any(|space| {
            too_long(&space.id, MAX_FIELD_CHARS)
                || too_long(&space.name, MAX_TITLE_CHARS)
                || too_long(&space.icon, MAX_FIELD_CHARS)
        });
        if spaces.len() > MAX_SPACES || oversized {
            return Err("more spaces than the core keeps".into());
        }
        self.change(|saved| {
            for project in &mut saved.projects {
                let gone = project
                    .space
                    .as_ref()
                    .is_some_and(|space| !spaces.iter().any(|kept| &kept.id == space));
                if gone {
                    project.space = None;
                }
            }
            saved.spaces = spaces;
            Ok(())
        })
    }

    pub(crate) fn folder_settings(&self, project: &str) -> BTreeMap<String, FolderSettings> {
        self.lock()
            .saved
            .folder_settings
            .get(project)
            .cloned()
            .unwrap_or_default()
    }

    fn set_folder_settings(
        &self,
        project: String,
        folder: String,
        settings: FolderSettings,
    ) -> CoreResult<()> {
        let one_folder = folder == "."
            || (!folder.is_empty() && !folder.contains('/') && !folder.starts_with('.'));
        let oversized = too_long(&project, MAX_FIELD_CHARS)
            || too_long(&folder, MAX_FIELD_CHARS)
            || settings
                .environment
                .as_ref()
                .is_some_and(|name| too_long(name, 64))
            || settings.env.len() > MAX_FOLDER_VARIABLES
            || settings.env.iter().any(|(key, value)| {
                too_long(key, MAX_TITLE_CHARS) || too_long(value, MAX_VARIABLE_CHARS)
            })
            || settings.done.len() > MAX_DONE_TASKS
            || settings
                .done
                .iter()
                .any(|task| too_long(task, MAX_FIELD_CHARS));
        if !one_folder || oversized {
            return Err("those folder settings are more than the core keeps".into());
        }
        if let Some(key) = settings.env.keys().find(|key| !is_variable_name(key)) {
            return Err(format!("{key} cannot name an environment variable").into());
        }
        self.change(|saved| {
            let folders = saved.folder_settings.entry(project.clone()).or_default();
            if settings == FolderSettings::default() {
                folders.remove(&folder);
            } else {
                folders.insert(folder, settings);
            }
            if folders.is_empty() {
                saved.folder_settings.remove(&project);
            } else if saved.folder_settings.len() > MAX_PROJECTS {
                return Err("settings for more projects than the core keeps".into());
            }
            Ok(())
        })
    }

    pub(crate) fn project_rules(&self, project: &str) -> ProjectRules {
        self.lock()
            .saved
            .project_rules
            .get(project)
            .cloned()
            .unwrap_or_default()
    }

    fn set_project_rules(&self, project: String, rules: ProjectRules) -> CoreResult<()> {
        if too_long(&project, MAX_FIELD_CHARS) || too_long(&rules.notes, MAX_NOTES_CHARS) {
            return Err("those notes are longer than the core keeps".into());
        }
        self.change(|saved| {
            if rules == ProjectRules::default() {
                saved.project_rules.remove(&project);
            } else if saved.project_rules.len() >= MAX_PROJECTS
                && !saved.project_rules.contains_key(&project)
            {
                return Err("rules for more projects than the core keeps".into());
            } else {
                saved.project_rules.insert(project, rules);
            }
            Ok(())
        })
    }

    /// What a process started in `cwd` gets from its project's folder settings: the root's,
    /// then those of the folder it is in. A worktree counts as its main checkout.
    pub(crate) fn folder_environment(&self, cwd: &Path) -> BTreeMap<String, String> {
        let projects = self.lock().saved.folder_settings.clone();
        if projects.is_empty() {
            return BTreeMap::new();
        }
        let path = super::names::in_main_checkout_or_self(cwd);
        let canonical = |project: &str| {
            std::fs::canonicalize(project).unwrap_or_else(|_| PathBuf::from(project))
        };
        let Some((project, folders)) = projects
            .iter()
            .map(|(project, folders)| (canonical(project), folders))
            .filter(|(project, _)| path.starts_with(project))
            .max_by_key(|(project, _)| project.as_os_str().len())
        else {
            return BTreeMap::new();
        };
        let inner = path
            .strip_prefix(&project)
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|first| first.as_os_str().to_string_lossy().into_owned());
        let mut env = BTreeMap::new();
        for folder in std::iter::once(".".to_owned()).chain(inner) {
            let Some(settings) = folders.get(&folder) else {
                continue;
            };
            if let Some(environment) = &settings.environment {
                env.extend(crate::tasks::folders::variables(
                    &project,
                    &folder,
                    environment,
                ));
            }
            env.extend(settings.env.clone());
        }
        env
    }

    fn set_pinned(&self, pinned: Vec<PinnedChat>) -> CoreResult<()> {
        let oversized = pinned.iter().any(|chat| {
            too_long(&chat.provider, 64)
                || too_long(&chat.session_id, MAX_FIELD_CHARS)
                || too_long(&chat.title, MAX_TITLE_CHARS)
        });
        if pinned.len() > MAX_PINNED || oversized {
            return Err("more pinned chats than the core keeps".into());
        }
        self.change(|saved| {
            saved.pinned = pinned;
            Ok(())
        })
    }

    /// Keeps the mode agents start in, and says whether it changed.
    fn set_permission_mode(&self, mode: String) -> CoreResult<bool> {
        if too_long(&mode, 64) {
            return Err("that is not a permission mode".into());
        }
        self.change(|saved| {
            let changed = saved.permission_mode.as_ref() != Some(&mode);
            saved.permission_mode = Some(mode);
            Ok(changed)
        })
    }

    pub(crate) fn agent(&self, agent_id: &str) -> Option<WorkspaceAgent> {
        self.lock()
            .saved
            .kept(agent_id)
            .map(|kept| kept.agent.clone())
    }

    /// Adds an agent, or replaces the one with its id. Without an id the core
    /// names it.
    pub(crate) fn put_agent(&self, mut draft: Map<String, Value>) -> CoreResult<WorkspaceAgent> {
        if !draft.contains_key("id") {
            let id = format!("agent-{}", uuid::Uuid::new_v4().simple());
            draft.insert("id".into(), Value::String(id));
        }
        if !draft.contains_key("createdAt") {
            draft.insert("createdAt".into(), now_ms().into());
        }
        let agent: WorkspaceAgent = serde_json::from_value(Value::Object(draft))
            .map_err(|error| CoreError::from(format!("that is not an agent: {error}")))?;
        let put = self.change(|saved| {
            check_agent(&agent, saved)?;
            let count = saved.agents.len();
            match saved.kept_mut(&agent.id) {
                Some(kept) => kept.agent = agent.clone(),
                None if count >= MAX_AGENTS => {
                    return Err("this host already keeps as many agents as it can".into())
                }
                None => saved.agents.push(Kept {
                    agent: agent.clone(),
                    account: None,
                    fallbacks: Vec::new(),
                }),
            }
            Ok(agent)
        })?;
        Ok(put)
    }

    pub(crate) fn update_agent(
        &self,
        agent_id: &str,
        patch: Map<String, Value>,
    ) -> CoreResult<WorkspaceAgent> {
        self.change(|saved| {
            let current = saved
                .kept(agent_id)
                .ok_or_else(|| CoreError::from("that agent is not on this host"))?;
            let agent = patched(&current.agent, patch, &["id", "createdAt"])?;
            check_agent(&agent, saved)?;
            if let Some(kept) = saved.kept_mut(agent_id) {
                kept.agent = agent.clone();
            }
            Ok(agent)
        })
    }

    /// Gives the agent the title its chat earned, unless the person named it.
    /// A first prompt's words stand in only while the agent has no title of
    /// its own, and a command never does. Answers whether the title changed.
    fn note_title(&self, agent_id: &str, title: &str, stand_in: bool) -> bool {
        if title.starts_with('/') || title.starts_with('<') {
            return false;
        }
        let title: String = match stand_in {
            true => title.chars().take(STAND_IN_TITLE_CHARS).collect(),
            false => title.to_owned(),
        };
        self.change(|saved| {
            let profile = |id: &str| {
                saved
                    .accounts
                    .as_ref()?
                    .profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .map(|profile| profile.name.clone())
            };
            let Some(kept) = saved.kept(agent_id) else {
                return Ok(false);
            };
            let agent = &kept.agent;
            let untitled = agent.title.trim().is_empty()
                || agent.title == agent.provider
                || agent.profile_id.as_deref().and_then(profile).as_deref() == Some(&agent.title);
            if agent.renamed || agent.title == title || (stand_in && !untitled) {
                return Ok(false);
            }
            if let Some(kept) = saved.kept_mut(agent_id) {
                kept.agent.title = title;
            }
            Ok(true)
        })
        .unwrap_or(false)
    }

    pub(crate) fn set_asleep(&self, agent_id: &str, asleep: bool) {
        let _ = self.change(|saved| {
            if let Some(kept) = saved.kept_mut(agent_id) {
                kept.agent.asleep = asleep;
            }
            Ok(())
        });
    }

    fn close_agents(&self, agent_ids: &[String]) -> CoreResult<()> {
        self.change(|saved| {
            saved
                .agents
                .retain(|kept| !agent_ids.contains(&kept.agent.id));
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// Every agent in the workspace, running or not.
    pub(crate) fn agent_ids(&self) -> std::collections::HashSet<String> {
        self.lock()
            .saved
            .agents
            .iter()
            .map(|kept| kept.agent.id.clone())
            .collect()
    }

    /// Keeps a chat that just started among the agents, so every client
    /// lists it whoever started it: a device, an agent, Auto or the app.
    pub(crate) fn note_begun(&self, chat: &chat::Chat) {
        let launch = &chat.launch;
        if let Some(known) = self.agent(chat.agent_id()) {
            if known.asleep {
                self.set_asleep(&known.id, false);
            }
            return;
        }
        let lead = chat
            .origin
            .started_by
            .as_deref()
            .and_then(|lead| self.agent(lead));
        let project = match &lead {
            Some(lead) if launch.cwd.starts_with(&lead.cwd) => lead.project.clone(),
            _ => match self.project_for(&launch.cwd) {
                Ok(project) => project,
                Err(error) => {
                    eprintln!("sikemux core: a chat that started was not kept: {error}");
                    return;
                }
            },
        };
        let profile_id = chat.origin.launcher.as_deref().and_then(|launcher| {
            launcher
                .strip_prefix(&format!("{}:", launch.provider))
                .map(str::to_owned)
        });
        let worktree = lead.as_ref().and_then(|lead| {
            let path = lead.worktree.as_ref()?.get("path")?.as_str()?;
            launch
                .cwd
                .starts_with(path)
                .then(|| lead.worktree.clone())?
        });
        let agent = WorkspaceAgent {
            id: chat.agent_id().to_owned(),
            provider: launch.provider.clone(),
            project,
            cwd: launch.cwd.clone(),
            title: launch.provider.clone(),
            renamed: false,
            lead: lead.map(|lead| lead.id),
            resume_id: launch.resume_id.clone(),
            permission_mode: launch.permission_mode.clone(),
            profile_id,
            model: launch.model.clone(),
            model_provider: launch.env.get(ROUTED_ENV).cloned(),
            effort: launch.effort.clone(),
            keep_alive: false,
            asleep: false,
            worktree,
            buddy: None,
            pty_id: None,
            created_at: now_ms(),
        };
        let Ok(Value::Object(draft)) = serde_json::to_value(agent) else {
            return;
        };
        if let Err(error) = self.put_agent(draft) {
            eprintln!("sikemux core: a chat that started was not kept: {error}");
        }
    }

    /// Keeps how the agent's chat runs as it stops, so it can be woken on its
    /// saved conversation with no app there.
    pub(crate) fn note_stopping(&self, chat: &chat::Chat) {
        let launch = chat.current_now();
        let _ = self.change(|saved| {
            if let Some(kept) = saved.kept_mut(chat.agent_id()) {
                let agent = &mut kept.agent;
                agent.resume_id = launch.resume_id.or(agent.resume_id.take());
                agent.permission_mode = launch.permission_mode;
                agent.model = launch.model;
                agent.effort = launch.effort;
                kept.account = launch.account;
                kept.fallbacks = launch.fallbacks;
            }
            Ok(())
        });
        self.recent_stale();
    }

    /// How the agent's chat was last running, to start it again.
    pub(crate) fn kept_launch(&self, agent_id: &str) -> Option<Kept> {
        self.lock().saved.kept(agent_id).cloned()
    }

    /// The launcher and mode an agent runs with.
    pub(crate) fn run_of(&self, agent_id: &str) -> Option<AgentRun> {
        self.agent(agent_id).map(|agent| AgentRun {
            launcher: launcher_of(&agent),
            permission_mode: agent.permission_mode,
        })
    }

    pub(crate) fn accounts(&self) -> Option<Book> {
        self.lock().saved.accounts.clone()
    }

    pub(crate) fn set_accounts(&self, book: Book) -> CoreResult<()> {
        book.check()?;
        self.change(|saved| {
            saved.accounts = Some(book);
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// The agent `provider` on the person's account in `config_path`.
    pub(crate) fn account_spec(&self, provider: &str, config_path: Option<&str>) -> AgentSpec {
        let published = self.lock();
        match &published.saved.accounts {
            Some(book) => book.spec(provider, config_path),
            None => Book::default().spec(provider, config_path),
        }
    }

    /// The hosted model providers' keys `provider` reads. Reads the Keychain, so it blocks.
    pub(crate) fn provider_keys(&self, provider: &str) -> BTreeMap<String, String> {
        self.providers
            .as_ref()
            .map(|providers| providers.environment(provider))
            .unwrap_or_default()
    }

    /// Each chat agent on each of the person's accounts, in the mode they
    /// start agents in: one launcher per profile, or one for an agent with none.
    fn offered(&self) -> Vec<Launcher> {
        let published = self.lock();
        let saved = &published.saved;
        let profiles = saved
            .accounts
            .as_ref()
            .map(|book| book.profiles.as_slice())
            .unwrap_or_default();
        let mut offered = Vec::new();
        // Pi has no chat mode of its own.
        for def in AGENTS.iter().filter(|def| def.kind != "pi") {
            let permission_mode = match saved.permission_mode.as_deref() {
                Some(mode) if def.bypass => mode.to_owned(),
                None if def.bypass => "bypass".to_owned(),
                _ => "workspace-write".to_owned(),
            };
            let launcher = |id: String, label: String, agent: AgentSpec| Launcher {
                id,
                label,
                agent,
                permission_mode: permission_mode.clone(),
                status: None,
                program: None,
            };
            let own: Vec<_> = profiles
                .iter()
                .filter(|profile| profile.provider == def.kind)
                .collect();
            if own.is_empty() {
                let agent = AgentSpec::new(def.kind);
                offered.push(launcher(def.kind.into(), def.label.into(), agent));
            }
            for profile in &own {
                let label = match own.len() {
                    1 => def.label.to_owned(),
                    _ => format!("{} · {}", def.label, profile.name),
                };
                let id = format!("{}:{}", def.kind, profile.id);
                offered.push(launcher(id, label, profile.spec()));
            }
        }
        offered.truncate(MAX_LAUNCHERS);
        offered
    }

    fn set_launchers(&self, launchers: Vec<Launcher>) {
        self.lock().launchers = launchers;
        self.wake.now();
    }

    /// Keeps what the core last heard of an account's sign-in on the
    /// launchers that start on it.
    pub(crate) fn note_status(
        &self,
        provider: &str,
        config_path: Option<&str>,
        status: &AgentStatus,
    ) {
        let mut published = self.lock();
        let on_account = published.launchers.iter_mut().filter(|launcher| {
            launcher.agent.provider == provider
                && launcher.agent.config_path.as_deref() == config_path
        });
        for launcher in on_account {
            launcher.status = Some(status.state().to_owned());
        }
    }

    /// What `agent` starts with to run on `provider`, its keys read from the
    /// Keychain. Blocks.
    pub(crate) fn wiring(
        &self,
        agent: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Wiring, String> {
        self.providers
            .as_ref()
            .ok_or("Sikemux keeps no provider keys here")?
            .wiring(agent, provider, model)
    }

    /// What helpers run on, as the person set it in the app.
    pub(crate) fn helper(&self) -> Option<Helper> {
        self.providers.as_ref().and_then(Providers::helper)
    }

    /// A chat of `launcher` running `program` in `cwd`, built afresh, for an
    /// agent not yet named, on `provider` when one is named, and on `model`
    /// or the provider's coding model. Reads the providers' keys from the
    /// Keychain, so it blocks.
    pub(crate) fn chat_launch(
        &self,
        launcher: &Launcher,
        program: PathBuf,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
    ) -> Result<ChatLaunch, String> {
        let agent = &launcher.agent;
        let model = model.or_else(|| {
            self.providers
                .as_ref()?
                .coding_model(provider?, &agent.provider)
        });
        let providers = match &self.providers {
            Some(providers) => providers.wiring(&agent.provider, provider, model.as_deref())?,
            None if provider.is_some() => return Err("Sikemux keeps no provider keys here".into()),
            None => Wiring::default(),
        };
        let skills = self.data_dir.as_deref().and_then(skills::ready);
        let built = agent.chat(program, providers, skills);
        Ok(ChatLaunch {
            agent_id: String::new(),
            provider: agent.provider.clone(),
            cwd: cwd.to_path_buf(),
            program: built.program,
            args: built.args,
            env: built.env,
            mcp_servers: Vec::new(),
            resume_id: None,
            permission_mode: launcher.permission_mode.clone(),
            model,
            effort: None,
            account: None,
            fallbacks: Vec::new(),
        })
    }

    pub(crate) fn project_info(&self, id: &str) -> Option<ProjectInfo> {
        self.lock().saved.project(id).cloned()
    }

    pub(crate) fn project_path(&self, id: &str) -> CoreResult<PathBuf> {
        self.lock()
            .saved
            .project(id)
            .map(|project| project.path.clone())
            .ok_or_else(|| CoreError::from("that project is not on this host"))
    }

    pub(crate) fn launcher(&self, id: &str) -> CoreResult<Launcher> {
        self.lock()
            .launchers
            .iter()
            .find(|launcher| launcher.id == id)
            .cloned()
            .ok_or_else(|| {
                CoreError::from("that agent is not one Sikemux on this host can start; open Sikemux on the host once so it can say which it can")
            })
    }

    pub(crate) fn publish_backdrop(
        &self,
        texture: bool,
        image: Option<BackdropImage>,
    ) -> CoreResult<()> {
        if let Some(image) = &image {
            if !image.data_url.starts_with("data:image/") {
                return Err("the backdrop picture is not an image".into());
            }
            if image.data_url.len() > MAX_IMAGE_BYTES {
                return Err("the backdrop picture is larger than the core keeps".into());
            }
        }
        let mut published = self.lock();
        published.texture = texture;
        published.image = image;
        Ok(())
    }

    pub(crate) fn backdrop_image(&self) -> Option<String> {
        self.lock()
            .image
            .as_ref()
            .map(|image| image.data_url.clone())
    }

    pub(crate) fn publish_palette(&self, palette: BTreeMap<String, String>) -> CoreResult<()> {
        let oversized = palette
            .iter()
            .any(|(name, value)| name.len() > MAX_COLOUR_CHARS || value.len() > MAX_COLOUR_CHARS);
        if palette.len() > MAX_COLOURS || oversized {
            return Err("the app published more theme colours than the core keeps".into());
        }
        self.lock().palette = palette;
        Ok(())
    }

    pub(crate) fn publish_buddies(&self, buddies: Option<Buddies>) -> CoreResult<()> {
        if buddies.as_ref().is_some_and(Buddies::oversized) {
            return Err("the app published more buddies than the core keeps".into());
        }
        self.lock().buddies = buddies;
        Ok(())
    }

    pub(crate) fn buddies(&self) -> Option<Buddies> {
        self.lock().buddies.clone()
    }

    /// The recent chats a device may take up again: those in a project on
    /// this host, leaving out any running now.
    pub(crate) fn recent(&self, open: &[ChatInfo]) -> Vec<RecentInfo> {
        let published = self.lock();
        let saved = &published.saved;
        published
            .recent
            .iter()
            .filter(|chat| !running_now(chat, open))
            .filter_map(|chat| {
                let project = saved
                    .projects
                    .iter()
                    .find(|project| project.path == chat.cwd)?;
                Some(RecentInfo {
                    id: recent_id(chat),
                    provider: chat.provider.clone(),
                    title: chat.title.clone(),
                    project: project.id.clone(),
                    cwd: chat.cwd.clone(),
                    active_at: chat.active_at,
                })
            })
            .collect()
    }

    /// What to look for recent chats with: each launcher's account once, the
    /// projects, and the sessions agents here already hold.
    fn recent_requests(&self) -> Vec<(String, RecentRequest)> {
        let published = self.lock();
        let saved = &published.saved;
        let projects: Vec<String> = saved
            .projects
            .iter()
            .take(saved_chats::MAX_PROJECTS)
            .map(|project| project.path.to_string_lossy().into_owned())
            .collect();
        let selected = saved.accounts.as_ref().map(|book| &book.selected);
        let mut requests: Vec<(String, RecentRequest)> = Vec::new();
        for launcher in &published.launchers {
            let Some(agent) = AgentKind::from_name(&launcher.agent.provider) else {
                continue;
            };
            let chosen = selected
                .and_then(|selected| selected.get(&launcher.agent.provider))
                .is_none_or(|profile| launcher.id.ends_with(&format!(":{profile}")));
            let taken = requests
                .iter()
                .any(|(_, request)| request.providers[0].agent == agent);
            if !chosen || taken {
                continue;
            }
            let exclude = saved
                .agents
                .iter()
                .filter(|kept| kept.agent.provider == launcher.agent.provider)
                .filter_map(|kept| kept.agent.resume_id.clone())
                .map(|id| RecentRef { agent, id })
                .collect();
            requests.push((
                launcher.id.clone(),
                RecentRequest {
                    providers: vec![RecentProvider {
                        agent,
                        config_path: launcher.agent.config_path.clone(),
                    }],
                    projects: projects.clone(),
                    limit: RECENT_LIMIT,
                    cursor: None,
                    query: None,
                    exclude,
                },
            ));
        }
        requests
    }

    fn set_recent(&self, recent: Vec<Recent>) -> bool {
        let mut published = self.lock();
        let changed = published.recent != recent;
        published.recent = recent;
        changed
    }

    /// How to start the recent chat a device chose, the way it would start a new one.
    pub(crate) fn resume_choice(
        &self,
        choice: ResumeChoice,
        open: &[ChatInfo],
    ) -> CoreResult<ChatChoice> {
        let published = self.lock();
        let saved = &published.saved;
        let chat = published
            .recent
            .iter()
            .find(|chat| recent_id(chat) == choice.recent)
            .ok_or_else(|| {
                CoreError::from("that chat is no longer among the recent ones on this host")
            })?;
        if running_now(chat, open) {
            return Err("that chat is already open on this host".into());
        }
        let project = saved
            .projects
            .iter()
            .find(|project| project.path == chat.cwd)
            .ok_or_else(|| CoreError::from("that chat's project is not on this host"))?;
        let launcher = published
            .launchers
            .iter()
            .find(|launcher| {
                launcher.id == chat.launcher && launcher.agent.provider == chat.provider
            })
            .ok_or_else(|| {
                CoreError::from("Sikemux on this host can no longer start that chat's agent")
            })?;
        Ok(ChatChoice {
            launcher: launcher.id.clone(),
            project: project.id.clone(),
            permission_mode: choice.permission_mode,
            model: choice.model,
            effort: choice.effort,
            resume_id: Some(chat.session_id.clone()),
        })
    }

    pub(crate) fn project_paths(&self) -> Vec<PathBuf> {
        self.lock()
            .saved
            .projects
            .iter()
            .map(|project| project.path.clone())
            .collect()
    }

    /// What the person or the agent named an agent, when it has a name beyond its CLI's.
    pub(crate) fn title(&self, agent_id: &str) -> Option<String> {
        self.agent(agent_id).as_ref().and_then(named)
    }

    /// Keeps the models and effort levels a provider's session offered, so a
    /// device choosing them before it starts one sees the same.
    pub(crate) fn note_config_options(&self, provider: &str, setup: &Value) {
        if let Some(options) = setup
            .get("configOptions")
            .filter(|options| options.is_array())
        {
            self.lock()
                .config_options
                .insert(provider.to_owned(), options.clone());
        }
    }

    /// The running chats under the workspace's titles, which include the
    /// agent's own unless the person renamed it, then the chat agents that are
    /// not running.
    pub(crate) fn listed(&self, mut running: Vec<ChatInfo>) -> Vec<ChatInfo> {
        let published = self.lock();
        let agents = &published.saved.agents;
        for chat in &mut running {
            let title = agents
                .iter()
                .find(|kept| kept.agent.id == chat.agent_id)
                .and_then(|kept| named(&kept.agent));
            if title.is_some() {
                chat.title = title;
            }
        }
        let stopped: Vec<ChatInfo> = agents
            .iter()
            .map(|kept| &kept.agent)
            .filter(|agent| is_chat_provider(&agent.provider) && agent.pty_id.is_none())
            .filter(|agent| !running.iter().any(|live| live.agent_id == agent.id))
            .map(|agent| ChatInfo {
                agent_id: agent.id.clone(),
                name: None,
                provider: agent.provider.clone(),
                title: named(agent),
                cwd: agent.cwd.clone(),
                session_id: None,
                state: ChatState::Stopped,
                running: false,
                pending_permissions: Vec::new(),
                subagents: 0,
                tasks: 0,
                reconnecting: None,
                started_by: None,
                launcher: None,
                permission_mode: String::new(),
                model: None,
                model_provider: None,
                effort: None,
                asleep: agent.asleep,
                unread: false,
                active_at: None,
                group: Default::default(),
                status: Default::default(),
                attention_rank: 0,
            })
            .collect();
        running.extend(stopped);
        running
    }

    /// Whether the agent is no longer in the workspace.
    pub(crate) fn closed(&self, agent_id: &str) -> bool {
        self.agent(agent_id).is_none()
    }

    /// Whether the agent is in the workspace, and if so whether it is asleep.
    pub(crate) fn published(&self, agent_id: &str) -> Option<bool> {
        self.agent(agent_id).map(|agent| agent.asleep)
    }

    pub(crate) fn view(&self) -> Workspace {
        let published = self.lock();
        let saved = &published.saved;
        Workspace {
            projects: saved.projects.clone(),
            spaces: saved.spaces.clone(),
            agents: saved.agents.iter().map(|kept| kept.agent.clone()).collect(),
            pinned: saved.pinned.clone(),
            permission_mode: saved
                .permission_mode
                .clone()
                .unwrap_or_else(|| "bypass".into()),
            disabled_plugins: super::plugins::disabled(),
            launchers: published
                .launchers
                .iter()
                .map(|launcher| {
                    let config_options = published
                        .config_options
                        .get(&launcher.agent.provider)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let (model, effort) = pickers::pickers(&config_options);
                    LauncherInfo {
                        id: launcher.id.clone(),
                        provider: launcher.agent.provider.clone(),
                        label: launcher.label.clone(),
                        permission_mode: launcher.permission_mode.clone(),
                        config_options,
                        status: launcher.status.clone(),
                        model,
                        effort,
                    }
                })
                .collect(),
            palette: published.palette.clone(),
            backdrop: Backdrop {
                texture: published.texture,
                image: published.image.as_ref().map(|image| image.id.clone()),
            },
        }
    }
}

/// How a lead runs, for an agent it starts to run the same way.
pub(crate) struct Lead {
    pub provider: String,
    /// Its launch, when it is a chat the core runs.
    pub launch: Option<ChatLaunch>,
    pub launcher: Option<String>,
    pub permission_mode: Option<String>,
}

impl Lead {
    /// The permission mode a worker of the lead running `provider` takes, never
    /// wider than the lead's, given `default` when the lead's is unknown.
    pub(crate) fn worker_mode(&self, provider: &str, default: &str) -> String {
        match self.permission_mode.as_deref() {
            Some(mode) if self.provider == provider => mode.to_owned(),
            Some("bypass") => "bypass".to_owned(),
            Some(_) => "workspace-write".to_owned(),
            None if matches!(default, "bypass" | "full-access") => "workspace-write".to_owned(),
            None => default.to_owned(),
        }
    }
}

/// Who pays for a worker's turns.
pub(crate) enum Paid {
    /// Whoever pays for its lead's, when it runs the lead's agent.
    AsLead,
    /// The provider named, or the agent's own account.
    By(Option<String>),
}

/// Agents whose chats keep a private home of their own, which a worker of
/// theirs must not share.
const OWN_HOME: [&str; 2] = ["hermes", "grok"];

impl Workspaces {
    /// How to start an agent of `provider` for `lead` in `cwd`: on the lead's
    /// account and in its permission mode, paid as `paid` says. The lead may
    /// pick the model.
    pub(crate) fn worker_launch(
        &self,
        lead: &Lead,
        provider: &str,
        cwd: &Path,
        model: Option<String>,
        paid: Paid,
    ) -> Result<(ChatLaunch, Option<String>), String> {
        let same = lead.provider == provider;
        let mode = |default: &str| lead.worker_mode(provider, default);
        let lead_own = lead.launch.as_ref().filter(|_| same);
        let lead_route = lead_own.and_then(|own| own.env.get(ROUTED_ENV).cloned());
        let route = match paid {
            Paid::AsLead => lead_route.clone(),
            Paid::By(route) => route,
        };
        let as_lead = route == lead_route;
        let own = lead_own.filter(|_| as_lead && !OWN_HOME.contains(&provider));
        if let Some(own) = own {
            let mut launch = own.clone();
            launch.cwd = cwd.to_path_buf();
            launch.resume_id = None;
            launch.permission_mode = mode(&own.permission_mode);
            launch.model = model.or(launch.model);
            return Ok((launch, lead.launcher.clone()));
        }
        let launcher = {
            let published = self.lock();
            let launchers = &published.launchers;
            let offered: Vec<&Launcher> = launchers
                .iter()
                .filter(|launcher| launcher.agent.provider == provider)
                .collect();
            let lead_own = offered
                .iter()
                .find(|launcher| Some(&launcher.id) == lead.launcher.as_ref());
            if same && lead_own.is_none() && offered.len() > 1 {
                return Err(format!(
                    "Sikemux cannot tell which {provider} account you run on, so it will not start one on another; pick another cli or ask the person to start it"
                ));
            }
            let launcher = lead_own.or_else(|| offered.first()).ok_or_else(|| {
                let known: Vec<&str> = launchers
                    .iter()
                    .map(|launcher| launcher.agent.provider.as_str())
                    .collect();
                if known.is_empty() {
                    "Sikemux has not said which agents it can start; open Sikemux once so it can"
                        .to_owned()
                } else {
                    format!("cli must be one of {}", known.join(", "))
                }
            })?;
            (*launcher).clone()
        };
        match launcher.status.as_deref() {
            Some("missing") => {
                return Err(format!("{} is not installed on this host", launcher.label))
            }
            Some("signedOut") => {
                return Err(format!(
                    "{} is signed out on this host; ask the person to sign in",
                    launcher.label
                ))
            }
            Some("broken") => {
                return Err(format!("{} does not start on this host", launcher.label))
            }
            _ => {}
        }
        let model = model.or_else(|| {
            lead_own
                .filter(|_| as_lead)
                .and_then(|own| own.model.clone())
        });
        let mut launch = self.chat_launch(
            &launcher,
            still_there(&launcher)?,
            cwd,
            route.as_deref(),
            model,
        )?;
        launch.permission_mode = mode(&launcher.permission_mode);
        if let Some(own) = lead_own {
            launch.account = own.account.clone();
            launch.fallbacks = own.fallbacks.clone();
        }
        Ok((launch, Some(launcher.id)))
    }
}

impl Workspaces {
    pub(crate) fn has_launchers(&self) -> bool {
        !self.lock().launchers.is_empty()
    }

    /// How Auto starts an agent of its own in `cwd`: with the launcher the
    /// project names, else the first the app offers that works, in that
    /// launcher's own permission mode. Reads the Keychain, so it blocks.
    pub(crate) fn auto_launch(
        &self,
        launcher: Option<&str>,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
    ) -> Result<(ChatLaunch, String), String> {
        let chosen = {
            let published = self.lock();
            let launchers = &published.launchers;
            let usable = |launcher: &&Launcher| {
                !matches!(
                    launcher.status.as_deref(),
                    Some("missing" | "signedOut" | "broken")
                )
            };
            match launcher {
                Some(id) => launchers
                    .iter()
                    .find(|launcher| launcher.id == id)
                    .ok_or_else(|| {
                        format!("Sikemux can no longer start {id}; pick another agent in the board's Auto settings")
                    })?,
                None => launchers.iter().find(usable).ok_or(
                    "Sikemux has not said which agents it can start; open Sikemux once so it can",
                )?,
            }
            .clone()
        };
        match chosen.status.as_deref() {
            Some("missing") => return Err(format!("{} is not installed", chosen.label)),
            Some("signedOut") => return Err(format!("{} is signed out", chosen.label)),
            Some("broken") => return Err(format!("{} does not start", chosen.label)),
            _ => {}
        }
        let launch = self.chat_launch(&chosen, still_there(&chosen)?, cwd, provider, model)?;
        Ok((launch, chosen.id))
    }

    /// The CLI the app's launcher `id` runs, such as `claude`.
    pub(crate) fn launcher_cli(&self, id: &str) -> Option<String> {
        self.lock()
            .launchers
            .iter()
            .find(|launcher| launcher.id == id)
            .map(|launcher| launcher.agent.provider.clone())
    }

    /// The launcher a reviewer starts with: `chosen`, else the first signed
    /// in that runs another CLI than `builder`. None when there is none.
    pub(crate) fn reviewer_launcher(
        &self,
        chosen: Option<&str>,
        builder: Option<&str>,
    ) -> Result<Option<String>, String> {
        let published = self.lock();
        let other = |launcher: &&Launcher| builder != Some(launcher.agent.provider.as_str());
        let Some(id) = chosen else {
            return Ok(published
                .launchers
                .iter()
                .filter(other)
                .find(|launcher| launcher.status.as_deref() == Some("ready"))
                .map(|launcher| launcher.id.clone()));
        };
        let launcher = published
            .launchers
            .iter()
            .find(|launcher| launcher.id == id)
            .ok_or_else(|| {
                format!("Sikemux can no longer start the reviewer {id}; pick another in the board's Auto settings")
            })?;
        if !other(&launcher) {
            return Err(format!(
                "The reviewer chosen in Auto's settings, {}, runs the same CLI as the card's agent; pick one on another",
                launcher.label
            ));
        }
        Ok(Some(launcher.id.clone()))
    }
}

/// The program the core last found for `launcher`, while it is still there.
fn still_there(launcher: &Launcher) -> Result<PathBuf, String> {
    launcher
        .program
        .clone()
        .filter(|program| program.is_file())
        .ok_or_else(|| format!("{} is no longer installed on this host", launcher.label))
}

#[derive(Deserialize)]
pub(crate) struct AddProjectParams {
    path: PathBuf,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

/// Makes a folder on this host a project, so agents can be started in it from
/// any device, with or without the app.
pub(crate) async fn add_project(
    core: &Arc<Core>,
    params: AddProjectParams,
) -> CoreResult<ProjectInfo> {
    if !params.path.is_absolute() {
        return Err("a project's folder must be an absolute path".into());
    }
    let path = params.path;
    let path = blocking(move || {
        let path = std::fs::canonicalize(&path)
            .map_err(|error| CoreError::from(format!("{}: {error}", path.display())))?;
        if path.is_dir() {
            Ok(path)
        } else {
            Err(format!("{} is not a folder", path.display()).into())
        }
    })
    .await?;
    let project = core.workspaces.add_project(path, params.name, params.id)?;
    super::git::follow_projects(core);
    core.publish_device_view();
    Ok(project)
}

#[derive(Deserialize)]
pub(crate) struct RemoveProjectParams {
    path: PathBuf,
}

/// Stops offering a project, and closes its agents.
pub(crate) fn remove_project(core: &Core, params: RemoveProjectParams) -> CoreResult<()> {
    let before = core.workspaces.agent_ids();
    core.workspaces.remove_project(&params.path)?;
    agents_closed(core, before);
    super::git::follow_projects(core);
    core.publish_device_view();
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct UpdateProjectParams {
    id: String,
    patch: Map<String, Value>,
}

/// Renames, pins or moves a project to a space.
pub(crate) fn update_project(core: &Core, params: UpdateProjectParams) -> CoreResult<ProjectInfo> {
    let project = core.workspaces.update_project(&params.id, params.patch)?;
    core.publish_device_view();
    Ok(project)
}

#[derive(Deserialize)]
pub(crate) struct MoveProjectParams {
    id: String,
    index: usize,
}

pub(crate) fn move_project(core: &Core, params: MoveProjectParams) -> CoreResult<()> {
    core.workspaces.move_project(&params.id, params.index)?;
    core.publish_device_view();
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct SpacesParams {
    spaces: Vec<SpaceInfo>,
}

pub(crate) fn set_spaces(core: &Core, params: SpacesParams) -> CoreResult<()> {
    core.workspaces.set_spaces(params.spaces)?;
    core.publish_device_view();
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct PinnedParams {
    chats: Vec<PinnedChat>,
}

pub(crate) fn set_pinned_chats(core: &Core, params: PinnedParams) -> CoreResult<()> {
    core.workspaces.set_pinned(params.chats)?;
    core.publish_device_view();
    Ok(())
}

fn is_variable_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderSettingsParams {
    project: String,
    folder: String,
    settings: FolderSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRulesParams {
    project: String,
    rules: ProjectRules,
}

pub(crate) fn set_project_rules(core: &Core, params: ProjectRulesParams) -> CoreResult<()> {
    core.workspaces
        .set_project_rules(params.project, params.rules)
}

pub(crate) fn set_folder_settings(core: &Core, params: FolderSettingsParams) -> CoreResult<()> {
    core.workspaces
        .set_folder_settings(params.project, params.folder, params.settings)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DefaultsParams {
    permission_mode: String,
}

/// Keeps the mode agents start in, and offers the launchers in it.
pub(crate) async fn set_defaults(core: &Arc<Core>, params: DefaultsParams) -> CoreResult<()> {
    if core
        .workspaces
        .set_permission_mode(params.permission_mode)?
    {
        refresh_launchers(core.clone()).await;
    }
    core.publish_device_view();
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct CreateAgentParams {
    agent: Map<String, Value>,
}

/// Adds an agent to the workspace, or replaces the one with its id.
pub(crate) fn create_agent(core: &Core, params: CreateAgentParams) -> CoreResult<WorkspaceAgent> {
    let agent = core.workspaces.put_agent(params.agent)?;
    core.publish_device_view();
    Ok(agent)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateAgentParams {
    agent_id: String,
    patch: Map<String, Value>,
}

/// Changes some of an agent's fields; a null one is cleared.
pub(crate) fn update_agent(core: &Core, params: UpdateAgentParams) -> CoreResult<WorkspaceAgent> {
    let agent = core
        .workspaces
        .update_agent(&params.agent_id, params.patch)?;
    core.publish_device_view();
    Ok(agent)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloseAgentsParams {
    agent_ids: Vec<String>,
}

/// Takes agents out of the workspace.
pub(crate) fn close_agents(core: &Core, params: CloseAgentsParams) -> CoreResult<()> {
    let before = core.workspaces.agent_ids();
    core.workspaces.close_agents(&params.agent_ids)?;
    agents_closed(core, before);
    core.publish_device_view();
    Ok(())
}

/// Titles the agent from what its chat said, as [`Workspaces::note_title`]
/// does, and shows every client.
pub(crate) fn retitle(core: &Core, agent_id: &str, title: Option<String>, stand_in: bool) {
    let Some(title) = title else {
        return;
    };
    if core.workspaces.note_title(agent_id, &title, stand_in) {
        core.publish_device_view();
    }
}

/// Takes out of the workspace agents that have nothing left to show.
pub(crate) fn close_gone(core: &Core, agent_ids: &[String]) {
    let before = core.workspaces.agent_ids();
    if core.workspaces.close_agents(agent_ids).is_ok() {
        agents_closed(core, before);
        core.publish_device_view();
    }
}

/// What follows agents leaving the workspace, however they left: their chats
/// stop, and their names are free again.
fn agents_closed(core: &Core, before: HashSet<String>) {
    let kept = core.workspaces.agent_ids();
    for agent_id in before.difference(&kept) {
        core.chats.stop(agent_id);
    }
    core.names.set_in_app(kept, core.running_agents());
}

/// Runs whenever the workspace changes: agents in it keep their names while
/// nothing of them runs.
pub(crate) fn settle(core: &Arc<Core>) {
    core.names
        .set_in_app(core.workspaces.agent_ids(), core.running_agents());
    super::session_watch::follow(core);
    core.agents_changed();
}

/// Looks for the newest saved chats whenever they may have changed, for
/// devices to take up again.
pub(crate) async fn recents(core: Arc<Core>) {
    loop {
        tokio::time::sleep(RECENT_SETTLE).await;
        let requests = core.workspaces.recent_requests();
        let found = blocking(move || Ok(newest(requests))).await;
        if let Ok(found) = found {
            if core.workspaces.set_recent(found) {
                core.publish_device_view();
            }
        }
        core.workspaces.recent_stale.notified().await;
    }
}

fn newest(requests: Vec<(String, RecentRequest)>) -> Vec<Recent> {
    let mut found: Vec<Recent> = requests
        .into_iter()
        .flat_map(|(launcher, request)| {
            let page = saved_chats::recent_page(request).ok()?;
            Some(page.sessions.into_iter().map(move |session| Recent {
                launcher: launcher.clone(),
                provider: session.agent.to_owned(),
                session_id: session.id,
                title: session.title.chars().take(MAX_TITLE_CHARS).collect(),
                cwd: session.cwd.unwrap_or(session.project).into(),
                active_at: session.mtime * 1000,
            }))
        })
        .flatten()
        .collect();
    found.sort_by_key(|chat| std::cmp::Reverse(chat.active_at));
    found.truncate(RECENT_LIMIT);
    found
}

/// Finds each offered agent's program and offers those found, with what
/// the core last heard of their sign-in. Runs as the core starts and when the
/// person's accounts or their mode change, or they may have installed an agent.
pub(crate) fn refresh_launchers(core: Arc<Core>) -> impl std::future::Future<Output = ()> {
    let asked = core.workspaces.asked.fetch_add(1, Ordering::SeqCst) + 1;
    async move { refresh(core, asked).await }
}

async fn refresh(core: Arc<Core>, asked: u64) {
    let looking: Vec<_> = core
        .workspaces
        .offered()
        .into_iter()
        .map(|launcher| {
            tokio::spawn(async move {
                let program = status::find(&launcher.agent).await.ok()?;
                let agent = &launcher.agent;
                let status = status::cached(&agent.provider, agent.config_path.as_deref());
                Some(Launcher {
                    status: status.map(|status| status.state().to_owned()),
                    program: Some(program),
                    ..launcher
                })
            })
        })
        .collect();
    let mut found = Vec::with_capacity(looking.len());
    for looked in looking {
        found.extend(looked.await.ok().flatten());
    }
    if core.workspaces.asked.load(Ordering::SeqCst) == asked {
        core.workspaces.set_launchers(found);
        core.publish_device_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher() -> Launcher {
        Launcher {
            id: "claude:work".into(),
            label: "Claude Code (work)".into(),
            agent: AgentSpec {
                config_path: Some("/Users/me/.claude-work".into()),
                ..AgentSpec::new("claude")
            },
            permission_mode: "bypass".into(),
            status: None,
            program: Some("/usr/local/bin/claude".into()),
        }
    }

    fn offer(workspaces: &Workspaces, launchers: Vec<Launcher>) {
        let project = project();
        workspaces
            .add_project(project.path, Some(project.name), Some(project.id))
            .unwrap();
        workspaces.set_launchers(launchers);
    }

    fn launch(workspaces: &Workspaces, choice: ChatChoice) -> CoreResult<ChatLaunch> {
        let launcher = workspaces.launcher(&choice.launcher)?;
        let cwd = workspaces.project_path(&choice.project)?;
        let program = launcher.program.clone().unwrap_or_default();
        let mut launch = workspaces.chat_launch(&launcher, program, &cwd, None, None)?;
        launch.resume_id = choice.resume_id;
        launch.model = choice.model;
        Ok(launch)
    }

    fn project() -> ProjectInfo {
        ProjectInfo {
            id: "sess-1".into(),
            name: "sikemux".into(),
            path: "/Users/me/sikemux".into(),
            pinned: false,
            space: None,
            code_hosts: Default::default(),
        }
    }

    fn choice() -> ChatChoice {
        ChatChoice {
            launcher: "claude:work".into(),
            project: "sess-1".into(),
            permission_mode: None,
            model: Some("opus".into()),
            effort: None,
            resume_id: None,
        }
    }

    fn recent(session_id: &str, cwd: &str) -> Recent {
        Recent {
            launcher: "claude:work".into(),
            provider: "claude".into(),
            session_id: session_id.into(),
            title: "Fix the login flake".into(),
            cwd: cwd.into(),
            active_at: 1_000,
        }
    }

    fn resuming(recent: &str) -> ResumeChoice {
        ResumeChoice {
            recent: recent.into(),
            permission_mode: None,
            model: None,
            effort: None,
        }
    }

    #[test]
    fn a_device_sees_launchers_without_their_program_or_environment() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        let shown = serde_json::to_string(&workspaces.view()).unwrap();
        assert!(shown.contains("Claude Code (work)"));
        assert!(!shown.contains("/usr/local/bin/claude"));
        assert!(!shown.contains(".claude-work"));
    }

    #[test]
    fn a_device_sees_whether_each_launcher_is_signed_in() {
        let workspaces = Workspaces::new(None);
        let signed_out = Launcher {
            status: Some("signedOut".into()),
            ..launcher()
        };
        offer(&workspaces, vec![signed_out]);
        let view = workspaces.view();
        assert_eq!(view.launchers[0].status.as_deref(), Some("signedOut"));
        offer(&workspaces, vec![launcher()]);
        let shown = serde_json::to_value(workspaces.view()).unwrap();
        assert!(shown["launchers"][0].get("status").is_none());
    }

    #[test]
    fn a_chat_starts_in_the_project_with_the_launcher_and_its_account() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        let launch = launch(&workspaces, choice()).unwrap();
        assert_eq!(launch.cwd, PathBuf::from("/Users/me/sikemux"));
        assert_eq!(launch.program, PathBuf::from("/usr/local/bin/claude"));
        assert_eq!(launch.permission_mode, "bypass");
        assert_eq!(launch.model.as_deref(), Some("opus"));
        assert_eq!(
            launch.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/Users/me/.claude-work")
        );
    }

    #[test]
    fn an_unknown_launcher_or_project_is_refused() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        let mut wrong = choice();
        wrong.project = "elsewhere".into();
        assert!(launch(&workspaces, wrong).is_err());
        let mut wrong = choice();
        wrong.launcher = "gemini".into();
        assert!(launch(&workspaces, wrong).is_err());
    }

    #[test]
    fn a_device_sees_recent_chats_in_open_projects_but_not_running_ones() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        workspaces.set_recent(vec![
            recent("s1", "/Users/me/sikemux"),
            recent("s2", "/Users/me/sikemux"),
            recent("s3", "/Users/me/closed"),
        ]);
        let running = workspaces.listed(Vec::new());
        let shown = workspaces.recent(&running);
        assert_eq!(
            shown
                .iter()
                .map(|chat| chat.id.as_str())
                .collect::<Vec<_>>(),
            ["claude:s1", "claude:s2"]
        );
        assert_eq!(shown[0].project, "sess-1");
        assert!(!serde_json::to_string(&shown)
            .unwrap()
            .contains("claude:work"));

        let mut live = ChatInfo {
            agent_id: "agent-1".into(),
            name: None,
            provider: "claude".into(),
            title: None,
            cwd: "/Users/me/sikemux".into(),
            session_id: Some("s1".into()),
            state: ChatState::Ready,
            running: false,
            pending_permissions: Vec::new(),
            subagents: 0,
            tasks: 0,
            reconnecting: None,
            started_by: None,
            launcher: None,
            permission_mode: String::new(),
            model: None,
            model_provider: None,
            effort: None,
            asleep: false,
            unread: false,
            active_at: None,
            group: Default::default(),
            status: Default::default(),
            attention_rank: 0,
        };
        assert_eq!(workspaces.recent(std::slice::from_ref(&live)).len(), 1);
        live.provider = "codex".into();
        assert_eq!(workspaces.recent(&[live]).len(), 2);
    }

    #[test]
    fn a_recent_chat_resumes_with_its_launcher_in_its_project() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        workspaces.set_recent(vec![recent("s1", "/Users/me/sikemux")]);
        let choice = workspaces
            .resume_choice(resuming("claude:s1"), &[])
            .unwrap();
        let launch = launch(&workspaces, choice).unwrap();
        assert_eq!(launch.resume_id.as_deref(), Some("s1"));
        assert_eq!(launch.cwd, PathBuf::from("/Users/me/sikemux"));
        assert_eq!(launch.provider, "claude");
        assert_eq!(launch.permission_mode, "bypass");
    }

    #[test]
    fn only_a_published_recent_chat_resumes() {
        let workspaces = Workspaces::new(None);
        offer(&workspaces, vec![launcher()]);
        workspaces.set_recent(vec![recent("s1", "/Users/me/sikemux")]);
        assert!(workspaces
            .resume_choice(resuming("claude:other"), &[])
            .is_err());
        assert!(workspaces.resume_choice(resuming("s1"), &[]).is_err());
        workspaces.set_recent(Vec::new());
        assert!(workspaces
            .resume_choice(resuming("claude:s1"), &[])
            .is_err());
        let mut elsewhere = recent("s2", "/Users/me/sikemux");
        elsewhere.launcher = "codex".into();
        workspaces.set_recent(vec![elsewhere]);
        assert!(workspaces
            .resume_choice(resuming("claude:s2"), &[])
            .is_err());
    }

    #[test]
    fn a_relative_project_path_is_refused() {
        let workspaces = Workspaces::new(None);
        assert!(workspaces
            .add_project("sikemux".into(), None, None)
            .is_err());
    }

    #[test]
    fn a_first_prompt_names_an_agent_until_its_chat_titles_it_and_never_one_the_person_named() {
        let workspaces = Workspaces::new(None);
        let project = project();
        workspaces
            .add_project(
                project.path.clone(),
                Some(project.name),
                Some(project.id.clone()),
            )
            .unwrap();
        let agent = |id: &str| {
            let agent = serde_json::json!({
                "id": id, "provider": "claude", "project": project.id, "cwd": project.path,
                "title": "claude", "permissionMode": "bypass",
            });
            workspaces
                .put_agent(agent.as_object().unwrap().clone())
                .unwrap();
        };
        let title = |id: &str| workspaces.agent(id).unwrap().title;
        agent("agent-1");
        assert!(!workspaces.note_title("agent-1", "/compact", true));
        let long = format!(
            "Need the binary size on the landing page {}",
            "x".repeat(80)
        );
        assert!(workspaces.note_title("agent-1", &long, true));
        assert_eq!(title("agent-1").chars().count(), STAND_IN_TITLE_CHARS);
        assert!(!workspaces.note_title("agent-1", "A second prompt", true));
        assert!(workspaces.note_title("agent-1", "Binary size on landing page", false));
        assert_eq!(title("agent-1"), "Binary size on landing page");

        agent("agent-2");
        let mut renamed = Map::new();
        renamed.insert("title".into(), "Parser rewrite".into());
        renamed.insert("renamed".into(), true.into());
        workspaces.update_agent("agent-2", renamed).unwrap();
        assert!(!workspaces.note_title("agent-2", "Fix the parser", true));
        assert!(!workspaces.note_title("agent-2", "Parser fixes", false));
        assert_eq!(title("agent-2"), "Parser rewrite");
    }

    #[test]
    fn each_chat_agent_is_offered_once_or_once_per_profile_in_a_mode_it_supports() {
        let profile = |id: &str, name: &str, provider: &str| crate::agent_accounts::Profile {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            accent: "#fff".into(),
            executable_path: None,
            config_path: Some(format!("~/.{id}")),
            environment_keys: Vec::new(),
        };
        let workspaces = Workspaces::new(None);
        let profiles = vec![
            profile("work", "Work", "claude"),
            profile("home", "Home", "claude"),
            profile("cx", "Default", "codex"),
        ];
        workspaces
            .set_accounts(Book {
                profiles,
                ..Book::default()
            })
            .unwrap();
        workspaces.set_permission_mode("bypass".into()).unwrap();
        let offered = workspaces.offered();
        let ids: Vec<&str> = offered
            .iter()
            .map(|launcher| launcher.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "claude:work",
                "claude:home",
                "codex:cx",
                "hermes",
                "opencode",
                "omp",
                "grok",
                "muse"
            ]
        );
        let by_id = |id: &str| offered.iter().find(|launcher| launcher.id == id).unwrap();
        assert_eq!(by_id("claude:work").label, "Claude · Work");
        assert_eq!(
            by_id("claude:work").agent.config_path.as_deref(),
            Some("~/.work")
        );
        assert_eq!(by_id("codex:cx").label, "Codex");
        assert_eq!(by_id("opencode").permission_mode, "workspace-write");
        assert_eq!(by_id("grok").permission_mode, "bypass");
    }
}
