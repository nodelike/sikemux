//! How the core starts an agent: the launch it builds from what the person
//! chose, with Sikemux's tools and the agent's environment, for terminals and
//! chats alike. Every chat, however it starts, passes through here or
//! [`own_tools`].

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent_catalog::AgentKind;
use crate::cli::protocol::TOOLS_OFFER_ENV;
use crate::launch::homes;
use crate::launch::terminal::{self, TerminalRun};
use crate::launch::tools::Tools;
use crate::launch::{AgentSpec, PermissionMode};
use crate::protocol::{
    ChatAccount, ChatLaunch, ChatStart, Continuation, Event, RequestId, Response, SessionId,
    SpawnTarget, TerminalSpawn, WAKE_WAIT,
};
use serde_json::{json, Value};
use sikemux_process::vars;
use sikemux_pty::launch::{PtyAgentProfile, PtyContext, PtyDirectCommand};

use super::chat;
use super::connection::{blocking, ClientConn};
use super::workspace::{launcher_of, ChatChoice, ResumeChoice, SessionChoice};
use super::{lifecycle, Core, CoreError, CoreResult};

/// Where this core's agents reach their tools; none for a core without a
/// tool endpoint.
pub(crate) fn tools(core: &Core) -> Option<Tools> {
    let config = &core.listening.get()?.config;
    Some(Tools {
        executable: std::env::current_exe().ok()?.canonicalize().ok()?,
        endpoint: config.cli_endpoint.clone()?,
        data_dir: config.data_dir.clone(),
    })
}

/// Gives a terminal agent its tools, its account and the hosted model
/// providers' keys. A launch that cannot be told about the tools still runs.
pub(crate) async fn terminal_agent(core: &Arc<Core>, spawn: &mut TerminalSpawn) {
    let (Some(command), Some(context)) = (spawn.direct_command.as_mut(), spawn.context.as_ref())
    else {
        return;
    };
    let Some(agent_type) = context.agent_type.clone() else {
        return;
    };
    if let (Some(agent_id), Some(tools)) = (context.agent_id.as_deref(), tools(core)) {
        match tools
            .terminal(agent_id, &agent_type, &command.program)
            .await
        {
            Ok(given) => spawn.env.extend(given.apply(&mut command.args)),
            Err(error) => {
                eprintln!("sikemux core: Sikemux tools are unavailable to this agent: {error}")
            }
        }
    }
    let profile = command.profile.as_ref();
    let spec = AgentSpec {
        provider: agent_type,
        executable_path: None,
        config_path: profile.and_then(|profile| profile.config_path.clone()),
        environment_keys: profile
            .map(|profile| profile.environment_keys.clone())
            .unwrap_or_default(),
    };
    let program = command.program.clone();
    let owner = core.clone();
    let launch = blocking(move || {
        let providers = owner.workspaces.provider_keys(&spec.provider);
        Ok(spec.launch(program.into(), providers))
    })
    .await;
    if let Ok(launch) = launch {
        spawn.env.extend(launch.env);
    }
}

/// Starts a terminal agent as its record says, on its saved session and the
/// account it runs on, below the screen of `continues` when it carries one on,
/// and answers its terminal. An agent whose terminal runs already, or that
/// another caller is starting, answers that terminal instead. Every terminal
/// agent, however it starts, starts here.
pub(crate) async fn start_terminal(
    core: &Arc<Core>,
    agent_id: &str,
    size: (u16, u16),
    continues: Option<Continuation>,
) -> CoreResult<SessionId> {
    lifecycle::start_once(
        core,
        agent_id,
        || running_terminal(core, agent_id),
        || launch_terminal(core, agent_id, size, continues),
    )
    .await
}

/// The agent's terminal, unless it ended or is being stopped.
fn running_terminal(core: &Core, agent_id: &str) -> Option<SessionId> {
    let pty = core.workspaces.agent(agent_id)?.pty_id?;
    core.session(pty)
        .filter(|session| session.is_running() && !session.is_killed())
        .map(|_| pty)
}

async fn launch_terminal(
    core: &Arc<Core>,
    agent_id: &str,
    (cols, rows): (u16, u16),
    continues: Option<Continuation>,
) -> CoreResult<SessionId> {
    let agent = core
        .workspaces
        .agent(agent_id)
        .ok_or_else(|| CoreError::from("that agent is no longer on this host"))?;
    if agent.resume_id.is_none() {
        super::session_watch::note_launch(core, &agent).await;
    }
    let project = core.workspaces.project_info(&agent.project);
    let book = core.workspaces.accounts().unwrap_or_default();
    let profile = agent.profile_id.as_deref().and_then(|id| book.profile(id));
    let run = TerminalRun {
        resume_id: agent.resume_id.as_deref(),
        permission_mode: PermissionMode::parse(&agent.permission_mode)?,
        model: agent.model.as_deref(),
        effort: agent.effort.as_deref(),
    };
    let cwd = agent.cwd.to_string_lossy().into_owned();
    let spawn = TerminalSpawn {
        cols,
        rows,
        cwd: Some(cwd.clone()),
        startup: None,
        direct_command: Some(PtyDirectCommand {
            program: profile
                .and_then(|profile| profile.executable_path.clone())
                .unwrap_or_else(|| agent.provider.clone()),
            args: terminal::args(&agent.provider, &run),
            profile: profile.map(|profile| PtyAgentProfile {
                config_path: profile.config_path.clone(),
                environment_keys: profile.environment_keys.clone(),
            }),
        }),
        context: Some(PtyContext {
            session_id: agent.project.clone(),
            session_name: project.map(|project| project.name).unwrap_or_default(),
            session_kind: "project".into(),
            project: Some(cwd),
            window_id: None,
            pane_id: None,
            agent_id: Some(agent.id.clone()),
            agent_type: Some(agent.provider.clone()),
            initial_prompt_submitted: false,
            shell_integration: false,
        }),
        env: Default::default(),
        continues,
    };
    let target = Box::new(SpawnTarget::Terminal(spawn));
    let pending =
        super::connection::spawn(core.clone(), core.launch_identity(), target, None).await?;
    let id = pending.id();
    pending.start(core);
    let patch = serde_json::Map::from_iter([("ptyId".to_owned(), serde_json::Value::from(id))]);
    core.workspaces.update_agent(agent_id, patch)?;
    core.view_changed();
    Ok(id)
}

/// Gives a chat about to start the tools that are its own: those the launch
/// offers in its environment, and only for its agent.
pub(crate) fn own_tools(core: &Core, launch: &mut ChatLaunch) {
    launch
        .env
        .insert(vars::AGENT_ID.into(), launch.agent_id.clone());
    let Some(tools) = tools(core) else {
        return;
    };
    let offer = launch.env.get(TOOLS_OFFER_ENV).map(String::as_str);
    match tools.chat_server(&launch.agent_id, offer) {
        Ok(server) => launch.mcp_servers = vec![server],
        Err(error) => {
            eprintln!("sikemux core: Sikemux tools are unavailable to this chat: {error}")
        }
    }
}

/// Gives a Hermes or Grok chat its own home with the skills library, unless
/// it has one. Answers whether the launch changed.
pub(crate) async fn own_home(core: &Core, launch: &mut ChatLaunch) -> bool {
    let private = AgentKind::from_name(&launch.provider).and_then(homes::private);
    if private.is_none_or(|(home, _)| launch.env.contains_key(home.variable)) {
        return false;
    }
    let Some(tools) = tools(core) else {
        return false;
    };
    tools
        .chat_home(
            &launch.agent_id,
            &launch.provider,
            &launch.program,
            &mut launch.env,
        )
        .await;
    true
}

/// A chat to start: on which launcher, where, and how.
pub(crate) struct Launch {
    pub agent_id: String,
    pub launcher: String,
    pub cwd: PathBuf,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    /// The hosted model provider it runs on in place of its maker's account.
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    /// The provider's session to load instead of opening a new one.
    pub resume_id: Option<String>,
    pub account: Option<ChatAccount>,
    pub fallbacks: Vec<ChatAccount>,
    /// The program to run in place of the launcher's own, for a chat a client started with one.
    pub program: Option<PathBuf>,
    /// A new chat whose agent starts only once the person writes in it or sends to it.
    pub waits_for_person: bool,
}

/// Starts the chat `chosen` describes, with `by` hearing it from its first
/// event. Every chat a person starts or wakes, from any client, starts here.
pub(crate) async fn start(
    core: &Arc<Core>,
    by: Option<&Arc<ClientConn>>,
    chosen: Launch,
) -> CoreResult<Arc<chat::Chat>> {
    let launcher = core.workspaces.launcher(&chosen.launcher)?;
    let program = match chosen.program {
        Some(program) => program,
        None => launcher.agent.program().await?,
    };
    let launcher_id = launcher.id.clone();
    let waits_for_person = chosen.waits_for_person;
    let owner = core.clone();
    let (cwd, model_provider, model) = (chosen.cwd, chosen.model_provider, chosen.model);
    let mut launch = blocking(move || {
        owner
            .workspaces
            .chat_launch(&launcher, program, &cwd, model_provider.as_deref(), model)
            .map_err(CoreError::from)
    })
    .await?;
    launch.agent_id = chosen.agent_id;
    launch.resume_id = chosen.resume_id;
    if let Some(mode) = chosen.permission_mode {
        launch.permission_mode = mode;
    }
    launch.effort = chosen.effort;
    launch.account = chosen.account;
    launch.fallbacks = chosen.fallbacks;
    let started = chat::begin(core, launch, by, Some(launcher_id), waits_for_person)?;
    core.broadcast_local(&Event::ChatBegun {
        chat: started.info(),
    });
    Ok(started)
}

/// Starts an agent of the workspace as its record says, on its saved
/// conversation and the account it last ran on, or with `waits_for_person`
/// lists it to start once the person writes in it.
pub(crate) async fn start_agent(
    core: &Arc<Core>,
    by: Option<&Arc<ClientConn>>,
    agent_id: &str,
    waits_for_person: bool,
) -> CoreResult<Arc<chat::Chat>> {
    let kept = core
        .workspaces
        .kept_launch(agent_id)
        .ok_or_else(|| CoreError::from("that agent is no longer on this host"))?;
    let agent = kept.agent;
    let chosen = Launch {
        agent_id: agent.id.clone(),
        launcher: launcher_of(&agent),
        cwd: agent.cwd,
        permission_mode: Some(agent.permission_mode),
        model: agent.model,
        model_provider: agent.model_provider,
        effort: agent.effort,
        account: kept.account,
        fallbacks: kept.fallbacks,
        program: kept.program,
        waits_for_person,
        resume_id: agent.resume_id,
    };
    start(core, by, chosen).await
}

/// Starts an agent's chat as its record says unless it runs already, and
/// answers once its session is ready or says why it never got there; with
/// `waits_for_person`, once it is listed to start when the person writes.
pub(crate) async fn start_saved(
    core: &Arc<Core>,
    agent_id: &str,
    waits_for_person: bool,
) -> CoreResult<()> {
    lifecycle::ensure_running(core, agent_id, waits_for_person).await?;
    let started = core.chats.running(agent_id)?;
    if started.unstarted() {
        return Ok(());
    }
    chat::until_started(core, started).await.map(drop)
}

/// Wakes an agent's chat and answers once it is ready, so a client taking it
/// up next finds it there.
pub(crate) async fn wake_chat(core: &Arc<Core>, agent_id: String) -> CoreResult<Response> {
    let deadline = tokio::time::Instant::now() + WAKE_WAIT;
    tokio::time::timeout_at(deadline, lifecycle::ensure_running(core, &agent_id, false))
        .await
        .map_err(|_| CoreError::from("the chat did not wake in time"))??;
    tokio::time::timeout_at(deadline, chat::until_ready(core, &agent_id))
        .await
        .map_err(|_| CoreError::from("the chat did not wake in time"))??;
    Ok(Response::Done)
}

/// Starts a new chat in one of the projects and answers `client` once its
/// session is ready.
pub(crate) fn start_chat(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    choice: ChatChoice,
) {
    let (core, client) = (core.clone(), client.clone());
    tokio::spawn(async move {
        let result = start_new(&core, Some(&client), choice)
            .await
            .map(|(agent_id, start)| Response::ChatBegun { agent_id, start });
        client.respond(request_id, result);
    });
}

/// Takes up an agent's session again in the project it ran in, recent or not, and
/// answers the agent that holds it once its session is ready.
pub(crate) async fn resume_session(core: &Arc<Core>, session: SessionChoice) -> CoreResult<Value> {
    let choice = core
        .workspaces
        .session_choice(session, &core.chats.list())?;
    let project = choice.project.clone();
    let (agent_id, _) = start_new(core, None, choice).await?;
    Ok(json!({ "agentId": agent_id, "project": project }))
}

async fn start_new(
    core: &Arc<Core>,
    client: Option<&Arc<ClientConn>>,
    choice: ChatChoice,
) -> CoreResult<(String, ChatStart)> {
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let kind = choice
        .launcher
        .split(':')
        .next()
        .unwrap_or_default()
        .to_owned();
    let chosen = Launch {
        agent_id: agent_id.clone(),
        launcher: choice.launcher,
        cwd: core.workspaces.project_path(&choice.project)?,
        permission_mode: choice.permission_mode,
        model: choice.model,
        model_provider: None,
        effort: choice.effort,
        resume_id: choice.resume_id,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    let started = start(core, client, chosen).await?;
    remember_kind(core, &kind);
    if started.unstarted() {
        let start = ChatStart {
            session_id: String::new(),
            capabilities: Value::Null,
            setup: Value::Null,
        };
        return Ok((agent_id, start));
    }
    match chat::until_started(core, started).await {
        Ok(start) => Ok((agent_id, start)),
        Err(error) => {
            super::workspace::close_gone(core, &[agent_id]);
            Err(error)
        }
    }
}

/// Keeps the kind of agent a chat was started as, so a project with no agent of its own
/// starts its next chat as that kind.
fn remember_kind(core: &Core, kind: &str) {
    if core.preferences.get().last_agent_type.as_deref() == Some(kind) {
        return;
    }
    if core.preferences.set("lastAgentType", kind.into()).is_ok() {
        core.changes
            .changed(super::changed::Topic::Preferences, None, None);
        core.view_changed();
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
