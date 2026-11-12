//! How the core starts an agent: the launch it builds from what the person
//! chose, with Sikemux's tools and the agent's environment, for terminals and
//! chats alike. Every chat, however it starts, passes through here or
//! [`own_tools`].

use crate::server::changed::What;
use serde::{Deserialize, Serialize};
use sikemux_view::prefs::PreferenceChange;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent_accounts::allocate::Aim;
use crate::agent_catalog::AgentKind;
use crate::cli::protocol::TOOLS_OFFER_ENV;
use crate::launch::homes;
use crate::launch::terminal::{self, TerminalRun};
use crate::launch::tools::Tools;
use crate::launch::{AgentSpec, PermissionMode};
use serde_json::{json, Value};
use sikemux_fold::UpdateKind;
use sikemux_process::vars;
use sikemux_pty::launch::{LaunchKind, PtyAgentProfile, PtyContext, PtyDirectCommand};
use sikemux_view::workspace::WorkspaceAgent;
use sikemux_wire::protocol::{
    ask, ChatAccount, ChatLaunch, ChatStart, Continuation, Event, LaunchIdentity, RequestId,
    SessionId, SpawnTarget, TerminalSpawn, WAKE_WAIT,
};

use super::chat;
use super::connection::{blocking, ClientConn};
use super::model_proxy::Grant as ModelGrant;
use super::prepare::{prepare_task, prepare_terminal};
use super::session::PendingStart;
use super::workspace::{
    launcher_of, new_agent, ChatChoice, LauncherId, ResumeChoice, SessionChoice,
};
use super::{lifecycle, Core, CoreError, CoreResult};
use crate::launch::providers::MODEL_TOKEN_SLOT;

/// Where this core's agents reach their tools; none for a core without a
/// tool endpoint.
pub(crate) fn tools(core: &Core) -> Option<Tools> {
    let config = &core.config;
    Some(Tools {
        executable: std::env::current_exe().ok()?.canonicalize().ok()?,
        endpoint: config.cli_endpoint.clone()?,
        data_dir: config.data_dir.clone(),
    })
}

/// Gives a terminal agent its tools, its account and the connected model providers, and
/// answers with what keeps its token for them good. A launch that cannot be told about the
/// tools still runs.
pub(crate) async fn terminal_agent(
    core: &Arc<Core>,
    spawn: &mut TerminalSpawn,
) -> Option<ModelGrant> {
    let (Some(command), Some(context)) = (spawn.direct_command.as_mut(), spawn.context.as_ref())
    else {
        return None;
    };
    let agent_type = context.agent_type.clone()?;
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
    let mut listed: BTreeMap<String, String> = spawn.env.clone().into_iter().collect();
    let launch = blocking(move || {
        let mut unused_args = Vec::new();
        let told = owner
            .workspaces
            .list_connected(&spec.provider, &mut listed, &mut unused_args);
        if let Err(error) = told {
            eprintln!("sikemux core: {error}");
        }
        Ok((spec.launch(program.into(), BTreeMap::new()), listed))
    })
    .await;
    let Ok((launch, mut listed)) = launch else {
        return None;
    };
    spawn.env.extend(launch.env);
    let grant = core
        .model_proxy
        .grant(core, &mut listed, &mut [], None)
        .inspect_err(|error| eprintln!("sikemux core: {error}"))
        .ok()
        .flatten();
    if grant.is_none() {
        listed.retain(|_, value| !value.contains(MODEL_TOKEN_SLOT));
    }
    spawn.env.extend(listed);
    grant
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

/// Starts again the terminal of each terminal agent a client brought on screen whose
/// terminal ended, as it does with the core before or when the agent sleeps.
pub(crate) fn show_terminals(core: &Arc<Core>, agent_ids: &[String]) {
    for agent_id in agent_ids {
        let ended = core
            .workspaces
            .agent(agent_id)
            .is_some_and(|agent| agent.terminal)
            && running_terminal(core, agent_id).is_none();
        if !ended {
            continue;
        }
        let (core, agent_id) = (core.clone(), agent_id.clone());
        tokio::spawn(async move {
            let size = super::spare::opening_size(&core);
            if let Err(error) = start_terminal(&core, &agent_id, size, None).await {
                eprintln!("sikemux core: {agent_id}'s terminal did not start again: {error}");
            }
        });
    }
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
    super::worktree::agent::restore(core, agent_id).await?;
    let agent = core.workspaces.agent_or_gone(agent_id)?;
    if agent.resume_id.is_none() {
        super::session_watch::note_launch(core, &agent).await;
    }
    let project = core.workspaces.project_info(&agent.project);
    let book = core.workspaces.accounts().unwrap_or_default();
    let profile = agent.profile_id.as_deref().and_then(|id| book.profile(id));
    let run = TerminalRun {
        resume_id: agent.resume_id.as_deref(),
        permission_mode: agent.permission_mode,
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
            session_kind: LaunchKind::Project,
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
    let pending = self::spawn(core.clone(), core.launch_identity(), target, None).await?;
    let id = pending.id();
    pending.start(core);
    core.workspaces.edit_agent(agent_id, |agent| {
        agent.pty_id = Some(id);
        agent.terminal = true;
        agent.asleep = false;
    })?;
    core.workspaces.woke(agent_id);
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
    if AgentKind::from_name(&launch.provider) == Some(AgentKind::Pi) {
        match tools.pi_extension() {
            Ok(path) => {
                launch.env.insert(
                    vars::PI_EXTENSION.into(),
                    path.to_string_lossy().into_owned(),
                );
            }
            Err(error) => eprintln!("sikemux core: {error}"),
        }
    }
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

pub(crate) struct Launch {
    pub agent_id: String,
    pub launcher: String,
    pub cwd: PathBuf,
    pub permission_mode: Option<PermissionMode>,
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
    mut chosen: Launch,
) -> CoreResult<Arc<chat::Chat>> {
    let notice = place(core, &mut chosen).await;
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
    launch.fallbacks = chosen.fallbacks;
    // The launcher set the folder the agent runs in; the account chosen for it moves it there.
    if let Some(account) = chosen.account {
        launch = chat::on_account(&launch.provider.clone(), &launch, &account);
    }
    let starting = chat::Starting {
        waits_for_person,
        briefed: false,
        notice,
    };
    let started = chat::begin_as(core, launch, by, Some(launcher_id), starting)?;
    core.changed(What::Event(&Event::ChatBegun {
        chat: started.info(),
    }));
    Ok(started)
}

#[derive(Serialize, Deserialize)]
pub struct SmartPickParams {
    pub launcher: String,
    pub project: String,
}

/// The account smart accounts would start a new chat of `launcher` in `project` on, and why, so a
/// client can say it before the chat exists. None when the launcher names its account or smart
/// accounts would not pick one.
pub(crate) async fn smart_pick(
    core: &Arc<Core>,
    params: SmartPickParams,
) -> CoreResult<Option<sikemux_view::chat::AccountMove>> {
    let kind = LauncherId::parse(&params.launcher).kind.to_owned();
    let mut chosen = Launch {
        agent_id: String::new(),
        launcher: params.launcher,
        cwd: core.workspaces.project_path(&params.project)?,
        permission_mode: None,
        model: core
            .workspaces
            .model_pick(&kind)
            .and_then(|pick| pick.model),
        model_provider: None,
        effort: None,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    Ok(place(core, &mut chosen)
        .await
        .map(|notice| sikemux_view::chat::AccountMove {
            label: notice["label"].as_str().unwrap_or_default().to_owned(),
            from: None,
            reason: sikemux_view::chat::MoveReason::Chosen,
            placed: notice["why"].as_str().map(str::to_owned),
        }))
}

/// Puts a chat that names only its agent on the account smart accounts picks for it, and
/// answers what the chat says of that once its agent is up. A chat that names its account, as
/// one the person picked or one woken on the account it ran on, keeps it. One shown to the person
/// before its agent starts is placed at once, on the readings already kept.
async fn place(core: &Arc<Core>, chosen: &mut Launch) -> Option<Value> {
    let named = LauncherId::parse(&chosen.launcher);
    if named.account.is_some() || chosen.account.is_some() || chosen.model_provider.is_some() {
        return None;
    }
    let kind = named.kind.to_owned();
    let book = core.workspaces.accounts().unwrap_or_default();
    let accounts = (book.clone().with_built_ins().profiles.iter())
        .filter(|profile| profile.provider == kind)
        .count();
    if !book.smart || accounts < 2 {
        return None;
    }
    let freshening = super::agent_accounts::freshen(core.clone(), book.clone(), kind.clone());
    match chosen.waits_for_person {
        true => drop(tokio::spawn(freshening)),
        false => freshening.await,
    }
    let (log, cwd) = (
        core.activity_log.clone(),
        chosen.cwd.to_string_lossy().into_owned(),
    );
    let typical_tokens = blocking(move || Ok(log.typical_chat_tokens(&cwd)))
        .await
        .ok()
        .flatten();
    let chat = super::agent_accounts::Placing {
        model: model_of(core, &book, &kind, chosen.model.clone()),
        typical_tokens,
        cwd: Some(chosen.cwd.clone()),
    };
    let placed = super::agent_accounts::placement(&book, &kind, &chat, Aim::Soonest, |_| true)?;
    let id = LauncherId::on(&kind, &placed.id).to_string();
    let launcher = core
        .workspaces
        .launcher(&id)
        .ok()
        .filter(|found| found.id == id)?;
    chosen.launcher = id;
    let label = book
        .with_built_ins()
        .profile(&placed.id)
        .map_or(launcher.label, |profile| profile.name.clone());
    Some(json!({
        "sessionUpdate": UpdateKind::AccountSwitched,
        "account": placed.id,
        "label": label,
        "reason": "placed",
        "why": placed.why,
    }))
}

/// The model a new chat of `kind` will run: the one chosen, else the agent's own default as its
/// settings name it, else the one its last session started on. None when all that says is
/// "default".
fn model_of(
    core: &Core,
    book: &crate::agent_accounts::Book,
    kind: &str,
    chosen: Option<String>,
) -> Option<String> {
    let config_path = (book.clone().with_built_ins().profile(&book.chosen(kind)))
        .and_then(|profile| profile.config_path.clone());
    chosen
        .or_else(|| {
            crate::agent_accounts::defaults::configured_default_model(kind, config_path.as_deref())
        })
        .or_else(|| {
            let options = core.workspaces.agent_options(kind)?;
            crate::acp::pickers::current_choice(&json!({ "configOptions": options }), "model")
        })
        .filter(|model| !model.eq_ignore_ascii_case("default"))
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
    super::worktree::agent::restore(core, agent_id).await?;
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

/// Answers [`StartParams`]: starts the agent as its record says, anew when it asks to be.
pub(crate) async fn start_asked(core: &Arc<Core>, params: StartParams) -> CoreResult<()> {
    let waiting = (core.chats.running(&params.agent_id)).is_ok_and(|chat| chat.unstarted());
    if params.again && waiting {
        core.chats.stop(&params.agent_id);
    }
    start_saved(core, &params.agent_id, params.waits_for_person).await
}

/// Wakes an agent on its saved session: in its terminal for one that runs there, otherwise
/// its chat, answering once it is ready.
pub(crate) async fn wake(core: &Arc<Core>, agent_id: String) -> CoreResult<()> {
    if core
        .workspaces
        .agent(&agent_id)
        .is_some_and(|agent| agent.terminal)
    {
        let size = super::spare::opening_size(core);
        return start_terminal(core, &agent_id, size, None).await.map(drop);
    }
    wake_chat(core, agent_id).await.map(drop)
}

/// Wakes an agent's chat and answers once it is ready, so a client taking it
/// up next finds it there.
pub(crate) async fn wake_chat(core: &Arc<Core>, agent_id: String) -> CoreResult<()> {
    let deadline = tokio::time::Instant::now() + WAKE_WAIT;
    tokio::time::timeout_at(deadline, lifecycle::ensure_running(core, &agent_id, false))
        .await
        .map_err(|_| CoreError::from("the chat did not wake in time"))??;
    tokio::time::timeout_at(deadline, chat::until_ready(core, &agent_id))
        .await
        .map_err(|_| CoreError::from("the chat did not wake in time"))?
        .map(drop)
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
        let result = start_new(&core, Some(&client), choice).await;
        client.answer::<ask::StartChat>(request_id, result);
    });
}

/// Takes up an agent's session again in the project it ran in, recent or not, and
/// answers the agent that holds it once its session is ready.
/// The agent a saved session came back as, and the project it is in.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTaken {
    pub agent_id: String,
    pub project: String,
}

pub(crate) async fn resume_session(
    core: &Arc<Core>,
    session: SessionChoice,
) -> CoreResult<SessionTaken> {
    let choice =
        core.workspaces
            .session_choice(session, &core.chats.list(), &core.triggers.held())?;
    let project = choice.project.clone();
    let (agent_id, _) = start_new(core, None, choice).await?;
    Ok(SessionTaken { agent_id, project })
}

/// The agent `agent_id`, open: itself while it is, else the session it ran taken up again as
/// from Recent, or the agent that already has that session open.
pub(crate) async fn reopen_agent(core: &Arc<Core>, agent_id: String) -> CoreResult<SessionTaken> {
    let project_of = |agent: &str| core.workspaces.agent_projects().remove(agent);
    if let Some(project) = project_of(&agent_id) {
        return Ok(SessionTaken { agent_id, project });
    }
    let ran = (core.names.ran_session(&agent_id))
        .ok_or_else(|| CoreError::from("its saved session can no longer be found"))?;
    let found = {
        let (core, session, cli) = (core.clone(), ran.session_id.clone(), ran.provider.clone());
        blocking(move || super::saved::locate(&core, &session, Some(&cli)).map_err(Into::into))
            .await
            .ok()
    };
    if let Some((open, _)) = found.as_ref().and_then(|saved| saved.open.clone()) {
        let project = project_of(&open).unwrap_or(ran.project);
        return Ok(SessionTaken {
            agent_id: open,
            project,
        });
    }
    resume_session(
        core,
        SessionChoice {
            provider: ran.provider,
            session_id: ran.session_id,
            project: PathBuf::from(&ran.project),
            cwd: found.map(|saved| saved.project),
            title: None,
        },
    )
    .await
}

/// Starts a new chat that waits for the person, and answers with its agent.
pub(crate) async fn start_waiting(core: &Arc<Core>, choice: ChatChoice) -> CoreResult<String> {
    let (agent_id, _) = start_new(core, None, choice).await?;
    Ok(agent_id)
}

/// The provider a saved session of `kind` last ran on, and its model unless `model` is
/// chosen, so taking the session up again keeps running there.
async fn resumed_on(
    core: &Arc<Core>,
    kind: &str,
    session: String,
    model: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(agent) = AgentKind::from_name(kind) else {
        return (None, model);
    };
    let core = core.clone();
    let last = blocking(move || {
        Ok(
            crate::sessions::last_model(agent, &session).and_then(|(provider, last_model)| {
                let provider = core.workspaces.providers()?.named_by_agent(&provider)?;
                Some((provider.id.clone(), last_model))
            }),
        )
    });
    match last.await {
        Ok(Some((id, last_model))) => (Some(id), model.or(Some(last_model))),
        _ => (None, model),
    }
}

async fn start_new(
    core: &Arc<Core>,
    client: Option<&Arc<ClientConn>>,
    choice: ChatChoice,
) -> CoreResult<(String, ChatStart)> {
    let agent_id = crate::server::names::new_agent_id();
    let kind = LauncherId::parse(&choice.launcher).kind.to_owned();
    let project = core.workspaces.project_path(&choice.project)?;
    let resumed = match choice.cwd {
        Some(cwd) => {
            Some(blocking(move || Ok(super::worktree::agent::resumed_in(&cwd, None)?)).await?)
        }
        None => None,
    };
    let (cwd, worktree) = resumed.as_ref().map_or((project, None), |resumed| {
        (resumed.cwd.clone(), resumed.worktree.clone())
    });
    if let Some(launcher) = core.workspaces.terminal_launcher(&choice.launcher) {
        core.workspaces.put_agent(WorkspaceAgent {
            profile_id: LauncherId::parse(&launcher.id).account.map(str::to_owned),
            resume_id: choice.resume_id,
            worktree,
            terminal: true,
            ..new_agent(
                agent_id.clone(),
                kind.clone(),
                choice.project,
                cwd,
                choice.permission_mode.unwrap_or(launcher.permission_mode),
            )
        })?;
        if let Some(resumed) = &resumed {
            super::worktree::agent::set_up_made_again(core, &agent_id, resumed);
        }
        let size = super::spare::opening_size(core);
        if let Err(error) = start_terminal(core, &agent_id, size, None).await {
            super::workspace::close_gone(core, std::slice::from_ref(&agent_id));
            return Err(error);
        }
        remember_kind(core, &kind);
        return Ok((agent_id, not_started()));
    }
    let pick = (choice.resume_id.is_none())
        .then(|| core.workspaces.model_pick(&kind))
        .flatten()
        .unwrap_or_default();
    let (model_provider, model) = match &choice.resume_id {
        Some(session) => resumed_on(core, &kind, session.clone(), choice.model).await,
        None => (None, choice.model.or(pick.model)),
    };
    let chosen = Launch {
        agent_id: agent_id.clone(),
        launcher: choice.launcher,
        cwd,
        permission_mode: choice.permission_mode,
        model,
        model_provider,
        effort: choice.effort.or(pick.effort),
        resume_id: choice.resume_id,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    let started = start(core, client, chosen).await?;
    super::workspace::retitle(core, &agent_id, choice.title, false);
    if let Some(worktree) = worktree {
        core.workspaces
            .edit_agent(&agent_id, |agent| agent.worktree = Some(worktree))?;
    }
    if let Some(resumed) = &resumed {
        super::worktree::agent::set_up_made_again(core, &agent_id, resumed);
    }
    remember_kind(core, &kind);
    if started.unstarted() {
        return Ok((agent_id, not_started()));
    }
    match chat::until_started(core, started).await {
        Ok(start) => Ok((agent_id, start)),
        Err(error) => {
            super::workspace::close_gone(core, &[agent_id]);
            Err(error)
        }
    }
}

/// What a client starting an agent hears before its chat session has begun, or of an
/// agent that runs in its terminal.
fn not_started() -> ChatStart {
    ChatStart {
        session_id: String::new(),
        capabilities: Value::Null,
        setup: Value::Null,
    }
}

/// Keeps the kind of agent a chat was started as, so a project with no agent of its own
/// starts its next chat as that kind.
fn remember_kind(core: &Core, kind: &str) {
    if core.preferences.get().last_agent_type.as_deref() == Some(kind) {
        return;
    }
    if let Err(error) =
        super::preferences::set(core, PreferenceChange::LastAgentType(Some(kind.into())))
    {
        eprintln!("sikemux core: the kind of agent last started was not kept: {error}");
    }
}

/// Takes up a recent chat again, answering like [`start_chat`].
pub(crate) fn resume_chat(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    choice: ResumeChoice,
) {
    match core
        .workspaces
        .resume_choice(choice, &core.chats.list(), &core.triggers.held())
    {
        Ok(choice) => start_chat(core, client, request_id, choice),
        Err(error) => client.respond(request_id, Err(error)),
    }
}

/// A saved agent to start; one moved to another agent before it ran waits for the person.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    pub agent_id: String,
    #[serde(default)]
    pub waits_for_person: bool,
    /// Drops a chat still waiting for the person first, as it would otherwise keep the launch
    /// it was made with and not what its record says now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub again: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalParams {
    agent_id: String,
    cols: u16,
    rows: u16,
}

/// Starts the agent's terminal at the size the app shows it.
pub(crate) async fn start_terminal_at(
    core: &Arc<Core>,
    params: TerminalParams,
) -> CoreResult<Value> {
    let size = (params.cols, params.rows);
    let pty = start_terminal(core, &params.agent_id, size, None).await?;
    Ok(json!({ "ptyId": pty }))
}

pub(crate) async fn spawn(
    core: Arc<Core>,
    launch: LaunchIdentity,
    mut target: Box<SpawnTarget>,
    started_by: Option<String>,
) -> CoreResult<PendingStart> {
    let mut model_grant = None;
    if let SpawnTarget::Terminal(spawn) = target.as_mut() {
        core.spares.note_size(spawn.cols, spawn.rows);
        model_grant = terminal_agent(&core, spawn).await;
    }
    blocking(move || {
        let mut prepared = match *target {
            SpawnTarget::Terminal(mut spawn) => {
                let folder = spawn.cwd.clone().or_else(|| {
                    spawn
                        .context
                        .as_ref()
                        .and_then(|context| context.project.clone())
                        .filter(|project| !project.is_empty())
                });
                if let Some(folder) = &folder {
                    give_agent_environment(&core, folder, &mut spawn.env);
                }
                let resumed = super::activity_log::resumed_by(&spawn);
                let prepared = prepare_terminal(&launch, spawn)?;
                let owner = &prepared.owner;
                if let Some(resumed) = resumed {
                    super::activity_log::terminal_launched(&core, owner, resumed);
                }
                if let (Some(agent_id), Some(folder)) = (&owner.agent_id, folder) {
                    core.name_agent(
                        agent_id,
                        std::path::Path::new(&folder),
                        owner.agent_type.as_deref(),
                        None,
                    );
                }
                prepared
            }
            SpawnTarget::Task { mut request } => {
                give_agent_environment(&core, &request.cwd, &mut request.env);
                prepare_task(&launch, request)?
            }
        };
        prepared.owner.started_by = started_by;
        prepared.model_grant = model_grant;
        super::session::spawn_session(&core, prepared)
    })
    .await
}

fn give_agent_environment(
    core: &Core,
    folder: &str,
    env: &mut std::collections::HashMap<String, String>,
) {
    let given = super::plugins::agent_environment(core, std::path::Path::new(folder));
    for (key, value) in given {
        env.entry(key).or_insert(value);
    }
}
