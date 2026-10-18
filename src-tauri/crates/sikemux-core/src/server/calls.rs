//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::{Needs, Peer};
use super::attachments;
use super::connection::{blocking, ClientConn};
use super::edits::{self, Edit, Filter, Owners};
use super::names::{project_of, AgentName};
use super::workspace::{self, AgentRun};
use super::{
    board, inbox, messages, plugins, questions, remote, saved, share, who_wrote, Core, CoreError,
    CoreResult,
};

pub(crate) fn needs(method: &str) -> Needs {
    match method {
        "chatFile" | "questions" | "boards" | "board" | "agentNames" | "edits" | "inbox"
        | "digest" | "agentMessages" => Needs::Watch,
        "answerQuestion" | "answerQuestions" | "boardAdd" | "boardUpdate" | "boardRemove"
        | "boardSettings" | "boardRetry" | "renameAgent" | "shares" | "agentAccounts"
        | "agentStatus" | "agentAccountStatus" | "agentUsage" | "cancelResume" => Needs::Full,
        "pairing" | "answerPairing" | "changeDevice" | "linkHost" | "shareChat" | "stopShare"
        | "commitStaged" | "setAgentAccounts" | "addAgentAccount" | "forgetAgentStatus"
        | "markAgentSignedOut" | "addProject" | "removeProject" => Needs::Person,
        "changeOwners" | "commitOwners" => Needs::Watch,
        _ => Needs::Local,
    }
}

/// Whether only Sikemux's own window or notch may call `method` from this
/// host; another program here may only read.
pub(crate) fn for_person(method: &str) -> bool {
    !matches!(
        method,
        "chatFile"
            | "questions"
            | "boards"
            | "board"
            | "agentNames"
            | "agentSessions"
            | "agentMessages"
            | "edits"
            | "changeOwners"
            | "commitOwners"
            | "inbox"
            | "digest"
            | "pairing"
            | "shares"
            | "agentAccounts"
            | "agentStatus"
            | "agentAccountStatus"
            | "agentUsage"
    )
}

pub(crate) async fn call(
    core: Arc<Core>,
    client: &ClientConn,
    method: &str,
    params: Value,
) -> CoreResult<Value> {
    match method {
        "chatFile" => answer(chat_file(&core, &client.peer, parse(params)?).await?),
        "nameAgent" => answer(name_agent(&core, parse(params)?).await?),
        "renameAgent" => answer(rename_agent(&core, parse(params)?).await?),
        "agentsInApp" => answer(agents_in_app(&core, parse(params)?).await?),
        "publishBuddies" => answer(core.workspaces.publish_buddies(parse(params)?)?),
        "agentNames" => answer(agent_names(&core).await?),
        "agentSessions" => answer(saved::call(&core, parse(params)?).await?),
        "setWorkspace" => answer(workspace::set_workspace(&core, parse(params)?).await?),
        "addProject" => answer(workspace::add_project(&core, parse(params)?).await?),
        "removeProject" => answer(workspace::remove_project(&core, parse(params)?)?),
        method if crate::agent_accounts::METHODS.contains(&method) => {
            super::agent_accounts::call(&core, method, params).await
        }
        "shareChat" => {
            let params: ShareChatParams = parse(params)?;
            answer(share::share_chat(
                &core,
                &params.agent_id,
                params.lifetime_ms,
                params.title,
            )?)
        }
        "stopShare" => {
            let params: StopShareParams = parse(params)?;
            answer(share::stop(&core, &params.id)?)
        }
        "shares" => answer(share::list(&core)),
        "pairing" => answer(remote::pairing(&core)),
        "linkHost" => answer(super::hosts::link_host(&core, parse(params)?).await?),
        "answerPairing" => answer(remote::answer_pairing(&core, parse(params)?)?),
        "changeDevice" => answer(remote::change_device(&core, parse(params)?)?),
        "questions" => answer(questions::views(&core)),
        "answerQuestion" => answer(questions::answer(&core, parse(params)?).await?),
        "answerQuestions" => answer(questions::answer_all(&core, parse(params)?).await?),
        "agentMessages" => answer(agent_messages(&core, parse(params)?).await?),
        "boards" => answer(board::boards(&core)),
        "board" => answer(board::board(&core, params).await?),
        "boardAdd" => answer(board::person_add(&core, params).await?),
        "boardUpdate" => answer(board::person_update(&core, params).await?),
        "boardRemove" => answer(board::person_remove(&core, params).await?),
        "boardSettings" => answer(board::person_settings(&core, params).await?),
        "boardRetry" => answer(board::person_retry(&core, params).await?),
        "edits" => answer(edits(&core, parse(params)?).await?),
        "changeOwners" => answer(change_owners(&core, parse(params)?).await?),
        "commitStaged" => answer(commit_staged(&core, parse(params)?).await?),
        "personSaved" => {
            only_window(&core, client)?;
            let params: PersonSavedParams = parse(params)?;
            core.edits.saved(params.path);
            answer(())
        }
        "commitOwners" => answer(who_wrote::call(&core, parse(params)?).await?),
        "inbox" => answer(inbox(&core).await?),
        "digest" => answer(digest(&core, parse(params)?).await?),
        "plugin" => {
            only_window(&core, client)?;
            answer(plugins::window_call(&core, parse(params)?).await)
        }
        "pluginManifests" => {
            only_window(&core, client)?;
            answer(plugins::manifests())
        }
        "pluginSetDisabled" => {
            only_window(&core, client)?;
            plugins::set_disabled(&core, parse(params)?);
            answer(())
        }
        "pluginStream" => {
            only_window(&core, client)?;
            answer(plugins::stream_ticket(parse(params)?)?)
        }
        _ => Err(format!("this host does not know {method}").into()),
    }
}

/// Plugins act as the person through their sign-ins, so only the window's
/// panes reach them directly; agents reach the methods offered as tools.
fn only_window(core: &Core, client: &ClientConn) -> CoreResult<()> {
    if core.window.is_window(client.id) {
        Ok(())
    } else {
        Err("only Sikemux's window may call a plugin".into())
    }
}

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMessagesParams {
    project: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// The project's log of messages between agents, newest first, under the
/// project's own path, which keys it in the inbox's `agentMessages`.
async fn agent_messages(core: &Arc<Core>, params: AgentMessagesParams) -> CoreResult<Value> {
    let path = Path::new(&params.project);
    if !path.is_absolute() {
        return Err("project must be the project's absolute path".into());
    }
    let path = path.to_owned();
    let core = core.clone();
    blocking(move || {
        let project = project_of(&path, &core.workspaces.project_paths());
        let entries = messages::logged(&core, &project, params.limit);
        Ok(json!({ "project": project, "entries": entries }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareChatParams {
    agent_id: String,
    lifetime_ms: u64,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct StopShareParams {
    id: String,
}

#[derive(Deserialize)]
struct ChatFileParams {
    path: PathBuf,
}

#[derive(Serialize)]
struct ChatFile {
    data: String,
}

/// A file a chat's message carried, in base64. Any device may read the
/// pictures pasted into chats; one that can drive sessions can already read
/// anything through a terminal, so it may read files dropped from elsewhere.
async fn chat_file(core: &Core, peer: &Peer, params: ChatFileParams) -> CoreResult<ChatFile> {
    let pasted = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone());
    let path = params.path;
    let kept = match pasted {
        Some(dir) => {
            let path = path.clone();
            blocking(move || Ok(attachments::is_kept(&dir, &path))).await?
        }
        None => false,
    };
    if !kept {
        core.permit(peer, Needs::Full).map_err(|_| {
            CoreError::from("this device can open only the pictures pasted into chats")
        })?;
    }
    let data = blocking(move || attachments::read(&path)).await?;
    Ok(ChatFile { data })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameAgentParams {
    agent_id: String,
    cwd: PathBuf,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    resume_id: Option<String>,
}

/// The name the core gave an agent the app starts or opens again, given now
/// when it has none.
async fn name_agent(core: &Arc<Core>, params: NameAgentParams) -> CoreResult<AgentName> {
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    if !params.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    let core = core.clone();
    let named = blocking(move || {
        Ok(core.name_agent(
            &params.agent_id,
            &params.cwd,
            params.provider.as_deref(),
            params.resume_id.as_deref(),
        ))
    })
    .await?;
    Ok(named)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameAgentParams {
    agent_id: String,
    name: String,
}

async fn rename_agent(core: &Arc<Core>, params: RenameAgentParams) -> CoreResult<AgentName> {
    let core = core.clone();
    blocking(move || {
        let named = core
            .names
            .rename(&params.agent_id, &params.name, core.running_agents())?;
        core.publish_device_view();
        Ok(named)
    })
    .await
}

/// The names of the agents running here and of the workers they started on
/// linked hosts, each with the agent that started it, so a device can list
/// workers after their lead.
async fn agent_names(core: &Arc<Core>) -> CoreResult<Vec<AgentName>> {
    let core = core.clone();
    blocking(move || {
        let mut agents = core.running_agents();
        agents.extend(core.hosts.team.ids());
        Ok(core.names.of(&agents))
    })
    .await
}

/// The most agents the app may say it holds.
const MAX_AGENTS_IN_APP: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentsInAppParams {
    agent_ids: HashSet<String>,
    runs: BTreeMap<String, AgentRun>,
}

/// Every agent the app holds, sleeping ones included, so their names stay
/// theirs while nothing of them runs here and closed ones' names are free.
async fn agents_in_app(core: &Arc<Core>, params: AgentsInAppParams) -> CoreResult<()> {
    if params.agent_ids.len() > MAX_AGENTS_IN_APP || params.runs.len() > MAX_AGENTS_IN_APP {
        return Err("the app named more agents than the core keeps".into());
    }
    if !params
        .agent_ids
        .iter()
        .all(|id| crate::cli::protocol::is_tool_agent_id(id))
    {
        return Err("that is not an agent id".into());
    }
    core.workspaces.publish_runs(params.runs)?;
    let core = core.clone();
    blocking(move || {
        core.names
            .set_in_app(params.agent_ids, core.running_agents());
        Ok(())
    })
    .await
}

#[derive(Deserialize)]
struct PersonSavedParams {
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutParams {
    dir: PathBuf,
    #[serde(default)]
    staged: bool,
    /// The asker's name for the checkout's current state, when it watches it.
    #[serde(default)]
    generation: Option<String>,
    #[serde(default)]
    moved: Option<edits::Moved>,
}

/// Which agent wrote each uncommitted line of a checkout.
async fn change_owners(core: &Arc<Core>, params: CheckoutParams) -> CoreResult<Owners> {
    let core = core.clone();
    blocking(move || {
        Ok(match &params.generation {
            Some(generation) => edits::owners_at(
                &core,
                &params.dir,
                params.staged,
                generation,
                params.moved.as_ref(),
            ),
            None => edits::owners(&core, &params.dir, params.staged),
        }?)
    })
    .await
}

#[derive(Deserialize)]
struct CommitParams {
    dir: PathBuf,
    message: String,
}

/// Commits what the person staged in the git panel, with the footer naming
/// the agents whose lines it holds. What git said.
async fn commit_staged(core: &Arc<Core>, params: CommitParams) -> CoreResult<String> {
    let core = core.clone();
    blocking(move || {
        let output = edits::footer::commit(&core, &params.dir, &[], &params.message, None)?;
        let said = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        if output.status.success() {
            Ok(said(&output.stdout))
        } else if output.stderr.trim_ascii().is_empty() {
            Err(said(&output.stdout).into())
        } else {
            Err(said(&output.stderr).into())
        }
    })
    .await
}

/// The files agents changed in a project and the lines they changed, by
/// agent, chat run, turn or path, oldest first.
async fn edits(core: &Arc<Core>, filter: Filter) -> CoreResult<Vec<Edit>> {
    let core = core.clone();
    blocking(move || {
        let project = project_of(&filter.project, &core.workspaces.project_paths());
        Ok(core.edits.read(&project, &filter))
    })
    .await
}

/// What waits on the person, ranked as devices' views list it.
async fn inbox(core: &Arc<Core>) -> CoreResult<inbox::View> {
    let core = core.clone();
    blocking(move || {
        let (chats, sessions) = core.ranked_infos();
        Ok(inbox::view(&core, &chats, &sessions))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DigestParams {
    #[serde(default)]
    since: Option<u64>,
}

/// What finished, failed and waits since the person last left, or `since`.
async fn digest(core: &Arc<Core>, params: DigestParams) -> CoreResult<inbox::Digest> {
    let core = core.clone();
    blocking(move || Ok(inbox::digest(&core, params.since))).await
}
