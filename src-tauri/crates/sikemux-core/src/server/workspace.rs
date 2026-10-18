//! The projects on this host, the app's and those the person added from a
//! device, and the agents it can start, kept by the core across restarts so a
//! paired device can start or wake a chat with no app running. The core offers each agent on each of the person's accounts
//! and finds its program itself; a launch builds its environment afresh,
//! keys and all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_accounts::Book;
use crate::agent_catalog::AGENTS;
use crate::cli::protocol::{REVIEWER_OFFER, TOOLS_OFFER_ENV};
use crate::launch::providers::{Helper, Providers, Wiring, ROUTED_ENV};
use crate::launch::{skills, AgentSpec};
use crate::protocol::{
    Backdrop, BackdropImage, ChatAccount, ChatInfo, ChatLaunch, ChatState, Event, LauncherInfo,
    ProjectInfo, PublishedChat, PublishedRecent, RecentInfo, RequestId, Response, Workspace,
    WAKE_WAIT,
};

use super::chat;
use super::connection::{blocking, ClientConn};
use super::wake::Wake;
use super::{Core, CoreError, CoreResult};
use crate::acp::pickers;

const MAX_PROJECTS: usize = 512;
const MAX_LAUNCHERS: usize = 64;
const MAX_CHATS: usize = 1024;
const MAX_RECENT: usize = 100;
const MAX_SESSION_ID_CHARS: usize = 512;
const MAX_TITLE_CHARS: usize = 200;
const MAX_COLOURS: usize = 64;
/// A phone-sized JPEG is a few hundred kilobytes; this leaves room without letting one fill a frame.
const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
const MAX_COLOUR_CHARS: usize = 64;
const MAX_BUDDY_AGENTS: usize = 4096;
const MAX_BUDDY_BOXES: usize = 512;

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

/// How a chat the app stopped was running, so the core can wake it alone.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asleep {
    agent_id: String,
    launcher: String,
    cwd: PathBuf,
    resume_id: Option<String>,
    permission_mode: String,
    model: Option<String>,
    effort: Option<String>,
    account: Option<ChatAccount>,
    #[serde(default)]
    fallbacks: Vec<ChatAccount>,
}

/// What survives a restart of the core.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    /// The projects the app has open.
    projects: Vec<ProjectInfo>,
    /// The projects the person added from a device, which no app holds.
    #[serde(default)]
    added: Vec<ProjectInfo>,
    /// The person's agent accounts, absent until the app first says which they are.
    accounts: Option<Book>,
    /// The mode the person starts agents in, absent until the app first says.
    permission_mode: Option<String>,
    /// The app's chats, absent until it has said which it holds.
    chats: Option<Vec<PublishedChat>>,
    recent: Vec<PublishedRecent>,
    asleep: Vec<Asleep>,
}

impl Saved {
    /// Every project on this host: the app's, then those added from devices.
    fn all_projects(&self) -> impl Iterator<Item = &ProjectInfo> {
        self.projects.iter().chain(&self.added)
    }
}

#[derive(Default)]
struct Published {
    saved: Saved,
    launchers: Vec<Launcher>,
    titles: BTreeMap<String, String>,
    /// Each provider's `configOptions` from its last session, by provider.
    config_options: BTreeMap<String, Value>,
    palette: BTreeMap<String, String>,
    texture: bool,
    image: Option<BackdropImage>,
    /// How the app runs each of its agents, by agent id.
    runs: BTreeMap<String, AgentRun>,
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

/// The launcher and permission mode the app runs an agent with, which the
/// core cannot read off a terminal agent itself.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRun {
    pub launcher: String,
    pub permission_mode: String,
}

pub(crate) struct Workspaces {
    published: Mutex<Published>,
    /// Where [`Saved`] is kept; nowhere for a core without a data folder.
    path: Option<PathBuf>,
    providers: Option<Providers>,
    /// The data folder, which holds the skills library.
    data_dir: Option<PathBuf>,
    /// Counts the looks over the agents, so one that took longer never replaces a newer one.
    asked: AtomicU64,
    /// Tells the core the agents the app holds, or the launchers it offers,
    /// changed.
    pub(crate) wake: Wake,
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

fn recent_id(chat: &PublishedRecent) -> String {
    format!("{}:{}", chat.provider, chat.session_id)
}

fn running_now(chat: &PublishedRecent, open: &[ChatInfo]) -> bool {
    open.iter().any(|live| {
        live.provider == chat.provider
            && live.session_id.as_deref() == Some(chat.session_id.as_str())
    })
}

impl Workspaces {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join("workspace.json"));
        let saved = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            published: Mutex::new(Published {
                saved,
                ..Published::default()
            }),
            path,
            providers: data_dir.map(Providers::new),
            data_dir: data_dir.map(Path::to_path_buf),
            asked: AtomicU64::new(0),
            wake: Wake::default(),
        }
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
            eprintln!("sikemux core: the app's projects and agents were not saved: {error}");
        }
    }

    /// Keeps the app's projects and the mode it starts agents in, and says
    /// whether that mode changed.
    fn set(&self, projects: Vec<ProjectInfo>, permission_mode: String) -> CoreResult<bool> {
        if projects.len() > MAX_PROJECTS || permission_mode.len() > 64 {
            return Err("the app published more projects than the core keeps".into());
        }
        if let Some(project) = projects.iter().find(|project| !project.path.is_absolute()) {
            return Err(format!("project {} has no absolute path", project.name).into());
        }
        let mut published = self.lock();
        let changed = published.saved.permission_mode.as_ref() != Some(&permission_mode);
        published
            .saved
            .added
            .retain(|added| !projects.iter().any(|project| project.path == added.path));
        published.saved.projects = projects;
        published.saved.permission_mode = Some(permission_mode);
        self.save(&published.saved);
        Ok(changed)
    }

    /// Adds a folder on this host as a project, or answers the project already there.
    fn add_project(&self, path: PathBuf, name: Option<String>) -> CoreResult<ProjectInfo> {
        let mut published = self.lock();
        if let Some(project) = published
            .saved
            .all_projects()
            .find(|project| project.path == path)
        {
            return Ok(project.clone());
        }
        if published.saved.all_projects().count() >= MAX_PROJECTS {
            return Err("this host already keeps as many projects as it can".into());
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
            id: uuid::Uuid::new_v4().to_string(),
            name,
            path,
        };
        published.saved.added.push(project.clone());
        self.save(&published.saved);
        Ok(project)
    }

    fn remove_project(&self, path: &Path) -> CoreResult<()> {
        let mut published = self.lock();
        if published
            .saved
            .projects
            .iter()
            .any(|project| project.path == path)
        {
            return Err(
                "that project is open in Sikemux's window on this host; close it there".into(),
            );
        }
        let before = published.saved.added.len();
        published.saved.added.retain(|project| project.path != path);
        if published.saved.added.len() == before {
            return Err("that folder is not a project on this host".into());
        }
        self.save(&published.saved);
        Ok(())
    }

    pub(crate) fn accounts(&self) -> Option<Book> {
        self.lock().saved.accounts.clone()
    }

    pub(crate) fn set_accounts(&self, book: Book) -> CoreResult<()> {
        book.check()?;
        let mut published = self.lock();
        published.saved.accounts = Some(book);
        self.save(&published.saved);
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
    fn chat_launch(
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

    fn project_path(&self, id: &str) -> CoreResult<PathBuf> {
        self.lock()
            .saved
            .all_projects()
            .find(|project| project.id == id)
            .map(|project| project.path.clone())
            .ok_or_else(|| CoreError::from("that project is not open in Sikemux on this host"))
    }

    fn launcher(&self, id: &str) -> CoreResult<Launcher> {
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

    pub(crate) fn publish_agents(
        &self,
        chats: Vec<PublishedChat>,
        titles: BTreeMap<String, String>,
    ) -> CoreResult<()> {
        let too_long = |title: &str| title.chars().count() > MAX_TITLE_CHARS;
        let chat_too_long = |chat: &PublishedChat| chat.title.as_deref().is_some_and(too_long);
        if chats.len() > MAX_CHATS
            || titles.len() > MAX_CHATS
            || chats.iter().any(chat_too_long)
            || titles.values().any(|title| too_long(title))
        {
            return Err("the app published more agents than the core keeps".into());
        }
        let mut published = self.lock();
        published.titles = titles;
        if published.saved.chats.as_ref() != Some(&chats) {
            published
                .saved
                .asleep
                .retain(|asleep| chats.iter().any(|chat| chat.agent_id == asleep.agent_id));
            published.saved.chats = Some(chats);
            self.save(&published.saved);
            self.wake.now();
        }
        Ok(())
    }

    pub(crate) fn publish_runs(&self, runs: BTreeMap<String, AgentRun>) -> CoreResult<()> {
        let oversized = |run: &AgentRun| run.launcher.len() > 256 || run.permission_mode.len() > 64;
        if runs.values().any(oversized) {
            return Err(
                "the app said an agent runs with a launcher or mode longer than the core keeps"
                    .into(),
            );
        }
        self.lock().runs = runs;
        Ok(())
    }

    pub(crate) fn run_of(&self, agent_id: &str) -> Option<AgentRun> {
        self.lock().runs.get(agent_id).cloned()
    }

    pub(crate) fn publish_recent(&self, chats: Vec<PublishedRecent>) -> CoreResult<()> {
        let oversized = |chat: &PublishedRecent| {
            chat.title.chars().count() > MAX_TITLE_CHARS
                || chat.session_id.is_empty()
                || chat.session_id.chars().count() > MAX_SESSION_ID_CHARS
        };
        if chats.len() > MAX_RECENT || chats.iter().any(oversized) {
            return Err("the app published more recent chats than the core keeps".into());
        }
        let mut published = self.lock();
        if published.saved.recent != chats {
            published.saved.recent = chats;
            self.save(&published.saved);
        }
        Ok(())
    }

    /// The recent chats a device may take up again: those in a project the
    /// app has open, leaving out any running now.
    pub(crate) fn recent(&self, open: &[ChatInfo]) -> Vec<RecentInfo> {
        let published = self.lock();
        let saved = &published.saved;
        saved
            .recent
            .iter()
            .filter(|chat| !running_now(chat, open))
            .filter_map(|chat| {
                let project = saved
                    .all_projects()
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

    /// How to start the recent chat a device chose, the way it would start a new one.
    fn resume_choice(&self, choice: ResumeChoice, open: &[ChatInfo]) -> CoreResult<ChatChoice> {
        let published = self.lock();
        let saved = &published.saved;
        let chat = saved
            .recent
            .iter()
            .find(|chat| recent_id(chat) == choice.recent)
            .ok_or_else(|| {
                CoreError::from(
                    "that chat is no longer among the recent ones in Sikemux on this host",
                )
            })?;
        if running_now(chat, open) {
            return Err("that chat is already open on this host".into());
        }
        let project = saved
            .all_projects()
            .find(|project| project.path == chat.cwd)
            .ok_or_else(|| {
                CoreError::from("that chat's project is not open in Sikemux on this host")
            })?;
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
            .all_projects()
            .map(|project| project.path.clone())
            .collect()
    }

    /// What the app calls an agent terminal.
    pub(crate) fn title(&self, agent_id: &str) -> Option<String> {
        self.lock().titles.get(agent_id).cloned()
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

    /// The running chats under the app's names, which include the agent's own
    /// unless the person renamed the chat, then the app's chats that are not running.
    pub(crate) fn listed(&self, mut running: Vec<ChatInfo>) -> Vec<ChatInfo> {
        let published = self.lock();
        let chats = published.saved.chats.as_deref().unwrap_or_default();
        for chat in &mut running {
            let title = chats
                .iter()
                .find(|known| known.agent_id == chat.agent_id)
                .and_then(|known| known.title.clone());
            if title.is_some() {
                chat.title = title;
            }
        }
        let stopped: Vec<ChatInfo> = chats
            .iter()
            .filter(|chat| !running.iter().any(|live| live.agent_id == chat.agent_id))
            .map(|chat| ChatInfo {
                agent_id: chat.agent_id.clone(),
                name: None,
                provider: chat.provider.clone(),
                title: chat.title.clone(),
                cwd: chat.cwd.clone(),
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
                asleep: chat.asleep,
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

    /// Whether the app is known to have closed the chat.
    pub(crate) fn closed(&self, agent_id: &str) -> bool {
        self.lock()
            .saved
            .chats
            .as_ref()
            .is_some_and(|chats| !chats.iter().any(|chat| chat.agent_id == agent_id))
    }

    /// Whether the app has the chat open, and if so whether it is asleep.
    pub(crate) fn published(&self, agent_id: &str) -> Option<bool> {
        self.lock()
            .saved
            .chats
            .iter()
            .flatten()
            .find(|chat| chat.agent_id == agent_id)
            .map(|chat| chat.asleep)
    }

    pub(crate) fn view(&self) -> Workspace {
        let published = self.lock();
        Workspace {
            projects: published.saved.all_projects().cloned().collect(),
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

    /// Keeps how the agent's chat runs as it stops, so a device can wake it
    /// when the app is not there to.
    pub(crate) fn note_stopping(&self, chat: &chat::Chat) {
        let agent_id = chat.agent_id();
        let Some(launcher) = chat
            .origin
            .launcher
            .clone()
            .or_else(|| self.run_of(agent_id).map(|run| run.launcher))
        else {
            return;
        };
        let launch = chat.current_now();
        let mut published = self.lock();
        let asleep = &mut published.saved.asleep;
        asleep.retain(|asleep| asleep.agent_id != agent_id);
        if asleep.len() >= MAX_CHATS {
            asleep.remove(0);
        }
        asleep.push(Asleep {
            agent_id: agent_id.to_owned(),
            launcher,
            cwd: launch.cwd,
            resume_id: launch.resume_id,
            permission_mode: launch.permission_mode,
            model: launch.model,
            effort: launch.effort,
            account: launch.account,
            fallbacks: launch.fallbacks,
        });
        self.save(&published.saved);
    }

    fn take_asleep(&self, agent_id: &str) -> Option<Asleep> {
        let mut published = self.lock();
        let asleep = &mut published.saved.asleep;
        let index = asleep
            .iter()
            .position(|asleep| asleep.agent_id == agent_id)?;
        let taken = asleep.remove(index);
        self.save(&published.saved);
        Some(taken)
    }
}

/// Gives `launch` to `agent_id`, with the tools that are its own.
pub(crate) fn own_launch(core: &Core, launch: &mut ChatLaunch, agent_id: &str) {
    own_launch_offering(core, launch, agent_id, None);
}

/// [`own_launch`] for a reviewer Auto starts, which also gets the tools only
/// a reviewer has.
pub(crate) fn own_review_launch(core: &Core, launch: &mut ChatLaunch, agent_id: &str) {
    own_launch_offering(core, launch, agent_id, Some(REVIEWER_OFFER));
}

fn own_launch_offering(core: &Core, launch: &mut ChatLaunch, agent_id: &str, offer: Option<&str>) {
    launch.agent_id = agent_id.to_owned();
    launch.mcp_servers = cli_endpoint(core)
        .and_then(|endpoint| tools_server(agent_id, &endpoint, offer))
        .into_iter()
        .collect();
    if let Some(offer) = offer {
        launch.env.insert(TOOLS_OFFER_ENV.into(), offer.to_owned());
    }
    launch
        .env
        .insert("SIKEMUX_AGENT_ID".into(), agent_id.to_owned());
}

/// The agents' own tools, served by this binary run as an MCP server, the
/// same way the app wires them into a chat it starts.
fn tools_server(agent_id: &str, endpoint: &Path, offer: Option<&str>) -> Option<Value> {
    let executable = std::env::current_exe().ok()?;
    let mut server = McpServerStdio::new("sikemux-tools", executable);
    server.args = vec!["--tools-mcp".into()];
    server.env = vec![
        EnvVariable::new("SIKEMUX_TOOLS_AGENT_ID", agent_id),
        EnvVariable::new("SIKEMUX_CLI_ENDPOINT", endpoint.to_string_lossy()),
    ];
    server
        .env
        .extend(offer.map(|offer| EnvVariable::new(TOOLS_OFFER_ENV, offer)));
    serde_json::to_value(McpServer::Stdio(server)).ok()
}

fn cli_endpoint(core: &Core) -> Option<PathBuf> {
    core.listening.get()?.config.cli_endpoint.clone()
}

fn running(core: &Core, agent_id: &str) -> bool {
    core.chats
        .list()
        .iter()
        .any(|chat| chat.agent_id == agent_id)
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
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceParams {
    projects: Vec<ProjectInfo>,
    permission_mode: String,
}

/// Keeps the projects the app has open and the mode it starts agents in.
pub(crate) async fn set_workspace(core: &Arc<Core>, params: WorkspaceParams) -> CoreResult<()> {
    if core
        .workspaces
        .set(params.projects, params.permission_mode)?
    {
        refresh_launchers(core.clone()).await;
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct AddProjectParams {
    path: PathBuf,
    #[serde(default)]
    name: Option<String>,
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
    let project = core.workspaces.add_project(path, params.name)?;
    core.publish_device_view();
    Ok(project)
}

#[derive(Deserialize)]
pub(crate) struct RemoveProjectParams {
    path: PathBuf,
}

/// Stops offering a project a device added. Agents already at work in it keep working.
pub(crate) fn remove_project(core: &Core, params: RemoveProjectParams) -> CoreResult<()> {
    core.workspaces.remove_project(&params.path)?;
    core.publish_device_view();
    Ok(())
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

/// A chat of the app's `launcher` in `cwd`, on its program found afresh,
/// built off the async threads.
async fn built(core: &Arc<Core>, launcher: &str, cwd: PathBuf) -> CoreResult<(ChatLaunch, String)> {
    let launcher = core.workspaces.launcher(launcher)?;
    let program = launcher.agent.program().await?;
    let core = core.clone();
    blocking(move || {
        let launch = core
            .workspaces
            .chat_launch(&launcher, program, &cwd, None, None)?;
        Ok((launch, launcher.id))
    })
    .await
}

/// Starts a chat for `by`, a device the app leaves it running for when it opens.
fn begin(
    core: &Arc<Core>,
    by: Option<&Arc<ClientConn>>,
    mut launch: ChatLaunch,
    agent_id: &str,
    launcher: String,
) -> CoreResult<Arc<chat::Chat>> {
    own_launch(core, &mut launch, agent_id);
    let started = chat::begin(core, launch, by, Some(launcher))?;
    core.broadcast_local(&Event::ChatBegun {
        chat: started.info(),
    });
    Ok(started)
}

/// Wakes a chat the app put to sleep and answers once it is ready, so a
/// device taking it up next finds it there. The app wakes it when its
/// window is open; otherwise the core starts it on its saved conversation.
pub(crate) async fn wake_chat(
    core: &Arc<Core>,
    by: Option<&Arc<ClientConn>>,
    agent_id: String,
) -> CoreResult<Response> {
    let deadline = tokio::time::Instant::now() + WAKE_WAIT;
    if !running(core, &agent_id) {
        match core.workspaces.published(&agent_id) {
            None => return Err("that chat is no longer open in Sikemux on this host".into()),
            Some(false) => {
                return Err(
                    "this chat stopped on the host; open it in Sikemux there to start it again"
                        .into(),
                )
            }
            Some(true) => {}
        }
        if core.window.is_open() {
            core.broadcast_local(&Event::WakeChat {
                agent_id: agent_id.clone(),
            });
            while !running(core, &agent_id) {
                if tokio::time::Instant::now() >= deadline {
                    return Err(DID_NOT_WAKE.into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } else {
            wake_here(core, by, &agent_id).await?;
        }
    }
    tokio::time::timeout_at(deadline, chat::until_ready(core, &agent_id))
        .await
        .map_err(|_| CoreError::from(DID_NOT_WAKE))??;
    Ok(Response::Done)
}

const DID_NOT_WAKE: &str = "the chat did not wake; open it in Sikemux on the host";

async fn wake_here(
    core: &Arc<Core>,
    by: Option<&Arc<ClientConn>>,
    agent_id: &str,
) -> CoreResult<()> {
    let asleep = core.workspaces.take_asleep(agent_id).ok_or_else(|| {
        CoreError::from(
            "Sikemux on this host does not know how this chat ran; open it there to wake it",
        )
    })?;
    let (mut launch, launcher) = built(core, &asleep.launcher, asleep.cwd).await?;
    launch.resume_id = asleep.resume_id;
    launch.permission_mode = asleep.permission_mode;
    launch.model = asleep.model;
    launch.effort = asleep.effort;
    launch.account = asleep.account;
    launch.fallbacks = asleep.fallbacks;
    begin(core, by, launch, agent_id, launcher)?;
    Ok(())
}

/// Starts the chat and answers `client` once its session is ready.
pub(crate) fn start_chat(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    choice: ChatChoice,
) {
    let (core, client) = (core.clone(), client.clone());
    tokio::spawn(async move {
        let result = start(&core, &client, choice).await;
        client.respond(request_id, result);
    });
}

async fn start(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    choice: ChatChoice,
) -> CoreResult<Response> {
    let cwd = core.workspaces.project_path(&choice.project)?;
    let (mut launch, launcher) = built(core, &choice.launcher, cwd).await?;
    launch.resume_id = choice.resume_id;
    if let Some(mode) = choice.permission_mode {
        launch.permission_mode = mode;
    }
    launch.model = choice.model;
    launch.effort = choice.effort;
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let started = begin(core, Some(client), launch, &agent_id, launcher)?;
    let start = chat::until_started(core, started).await?;
    Ok(Response::ChatBegun { agent_id, start })
}

/// Takes up a recent chat again, answering like [`start_chat`].
pub(crate) fn resume_chat(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    choice: ResumeChoice,
) {
    match core.workspaces.resume_choice(choice, &core.chats.list()) {
        Ok(choice) => start_chat(core, client, request_id, choice),
        Err(error) => client.respond(request_id, Err(error)),
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
        workspaces.set(vec![project()], "bypass".into()).unwrap();
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

    fn recent(session_id: &str, cwd: &str) -> PublishedRecent {
        PublishedRecent {
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
        workspaces
            .publish_recent(vec![
                recent("s1", "/Users/me/sikemux"),
                recent("s2", "/Users/me/sikemux"),
                recent("s3", "/Users/me/closed"),
            ])
            .unwrap();
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
        workspaces
            .publish_recent(vec![recent("s1", "/Users/me/sikemux")])
            .unwrap();
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
        workspaces
            .publish_recent(vec![recent("s1", "/Users/me/sikemux")])
            .unwrap();
        assert!(workspaces
            .resume_choice(resuming("claude:other"), &[])
            .is_err());
        assert!(workspaces.resume_choice(resuming("s1"), &[]).is_err());
        workspaces.publish_recent(Vec::new()).unwrap();
        assert!(workspaces
            .resume_choice(resuming("claude:s1"), &[])
            .is_err());
        let mut elsewhere = recent("s2", "/Users/me/sikemux");
        elsewhere.launcher = "codex".into();
        workspaces.publish_recent(vec![elsewhere]).unwrap();
        assert!(workspaces
            .resume_choice(resuming("claude:s2"), &[])
            .is_err());
    }

    #[test]
    fn a_relative_project_path_is_refused() {
        let workspaces = Workspaces::new(None);
        let mut relative = project();
        relative.path = "sikemux".into();
        assert!(workspaces.set(vec![relative], "bypass".into()).is_err());
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
        workspaces.set(vec![project()], "bypass".into()).unwrap();
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
