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
    ProjectInfo, PublishedChat, RequestId, Response, Workspace,
};

use super::chat;
use super::connection::ClientConn;
use super::{Core, CoreError, CoreResult};

const MAX_PROJECTS: usize = 512;
const MAX_LAUNCHERS: usize = 64;
const MAX_CHATS: usize = 1024;
const MAX_TITLE_CHARS: usize = 200;
const MAX_COLOURS: usize = 64;
/// A phone-sized JPEG is a few hundred kilobytes; this leaves room without letting one fill a frame.
const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
const MAX_COLOUR_CHARS: usize = 64;
/// Long enough for an agent's adapter and CLI to come back up.
const WAKE_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Published {
    projects: Vec<ProjectInfo>,
    launchers: Vec<ChatLauncher>,
    chats: Vec<PublishedChat>,
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

    pub(crate) fn publish_chats(&self, chats: Vec<PublishedChat>) -> CoreResult<()> {
        let too_long = |chat: &PublishedChat| {
            chat.title
                .as_ref()
                .is_some_and(|title| title.chars().count() > MAX_TITLE_CHARS)
        };
        if chats.len() > MAX_CHATS || chats.iter().any(too_long) {
            return Err("the app published more chats than the core keeps".into());
        }
        self.lock().chats = chats;
        Ok(())
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
                started_by: None,
                launcher: None,
                permission_mode: String::new(),
                model: None,
                effort: None,
                asleep: chat.asleep,
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
                CoreError::from("that agent is not one Sikemux on this Mac can start; open Sikemux on the Mac once so it can say which it can")
            })?;
        let project = published
            .projects
            .iter()
            .find(|project| project.id == choice.project)
            .ok_or_else(|| CoreError::from("that project is not open in Sikemux on this Mac"))?;
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
            resume_id: None,
            permission_mode: choice
                .permission_mode
                .unwrap_or_else(|| launcher.permission_mode.clone()),
            model: choice.model,
            effort: choice.effort,
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

/// Has the app start a chat it put to sleep, and waits for it to run.
pub(crate) async fn wake_chat(core: &Arc<Core>, agent_id: String) -> CoreResult<Response> {
    if running(core, &agent_id) {
        return Ok(Response::Done);
    }
    match core.workspaces.published(&agent_id) {
        None => return Err("that chat is no longer open in Sikemux on this Mac".into()),
        Some(false) => {
            return Err(
                "this chat stopped on the Mac; open it in Sikemux there to start it again".into(),
            )
        }
        Some(true) => {}
    }
    if !core.has_local_client() {
        return Err("open Sikemux on the Mac to wake this chat".into());
    }
    core.broadcast_local(&Event::WakeChat {
        agent_id: agent_id.clone(),
    });
    let deadline = tokio::time::Instant::now() + WAKE_WAIT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if running(core, &agent_id) {
            return Ok(Response::Done);
        }
    }
    Err("the chat did not wake; open it in Sikemux on the Mac".into())
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
    fn a_relative_project_path_is_refused() {
        let workspaces = Workspaces::default();
        let mut relative = project();
        relative.path = "sikemux".into();
        assert!(workspaces.publish(vec![relative], Vec::new()).is_err());
    }

    #[test]
    fn the_tools_server_names_the_agent_and_the_endpoint() {
        let server = tools_server("agent-1", Path::new("/tmp/cli.json")).unwrap();
        let text = server.to_string();
        assert!(text.contains("sikemux-tools"));
        assert!(text.contains("--tools-mcp"));
        assert!(text.contains("agent-1"));
        assert!(text.contains("/tmp/cli.json"));
    }
}
