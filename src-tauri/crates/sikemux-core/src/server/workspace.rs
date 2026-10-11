//! What the app has open and how it starts chat agents, so a paired device
//! can start one with the window closed. Held in memory only: a launcher's
//! environment may carry the person's API keys.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use serde_json::Value;

use crate::protocol::{
    Backdrop, BackdropImage, ChatInfo, ChatLaunch, ChatLauncher, ChatState, Event, LauncherInfo,
    ProjectInfo, PublishedChat, PublishedRecent, RecentInfo, RequestId, Response, Workspace,
    WAKE_WAIT,
};

use super::chat;
use super::connection::ClientConn;
use super::{Core, CoreError, CoreResult};

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

#[derive(Default)]
struct Published {
    projects: Vec<ProjectInfo>,
    launchers: Vec<ChatLauncher>,
    chats: Vec<PublishedChat>,
    titles: BTreeMap<String, String>,
    recent: Vec<PublishedRecent>,
    /// Each provider's `configOptions` from its last session, by provider.
    config_options: BTreeMap<String, Value>,
    palette: BTreeMap<String, String>,
    texture: bool,
    image: Option<BackdropImage>,
}

#[derive(Default)]
pub(crate) struct Workspaces {
    published: Mutex<Published>,
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
    fn lock(&self) -> MutexGuard<'_, Published> {
        self.published
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn publish(
        &self,
        projects: Vec<ProjectInfo>,
        launchers: Vec<ChatLauncher>,
    ) -> CoreResult<()> {
        if projects.len() > MAX_PROJECTS || launchers.len() > MAX_LAUNCHERS {
            return Err("the app published more projects or launchers than the core keeps".into());
        }
        if let Some(project) = projects.iter().find(|project| !project.path.is_absolute()) {
            return Err(format!("project {} has no absolute path", project.name).into());
        }
        let mut published = self.lock();
        published.projects = projects;
        published.launchers = launchers;
        Ok(())
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
        published.chats = chats;
        published.titles = titles;
        Ok(())
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
        self.lock().recent = chats;
        Ok(())
    }

    /// The recent chats a device may take up again: those in a project the
    /// app has open, leaving out any running now.
    pub(crate) fn recent(&self, open: &[ChatInfo]) -> Vec<RecentInfo> {
        let published = self.lock();
        published
            .recent
            .iter()
            .filter(|chat| !running_now(chat, open))
            .filter_map(|chat| {
                let project = published
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

    /// How to start the recent chat a device chose, the way it would start a new one.
    fn resume_choice(&self, choice: ResumeChoice, open: &[ChatInfo]) -> CoreResult<ChatChoice> {
        let published = self.lock();
        let chat = published
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
        let project = published
            .projects
            .iter()
            .find(|project| project.path == chat.cwd)
            .ok_or_else(|| {
                CoreError::from("that chat's project is not open in Sikemux on this host")
            })?;
        let launcher = published
            .launchers
            .iter()
            .find(|launcher| launcher.id == chat.launcher && launcher.provider == chat.provider)
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
        for chat in &mut running {
            let title = published
                .chats
                .iter()
                .find(|known| known.agent_id == chat.agent_id)
                .and_then(|known| known.title.clone());
            if title.is_some() {
                chat.title = title;
            }
        }
        let stopped: Vec<ChatInfo> = published
            .chats
            .iter()
            .filter(|chat| !running.iter().any(|live| live.agent_id == chat.agent_id))
            .map(|chat| ChatInfo {
                agent_id: chat.agent_id.clone(),
                provider: chat.provider.clone(),
                title: chat.title.clone(),
                cwd: chat.cwd.clone(),
                session_id: None,
                state: ChatState::Stopped,
                running: false,
                pending_permissions: Vec::new(),
                subagents: 0,
                started_by: None,
                launcher: None,
                permission_mode: String::new(),
                model: None,
                effort: None,
                asleep: chat.asleep,
                unread: false,
                active_at: None,
            })
            .collect();
        running.extend(stopped);
        running
    }

    /// Whether the app has the chat open, and if so whether it is asleep.
    fn published(&self, agent_id: &str) -> Option<bool> {
        self.lock()
            .chats
            .iter()
            .find(|chat| chat.agent_id == agent_id)
            .map(|chat| chat.asleep)
    }

    pub(crate) fn view(&self) -> Workspace {
        let published = self.lock();
        Workspace {
            projects: published.projects.clone(),
            launchers: published
                .launchers
                .iter()
                .map(|launcher| LauncherInfo {
                    id: launcher.id.clone(),
                    provider: launcher.provider.clone(),
                    label: launcher.label.clone(),
                    permission_mode: launcher.permission_mode.clone(),
                    config_options: published
                        .config_options
                        .get(&launcher.provider)
                        .cloned()
                        .unwrap_or(Value::Null),
                    status: launcher.status.clone(),
                })
                .collect(),
            palette: published.palette.clone(),
            backdrop: Backdrop {
                texture: published.texture,
                image: published.image.as_ref().map(|image| image.id.clone()),
            },
        }
    }

    fn launch(
        &self,
        choice: ChatChoice,
        agent_id: &str,
        tools: Option<Value>,
    ) -> CoreResult<ChatLaunch> {
        let published = self.lock();
        let launcher = published
            .launchers
            .iter()
            .find(|launcher| launcher.id == choice.launcher)
            .ok_or_else(|| {
                CoreError::from("that agent is not one Sikemux on this host can start; open Sikemux on the host once so it can say which it can")
            })?;
        let project = published
            .projects
            .iter()
            .find(|project| project.id == choice.project)
            .ok_or_else(|| CoreError::from("that project is not open in Sikemux on this host"))?;
        let mut env = launcher.env.clone();
        env.insert("SIKEMUX_AGENT_ID".into(), agent_id.to_owned());
        Ok(ChatLaunch {
            agent_id: agent_id.to_owned(),
            provider: launcher.provider.clone(),
            cwd: project.path.clone(),
            program: launcher.program.clone(),
            args: launcher.args.clone(),
            env,
            mcp_servers: tools.into_iter().collect(),
            resume_id: choice.resume_id,
            permission_mode: choice
                .permission_mode
                .unwrap_or_else(|| launcher.permission_mode.clone()),
            model: choice.model,
            effort: choice.effort,
            account: launcher.account.clone(),
            fallbacks: launcher.fallbacks.clone(),
        })
    }
}

/// The agents' own tools, served by this binary run as an MCP server, the
/// same way the app wires them into a chat it starts.
fn tools_server(agent_id: &str, endpoint: &Path) -> Option<Value> {
    let executable = std::env::current_exe().ok()?;
    let mut server = McpServerStdio::new("sikemux-tools", executable);
    server.args = vec!["--tools-mcp".into()];
    server.env = vec![
        EnvVariable::new("SIKEMUX_TOOLS_AGENT_ID", agent_id),
        EnvVariable::new("SIKEMUX_CLI_ENDPOINT", endpoint.to_string_lossy()),
    ];
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

/// Has the app start a chat it put to sleep, and answers once the chat is
/// ready, so a device taking it up next finds it there.
pub(crate) async fn wake_chat(core: &Arc<Core>, agent_id: String) -> CoreResult<Response> {
    let deadline = tokio::time::Instant::now() + WAKE_WAIT;
    if !running(core, &agent_id) {
        ask_to_wake(core, &agent_id)?;
        while !running(core, &agent_id) {
            if tokio::time::Instant::now() >= deadline {
                return Err(DID_NOT_WAKE.into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    tokio::time::timeout_at(deadline, chat::until_ready(core, &agent_id))
        .await
        .map_err(|_| CoreError::from(DID_NOT_WAKE))??;
    Ok(Response::Done)
}

const DID_NOT_WAKE: &str = "the chat did not wake; open it in Sikemux on the host";

fn ask_to_wake(core: &Core, agent_id: &str) -> CoreResult<()> {
    match core.workspaces.published(agent_id) {
        None => return Err("that chat is no longer open in Sikemux on this host".into()),
        Some(false) => {
            return Err(
                "this chat stopped on the host; open it in Sikemux there to start it again".into(),
            )
        }
        Some(true) => {}
    }
    if !core.has_local_client() {
        return Err("open Sikemux on the host to wake this chat".into());
    }
    core.broadcast_local(&Event::WakeChat {
        agent_id: agent_id.to_owned(),
    });
    Ok(())
}

/// Starts the chat and answers `client` once its session is ready. The chat
/// is the device's: the app leaves it running when it opens.
pub(crate) fn start_chat(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    choice: ChatChoice,
) {
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let tools = cli_endpoint(core).and_then(|endpoint| tools_server(&agent_id, &endpoint));
    let launcher = choice.launcher.clone();
    let launch = match core.workspaces.launch(choice, &agent_id, tools) {
        Ok(launch) => launch,
        Err(error) => {
            client.respond(request_id, Err(error));
            return;
        }
    };
    match chat::begin(core, launch, Some(client), Some(launcher)) {
        Ok(started) => {
            core.broadcast_local(&Event::ChatBegun {
                chat: started.info(),
            });
            let core = core.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let result = chat::until_started(&core, started).await;
                client.respond(
                    request_id,
                    result.map(|start| Response::ChatBegun { agent_id, start }),
                );
            });
        }
        Err(error) => client.respond(request_id, Err(error)),
    }
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
    use std::collections::BTreeMap;

    use super::*;

    fn launcher() -> ChatLauncher {
        ChatLauncher {
            id: "claude:work".into(),
            provider: "claude".into(),
            label: "Claude Code (work)".into(),
            program: "/usr/bin/node".into(),
            args: vec!["adapter.js".into()],
            env: BTreeMap::from([("ANTHROPIC_API_KEY".into(), "secret".into())]),
            permission_mode: "bypass".into(),
            account: None,
            fallbacks: Vec::new(),
            status: None,
        }
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
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
        let shown = serde_json::to_string(&workspaces.view()).unwrap();
        assert!(shown.contains("Claude Code (work)"));
        assert!(!shown.contains("secret"));
        assert!(!shown.contains("adapter.js"));
    }

    #[test]
    fn a_device_sees_whether_each_launcher_is_signed_in() {
        let workspaces = Workspaces::default();
        let signed_out = ChatLauncher {
            status: Some("signedOut".into()),
            ..launcher()
        };
        workspaces
            .publish(vec![project()], vec![signed_out])
            .unwrap();
        let view = workspaces.view();
        assert_eq!(view.launchers[0].status.as_deref(), Some("signedOut"));
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
        let shown = serde_json::to_value(workspaces.view()).unwrap();
        assert!(shown["launchers"][0].get("status").is_none());
    }

    #[test]
    fn a_chat_starts_in_the_project_with_the_launcher_and_its_own_id() {
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
        let launch = workspaces.launch(choice(), "agent-1", None).unwrap();
        assert_eq!(launch.cwd, PathBuf::from("/Users/me/sikemux"));
        assert_eq!(launch.program, PathBuf::from("/usr/bin/node"));
        assert_eq!(launch.permission_mode, "bypass");
        assert_eq!(launch.model.as_deref(), Some("opus"));
        assert_eq!(
            launch.env.get("SIKEMUX_AGENT_ID").map(String::as_str),
            Some("agent-1")
        );
        assert_eq!(
            launch.env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("secret")
        );
    }

    #[test]
    fn an_unknown_launcher_or_project_is_refused() {
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
        let mut wrong = choice();
        wrong.project = "elsewhere".into();
        assert!(workspaces.launch(wrong, "agent-1", None).is_err());
        let mut wrong = choice();
        wrong.launcher = "gemini".into();
        assert!(workspaces.launch(wrong, "agent-1", None).is_err());
    }

    #[test]
    fn a_device_sees_recent_chats_in_open_projects_but_not_running_ones() {
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
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
            provider: "claude".into(),
            title: None,
            cwd: "/Users/me/sikemux".into(),
            session_id: Some("s1".into()),
            state: ChatState::Ready,
            running: false,
            pending_permissions: Vec::new(),
            subagents: 0,
            started_by: None,
            launcher: None,
            permission_mode: String::new(),
            model: None,
            effort: None,
            asleep: false,
            unread: false,
            active_at: None,
        };
        assert_eq!(workspaces.recent(std::slice::from_ref(&live)).len(), 1);
        live.provider = "codex".into();
        assert_eq!(workspaces.recent(&[live]).len(), 2);
    }

    #[test]
    fn a_recent_chat_resumes_with_its_launcher_in_its_project() {
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
        workspaces
            .publish_recent(vec![recent("s1", "/Users/me/sikemux")])
            .unwrap();
        let choice = workspaces
            .resume_choice(resuming("claude:s1"), &[])
            .unwrap();
        let launch = workspaces.launch(choice, "agent-1", None).unwrap();
        assert_eq!(launch.resume_id.as_deref(), Some("s1"));
        assert_eq!(launch.cwd, PathBuf::from("/Users/me/sikemux"));
        assert_eq!(launch.provider, "claude");
        assert_eq!(launch.permission_mode, "bypass");
    }

    #[test]
    fn only_a_published_recent_chat_resumes() {
        let workspaces = Workspaces::default();
        workspaces
            .publish(vec![project()], vec![launcher()])
            .unwrap();
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
        let workspaces = Workspaces::default();
        let mut relative = project();
        relative.path = "sikemux".into();
        assert!(workspaces.publish(vec![relative], Vec::new()).is_err());
    }
}
