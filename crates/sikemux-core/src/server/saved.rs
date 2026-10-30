//! Sessions the agent CLIs saved, found from a vague memory of them for
//! `agent_sessions` and the composer, and which of them are open now.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_accounts::folders::{sessions_root, sessions_shared};
use crate::agent_catalog::AgentKind;
use crate::cli::protocol::HarnessRequest;
use crate::launch::{account_variable, named_account};
use crate::protocol::ChatLaunch;
use crate::sessions::recent::RecentProvider;
use crate::sessions::search::{self, Scope, SearchRequest, SearchResult, SessionMatch, MAX_LIMIT};

use super::connection::blocking;
use super::messages::{self, Reach};
use super::workspace::project_of;
use super::{Core, CoreResult};

const DEFAULT_LIMIT: usize = 8;
const MAX_QUERY_CHARS: usize = 500;
/// How long an agent's search may read transcripts before it answers.
const TOOL_BUDGET: Duration = Duration::from_millis(2_500);
/// The composer searches as the person types, so it answers sooner.
const MAX_CALL_BUDGET: Duration = Duration::from_millis(1_500);

/// Every CLI's sessions, on each of the person's accounts whose sessions are kept apart.
fn providers(core: &Core) -> Vec<RecentProvider> {
    let mut folders = HashSet::new();
    let mut kept = |agent: AgentKind, config_path: Option<String>| {
        let folder = sessions_root(agent.as_str(), config_path.as_deref());
        folders
            .insert((agent, folder))
            .then_some(RecentProvider { agent, config_path })
    };
    let mut providers: Vec<RecentProvider> = AgentKind::ALL
        .into_iter()
        .filter_map(|agent| kept(agent, None))
        .collect();
    for profile in core
        .workspaces
        .accounts()
        .into_iter()
        .flat_map(|book| book.profiles)
    {
        let Some(agent) = AgentKind::from_name(&profile.provider) else {
            continue;
        };
        if agent.entry().accounts && profile.config_path.is_some() {
            providers.extend(kept(agent, profile.config_path));
        }
    }
    providers
}

/// The agent now running each saved session, by CLI and session id.
fn open_sessions(core: &Core) -> HashMap<(String, String), (String, Option<String>)> {
    messages::listed(core)
        .into_iter()
        .filter(|agent| agent.reach != Reach::Stopped)
        .filter_map(|agent| {
            let session = core.names.session_of(&agent.agent_id)?;
            Some(((agent.cli, session), (agent.agent_id, agent.name)))
        })
        .collect()
}

/// Sessions that ran in one of `projects`, in a folder inside one, or in a
/// git worktree of one, as an agent working there is counted in its project.
fn scope_of(core: &Core, projects: Vec<String>) -> Scope {
    let wanted: Vec<PathBuf> = projects
        .iter()
        .map(|project| std::fs::canonicalize(project).unwrap_or_else(|_| project.into()))
        .collect();
    let mut known = core.workspaces.project_paths();
    known.extend(wanted.iter().cloned());
    let auto_worktrees: Vec<PathBuf> = wanted
        .iter()
        .filter_map(|project| {
            let name = project.file_name()?.to_string_lossy();
            Some(project.with_file_name(format!("{name}.worktrees")))
        })
        .collect();
    Box::new(move |cwd: &str| {
        let project = PathBuf::from(project_of(Path::new(cwd), &known));
        wanted.contains(&project)
            || auto_worktrees
                .iter()
                .any(|folder| project.starts_with(folder))
    })
}

fn run_search(
    core: &Core,
    query: String,
    projects: Option<Vec<String>>,
    limit: usize,
    budget: Duration,
) -> SearchResult {
    search::search(SearchRequest {
        query,
        providers: providers(core),
        scope: projects.map(|projects| scope_of(core, projects)),
        limit,
        budget,
    })
}

fn query_param(params: &Value) -> Result<String, String> {
    match params.get("query") {
        Some(Value::String(query))
            if !query.trim().is_empty() && query.chars().count() <= MAX_QUERY_CHARS =>
        {
            Ok(query.trim().to_owned())
        }
        _ => Err(format!(
            "query must be the words you remember, up to {MAX_QUERY_CHARS} characters"
        )),
    }
}

fn limit_param(params: &Value) -> Result<usize, String> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(Value::Number(limit)) => limit
            .as_u64()
            .filter(|limit| (1..=MAX_LIMIT as u64).contains(limit))
            .map(|limit| limit as usize)
            .ok_or_else(|| format!("limit must be between 1 and {MAX_LIMIT}")),
        Some(_) => Err("limit must be a number".into()),
    }
}

/// The project folder to search: the one named, or the caller's.
fn scope(core: &Core, request: &HarnessRequest) -> Result<Option<Vec<String>>, String> {
    match request.params.get("allProjects") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => return Ok(None),
        Some(_) => return Err("allProjects must be a boolean".into()),
    }
    let project = match request.params.get("project") {
        None | Some(Value::Null) => messages::caller_project(core, request),
        Some(Value::String(path)) if !path.trim().is_empty() => {
            let path = Path::new(path.trim());
            std::fs::canonicalize(path)
                .map_err(|_| format!("{} is not a folder on this host", path.display()))?
                .to_string_lossy()
                .into_owned()
        }
        Some(_) => return Err("project must be a folder's path".into()),
    };
    Ok(Some(vec![project]))
}

fn day(unix_ms: u64) -> String {
    let at = crate::accounts::live::rfc3339(unix_ms);
    at.get(..10).unwrap_or(&at).to_owned()
}

/// What `agent_sessions` answers.
pub(crate) async fn find(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let query = query_param(&request.params)?;
    let limit = limit_param(&request.params)?;
    let (core, request) = (core.clone(), request.clone());
    blocking(move || {
        let projects = scope(&core, &request)?;
        let all = projects.is_none();
        let result = run_search(&core, query, projects, limit, TOOL_BUDGET);
        let open = open_sessions(&core);
        let sessions: Vec<Value> = result
            .matches
            .iter()
            .map(|found| {
                let running = open.get(&(found.cli.to_owned(), found.session_id.clone()));
                json!({
                    "title": found.title,
                    "cli": found.cli,
                    "project": found.project,
                    "date": day(found.active_at),
                    "sessionId": found.session_id,
                    "openNow": running.map(|(agent_id, name)| json!({ "agentId": agent_id, "name": name })),
                    "snippets": found.snippets,
                })
            })
            .collect();
        let mut note = String::new();
        if sessions.is_empty() {
            note.push_str("Nothing matched. Try other words, fewer of them");
            note.push_str(if all { "." } else { ", or allProjects." });
        } else {
            note.push_str("Resume one with agent_spawn and resume set to its sessionId.");
        }
        if result.cut_short {
            note.push_str(&format!(
                " Time ran out after reading {} of {} sessions, newest first; older ones matched by title only.",
                result.read, result.sessions
            ));
        }
        Ok(json!({ "sessions": sessions, "note": note }))
    })
    .await
    .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    query: String,
    #[serde(default)]
    projects: Option<Vec<PathBuf>>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    budget_ms: Option<u64>,
}

/// The `agentSessions` call: the same search for the person's composer, which
/// leaves out sessions open now.
pub(crate) async fn call(core: &Arc<Core>, params: SearchParams) -> CoreResult<Vec<SessionMatch>> {
    let core = core.clone();
    blocking(move || {
        let query = query_param(&json!({ "query": params.query }))?;
        let projects = params.projects.map(|projects| {
            projects
                .iter()
                .map(|project| project.to_string_lossy().into_owned())
                .collect()
        });
        let budget = params
            .budget_ms
            .map_or(MAX_CALL_BUDGET, Duration::from_millis)
            .min(MAX_CALL_BUDGET);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let result = run_search(&core, query, projects, limit, budget);
        let open = open_sessions(&core);
        Ok(result
            .matches
            .into_iter()
            .filter(|found| !open.contains_key(&(found.cli.to_owned(), found.session_id.clone())))
            .collect())
    })
    .await
}

/// The agent that has the session `session_id` of `cli` open now, and its name.
pub(crate) fn open_now(
    core: &Core,
    cli: &str,
    session_id: &str,
) -> Option<(String, Option<String>)> {
    open_sessions(core).remove(&(cli.to_owned(), session_id.to_owned()))
}

/// Refuses to reopen a session on an account whose CLI cannot find it, which
/// is one keeping its sessions apart from the account that saved it.
pub(crate) fn same_account(
    provider: &str,
    launch: &ChatLaunch,
    saved: &Saved,
) -> Result<(), String> {
    if account_variable(provider).is_none() {
        return Ok(());
    }
    let folder = named_account(provider, &launch.env);
    if sessions_shared(provider, folder, saved.config_path.as_deref()) {
        return Ok(());
    }
    Err(format!(
        "\"{}\" was saved on another {provider} account than the one you run on, which keeps its sessions apart, so it cannot be reopened from here; ask the person to open it from Recent",
        saved.title
    ))
}

/// A saved session `agent_spawn` resumes.
#[derive(Clone)]
pub(crate) struct Saved {
    pub session_id: String,
    pub cli: String,
    pub project: PathBuf,
    pub title: String,
    /// The account folder of the CLI that saved it, when not the default one.
    pub config_path: Option<String>,
    /// The agent running it now, and its name.
    pub open: Option<(String, Option<String>)>,
}

/// Finds the session `session_id`, of `cli` when given, among every project's.
pub(crate) fn locate(core: &Core, session_id: &str, cli: Option<&str>) -> Result<Saved, String> {
    let agent = cli
        .map(|cli| {
            AgentKind::from_name(cli)
                .ok_or_else(|| format!("{cli} keeps no sessions Sikemux reads"))
        })
        .transpose()?;
    let providers: Vec<RecentProvider> = providers(core)
        .into_iter()
        .filter(|provider| agent.is_none_or(|agent| provider.agent == agent))
        .collect();
    let found = search::locate(&providers, session_id).ok_or_else(|| {
        format!("no saved session has the id {session_id}; find it with agent_sessions first")
    })?;
    let open = open_now(core, found.cli, &found.session_id);
    Ok(Saved {
        session_id: found.session_id,
        cli: found.cli.to_owned(),
        project: PathBuf::from(found.project),
        title: found.title,
        config_path: found.config_path,
        open,
    })
}
