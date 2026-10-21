//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::{Needs, Peer};
use super::attachments;
use super::changed::Topic;
use super::connection::{blocking, ClientConn};
use super::edits::{self, Edit, Filter, Owners};
use super::names::{project_of, AgentName, ClosedAgent, Starting};
use super::workspace;
use super::{
    agent_accounts, agent_sign_in, board, inbox, messages, model_providers, plugins, questions,
    remote, saved, saved_sessions, share, skill_library, who_wrote, Core, CoreError, CoreResult,
};

/// Each method once: what a client must be allowed, `any_program` when any
/// program on this host may call it rather than only Sikemux's own window or
/// notch, and what it runs.
macro_rules! calls {
    ($core:ident, $client:ident, $params:ident;
     $($name:literal: $needs:ident $(, $any:ident)? => $run:expr,)*) => {
        fn rule(method: &str) -> Option<(Needs, bool)> {
            match method {
                $($name => Some((Needs::$needs, calls!(@any $($any)?))),)*
                _ => None,
            }
        }

        pub(crate) async fn call(
            $core: Arc<Core>,
            $client: &ClientConn,
            method: &str,
            $params: Value,
        ) -> CoreResult<Value> {
            match method {
                $($name => $run,)*
                method if method.starts_with("git.") => {
                    super::git::call(&$core, &$client.peer, method, $params).await
                }
                method if method.starts_with("files.") => files(&$core, method, $params).await,
                _ => Err(format!("this host does not know {method}").into()),
            }
        }
    };
    (@any any_program) => { true };
    (@any) => { false };
}

pub(crate) fn needs(method: &str) -> Needs {
    if method.starts_with("git.") {
        return super::git::needs(method);
    }
    if method.starts_with("files.") {
        return Needs::Full;
    }
    rule(method).map_or(Needs::Local, |(needs, _)| needs)
}

/// Whether only Sikemux's own window or notch may call `method` from this
/// host; another program here may only read, or start a plugin's browser
/// sign-in, which only the person can finish in their browser.
pub(crate) fn for_person(method: &str) -> bool {
    if method.starts_with("git.") {
        return super::git::for_person(method);
    }
    rule(method).is_none_or(|(_, any_program)| !any_program)
}

calls! { core, client, params;
    "chatFile": Watch, any_program => answer(chat_file(&core, &client.peer, parse(params)?).await?),
    "nameAgent": Local => answer(name_agent(&core, parse(params)?).await?),
    "renameAgent": Full => answer(rename_agent(&core, parse(params)?).await?),
    "closedAgents": Local => answer(closed_agents(&core, parse(params)?).await?),
    "publishBuddies": Local => answer(core.workspaces.publish_buddies(parse(params)?)?),
    "agentNames": Watch, any_program => answer(agent_names(&core).await?),
    "hostLoad": Watch, any_program => answer(super::load::now()),
    "stopChatIfIdle": Local => {
        let params: AgentParams = parse(params)?;
        answer(json!({ "stopped": super::lifecycle::sleep(&core, &params.agent_id) }))
    },
    "sleepIdleChats": Local => answer(json!({ "slept": super::lifecycle::sleep_idle(&core) })),
    "startTerminalAgent": Local => {
        let params: TerminalParams = parse(params)?;
        let size = (params.cols, params.rows);
        let pty = super::launching::start_terminal(&core, &params.agent_id, size, None).await?;
        answer(json!({ "ptyId": pty }))
    },
    "startChat": Local => {
        let params: AgentParams = parse(params)?;
        answer(super::launching::start_saved(&core, &params.agent_id).await?)
    },
    "activitySummary": Watch, any_program => answer(activity_summary(&core, parse(params)?).await?),
    "setActivityPrice": Person => {
        let params: ActivityPriceParams = parse(params)?;
        core.activity_log.set_price(&params.model, params.price)?;
        core.changes.changed(Topic::Activity, None, None);
        answer(())
    },
    "steerQueued": Full => {
        let params: QueuedParams = parse(params)?;
        answer(
            super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message)
                .await?,
        )
    },
    "dropQueued": Full => {
        let params: QueuedParams = parse(params)?;
        answer(super::chat::queue::remove(&core, &params.agent_id, &params.ids)?)
    },
    "agentSessions": Local, any_program => answer(saved::call(&core, parse(params)?).await?),
    "workspace": Watch, any_program => answer(core.workspaces.view()),
    "runCommand": Local => answer(super::custom_command::run(parse(params)?).await?),
    "addProject": Person => answer(workspace::add_project(&core, parse(params)?).await?),
    "removeProject": Person => answer(workspace::remove_project(&core, parse(params)?)?),
    "updateProject": Person => answer(workspace::update_project(&core, parse(params)?)?),
    "moveProject": Person => answer(workspace::move_project(&core, parse(params)?)?),
    "setSpaces": Person => answer(workspace::set_spaces(&core, parse(params)?)?),
    "setPinnedChats": Person => answer(workspace::set_pinned_chats(&core, parse(params)?)?),
    "setAgentDefaults": Person => answer(workspace::set_defaults(&core, parse(params)?).await?),
    "createAgent": Person => answer(workspace::create_agent(&core, parse(params)?)?),
    "updateAgent": Person => answer(workspace::update_agent(&core, parse(params)?)?),
    "closeAgents": Person => answer(workspace::close_agents(&core, parse(params)?)?),
    "closeChat": Full => {
        let params: AgentParams = parse(params)?;
        answer(workspace::close_agent(&core, params.agent_id)?)
    },
    "createAgentWorktree": Person => answer(super::sandbox::for_agent(&core, parse(params)?).await?),
    "agentWorktreeAt": Full => answer(super::sandbox::at(parse(params)?).await?),
    "savedSessions": Full, any_program => answer(saved_sessions::list(parse(params)?).await?),
    "recentSessions": Full, any_program => answer(saved_sessions::recent(parse(params)?).await?),
    "renameSession": Person => answer(saved_sessions::rename(parse(params)?).await?),
    "deleteSession": Person => answer(saved_sessions::delete(parse(params)?).await?),
    "relocateSession": Person => answer(saved_sessions::relocate(parse(params)?).await?),
    "skillsOverview": Full, any_program => skill_library::overview(&core).await,
    "skillsAdded": Full, any_program => skill_library::added(&core).await,
    "createSkill": Person => skill_library::create(&core, parse(params)?).await,
    "installSkill": Person => skill_library::install(&core, parse(params)?).await,
    "removeSkill": Person => skill_library::remove(&core, parse(params)?).await,
    "adoptSkill": Person => skill_library::adopt(&core, parse(params)?).await,
    "agentSignIns": Full, any_program => answer(agent_sign_in::pages()),
    "agentSignIn": Person => answer(agent_sign_in::sign_in(&core, parse(params)?).await?),
    "agentSignInCode": Person => answer(agent_sign_in::sign_in_code(parse(params)?).await?),
    "cancelAgentSignIn": Person => {
        agent_sign_in::cancel(parse(params)?);
        answer(())
    },
    "agentSignOut": Person => answer(agent_sign_in::sign_out(parse(params)?).await?),
    "modelProviders": Full, any_program => answer(model_providers::list(&core).await?),
    "modelProviderModels": Full, any_program => {
        answer(model_providers::models(&core, parse(params)?).await?)
    },
    "modelHelper": Full, any_program => answer(model_providers::helper(&core).await?),
    "modelProviderCheck": Person => answer(model_providers::check(&core, parse(params)?).await?),
    "modelProviderConnect": Person => {
        answer(model_providers::connect(&core, parse(params)?).await?)
    },
    "modelProviderDisconnect": Person => {
        answer(model_providers::disconnect(&core, parse(params)?).await?)
    },
    "setModelHelper": Person => answer(model_providers::set_helper(&core, parse(params)?).await?),
    "agentAccounts": Full, any_program => answer(core.workspaces.accounts()),
    "agentStatus": Full, any_program => answer(agent_accounts::status(&core, parse(params)?).await?),
    "agentAvailability": Full, any_program => {
        answer(agent_accounts::availability(&core, parse(params)?).await?)
    },
    "agentAccountStatus": Full, any_program => {
        answer(agent_accounts::account_status(&core, parse(params)?).await?)
    },
    "agentUsage": Full, any_program => answer(agent_accounts::usage(&core, parse(params)?).await?),
    "cancelResume": Full => {
        agent_accounts::cancel_resume(&core, parse(params)?);
        answer(())
    },
    "setAgentAccounts": Person => {
        agent_accounts::set(&core, parse(params)?).await?;
        answer(())
    },
    "addAgentAccount": Person => answer(agent_accounts::add(parse(params)?).await?),
    "forgetAgentStatus": Person => {
        agent_accounts::forget(&core, parse(params)?);
        answer(())
    },
    "markAgentSignedOut": Person => {
        agent_accounts::mark_signed_out(&core, parse(params)?);
        answer(())
    },
    "saveAgentAccount": Local => answer(agent_accounts::save(&core, parse(params)?).await?),
    "removeAgentAccount": Local => answer(agent_accounts::remove(&core, parse(params)?).await?),
    "selectAgentAccount": Local => answer(agent_accounts::select(&core, parse(params)?).await?),
    "setAccountAutoSwitch": Local => {
        answer(agent_accounts::set_auto_switch(&core, parse(params)?).await?)
    },
    "retryFailed": Local => answer(agent_accounts::retry_failed(&core, parse(params)?)?),
    "discardFailed": Local => answer(agent_accounts::discard_failed(&core, parse(params)?)?),
    "authenticateChat": Local => {
        agent_accounts::authenticate(&core, parse(params)?).await?;
        answer(())
    },
    "shareChat": Person => {
        let params: ShareChatParams = parse(params)?;
        answer(share::share_chat(
            &core,
            &params.agent_id,
            params.lifetime_ms,
            params.title,
        )?)
    },
    "stopShare": Person => {
        let params: StopShareParams = parse(params)?;
        answer(share::stop(&core, &params.id)?)
    },
    "shares": Full, any_program => answer(share::list(&core)),
    "pairing": Person, any_program => answer(remote::pairing(&core)),
    "signHostRequest": Local => {
        let request: crate::accounts::HostRequest = parse(params)?;
        answer(signed(core.remote.sign_host_request(&request)?))
    },
    "signShareViewer": Local => {
        let params: NonceParams = parse(params)?;
        answer(signed(core.remote.sign_share_viewer(&params.nonce)?))
    },
    "answerPairing": Person => answer(remote::answer_pairing(&core, parse(params)?)?),
    "changeDevice": Person => answer(remote::change_device(&core, parse(params)?)?),
    "linkHost": Person => answer(super::hosts::link_host(&core, parse(params)?).await?),
    "requireApproval": Person => answer(json!({ "requireApproval": core.remote.requires_approval() })),
    "setRequireApproval": Person => answer(remote::set_require_approval(&core, parse(params)?)?),
    "questions": Watch, any_program => answer(questions::views(&core)),
    "answerQuestion": Full => answer(questions::answer(&core, parse(params)?).await?),
    "answerQuestions": Full => answer(questions::answer_all(&core, parse(params)?).await?),
    "agentMessages": Watch, any_program => answer(agent_messages(&core, parse(params)?).await?),
    "boards": Watch, any_program => answer(board::boards(&core)),
    "board": Watch, any_program => answer(board::board(&core, params).await?),
    "boardAdd": Full => answer(board::person_add(&core, params).await?),
    "boardAddTicket": Full => answer(board::person_add_ticket(&core, params).await?),
    "boardUpdate": Full => answer(board::person_update(&core, params).await?),
    "boardRemove": Full => answer(board::person_remove(&core, params).await?),
    "boardSettings": Full => answer(board::person_settings(&core, params).await?),
    "boardRetry": Full => answer(board::person_retry(&core, params).await?),
    "edits": Watch, any_program => answer(edits(&core, parse(params)?).await?),
    "changeOwners": Watch, any_program => answer(change_owners(&core, parse(params)?).await?),
    "commitStaged": Person => answer(commit_staged(&core, parse(params)?).await?),
    "personSaved": Local => {
        only_window(&core, client)?;
        let params: PersonSavedParams = parse(params)?;
        core.edits.saved(params.path);
        answer(())
    },
    "commitOwners": Watch, any_program => answer(who_wrote::call(&core, parse(params)?).await?),
    "inbox": Watch, any_program => answer(inbox(&core).await?),
    "digest": Watch, any_program => answer(digest(&core, parse(params)?).await?),
    "ports": Local => {
        let params: PortsParams = parse(params)?;
        answer(super::ports::project_ports(&core, client.id, params.project).await?)
    },
    "testResults": Local => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::module_tests(&core, params.project).await?)
    },
    "projectConfig": Local => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::project_config(&core, params.project).await?)
    },
    "trustProject": Local => {
        let params: TrustProjectParams = parse(params)?;
        answer(super::tasks::trust_project(&core, params.project, params.fingerprint).await?)
    },
    "projectFolders": Local => {
        let params: ProjectParams = parse(params)?;
        let settings = core.workspaces.folder_settings(&params.project);
        let rules = core.workspaces.project_rules(&params.project);
        let folders =
            blocking(move || Ok(crate::tasks::folders::folders(Path::new(&params.project))))
                .await?;
        let fields: serde_json::Map<String, Value> = crate::tasks::languages::catalog()
            .into_iter()
            .map(|(language, fields)| (language.to_owned(), json!(fields)))
            .collect();
        answer(json!({ "folders": folders, "settings": settings, "rules": rules, "fields": fields }))
    },
    "projectServices": Local => {
        let params: ProjectParams = parse(params)?;
        answer(blocking(move || Ok(super::services::list(Path::new(&params.project)))).await?)
    },
    "serviceAction": Local => answer(super::services::action(parse(params)?).await?),
    "javaRuntimes": Local => answer(blocking(|| Ok(crate::tasks::java::runtimes())).await?),
    "projectSuggestions": Local => answer(super::suggestions::list(&core, parse(params)?).await?),
    "answerSuggestion": Person => answer(super::suggestions::answer(&core, parse(params)?).await?),
    "projectBrief": Local => {
        let params: ProjectParams = parse(params)?;
        let core = core.clone();
        answer(blocking(move || Ok(super::brief::write(&core, Path::new(&params.project)))).await?)
    },
    "changeProjectSettings": Person => answer(workspace::change_project(&core, parse(params)?)?),
    "untrustProject": Local => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::untrust_project(&core, params.project).await?)
    },
    "tasks": Local => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::list(&core, params.project).await?)
    },
    "taskRuns": Local => answer(super::tasks::runs(&core)),
    "taskStart": Local => answer(super::tasks::start(&core, parse(params)?).await?),
    "taskRerunFailed": Local => answer(super::tasks::rerun_failed(&core, parse(params)?).await?),
    "taskStop": Local => {
        super::harness::stop_runs(&core, parse(params)?).await;
        answer(())
    },
    "trustAsks": Local => answer(super::tasks::asks(&core)),
    "showCi": Watch, any_program => {
        super::ci::show(&core, client.id, parse(params)?);
        answer(())
    },
    "answerTrust": Local => {
        let params: TrustAnswerParams = parse(params)?;
        answer(super::tasks::answer_trust(&core, &params.id, params.trust)?)
    },
    "addTask": Local => answer(super::tasks::add(&core, parse(params)?).await?),
    "removeTask": Local => answer(super::tasks::remove(&core, parse(params)?).await?),
    "setCheck": Local => answer(super::tasks::set_check(&core, parse(params)?).await?),
    "agentModels": Local => {
        let params: AgentModelsParams = parse(params)?;
        answer(super::chat::offered::models(&params.agent))
    },
    "plugin": Person => answer(plugins::window_call(&core, parse(params)?).await),
    "pluginManifests": Person => answer(plugins::manifests()),
    "pluginSetDisabled": Person => {
        plugins::set_disabled(&core, parse(params)?);
        answer(())
    },
    "pluginStream": Person => answer(plugins::stream_ticket(parse(params)?)?),
    "pluginSignIn": Person, any_program => {
        not_an_agent(client).await?;
        answer(plugins::sign_in_ticket(parse(params)?)?)
    },
    "signInCallback": Person, any_program => {
        not_an_agent(client).await?;
        answer(plugins::sign_in_callback(parse(params)?)?)
    },
}

/// The file tree's and the editor's methods on this host's disk, each read
/// answering with at most a piece of a file so other replies keep moving.
async fn files(core: &Core, method: &str, params: Value) -> CoreResult<Value> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let method = method.to_owned();
    let (method, answer) = blocking(move || {
        let answer = crate::files::run(&method, params, crate::files::PIECE_BYTES)
            .map_err(|error| CoreError::from(error.to_string()))?;
        Ok((method, answer))
    })
    .await?;
    if let Some(path) = path.filter(|_| crate::files::saved(&method, &answer)) {
        core.edits.saved(path);
    }
    Ok(answer)
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

pub(super) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
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
    project: Option<String>,
}

#[derive(Deserialize)]
struct TrustProjectParams {
    project: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct AgentModelsParams {
    agent: String,
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
    provider: Option<String>,
    resume_id: Option<String>,
    /// How the app draws it, kept for when it is opened again.
    buddy: Option<Value>,
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
        let kept = core.names.reopen(&starting, core.open_agents());
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
        Ok(core.names.closed(&project, core.open_agents()))
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
            .rename(&params.agent_id, &params.name, core.open_agents())?;
        core.view_changed();
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
    since: Option<u64>,
}

/// What finished, failed and waits since the person last left, or `since`.
async fn digest(core: &Arc<Core>, params: DigestParams) -> CoreResult<inbox::Digest> {
    let core = core.clone();
    blocking(move || Ok(inbox::digest(&core, params.since))).await
}
