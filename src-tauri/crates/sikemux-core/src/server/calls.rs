//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::{Needs, Peer};
use super::attachments;
use super::connection::{blocking, ClientConn};
use super::edits::{self, Edit, Filter, Owners};
use super::names::{project_of, AgentName, ClosedAgent, Starting};
use super::workspace;
use super::{
    board, inbox, messages, plugins, questions, remote, saved, share, who_wrote, Core, CoreError,
    CoreResult,
};

pub(crate) fn needs(method: &str) -> Needs {
    match method {
        "chatFile" | "questions" | "boards" | "board" | "agentNames" | "edits" | "inbox"
        | "digest" | "agentMessages" | "workspace" => Needs::Watch,
        "answerQuestion" | "answerQuestions" | "boardAdd" | "boardUpdate" | "boardRemove"
        | "boardSettings" | "boardRetry" | "renameAgent" | "shares" | "agentAccounts"
        | "agentStatus" | "agentAccountStatus" | "agentUsage" | "cancelResume" | "steerQueued"
        | "dropQueued" => Needs::Full,
        "boardAddTicket" => Needs::Full,
        "pairing" | "answerPairing" | "changeDevice" | "linkHost" | "shareChat" | "stopShare"
        | "commitStaged" | "setAgentAccounts" | "addAgentAccount" | "forgetAgentStatus"
        | "markAgentSignedOut" | "addProject" | "removeProject" | "updateProject"
        | "moveProject" | "setSpaces" | "setPinnedChats" | "setAgentDefaults" | "createAgent"
        | "updateAgent" | "closeAgents" | "plugin" | "pluginManifests" | "pluginSetDisabled"
        | "pluginStream" | "pluginSignIn" | "signInCallback" => Needs::Person,
        "changeOwners" | "commitOwners" | "activitySummary" | "showCi" => Needs::Watch,
        "setActivityPrice" | "setLinkSignIns" => Needs::Person,
        "createAgentWorktree" => Needs::Person,
        "agentWorktreeAt" => Needs::Full,
        "modelProviders" | "modelProviderModels" | "modelHelper" | "agentSignIns" => Needs::Full,
        "agentSignIn" | "agentSignInCode" | "cancelAgentSignIn" | "agentSignOut" => Needs::Person,
        "skillsOverview" | "skillsAdded" | "savedSessions" | "recentSessions" => Needs::Full,
        "renameSession" | "deleteSession" | "relocateSession" => Needs::Person,
        "createSkill" | "installSkill" | "removeSkill" | "adoptSkill" => Needs::Person,
        "modelProviderCheck"
        | "modelProviderConnect"
        | "modelProviderDisconnect"
        | "setModelHelper" => Needs::Person,
        method if method.starts_with("git.") => super::git::needs(method),
        _ => Needs::Local,
    }
}

/// Whether only Sikemux's own window or notch may call `method` from this
/// host; another program here may only read, or start a plugin's browser
/// sign-in, which only the person can finish in their browser.
pub(crate) fn for_person(method: &str) -> bool {
    if method.starts_with("git.") {
        return super::git::for_person(method);
    }
    !matches!(
        method,
        "pluginSignIn"
            | "signInCallback"
            | "chatFile"
            | "workspace"
            | "questions"
            | "boards"
            | "board"
            | "agentNames"
            | "agentSessions"
            | "agentMessages"
            | "edits"
            | "changeOwners"
            | "commitOwners"
            | "showCi"
            | "inbox"
            | "digest"
            | "activitySummary"
            | "pairing"
            | "shares"
            | "agentAccounts"
            | "agentStatus"
            | "agentAccountStatus"
            | "agentUsage"
            | "modelProviders"
            | "modelProviderModels"
            | "modelHelper"
            | "agentSignIns"
            | "skillsOverview"
            | "skillsAdded"
            | "savedSessions"
            | "recentSessions"
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
        "closedAgents" => answer(closed_agents(&core, parse(params)?).await?),
        "publishBuddies" => answer(core.workspaces.publish_buddies(parse(params)?)?),
        "agentNames" => answer(agent_names(&core).await?),
        "stopChatIfIdle" => {
            let params: AgentParams = parse(params)?;
            answer(json!({ "stopped": super::lifecycle::sleep(&core, &params.agent_id) }))
        }
        "sleepIdleChats" => answer(json!({ "slept": super::lifecycle::sleep_idle(&core) })),
        "startTerminalAgent" => {
            let params: TerminalParams = parse(params)?;
            let size = (params.cols, params.rows);
            let pty = super::launching::start_terminal(&core, &params.agent_id, size, None).await?;
            answer(json!({ "ptyId": pty }))
        }
        "startChat" => {
            let params: AgentParams = parse(params)?;
            answer(super::launching::start_saved(&core, &params.agent_id).await?)
        }
        "activitySummary" => answer(activity_summary(&core, parse(params)?).await?),
        "setActivityPrice" => {
            let params: ActivityPriceParams = parse(params)?;
            core.activity_log.set_price(&params.model, params.price)?;
            core.changes.changed("activity", None, None);
            answer(())
        }
        "steerQueued" => {
            let params: QueuedParams = parse(params)?;
            answer(
                super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message)
                    .await?,
            )
        }
        "dropQueued" => {
            let params: QueuedParams = parse(params)?;
            answer(super::chat::queue::remove(
                &core,
                &params.agent_id,
                &params.ids,
            )?)
        }
        "agentSessions" => answer(saved::call(&core, parse(params)?).await?),
        "workspace" => answer(core.workspaces.view()),
        "runCommand" => answer(super::custom_command::run(parse(params)?).await?),
        "addProject" => answer(workspace::add_project(&core, parse(params)?).await?),
        "removeProject" => answer(workspace::remove_project(&core, parse(params)?)?),
        "updateProject" => answer(workspace::update_project(&core, parse(params)?)?),
        "moveProject" => answer(workspace::move_project(&core, parse(params)?)?),
        "setSpaces" => answer(workspace::set_spaces(&core, parse(params)?)?),
        "setPinnedChats" => answer(workspace::set_pinned_chats(&core, parse(params)?)?),
        "setAgentDefaults" => answer(workspace::set_defaults(&core, parse(params)?).await?),
        "createAgent" => answer(workspace::create_agent(&core, parse(params)?)?),
        "updateAgent" => answer(workspace::update_agent(&core, parse(params)?)?),
        "closeAgents" => answer(workspace::close_agents(&core, parse(params)?)?),
        "createAgentWorktree" => answer(super::sandbox::for_agent(&core, parse(params)?).await?),
        "agentWorktreeAt" => answer(super::sandbox::at(parse(params)?).await?),
        method if method.starts_with("git.") => {
            super::git::call(&core, &client.peer, method, params).await
        }
        method if super::saved_sessions::METHODS.contains(&method) => {
            super::saved_sessions::call(method, params).await
        }
        method if super::skill_library::METHODS.contains(&method) => {
            super::skill_library::call(&core, method, params).await
        }
        method if super::agent_sign_in::METHODS.contains(&method) => {
            super::agent_sign_in::call(&core, method, params).await
        }
        method if super::model_providers::METHODS.contains(&method) => {
            super::model_providers::call(&core, method, params).await
        }
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
        "signHostRequest" => {
            let request: crate::accounts::HostRequest = parse(params)?;
            answer(signed(core.remote.sign_host_request(&request)?))
        }
        "signShareViewer" => {
            let params: NonceParams = parse(params)?;
            answer(signed(core.remote.sign_share_viewer(&params.nonce)?))
        }
        "setLinkSignIns" => answer(super::hosts::sign_ins::set(&core, parse(params)?)?),
        "answerPairing" => answer(remote::answer_pairing(&core, parse(params)?)?),
        "changeDevice" => answer(remote::change_device(&core, parse(params)?)?),
        "questions" => answer(questions::views(&core)),
        "answerQuestion" => answer(questions::answer(&core, parse(params)?).await?),
        "answerQuestions" => answer(questions::answer_all(&core, parse(params)?).await?),
        "agentMessages" => answer(agent_messages(&core, parse(params)?).await?),
        "boards" => answer(board::boards(&core)),
        "board" => answer(board::board(&core, params).await?),
        "boardAdd" => answer(board::person_add(&core, params).await?),
        "boardAddTicket" => answer(board::person_add_ticket(&core, params).await?),
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
        "ports" => {
            let params: PortsParams = parse(params)?;
            answer(super::ports::project_ports(&core, client.id, params.project).await?)
        }
        "testResults" => {
            let params: ProjectParams = parse(params)?;
            answer(super::tasks::module_tests(&core, params.project).await?)
        }
        "projectConfig" => {
            let params: ProjectParams = parse(params)?;
            answer(super::tasks::project_config(&core, params.project).await?)
        }
        "trustProject" => {
            let params: TrustProjectParams = parse(params)?;
            answer(super::tasks::trust_project(&core, params.project, params.fingerprint).await?)
        }
        "projectFolders" => {
            let params: ProjectParams = parse(params)?;
            let settings = core.workspaces.folder_settings(&params.project);
            let rules = core.workspaces.project_rules(&params.project);
            let folders =
                blocking(move || Ok(crate::tasks::folders::folders(Path::new(&params.project))))
                    .await?;
            answer(json!({ "folders": folders, "settings": settings, "rules": rules }))
        }
        "javaRuntimes" => answer(blocking(|| Ok(crate::tasks::java::runtimes())).await?),
        "setProjectRules" => answer(workspace::set_project_rules(&core, parse(params)?)?),
        "projectBrief" => {
            let params: ProjectParams = parse(params)?;
            let core = core.clone();
            answer(
                blocking(move || Ok(super::brief::write(&core, Path::new(&params.project))))
                    .await?,
            )
        }
        "setFolderSettings" => answer(workspace::set_folder_settings(&core, parse(params)?)?),
        "untrustProject" => {
            let params: ProjectParams = parse(params)?;
            answer(super::tasks::untrust_project(&core, params.project).await?)
        }
        "tasks" => {
            let params: ProjectParams = parse(params)?;
            answer(super::tasks::list(&core, params.project).await?)
        }
        "taskRuns" => answer(super::tasks::runs(&core)),
        "taskStart" => answer(super::tasks::start(&core, parse(params)?).await?),
        "taskRerunFailed" => answer(super::tasks::rerun_failed(&core, parse(params)?).await?),
        "taskStop" => {
            super::harness::stop_runs(&core, parse(params)?).await;
            answer(())
        }
        "trustAsks" => answer(super::tasks::asks(&core)),
        "showCi" => {
            super::ci::show(&core, client.id, parse(params)?);
            answer(())
        }
        "answerTrust" => {
            let params: TrustAnswerParams = parse(params)?;
            answer(super::tasks::answer_trust(&core, &params.id, params.trust)?)
        }
        "addTask" => answer(super::tasks::add(&core, parse(params)?).await?),
        "digest" => answer(digest(&core, parse(params)?).await?),
        "plugin" => answer(plugins::window_call(&core, parse(params)?).await),
        "pluginManifests" => answer(plugins::manifests()),
        "pluginSetDisabled" => {
            plugins::set_disabled(&core, parse(params)?);
            answer(())
        }
        "pluginStream" => answer(plugins::stream_ticket(parse(params)?)?),
        "pluginSignIn" => {
            not_an_agent(client).await?;
            answer(plugins::sign_in_ticket(parse(params)?)?)
        }
        "signInCallback" => {
            not_an_agent(client).await?;
            answer(plugins::sign_in_callback(parse(params)?)?)
        }
        _ => Err(format!("this host does not know {method}").into()),
    }
}

/// Refuses a program an agent started: it could sign the person's plugin in
/// to an account of its own choosing.
async fn not_an_agent(client: &ClientConn) -> CoreResult<()> {
    let Some(pid) = client.pid else {
        return Ok(());
    };
    match blocking(move || Ok(super::callers::agent_of(pid))).await? {
        Some(_) => {
            Err("an agent cannot sign the person in to a service; the person runs this".into())
        }
        None => Ok(()),
    }
}

fn only_window(core: &Core, client: &ClientConn) -> CoreResult<()> {
    if core.window.is_window(client.id) {
        Ok(())
    } else {
        Err("only Sikemux's window may say the person saved a file".into())
    }
}

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
}

fn signed((key, signature): (String, String)) -> Value {
    json!({ "key": key, "signature": signature })
}

#[derive(Debug, Deserialize)]
struct NonceParams {
    nonce: String,
}

#[derive(Deserialize)]
struct ProjectParams {
    project: String,
}

#[derive(Deserialize)]
struct PortsParams {
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize)]
struct TrustProjectParams {
    project: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct TrustAnswerParams {
    id: String,
    trust: bool,
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
    /// How the app draws it, kept for when it is opened again.
    #[serde(default)]
    buddy: Option<Value>,
    #[serde(default)]
    title: Option<String>,
}

/// An agent's name, with the character it wore before it closed when it
/// opens again under a name the person chose.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NamedAgent {
    #[serde(flatten)]
    named: AgentName,
    #[serde(skip_serializing_if = "Option::is_none")]
    buddy: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivitySummaryParams {
    /// Minutes the person's clock is ahead of UTC, which places each event on their own day.
    utc_offset_minutes: i64,
    project: Option<String>,
}

#[derive(Deserialize)]
struct ActivityPriceParams {
    model: String,
    price: Option<super::activity_log::PriceOverride>,
}

async fn activity_summary(
    core: &Arc<Core>,
    params: ActivitySummaryParams,
) -> CoreResult<super::activity_log::ActivitySummary> {
    let core = core.clone();
    blocking(move || {
        Ok(core.activity_log.summary(
            params.utc_offset_minutes * 60_000,
            params.project.as_deref(),
        ))
    })
    .await
}

/// Messages waiting behind an agent's turn, by id.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueuedParams {
    agent_id: String,
    ids: Vec<String>,
    /// A message sent with them, after them.
    #[serde(default)]
    message: Option<super::chat::queue::Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentParams {
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalParams {
    agent_id: String,
    cols: u16,
    rows: u16,
}

/// The name the core gave an agent the app starts or opens again, given now
/// when it has none.
async fn name_agent(core: &Arc<Core>, params: NameAgentParams) -> CoreResult<NamedAgent> {
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    if !params.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    let core = core.clone();
    blocking(move || {
        let starting = Starting {
            agent_id: &params.agent_id,
            project: project_of(&params.cwd, &core.workspaces.project_paths()),
            provider: params.provider.as_deref(),
            resume_id: params.resume_id.as_deref(),
        };
        let kept = core.names.reopen(&starting, core.running_agents());
        let named = core.name_agent(
            &params.agent_id,
            &params.cwd,
            params.provider.as_deref(),
            params.resume_id.as_deref(),
        );
        let buddy = match kept {
            Some(kept) => kept,
            None => {
                core.names
                    .note_look(&params.agent_id, params.buddy, params.title);
                None
            }
        };
        Ok(NamedAgent { named, buddy })
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClosedAgentsParams {
    cwd: PathBuf,
}

/// The closed agents of the project at `cwd` that keep the names the person
/// gave them, newest first.
async fn closed_agents(
    core: &Arc<Core>,
    params: ClosedAgentsParams,
) -> CoreResult<Vec<ClosedAgent>> {
    let core = core.clone();
    blocking(move || {
        let project = project_of(&params.cwd, &core.workspaces.project_paths());
        Ok(core.names.closed(&project, core.running_agents()))
    })
    .await
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

#[derive(Deserialize)]
struct PersonSavedParams {
    path: PathBuf,
}

#[derive(Deserialize)]
struct CheckoutParams {
    dir: PathBuf,
    #[serde(default)]
    staged: bool,
}

/// Which agent wrote each uncommitted line of a checkout.
async fn change_owners(core: &Arc<Core>, params: CheckoutParams) -> CoreResult<Owners> {
    let core = core.clone();
    blocking(move || Ok(edits::owners_at(&core, &params.dir, params.staged)?)).await
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
            core.activity_log
                .record_commit(&params.dir.to_string_lossy());
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
