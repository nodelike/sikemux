//! What `agent_spawn` answers: an agent starting another as a full chat of
//! its own, or in a terminal the app opens, on its account and in its
//! permission mode, in a tab that does not move the person there. Each time
//! the new agent's turn ends, its final answer goes back to the agent that
//! started it.

pub(crate) use super::chat::Ending;
use crate::server::activity::ActivityCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_view::workspace::WorkspaceAgent;

use crate::acp::failure::{out_of_usage, PAUSED_FOR_AGENTS};
use crate::cli::protocol::HarnessRequest;
use crate::launch::PermissionMode;
use sikemux_wire::protocol::Event;

use super::caller::text_param;
use super::chat;
use super::connection::blocking;
use super::follow::{self, TerminalAnswer};
use super::hosts::spawn::{self as elsewhere, Hired};
use super::launching;
use super::messages;
use super::names::{Adopted, AgentName, Worker};
use super::overlap;
use super::saved::{self, Saved};
use super::workspace::{new_agent, LauncherId};
use super::workspace::{Lead, Paid};
use super::worktree::agent::Unclaimed;
use super::worktree::{self, AgentWorktree};
use super::Core;

const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_MODEL_CHARS: usize = 200;
const MAX_BRANCH_BYTES: usize = 200;
const MAX_REPO_BYTES: usize = 4096;
const MAX_SESSION_ID_CHARS: usize = 4_096;
const MAX_TITLE_CHARS: usize = 80;
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

/// The agents that keep their names, with those being opened now.
fn open(core: &Core) -> HashSet<String> {
    let mut open = core.open_agents();
    open.extend(core.harness().spawning.opening().iter().cloned());
    open
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

/// The reasoning effort asked for, when `provider`'s CLI takes it.
fn effort_param(params: &Value, provider: &str) -> Result<Option<String>, String> {
    let Some(effort) = text_param(params, "effort", 32)? else {
        return Ok(None);
    };
    let efforts = crate::agent_catalog::agent(provider)
        .map(|agent| agent.efforts)
        .unwrap_or_default();
    crate::agent_accounts::defaults::clean_effort(provider, effort)
        .map(Some)
        .ok_or_else(|| match efforts.is_empty() {
            true => format!("{provider} takes no effort; leave effort out"),
            false => format!("{provider} takes effort {}", efforts.join(", ")),
        })
}

/// Refuses a model `provider` would not run, such as a picker's label in place of its id,
/// before a worker is made whose first turn would fail on it. An agent that has not yet
/// offered its models is not checked.
fn known_model(core: &Core, provider: &str, model: &str) -> Result<(), String> {
    let Some(picker) = super::chat::models_of(core, provider) else {
        return Ok(());
    };
    let catalog = crate::agent_catalog::agent(provider);
    let mut ids: Vec<&str> = (picker.choices.iter())
        .map(|choice| choice.value.as_str())
        .collect();
    for id in (catalog.iter()).flat_map(|agent| agent.models_since.iter().map(|since| since.model))
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let aliases = catalog.map(|agent| agent.commit_models).unwrap_or_default();
    let bare = model.split_once('[').map_or(model, |(bare, _)| bare);
    if ids.contains(&bare) || aliases.contains(&model) || aliases.contains(&bare) {
        return Ok(());
    }
    let meant = (picker.choices.iter())
        .find(|choice| choice.label.eq_ignore_ascii_case(model))
        .map(|choice| format!("; for {}, pass {}", choice.label, choice.value))
        .unwrap_or_default();
    let mut shown: Vec<&str> = picker
        .choices
        .iter()
        .filter(|choice| {
            !choice.superseded && !crate::launch::providers::routed_model(&choice.value)
        })
        .map(|choice| choice.value.as_str())
        .collect();
    if shown.is_empty() {
        shown = ids;
    }
    let listed = if shown.len() > 20 {
        let head: Vec<&str> = shown.iter().copied().take(12).collect();
        format!(
            "{} (+{} more; call agent_list with models: \"{provider}\" for the full list)",
            head.join(", "),
            shown.len() - head.len()
        )
    } else {
        shown.join(", ")
    };
    Err(format!(
        "{provider} has no model {model}{meant}. Pass one of its ids: {listed}"
    ))
}

fn reports_back(params: &Value) -> Result<bool, String> {
    match params.get("report") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(reports)) => Ok(*reports),
        Some(_) => Err("report must be a boolean".into()),
    }
}

/// The short title of the task the new agent is started on, which names its row and its tab.
fn task_title(params: &Value) -> Result<String, String> {
    match text_param(params, "title", MAX_TITLE_CHARS * 4)?.map(str::trim) {
        Some(title) if !title.is_empty() && title.chars().count() <= MAX_TITLE_CHARS => {
            Ok(title.to_owned())
        }
        Some(title) if !title.is_empty() => Err(format!(
            "title is {} characters; keep it to {MAX_TITLE_CHARS}, such as \"Fix the cart total\"",
            title.chars().count()
        )),
        _ => Err("title must name the task in a few words, such as \"Fix the cart total\"".into()),
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
    let run = core.workspaces().run_of(lead_id);
    if let Some(chat) = core.chats().get(lead_id) {
        let launch = chat.current_now();
        return Ok((
            Lead {
                provider: chat.provider().to_owned(),
                permission_mode: Some(launch.permission_mode),
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
    let _one_at_a_time = core.harness().spawning.one_at_a_time();
    hire_held(core, worker)
}

/// Names a worker for its lead while this spawn already holds its turn.
fn hire_held(core: &Core, worker: Worker<'_>) -> Result<AgentName, String> {
    core.names().hire(worker, open(core))
}

/// What the agent holds that closing it would leave behind: files it changed and has not
/// committed, or commits in its own checkout that are not on its base branch yet.
pub(crate) fn held_work(core: &Core, agent_id: &str) -> Option<&'static str> {
    let agent = core.workspaces().agent(agent_id)?;
    let project = core
        .names()
        .get(agent_id)
        .map_or(agent.project.clone(), |named| named.project);
    if super::inbox::uncommitted(core, &project, agent_id, &agent.cwd) {
        return Some("has uncommitted changes");
    }
    agent
        .worktree
        .as_ref()
        .is_some_and(|worktree| !matches!(super::edits::unlanded(worktree), Ok(false)))
        .then_some("has unlanded commits")
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
    pub effort: Option<String>,
    pub reports: bool,
    pub cwd: PathBuf,
    pub prompt: String,
    /// The account's launcher it starts on, as this host picked it.
    pub launcher: Option<String>,
    /// The session it ran before it closed, which it takes up again.
    pub resume: Option<Saved>,
}

/// Starts a worker for a lead on a linked host, on this host's own sign-in
/// for its CLI, and hands it its task once it is ready.
pub(crate) fn start_for_host(core: &Arc<Core>, plan: ForHost) -> Result<Value, String> {
    let _one_at_a_time = core.harness().spawning.one_at_a_time();
    let lead = Lead {
        provider: String::new(),
        launch: None,
        launcher: plan.launcher.clone(),
        permission_mode: plan
            .mode
            .as_deref()
            .and_then(|mode| PermissionMode::parse(mode).ok()),
    };
    let (mut launch, launcher) = core.workspaces().worker_launch(
        &lead,
        &plan.provider,
        &plan.cwd,
        plan.model.clone(),
        Paid::By(None),
    )?;
    if let Some(effort) = plan.effort.clone() {
        launch.effort = Some(effort);
    }
    if let Some(saved) = &plan.resume {
        saved::same_account(&plan.provider, &launch, saved)?;
        launch.resume_id = Some(saved.session_id.clone());
    }
    if let Some(mode) = plan
        .mode
        .as_ref()
        .filter(|_| plan.lead_cli == plan.provider)
    {
        launch.permission_mode = PermissionMode::parse(mode)?;
    }
    let project = core.workspaces().project_of(&launch.cwd);
    let named = core.names().adopt(
        Adopted {
            agent_id: &plan.agent_id,
            name: &plan.name,
            project,
            parent: Some(&plan.lead_id),
            provider: &plan.provider,
            reports: plan.reports,
            task: None,
        },
        open(core),
    );
    launch.agent_id = plan.agent_id.clone();
    let model = launch.model.clone();
    let chat = chat::begin_worker(core, launch, &plan.lead_id, launcher).map_err(|error| {
        core.names().forget(&plan.agent_id);
        error.to_string()
    })?;
    core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
    core.changed(What::View);
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
    let reports = started.reports;
    let core = core.clone();
    tokio::spawn(async move { hand_task(&core, started, plan.prompt).await });
    Ok(woken_by_answers(answer, reports))
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
    let named_provider = text_param(params, "provider", 64)?
        .map(|id| id.trim().to_owned())
        .or_else(|| {
            let providers = core.workspaces().providers()?;
            Some(providers.routing(model.as_deref()?)?.id.clone())
        });
    let reports = reports_back(params)?;
    let task = task_title(params)?;
    let own = own_checkout(params)?;
    if own && resume.is_some() {
        return Err("resume reopens a session where it ran; leave checkout out".into());
    }
    let branch = text_param(params, "branch", MAX_BRANCH_BYTES)?.map(str::trim);
    if branch.is_some() && !own {
        return Err(
            "branch names the branch of an own checkout; pass checkout: \"own\" too".into(),
        );
    }
    let repo = text_param(params, "repo", MAX_REPO_BYTES)?.map(str::trim);
    if repo.is_some() && !own {
        return Err(
            "repo names the repository of an own checkout; pass checkout: \"own\" too".into(),
        );
    }
    let _one_at_a_time = core.harness().spawning.one_at_a_time();
    if let Some((agent_id, name)) =
        resume.and_then(|saved| saved::open_now(core, &saved.cli, &saved.session_id))
    {
        return Ok(Opened::AlreadyOpen(agent_id, name));
    }
    let (lead, lead_cwd) = lead_of(core, &lead_id, &request.project)?;
    let was = resume.and_then(|saved| {
        let agent_id = (core.names()).returning_id(Some(&saved.cli), &saved.session_id, open(core));
        core.workspaces().closed_agent(&agent_id)
    });
    let branch = (was.as_ref()).and_then(|was| Some(was.worktree.as_ref()?.branch.as_str()));
    let resumed = resume
        .map(|saved| worktree::agent::resumed_in(&saved.project, branch))
        .transpose()?;
    let cwd = resumed
        .as_ref()
        .map_or(lead_cwd, |resumed| resumed.cwd.clone());
    let (mut checkout, cwd) = if own {
        let branch = match branch {
            Some(branch) => branch.to_owned(),
            None => worktree::branch_for(
                text_param(params, "prompt", MAX_PROMPT_BYTES)?.unwrap_or_default(),
            ),
        };
        let from = repo.map_or_else(|| cwd.clone(), |repo| cwd.join(repo));
        let (checkout, tree_cwd) =
            worktree::agent::cut_for(&from, &cwd, &branch).map_err(|error| {
                match repo.is_none() && error.ends_with(worktree::agent::NOT_IN_GIT) {
                    true => format!("{error}; name the repository to cut it from with repo"),
                    false => error,
                }
            })?;
        (Unclaimed(Some(checkout)), tree_cwd)
    } else {
        (Unclaimed(None), cwd)
    };
    let helper = core.workspaces().helper().filter(|helper| {
        resume.is_none()
            && model.is_none()
            && named_provider.is_none()
            && cli.as_ref().is_none_or(|cli| *cli == helper.agent)
    });
    let provider = resume
        .map(|saved| saved.cli.clone())
        .or(cli)
        .or_else(|| helper.as_ref().map(|helper| helper.agent.clone()))
        .or_else(|| lead.cli().map(str::to_owned))
        .ok_or("cli must name the agent to start, such as claude or codex")?;
    if let (Some(model), None) = (&model, &named_provider) {
        known_model(core, &provider, model)?;
    }
    let (model, paid) = match (helper, named_provider) {
        (_, Some(id)) => (model, Paid::By(Some(id))),
        (Some(helper), None) => (helper.model, Paid::By(helper.provider)),
        (None, None) => (model, Paid::AsLead),
    };
    let as_lead = matches!(paid, Paid::AsLead);
    let (mut launch, mut launcher) = core
        .workspaces()
        .worker_launch(&lead, &provider, &cwd, model, paid)?;
    let route = launch.model_provider.as_deref();
    if let Some(why) = super::org_routes::refused(core, &cwd, route, launch.model.as_deref()) {
        if !as_lead {
            return Err(why);
        }
        (launch, launcher) =
            (core.workspaces()).worker_launch(&lead, &provider, &cwd, None, Paid::By(None))?;
    }
    if let Some(was) = &was {
        launch.effort = was.effort.clone();
        launch.permission_mode = was.permission_mode;
    }
    if let Some(effort) = effort_param(params, &provider)? {
        launch.effort = Some(effort);
    }
    if let Some(saved) = resume {
        saved::same_account(&provider, &launch, saved)?;
    }
    launch.resume_id = resume.map(|saved| saved.session_id.clone());
    let project = super::caller::project_of(core, request);
    let (agent_id, named) = match hired {
        Some(hired) => {
            let named = core.names().adopt(
                Adopted {
                    agent_id: &hired.agent_id,
                    name: &hired.name,
                    project,
                    parent: Some(&lead_id),
                    provider: &provider,
                    reports,
                    task: Some(&task),
                },
                open(core),
            );
            (hired.agent_id.clone(), named)
        }
        None => {
            let agent_id = match resume {
                Some(saved) => {
                    (core.names()).returning_id(Some(&saved.cli), &saved.session_id, open(core))
                }
                None => crate::server::names::new_agent_id(),
            };
            let named = hire_held(
                core,
                Worker {
                    agent_id: &agent_id,
                    lead: &lead_id,
                    project: Some(project),
                    provider: &provider,
                    session: resume.map(|saved| saved.session_id.as_str()),
                    reports,
                    task: Some(&task),
                },
            )?;
            (agent_id, named)
        }
    };
    launch.agent_id = agent_id.clone();
    let model = launch.model.clone();
    let paid_by = launch.model_provider.clone();
    let chat = chat::begin_worker(core, launch, &lead_id, launcher).map_err(|error| {
        core.names().forget(&agent_id);
        error.to_string()
    })?;
    let own_tree = (checkout.0.as_ref()).or_else(|| resumed.as_ref()?.worktree.as_ref());
    if let Some(checkout) = own_tree {
        let mut checkout = checkout.clone();
        super::ports::checkout::claim(core, &mut checkout);
        let kept = core
            .workspaces()
            .edit_agent(&agent_id, |agent| agent.worktree = Some(checkout));
        if let Err(error) = kept {
            eprintln!("sikemux core: a worker's own checkout was not kept on its record: {error}");
        }
    }
    if let Some(was) = was {
        launching::put_back(core, was);
    }
    super::workspace::retitle(core, &agent_id, Some(task), false);
    core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
    core.changed(What::View);
    let lead_name = core
        .names()
        .name(&lead_id)
        .unwrap_or_else(|| lead_id.clone());
    Ok(Opened::New(Started {
        agent_id,
        name: named.name,
        provider,
        model,
        paid_by,
        lead_name,
        lead_cli: lead.cli_word().to_owned(),
        reports,
        chat,
        checkout: (checkout.0.take())
            .or_else(|| resumed.filter(|resumed| resumed.made_again)?.worktree)
            .map(Box::new),
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
    if let (Some(wanted), Some(lead)) = (&resume, &request.agent_id) {
        if super::hosts::reopen::closed_elsewhere(core, lead, wanted).is_some() {
            return super::hosts::reopen::reopen(core, request, None, wanted, prompt).await;
        }
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
    if let (Some(saved), Some(caller)) = (&saved, &request.agent_id) {
        let returning =
            (core.names()).returning_id(Some(&saved.cli), &saved.session_id, open(core));
        if core.names().leads(&returning, caller) {
            return reopen_own_lead(core, request, saved, returning, prompt).await;
        }
    }
    let hired = match (&saved, &request.agent_id) {
        (None, Some(lead)) => {
            let provider = text_param(&request.params, "cli", 64)?
                .map(|cli| cli.trim().to_lowercase())
                .or_else(|| {
                    core.chats()
                        .get(lead)
                        .map(|chat| chat.provider().to_owned())
                })
                .unwrap_or_default();
            let reports = reports_back(&request.params)?;
            match elsewhere::hire_elsewhere(core, lead, &provider, reports).await? {
                Some(hired) => Some(hired),
                None => Some(hire_here(
                    core,
                    lead,
                    &provider,
                    reports,
                    &task_title(&request.params)?,
                )?),
            }
        }
        _ => None,
    };
    if let Some(lead) = &request.agent_id {
        rename_in(core, lead);
    }
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
    if let Some(lead) = &request.agent_id {
        rename_in(core, lead);
    }
    #[expect(
        clippy::unreachable,
        reason = "start answers AlreadyOpen only for a session it was asked to resume"
    )]
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
    let unproved = started.checkout.is_some() && {
        let project = core.workspaces().project_of(Path::new(&request.project));
        !super::setup_check::proved(core, &project)
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
            "{starting} and gets your prompt once it is ready.{}{}",
            if reports { REPORTS } else { "" },
            if unproved { UNPROVED_SETUP } else { "" },
        ),
    });
    let answer = match (&saved, &request.agent_id) {
        (None, Some(lead)) => with_overlap(core, lead, &started.agent_id, &prompt, answer).await,
        _ => answer,
    };
    let core = core.clone();
    tokio::spawn(async move { hand_task(&core, started, prompt).await });
    Ok(woken_by_answers(answer, reports))
}

/// Names a worker for `lead` here before anything slow, so the lead's row shows it at once.
fn hire_here(
    core: &Core,
    lead: &str,
    provider: &str,
    reports: bool,
    task: &str,
) -> Result<Hired, String> {
    let agent_id = crate::server::names::new_agent_id();
    let named = hire_held(
        core,
        Worker {
            agent_id: &agent_id,
            lead,
            project: None,
            provider,
            session: None,
            reports,
            task: Some(task),
        },
    )?;
    Ok(Hired {
        host: None,
        agent_id,
        name: named.name,
    })
}

/// The lead's running calls name the agents just hired for them.
pub(crate) fn rename_in(core: &Core, lead: &str) {
    if let Some(chat) = core.chats().get(lead) {
        chat.feed.rename_agents();
    }
}

/// Hands the prompt to the agent that already has the session open, as
/// `agent_send` would, rather than opening a second copy of it. One that has
/// stopped starts again on the session first, keeping its name, team and tab.
async fn send_to_open(
    core: &Arc<Core>,
    request: &HarnessRequest,
    saved: &Saved,
    agent_id: &str,
    name: Option<&str>,
    prompt: String,
) -> Result<Value, String> {
    let name = name.unwrap_or(agent_id);
    let stopped = {
        let (core, agent_id) = (core.clone(), agent_id.to_owned());
        blocking(move || {
            Ok(crate::server::roster::listed(&core).iter().any(|agent| {
                agent.agent_id == agent_id && agent.reach == crate::server::roster::Reach::Stopped
            }))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    if stopped {
        launching::wake(core, agent_id.to_owned())
            .await
            .map_err(|error| {
                format!(
                    "{name} has \"{}\" open but did not start again: {error}",
                    saved.title
                )
            })?;
    }
    let mut message = request.clone();
    message.params = json!({ "to": agent_id, "text": prompt });
    let sent = messages::send(core, &message).await?;
    let title = &saved.title;
    let opened = match stopped {
        true => {
            format!("had \"{title}\" open and had stopped, so Sikemux started it again there and")
        }
        false => format!("already has \"{title}\" open, so"),
    };
    Ok(json!({
        "name": name,
        "agentId": agent_id,
        "cli": saved.cli,
        "alreadyOpen": true,
        "sent": sent,
        "note": format!("{name} {opened} your prompt went to it as a message, as agent_send sends one."),
    }))
}

/// Opens again, as itself, a closed agent that leads the caller, and hands it the prompt as
/// a message: it comes back under its own lead rather than as the caller's worker.
async fn reopen_own_lead(
    core: &Arc<Core>,
    request: &HarnessRequest,
    saved: &Saved,
    agent_id: String,
    prompt: String,
) -> Result<Value, String> {
    let title = &saved.title;
    let taken = launching::reopen_agent(core, agent_id)
        .await
        .map_err(|error| format!("reopening \"{title}\" failed: {error}"))?;
    let agent_id = taken.agent_id;
    let name = core.names().name_or_id(&agent_id);
    let mut message = request.clone();
    message.params = json!({ "to": agent_id, "text": prompt });
    let sent = messages::send(core, &message).await?;
    Ok(json!({
        "name": name,
        "agentId": agent_id,
        "cli": saved.cli,
        "resumed": saved.session_id,
        "reportsBack": false,
        "sent": sent,
        "note": format!("{} leads you, so Sikemux reopened \"{title}\" as that agent itself, not as your worker, and your prompt went to it as a message, as agent_send sends one. It does not report back to you: a reply comes as a message only if it sends one.", messages::called(&name, &agent_id)),
    }))
}

/// Adds to the answer the other agents already changing files the brief names.
async fn with_overlap(
    core: &Arc<Core>,
    lead: &str,
    started: &str,
    prompt: &str,
    mut answer: Value,
) -> Value {
    let (owner, lead, started, prompt) = (
        core.clone(),
        lead.to_owned(),
        started.to_owned(),
        prompt.to_owned(),
    );
    let found = blocking(move || Ok(overlap::of_brief(&owner, &lead, &started, &prompt))).await;
    if let Ok(Some(overlap)) = found {
        if let Some(object) = answer.as_object_mut() {
            object.insert("overlap".to_owned(), overlap);
        }
    }
    answer
}

/// A lead that started a teammate reporting back hears from it without asking.
fn woken_by_answers(mut answer: Value, reports: bool) -> Value {
    if reports {
        if let Some(object) = answer.as_object_mut() {
            object.insert("next".to_owned(), "Its answer wakes you, so end your turn once your own work is done rather than waiting, polling or calling agent_wait.".into());
        }
    }
    answer
}

/// What a lead is told when it gives a worker a checkout of its own in a project whose setup
/// was never proved on a fresh checkout.
const UNPROVED_SETUP: &str = " This project's setup is not proved on a fresh checkout, so checkouts may share ports and miss steps: load the project-setup skill and follow it.";

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
        if let Some(error) =
            worktree::agent::set_up(core, &checkout.repo, Some(&agent_id), &cwd).await
        {
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
    chat.keep_task(&prompt);
    let failed = match chat::until_started(core, chat).await {
        Ok(_) => {
            let task = messages::task(
                core, &agent_id, &lead_id, &lead_name, &lead_cli, reports, prompt,
            );
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

struct Opening {
    agent_id: String,
    name: String,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: PermissionMode,
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
    if crate::agent_catalog::agent(&provider).is_none() {
        return Err(format!(
            "{provider} is not an agent Sikemux runs in a terminal"
        ));
    }
    let model = text_param(params, "model", MAX_MODEL_CHARS)?.map(|model| model.trim().to_owned());
    if text_param(params, "provider", 64)?.is_some() {
        return Err("provider works only for a chat; leave mode out to start one".into());
    }
    let reports = reports_back(params)?;
    let task = task_title(params)?;
    if super::hosts::route::lead_host(core, &lead_id).is_some() {
        return Err("your team's lead runs on another host, so the agents you start run as chats; leave out mode".into());
    }
    let _one_at_a_time = core.harness().spawning.one_at_a_time();
    let (lead, _) = lead_of(core, &lead_id, &request.project)?;
    let same = lead.provider == provider;
    let permission_mode = lead.worker_mode();
    let model = model.or_else(|| {
        lead.launch
            .as_ref()
            .filter(|_| same)
            .and_then(|launch| launch.model.clone())
    });
    let effort = effort_param(params, &provider)?;
    let agent_id = crate::server::names::new_agent_id();
    let named = hire_held(
        core,
        Worker {
            agent_id: &agent_id,
            lead: &lead_id,
            project: None,
            provider: &provider,
            session: None,
            reports,
            task: Some(&task),
        },
    )?;
    core.harness().spawning.opening().insert(agent_id.clone());
    rename_in(core, &lead_id);
    let lead_name = core
        .names()
        .name(&lead_id)
        .unwrap_or_else(|| lead_id.clone());
    Ok(Opening {
        agent_id,
        name: named.name,
        model,
        effort,
        permission_mode,
        lead_cli: lead.cli_word().to_owned(),
        launcher: lead.launcher.filter(|_| same),
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
        .workspaces()
        .agent(&opening.lead_id)
        .ok_or("the agent that asked for it is no longer open in Sikemux")?;
    let same = lead.provider == opening.provider;
    let on_launcher = opening
        .launcher
        .as_deref()
        .map(LauncherId::parse)
        .filter(|launcher| launcher.kind == opening.provider)
        .and_then(|launcher| launcher.account)
        .map(str::to_owned);
    core.workspaces()
        .put_agent(WorkspaceAgent {
            lead: Some(lead.id),
            profile_id: on_launcher.or(lead.profile_id.filter(|_| same)),
            model: opening.model.clone(),
            effort: opening.effort.clone().or(lead.effort.filter(|_| same)),
            worktree: lead.worktree,
            ..new_agent(
                opening.agent_id.clone(),
                opening.provider.clone(),
                lead.project,
                lead.cwd,
                opening.permission_mode,
            )
        })
        .map_err(|error| error.to_string())?;
    super::launching::start_terminal(
        core,
        &opening.agent_id,
        super::spare::opening_size(core),
        None,
    )
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
    core.harness().spawning.opening().remove(&opening.agent_id);
    if let Err(error) = opened {
        core.names().forget(&opening.agent_id);
        return Err(format!("{} did not start: {error}", opening.name));
    }
    let task = messages::task(
        core,
        &opening.agent_id,
        &opening.lead_id,
        &opening.lead_name,
        &opening.lead_cli,
        opening.reports,
        prompt,
    );
    core.harness().mailbox.task_waits(&opening.agent_id, task);
    let watcher = core.clone();
    let (agent_id, name, lead_id) = (
        opening.agent_id.clone(),
        opening.name.clone(),
        opening.lead_id.clone(),
    );
    tokio::spawn(async move {
        tokio::time::sleep(TERMINAL_READY_WAIT).await;
        if messages::is_waiting(&watcher, &agent_id, messages::TASK)
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
    let reports = opening.reports;
    let answer = json!({
        "name": opening.name,
        "agentId": opening.agent_id,
        "cli": opening.provider,
        "model": opening.model,
        "mode": "terminal",
        "reportsBack": opening.reports,
        "note": format!(
            "{} is starting in a terminal tab and gets your prompt once its prompt box shows.{}",
            opening.name,
            if opening.reports { REPORTS } else { "" },
        ),
    });
    Ok(woken_by_answers(answer, reports))
}

/// What a lead is told of a worker it starts that reports to it.
const REPORTS: &str = " When its turn fails, stops or asks something, or its land is sent back, you hear at once. A turn that finished calmly with its work landed, or a land that went in, comes as one line with your next turn, and wakes you only once no other worker of yours is at work. A turn that ends on something set to wake it sends nothing.";

/// Sends a worker's lead how its turn ended, unless it finished on something set to wake it and
/// asks nothing: its row says what it waits on, and the lead hears once a land of its goes in.
/// A turn that finished saying nothing is not sent either.
pub(super) fn report_to_lead(core: &Arc<Core>, agent_id: &str, ending: Ending) {
    let Some(lead) = core.names().reports_to(agent_id) else {
        return;
    };
    let chat_answer = core
        .chats()
        .get(agent_id)
        .map(|chat| chat.feed.last_answer().map(TerminalAnswer::Said));
    let wrote_nothing = matches!((&ending, &chat_answer), (Ending::Finished, Some(None)));
    let sent_back = super::questions::sent_back_unwoken(core, agent_id);
    let unwoken = super::questions::waits_unwoken(core, agent_id);
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
                let mut reported = core.harness().spawning.reported();
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
        if wrote_nothing || quiet(&core, &worker, &ending, answer.as_ref(), sent_back) {
            elsewhere::send_commits(&core, &worker).await;
            return;
        }
        let settled = {
            let (core, worker) = (core.clone(), worker.clone());
            blocking(move || Ok(work_settled(&core, &worker))).await
        };
        let calm = is_calm(&ending, answer.as_ref(), settled.unwrap_or(false), unwoken);
        if let Some(line) = calm
            .then(|| calm_line(&core, &worker, answer.as_ref()))
            .flatten()
        {
            elsewhere::send_commits(&core, &worker).await;
            messages::report_calm(&core, worker, lead, line).await;
            return;
        }
        let mut text = report(&core, &worker, ending, answer);
        if unwoken {
            text.push_str("\n\nIt says it waits, but nothing is set to wake it, and Sikemux already sent it back once to set a wake. Wake it with what it needs, take over what it waits on, or close it.");
        }
        elsewhere::send_commits(&core, &worker).await;
        messages::report(&core, worker, lead, text).await;
    });
}

/// What of a worker's report quotes its final answer, for a calm one.
const CALM_QUOTE_CHARS: usize = 200;

/// Whether the worker's work went in and its turn left nothing of its plan open.
fn work_settled(core: &Arc<Core>, worker: &str) -> bool {
    went_in(super::turn_state::work_now(core, worker))
        && super::turn_state::left_open(core, worker).is_none()
}

/// A turn that changed nothing leaves only its answer, which may say a run failed or ask
/// something in words no check reads, so only work that went in lets a report wait.
fn went_in(work: Option<sikemux_view::turn::WorkAt>) -> bool {
    use sikemux_view::turn::WorkAt;
    matches!(
        work,
        Some(WorkAt::Landed | WorkAt::Staged | WorkAt::Shipped)
    )
}

/// Whether a report can wait for its lead's next turn: the turn finished with its work
/// `settled`, and its answer asks nothing and does not say it waits with nothing set to wake it,
/// being `unwoken`. Any other report wakes the lead at once.
fn is_calm(ending: &Ending, answer: Option<&TerminalAnswer>, settled: bool, unwoken: bool) -> bool {
    let Some(TerminalAnswer::Said(said)) = answer else {
        return false;
    };
    matches!(ending, Ending::Finished)
        && settled
        && !unwoken
        && !super::questions::asks_in(said)
        && !out_of_usage(said)
}

/// The one line a calm report is, which quotes the start of the worker's final answer.
fn calm_line(core: &Core, worker: &str, answer: Option<&TerminalAnswer>) -> Option<String> {
    let Some(TerminalAnswer::Said(said)) = answer else {
        return None;
    };
    let quote = crate::text::one_line(said).replace('"', "'");
    let quote = crate::text::clipped(&quote, CALM_QUOTE_CHARS);
    Some(
        match core
            .workspaces()
            .title(worker)
            .filter(|title| !title.is_empty())
        {
            Some(title) => format!(
                "finished its turn on \"{}\": \"{quote}\"",
                crate::text::one_line(&title).replace('"', "'")
            ),
            None => format!("finished its turn: \"{quote}\""),
        },
    )
}

fn quiet(
    core: &Core,
    worker: &str,
    ending: &Ending,
    answer: Option<&TerminalAnswer>,
    sent_back: bool,
) -> bool {
    if !matches!(ending, Ending::Finished) {
        return false;
    }
    match answer {
        Some(TerminalAnswer::Said(said)) if says_nothing(said) => true,
        Some(TerminalAnswer::Said(said))
            if super::questions::asks_in(said) || out_of_usage(said) =>
        {
            false
        }
        _ => sent_back || super::turn_state::set_a_wake(core, worker),
    }
}

/// Whether an answer has no letter or digit in it, such as a lone dash.
fn says_nothing(answer: &str) -> bool {
    !answer.chars().any(char::is_alphanumeric)
}

/// [`turn_ended`] for an agent in a terminal.
pub(crate) fn terminal_turn_ended(core: &Core, agent_id: &str, ending: Ending) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if let Some(core) = core.arc() {
        core.note_ending(agent_id, ending);
    }
}

fn report(core: &Core, worker: &str, ending: Ending, answer: Option<TerminalAnswer>) -> String {
    let at_limit = |answer: &Option<TerminalAnswer>| matches!(answer, Some(TerminalAnswer::Said(said)) if out_of_usage(said));
    let paused = match &ending {
        Ending::Failed(reason) => reason == PAUSED_FOR_AGENTS || at_limit(&answer),
        Ending::Finished => at_limit(&answer),
        _ => false,
    };
    let answer = answer.filter(|_| !paused);
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
        _ if paused => (PAUSED_FOR_AGENTS.to_owned(), None),
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
        Ending::Stopped { by_person: true } => (
            "The person stopped it before it finished its turn.".to_owned(),
            answer,
        ),
        Ending::Stopped { by_person: false } => (
            "Its turn was cut off before it finished. The person did not stop it: Sikemux restarted it, or its CLI ended the turn.".to_owned(),
            answer,
        ),
        Ending::Failed(reason) => (format!("Its turn failed{}", because(Some(reason))), answer),
        Ending::Gone {
            reason,
            mid_turn: true,
            ..
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
            ..
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
        answer.get(..end).unwrap_or_default().trim_end(),
        (start - end).div_ceil(1024),
        answer.get(start..).unwrap_or_default().trim_start()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_view::turn::WorkAt;

    /// @Jinichi's final answer after a terraform apply failed on all 24 resources, which reached
    /// its lead only as a line in the person's next message.
    const FAILED_APPLY: &str = "I didn't apply PR #88. The dev apply failed before it changed \
        anything, and I didn't touch production.\n\n\
        **Fix:** remove the parentheses. It needs a small terraform PR (Irwan wrote #88).\n\n\
        **Decision for you:** should I open the one-line fix PR and, once it's merged, redo the \
        dev targeted plan and apply? Or does it go back to Irwan? The plan files and logs are \
        next to the worktree, in `~/proj/swishx/worktrees/`.";

    #[test]
    fn a_turn_that_changed_nothing_reaches_its_lead_at_once() {
        let answer = TerminalAnswer::Said(FAILED_APPLY.to_owned());
        let settled = went_in(Some(WorkAt::None));
        assert!(!is_calm(&Ending::Finished, Some(&answer), settled, false));
    }
}
