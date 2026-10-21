//! What `agent_spawn` answers: an agent starting another as a full chat of
//! its own, or in a terminal the app opens, on its account and in its
//! permission mode, in a tab that does not move the person there. Each time
//! the new agent's turn ends, its final answer goes back to the agent that
//! started it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_pty::agent_detection::AgentKind;

use crate::cli::protocol::HarnessRequest;
use crate::launch::providers::ROUTED_ENV;
use crate::protocol::Event;

use super::chat;
use super::connection::blocking;
use super::follow::{self, TerminalAnswer};
use super::hosts::spawn::{self as elsewhere, Hired};
use super::messages;
use super::names::{self, Adopted, AgentName, Worker};
use super::sandbox::{self, AgentWorktree};
use super::saved::{self, Saved};
use super::workspace::{Lead, Paid};
use super::Core;

const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_MODEL_CHARS: usize = 200;
const MAX_SESSION_ID_CHARS: usize = 4_096;
/// How much of a worker's final answer its lead is sent.
const MAX_REPORT_BYTES: usize = 6 * 1024;

/// How long a terminal agent may take to show its prompt box before its lead
/// hears that it has not got its task yet.
const TERMINAL_READY_WAIT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(crate) struct Spawning {
    /// Spawns one at a time, so two at once cannot both take the last place a
    /// cap leaves.
    one_at_a_time: Mutex<()>,
    /// Terminal agents the window is opening, which count against the caps
    /// before they run.
    opening: Mutex<HashSet<String>>,
    /// Where the turn each terminal worker last reported starts in its session
    /// file. Its screen redrawing after a turn reads as another turn, which
    /// must not report again.
    reported: Mutex<HashMap<String, u64>>,
}

impl Spawning {
    fn one_at_a_time(&self) -> std::sync::MutexGuard<'_, ()> {
        self.one_at_a_time.locked()
    }

    fn opening(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.opening.locked()
    }

    fn reported(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.reported.locked()
    }
}

/// The agents running here and those the window is opening.
/// The agents that keep their names, with those being opened now.
fn open(core: &Core) -> HashSet<String> {
    let mut open = core.open_agents();
    open.extend(core.harness.spawning.opening().iter().cloned());
    open
}

fn text_param<'a>(params: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() && text.len() <= max => Ok(Some(text)),
        Some(Value::String(text)) if !text.trim().is_empty() => Err(format!(
            "{key} is {} bytes, more than the {max} it may hold",
            text.len()
        )),
        Some(_) => Err(format!("{key} must be nonempty text")),
    }
}

/// Whether the agent gets a checkout of its own, cut from its lead's, rather
/// than working in its lead's beside the other agents there.
fn own_checkout(params: &Value) -> Result<bool, String> {
    match params.get("checkout") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(checkout)) if checkout == "own" => Ok(true),
        Some(Value::String(checkout)) if checkout == "shared" => Ok(false),
        Some(_) => Err("checkout must be own or shared".into()),
    }
}

fn reports_back(params: &Value) -> Result<bool, String> {
    match params.get("report") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(reports)) => Ok(*reports),
        Some(_) => Err("report must be a boolean".into()),
    }
}

/// What a spawn came to: a new agent, or the agent that already has the
/// session it was asked to reopen.
enum Opened {
    New(Started),
    AlreadyOpen(String, Option<String>),
}

struct Started {
    agent_id: String,
    name: String,
    provider: String,
    model: Option<String>,
    paid_by: Option<String>,
    lead_name: String,
    lead_cli: String,
    reports: bool,
    chat: Arc<chat::Chat>,
    /// The checkout of its own it works in, set up before it gets its task.
    checkout: Option<Box<AgentWorktree>>,
}

/// How the calling agent runs and where, for its worker to do the same.
pub(crate) fn lead_of(
    core: &Core,
    lead_id: &str,
    project: &str,
) -> Result<(Lead, PathBuf), String> {
    let run = core.workspaces.run_of(lead_id);
    if let Some(chat) = core.chats.get(lead_id) {
        let launch = chat.current_now();
        return Ok((
            Lead {
                provider: chat.provider().to_owned(),
                permission_mode: Some(launch.permission_mode.clone()),
                launcher: chat
                    .origin
                    .launcher
                    .clone()
                    .or_else(|| run.map(|run| run.launcher)),
                launch: Some(launch),
            },
            chat.launch.cwd.clone(),
        ));
    }
    let session = core
        .agent_session(lead_id)
        .ok_or("agent_spawn works only for an agent running in Sikemux")?;
    let info = session.info();
    let cwd = info.project.unwrap_or_else(|| project.to_owned());
    Ok((
        Lead {
            provider: info.agent_type.unwrap_or_default(),
            launch: None,
            launcher: run.as_ref().map(|run| run.launcher.clone()),
            permission_mode: run.map(|run| run.permission_mode),
        },
        PathBuf::from(cwd),
    ))
}

/// Names a worker for its lead, one spawn at a time.
pub(crate) fn hire(core: &Core, worker: Worker<'_>) -> Result<AgentName, String> {
    let _one_at_a_time = core.harness.spawning.one_at_a_time();
    core.names.hire(worker, open(core))
}

/// A worker here for a lead on a linked host, as that host named it.
pub(crate) struct ForHost {
    pub agent_id: String,
    pub name: String,
    pub lead_id: String,
    /// The lead's name with its host's, as the worker addresses it.
    pub lead_name: String,
    pub lead_cli: String,
    /// The lead's permission mode, which bounds the worker's.
    pub mode: Option<String>,
    pub provider: String,
    pub model: Option<String>,
    pub reports: bool,
    pub cwd: PathBuf,
    pub prompt: String,
}

/// Starts a worker for a lead on a linked host, on this host's own sign-in
/// for its CLI, and hands it its task once it is ready.
pub(crate) fn start_for_host(core: &Arc<Core>, plan: ForHost) -> Result<Value, String> {
    let _one_at_a_time = core.harness.spawning.one_at_a_time();
    let lead = Lead {
        provider: String::new(),
        launch: None,
        launcher: None,
        permission_mode: plan.mode.clone(),
    };
    let (mut launch, launcher) = core.workspaces.worker_launch(
        &lead,
        &plan.provider,
        &plan.cwd,
        plan.model.clone(),
        Paid::By(None),
    )?;
    if let Some(mode) = plan
        .mode
        .as_ref()
        .filter(|_| plan.lead_cli == plan.provider)
    {
        launch.permission_mode = mode.clone();
    }
    let project = names::project_of(&launch.cwd, &core.workspaces.project_paths());
    let named = core.names.adopt(
        Adopted {
            agent_id: &plan.agent_id,
            name: &plan.name,
            project,
            parent: Some(&plan.lead_id),
            provider: &plan.provider,
            reports: plan.reports,
            depth: 0,
        },
        open(core),
    );
    launch.agent_id = plan.agent_id.clone();
    let model = launch.model.clone();
    let chat = chat::begin_worker(core, launch, &plan.lead_id, launcher).map_err(|error| {
        core.names.forget(&plan.agent_id);
        error.to_string()
    })?;
    core.broadcast_local(&Event::ChatBegun { chat: chat.info() });
    core.view_changed();
    let answer = json!({
        "agentId": plan.agent_id,
        "name": named.name,
        "cli": plan.provider,
        "model": model,
    });
    let started = Started {
        agent_id: plan.agent_id,
        name: named.name,
        provider: plan.provider,
        model,
        paid_by: None,
        lead_name: plan.lead_name,
        lead_cli: plan.lead_cli,
        reports: plan.reports,
        chat,
        checkout: None,
    };
    let core = core.clone();
    tokio::spawn(async move { hand_task(&core, started, plan.prompt).await });
    Ok(answer)
}

fn start(
    core: &Arc<Core>,
    request: &HarnessRequest,
    resume: Option<&Saved>,
    hired: Option<&Hired>,
) -> Result<Opened, String> {
    let lead_id = request
        .agent_id
        .clone()
        .ok_or("agent_spawn needs the calling agent's id")?;
    let params = &request.params;
    let cli = text_param(params, "cli", 64)?.map(|cli| cli.trim().to_lowercase());
    let model = text_param(params, "model", MAX_MODEL_CHARS)?.map(|model| model.trim().to_owned());
    let named_provider = text_param(params, "provider", 64)?.map(|id| id.trim().to_owned());
    let reports = reports_back(params)?;
    let own = own_checkout(params)?;
    if own && resume.is_some() {
        return Err("resume reopens a session where it ran; leave checkout out".into());
    }
    let _one_at_a_time = core.harness.spawning.one_at_a_time();
    if let Some((agent_id, name)) =
        resume.and_then(|saved| saved::open_now(core, &saved.cli, &saved.session_id))
    {
        return Ok(Opened::AlreadyOpen(agent_id, name));
    }
    let (lead, lead_cwd) = lead_of(core, &lead_id, &request.project)?;
    let cwd = resume.map_or(lead_cwd, |saved| saved.project.clone());
    let (checkout, cwd) = if own {
        let name = text_param(params, "prompt", MAX_PROMPT_BYTES)?.unwrap_or("teammate");
        let name: String = name.chars().take(48).collect();
        let (checkout, tree_cwd) = sandbox::cut_for(&cwd, &cwd, &name)?;
        (Some(Box::new(checkout)), tree_cwd)
    } else {
        (None, cwd)
    };
    let helper = core.workspaces.helper().filter(|helper| {
        resume.is_none()
            && model.is_none()
            && named_provider.is_none()
            && cli.as_ref().is_none_or(|cli| *cli == helper.agent)
    });
    let provider = resume
        .map(|saved| saved.cli.clone())
        .or(cli)
        .or_else(|| helper.as_ref().map(|helper| helper.agent.clone()))
        .or_else(|| (!lead.provider.is_empty()).then(|| lead.provider.clone()))
        .ok_or("cli must name the agent to start, such as claude or codex")?;
    let (model, paid) = match (helper, named_provider) {
        (_, Some(id)) => (model, Paid::By(Some(id))),
        (Some(helper), None) => (helper.model, Paid::By(helper.provider)),
        (None, None) => (model, Paid::AsLead),
    };
    let (mut launch, launcher) = core
        .workspaces
        .worker_launch(&lead, &provider, &cwd, model, paid)?;
    if let Some(saved) = resume {
        saved::same_account(&provider, &launch, saved)?;
    }
    launch.resume_id = resume.map(|saved| saved.session_id.clone());
    let project = names::project_of(&launch.cwd, &core.workspaces.project_paths());
    let (agent_id, named) = match hired {
        Some(hired) => {
            let named = core.names.adopt(
                Adopted {
                    agent_id: &hired.agent_id,
                    name: &hired.name,
                    project,
                    parent: Some(&lead_id),
                    provider: &provider,
                    reports,
                    depth: 0,
                },
                open(core),
            );
            (hired.agent_id.clone(), named)
        }
        None => {
            let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
            let named = core.names.hire(
                Worker {
                    agent_id: &agent_id,
                    lead: &lead_id,
                    project: Some(project),
                    provider: &provider,
                    session: resume.map(|saved| saved.session_id.as_str()),
                    reports,
                },
                open(core),
            )?;
            (agent_id, named)
        }
    };
    launch.agent_id = agent_id.clone();
    let model = launch.model.clone();
    let paid_by = launch.env.get(ROUTED_ENV).cloned();
    let chat = chat::begin_worker(core, launch, &lead_id, launcher).map_err(|error| {
        core.names.forget(&agent_id);
        error.to_string()
    })?;
    if let Some(checkout) = &checkout {
        let mut patch = serde_json::Map::new();
        patch.insert(
            "worktree".into(),
            serde_json::to_value(checkout).map_err(|error| error.to_string())?,
        );
        if let Err(error) = core.workspaces.update_agent(&agent_id, patch) {
            eprintln!("sikemux core: a worker's own checkout was not kept on its record: {error}");
        }
    }
    core.broadcast_local(&Event::ChatBegun { chat: chat.info() });
    core.view_changed();
    let lead_name = core.names.name(&lead_id).unwrap_or_else(|| lead_id.clone());
    Ok(Opened::New(Started {
        agent_id,
        name: named.name,
        provider,
        model,
        paid_by,
        lead_name,
        lead_cli: if lead.provider.is_empty() {
            "agent".to_owned()
        } else {
            lead.provider
        },
        reports,
        chat,
        checkout,
    }))
}

pub(crate) async fn spawn(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let prompt = match request.params.get("prompt") {
        Some(Value::String(prompt)) if !prompt.trim().is_empty() && prompt.len() <= MAX_PROMPT_BYTES => {
            prompt.clone()
        }
        Some(Value::String(prompt)) if !prompt.trim().is_empty() => {
            return Err(format!(
                "prompt is {} bytes; it holds at most {MAX_PROMPT_BYTES}, so put long material in a file and name its path",
                prompt.len()
            ))
        }
        _ => return Err("prompt must be the new agent's task, as nonempty text".into()),
    };
    if let Some(host) = text_param(&request.params, "host", 200)? {
        return elsewhere::spawn(core, request, host, prompt).await;
    }
    let resume = text_param(&request.params, "resume", MAX_SESSION_ID_CHARS)?.map(str::to_owned);
    match request.params.get("mode") {
        None | Some(Value::Null) => {}
        Some(Value::String(mode)) if mode == "chat" => {}
        Some(Value::String(mode)) if mode == "terminal" && resume.is_none() => {
            return spawn_terminal(core, request, prompt).await
        }
        Some(Value::String(mode)) if mode == "terminal" => {
            return Err("resume reopens the session as a chat; leave mode out".into())
        }
        Some(_) => return Err("mode must be chat or terminal".into()),
    }
    let saved = match resume {
        Some(session_id) => {
            let (owner, cli) = (core.clone(), request.params.get("cli").cloned());
            let saved = blocking(move || {
                let cli = cli.as_ref().and_then(Value::as_str).map(str::to_lowercase);
                saved::locate(&owner, session_id.trim(), cli.as_deref()).map_err(Into::into)
            })
            .await
            .map_err(|error| error.to_string())?;
            if let Some((agent_id, name)) = &saved.open {
                return send_to_open(core, request, &saved, agent_id, name.as_deref(), prompt)
                    .await;
            }
            Some(saved)
        }
        None => None,
    };
    let hired = match (&saved, &request.agent_id) {
        (None, Some(lead)) => {
            let provider = text_param(&request.params, "cli", 64)?
                .map(|cli| cli.trim().to_lowercase())
                .or_else(|| core.chats.get(lead).map(|chat| chat.provider().to_owned()))
                .unwrap_or_default();
            elsewhere::hire_elsewhere(core, lead, &provider, reports_back(&request.params)?).await?
        }
        _ => None,
    };
    let opened = {
        let (owner, request, resumed) = (core.clone(), request.clone(), saved.clone());
        let named = hired.clone();
        blocking(move || {
            start(&owner, &request, resumed.as_ref(), named.as_ref()).map_err(Into::into)
        })
        .await
        .map_err(|error| {
            if let Some(hired) = &hired {
                elsewhere::never_started(core, hired);
            }
            error.to_string()
        })?
    };
    let started = match (opened, &saved) {
        (Opened::New(started), _) => started,
        (Opened::AlreadyOpen(agent_id, name), Some(saved)) => {
            return send_to_open(core, request, saved, &agent_id, name.as_deref(), prompt).await
        }
        (Opened::AlreadyOpen(..), None) => unreachable!("only a resumed session is already open"),
    };
    let reports = started.reports;
    let starting = match &saved {
        Some(saved) => format!(
            "{} is reopening \"{}\" in its own tab",
            started.name, saved.title
        ),
        None => format!("{} is starting in its own tab", started.name),
    };
    let answer = json!({
        "name": started.name,
        "agentId": started.agent_id,
        "cli": started.provider,
        "model": started.model,
        "provider": started.paid_by,
        "resumed": saved.as_ref().map(|saved| &saved.session_id),
        "reportsBack": reports,
        "note": format!(
            "{starting} and gets your prompt once it is ready.{}",
            if reports { " Each time its turn ends, its final answer comes to you as a message." } else { "" }
        ),
    });
    let core = core.clone();
    tokio::spawn(async move { hand_task(&core, started, prompt).await });
    Ok(answer)
}

/// Hands the prompt to the agent that already has the session open, as
/// `agent_send` would, rather than opening a second copy of it.
async fn send_to_open(
    core: &Arc<Core>,
    request: &HarnessRequest,
    saved: &Saved,
    agent_id: &str,
    name: Option<&str>,
    prompt: String,
) -> Result<Value, String> {
    let mut message = request.clone();
    message.params = json!({ "to": agent_id, "text": prompt });
    let sent = messages::send(core, &message).await?;
    let name = name.unwrap_or(agent_id);
    Ok(json!({
        "name": name,
        "agentId": agent_id,
        "cli": saved.cli,
        "alreadyOpen": true,
        "sent": sent,
        "note": format!("{name} already has \"{}\" open, so your prompt went to it as a message, as agent_send sends one.", saved.title),
    }))
}

/// Gives the new agent its task once it is ready, or tells its lead it
/// never got there.
async fn hand_task(core: &Arc<Core>, started: Started, prompt: String) {
    let Started {
        agent_id,
        name,
        lead_name,
        lead_cli,
        reports,
        chat,
        checkout,
        ..
    } = started;
    let lead_id = chat.origin.started_by.clone().unwrap_or_default();
    if let Some(checkout) = checkout {
        let cwd = chat.launch.cwd.clone();
        if let Some(error) = sandbox::set_up(core, &checkout.repo, Some(&agent_id), &cwd).await {
            messages::report_as(
                core,
                agent_id.clone(),
                Some(name.clone()),
                lead_id.clone(),
                format!("{name}'s own checkout is not fully set up: {error}"),
            )
            .await;
        }
    }
    let failed = match chat::until_started(core, chat).await {
        Ok(_) => {
            let task = messages::task(&lead_id, &lead_name, &lead_cli, reports, prompt);
            match messages::hand_task(core, &agent_id, task).await {
                Ok(()) => return,
                Err(error) => error,
            }
        }
        Err(error) => error.to_string(),
    };
    messages::report_as(
        core,
        agent_id,
        Some(name.clone()),
        lead_id,
        format!("{name} did not start, so it never got your prompt: {failed}"),
    )
    .await;
}

/// A terminal agent the window is asked to open.
struct Opening {
    agent_id: String,
    name: String,
    provider: String,
    model: Option<String>,
    permission_mode: String,
    launcher: Option<String>,
    lead_id: String,
    lead_name: String,
    lead_cli: String,
    reports: bool,
}

/// Names a terminal worker and says how it runs, for the window to open it.
fn plan_terminal(core: &Core, request: &HarnessRequest) -> Result<Opening, String> {
    let lead_id = request
        .agent_id
        .clone()
        .ok_or("agent_spawn needs the calling agent's id")?;
    let params = &request.params;
    let provider = text_param(params, "cli", 64)?
        .ok_or("cli must name the agent to start, such as claude or codex")?
        .trim()
        .to_lowercase();
    if AgentKind::from_label(&provider).is_none() {
        return Err(format!(
            "{provider} is not an agent Sikemux runs in a terminal"
        ));
    }
    let model = text_param(params, "model", MAX_MODEL_CHARS)?.map(|model| model.trim().to_owned());
    if text_param(params, "provider", 64)?.is_some() {
        return Err("provider works only for a chat; leave mode out to start one".into());
    }
    let reports = reports_back(params)?;
    if super::hosts::route::lead_host(core, &lead_id).is_some() {
        return Err("your team's lead runs on another host, so the agents you start run as chats; leave out mode".into());
    }
    let _one_at_a_time = core.harness.spawning.one_at_a_time();
    let (lead, _) = lead_of(core, &lead_id, &request.project)?;
    let same = lead.provider == provider;
    let permission_mode = lead.worker_mode();
    let model = model.or_else(|| {
        lead.launch
            .as_ref()
            .filter(|_| same)
            .and_then(|launch| launch.model.clone())
    });
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let named = core.names.hire(
        Worker {
            agent_id: &agent_id,
            lead: &lead_id,
            project: None,
            provider: &provider,
            session: None,
            reports,
        },
        open(core),
    )?;
    core.harness.spawning.opening().insert(agent_id.clone());
    let lead_name = core.names.name(&lead_id).unwrap_or_else(|| lead_id.clone());
    Ok(Opening {
        agent_id,
        name: named.name,
        model,
        permission_mode,
        launcher: lead.launcher.filter(|_| same),
        lead_cli: if lead.provider.is_empty() {
            "agent".to_owned()
        } else {
            lead.provider
        },
        provider,
        lead_id,
        lead_name,
        reports,
    })
}

/// Starts the terminal agent in its lead's project, as an agent of the
/// workspace every client shows, on the lead's account when it runs the same CLI.
async fn open_terminal(core: &Arc<Core>, opening: &Opening) -> Result<(), String> {
    let lead = core
        .workspaces
        .agent(&opening.lead_id)
        .ok_or("the agent that asked for it is no longer open in Sikemux")?;
    let same = lead.provider == opening.provider;
    let on_launcher = opening.launcher.as_deref().and_then(|launcher| {
        launcher
            .strip_prefix(&format!("{}:", opening.provider))
            .map(str::to_owned)
    });
    let agent = json!({
        "id": opening.agent_id,
        "provider": opening.provider,
        "project": lead.project,
        "cwd": lead.cwd,
        "title": opening.provider,
        "lead": lead.id,
        "permissionMode": opening.permission_mode,
        "profileId": on_launcher.or(lead.profile_id.filter(|_| same)),
        "model": opening.model,
        "effort": lead.effort.filter(|_| same),
        "worktree": lead.worktree,
    });
    let Value::Object(agent) = agent else {
        return Err("the agent could not be described".into());
    };
    core.workspaces
        .put_agent(agent)
        .map_err(|error| error.to_string())?;
    super::launching::start_terminal(core, &opening.agent_id, (100, 30), None)
        .await
        .map(drop)
        .map_err(|error| error.to_string())
}

async fn spawn_terminal(
    core: &Arc<Core>,
    request: &HarnessRequest,
    prompt: String,
) -> Result<Value, String> {
    let opening = {
        let (core, request) = (core.clone(), request.clone());
        blocking(move || plan_terminal(&core, &request).map_err(Into::into))
            .await
            .map_err(|error| error.to_string())?
    };
    let opened = open_terminal(core, &opening).await;
    core.harness.spawning.opening().remove(&opening.agent_id);
    if let Err(error) = opened {
        core.names.forget(&opening.agent_id);
        return Err(format!("{} did not start: {error}", opening.name));
    }
    let task = messages::task(
        &opening.lead_id,
        &opening.lead_name,
        &opening.lead_cli,
        opening.reports,
        prompt,
    );
    core.harness
        .mailbox
        .returned(&opening.agent_id, task.clone());
    let watcher = core.clone();
    let (agent_id, name, lead_id) = (
        opening.agent_id.clone(),
        opening.name.clone(),
        opening.lead_id.clone(),
    );
    tokio::spawn(async move {
        tokio::time::sleep(TERMINAL_READY_WAIT).await;
        if messages::is_waiting(&watcher, &agent_id, &task)
            && watcher.agent_session(&agent_id).is_some()
        {
            messages::report_as(
                &watcher,
                agent_id,
                Some(name.clone()),
                lead_id,
                format!("{name} has not shown its prompt in its terminal yet, so it has not got your prompt. It may be waiting on the person, such as to trust the folder; it gets your prompt once it is ready."),
            )
            .await;
        }
    });
    Ok(json!({
        "name": opening.name,
        "agentId": opening.agent_id,
        "cli": opening.provider,
        "model": opening.model,
        "mode": "terminal",
        "reportsBack": opening.reports,
        "note": format!(
            "{} is starting in a terminal tab and gets your prompt once its prompt box shows.{}",
            opening.name,
            if opening.reports { " Each time its turn ends, its final answer comes to you as a message." } else { "" }
        ),
    }))
}

/// How a worker's turn, or the worker, came to an end.
pub(crate) enum Ending {
    Finished,
    Stopped,
    Failed(String),
    /// The chat ended, `mid_turn` or while the worker was idle.
    Gone {
        reason: Option<String>,
        mid_turn: bool,
    },
}

/// Sends a worker's lead how the worker's turn ended.
pub(crate) fn turn_ended(core: &Arc<Core>, agent_id: &str, ending: Ending) {
    super::board::auto_turn_ended(core, agent_id, &ending);
    let ignored = super::board::offer_turn_ignored(core, agent_id);
    let Some(lead) = core.names.reports_to(agent_id) else {
        return;
    };
    if ignored && matches!(ending, Ending::Finished) {
        return;
    }
    let chat_answer = core
        .chats
        .get(agent_id)
        .map(|chat| chat.feed.last_answer().map(TerminalAnswer::Said));
    let (core, worker) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        let answer = match chat_answer {
            Some(answer) => answer,
            None => {
                let turn = {
                    let (core, worker) = (core.clone(), worker.clone());
                    blocking(move || Ok(follow::terminal_answer(&core, &worker))).await
                };
                let Ok(turn) = turn else { return };
                let running = core.running_agents();
                let mut reported = core.harness.spawning.reported();
                reported.retain(|agent, _| *agent == worker || running.contains(agent));
                if matches!(ending, Ending::Finished) {
                    if let Some(start) = turn.start {
                        if reported.insert(worker.clone(), start) == Some(start) {
                            return;
                        }
                    }
                } else {
                    reported.remove(&worker);
                }
                drop(reported);
                turn.answer
            }
        };
        let text = report(&core, &worker, ending, answer);
        elsewhere::send_commits(&core, &worker).await;
        messages::report(&core, worker, lead, text).await;
    });
}

/// [`turn_ended`] for an agent in a terminal.
pub(crate) fn terminal_turn_ended(core: &Core, agent_id: &str, ending: Ending) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if let Some(core) = core.arc() {
        turn_ended(&core, agent_id, ending);
    }
}

/// What the lead is told about the worker's turn.
fn report(core: &Core, worker: &str, ending: Ending, answer: Option<TerminalAnswer>) -> String {
    let because = |reason: Option<String>| match reason.filter(|reason| !reason.trim().is_empty()) {
        Some(reason) => format!(": {reason}"),
        None => ".".to_owned(),
    };
    let wrote = |answer: TerminalAnswer, heading: &str| match answer {
        TerminalAnswer::Said(said) => format!("{heading}\n\n{}", cut(&said)),
        TerminalAnswer::Screen(screen) => format!(
            "Sikemux cannot read its session, so this is the end of its screen:\n\n{}",
            cut(&screen)
        ),
    };
    let (what, last) = match ending {
        Ending::Finished => match answer {
            Some(answer) => (
                format!("Finished its turn. {}", wrote(answer, "Its final answer:")),
                None,
            ),
            None => (
                "Finished its turn without writing an answer.".to_owned(),
                None,
            ),
        },
        Ending::Stopped => (
            "Was stopped before it finished its turn.".to_owned(),
            answer,
        ),
        Ending::Failed(reason) => (format!("Its turn failed{}", because(Some(reason))), answer),
        Ending::Gone {
            reason,
            mid_turn: true,
        } => (
            format!(
                "Stopped running before it finished its turn{}",
                because(reason)
            ),
            answer,
        ),
        Ending::Gone {
            reason,
            mid_turn: false,
        } => (format!("Stopped running{}", because(reason)), None),
    };
    let mut text = match last {
        Some(last) => format!("{what}\n\n{}", wrote(last, "What it wrote last:")),
        None => what,
    };
    if let Some(card) = super::board::card_line(core, worker) {
        text.push_str(&format!("\n\n{card}"));
    }
    text
}

/// `answer` within the report's size, its start and its end kept: a report
/// ends with what is left undone, which its lead most needs.
fn cut(answer: &str) -> String {
    if answer.len() <= MAX_REPORT_BYTES {
        return answer.to_owned();
    }
    let half = MAX_REPORT_BYTES / 2;
    let mut end = half;
    while !answer.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = answer.len() - half;
    while !answer.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "{}\n\n[{} KB in the middle left out; read it with agent_read.]\n\n{}",
        answer[..end].trim_end(),
        (start - end).div_ceil(1024),
        answer[start..].trim_start()
    )
}
