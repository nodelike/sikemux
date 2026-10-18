//! Chat agents run in the background core, so a turn keeps going when the
//! window reloads or the app quits. The app prepares each launch — the agent
//! binary, the browser tools and the environment the agent gets — and
//! forwards every command to the core. The core's events come back on its
//! connection and reach the page as `acp_event`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_core::acp::bounded_text;
use sikemux_core::agent_accounts;
use sikemux_core::client::{ClientError, CoreClient, Reply};
use sikemux_core::launch::{AgentLaunch, AgentSpec};
use sikemux_core::protocol::{
    ChatAccount, ChatAttachment, ChatContext, ChatEvent, ChatEventKind, ChatInfo, ChatLaunch,
    ChatStart, Request, Response,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::pty::PtyManager;

const MAX_AGENT_ID: usize = 200;
/// Streamed updates in one replayed event, so a long chat comes back in a
/// few script evals rather than one per update.
const REPLAY_BATCH: usize = 2_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpEvent<'a> {
    agent_id: &'a str,
    kind: &'a str,
    payload: Value,
}

/// Names the conversation on disk so the session watcher can leave it alone
/// while a turn writes to it.
#[derive(Clone)]
struct StreamMark {
    provider: String,
    cwd: String,
    config_path: Option<String>,
    session_id: String,
}

impl StreamMark {
    fn set(&self, streaming: bool) {
        crate::agents::note_streaming_session(
            &self.provider,
            &self.cwd,
            self.config_path.as_deref(),
            &self.session_id,
            streaming,
        );
    }
}

#[derive(Default)]
pub struct AcpManager {
    /// Launches being prepared, which a stop cancels. The id tells a launch
    /// from a later one for the same agent.
    preparing: Mutex<HashMap<String, uuid::Uuid>>,
    /// The chats this app shows, which hear the core's events for them.
    shown: Mutex<HashMap<String, StreamMark>>,
}

impl AcpManager {
    fn preparing(&self) -> std::sync::MutexGuard<'_, HashMap<String, uuid::Uuid>> {
        self.preparing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Ends `launch_id`'s preparation, and says whether it was still going,
    /// that is, whether no stop came for it.
    fn finish_preparing(&self, agent_id: &str, launch_id: uuid::Uuid) -> bool {
        let mut preparing = self.preparing();
        let current = preparing.get(agent_id) == Some(&launch_id);
        if current {
            preparing.remove(agent_id);
        }
        current
    }

    fn show(&self, agent_id: &str, mark: StreamMark, streaming: bool) {
        mark.set(streaming);
        if let Ok(mut shown) = self.shown.lock() {
            if let Some(previous) = shown.insert(agent_id.to_owned(), mark) {
                previous.set(false);
            }
        }
    }

    fn forget(&self, agent_id: &str) {
        let mark = self
            .shown
            .lock()
            .ok()
            .and_then(|mut shown| shown.remove(agent_id));
        if let Some(mark) = mark {
            mark.set(false);
        }
    }

    fn mark(&self, agent_id: &str, streaming: bool) {
        if let Some(mark) = self
            .shown
            .lock()
            .ok()
            .and_then(|shown| shown.get(agent_id).cloned())
        {
            mark.set(streaming);
        }
    }

    fn take_shown(&self) -> Vec<String> {
        let shown = self
            .shown
            .lock()
            .map(|mut shown| std::mem::take(&mut *shown))
            .unwrap_or_default();
        shown
            .into_iter()
            .map(|(agent_id, mark)| {
                mark.set(false);
                agent_id
            })
            .collect()
    }
}

fn emit(app: &AppHandle, agent_id: &str, kind: &str, payload: Value) {
    let _ = app.emit_to(
        "main",
        "acp_event",
        AcpEvent {
            agent_id,
            kind,
            payload,
        },
    );
}

fn kind_name(kind: ChatEventKind) -> &'static str {
    match kind {
        ChatEventKind::Status => "status",
        ChatEventKind::Ready => "ready",
        ChatEventKind::SessionUpdate => "session_update",
        ChatEventKind::Prompt => "prompt",
        ChatEventKind::TurnStarted => "turn_started",
        ChatEventKind::TurnCompleted => "turn_completed",
        ChatEventKind::PermissionRequest => "permission_request",
        ChatEventKind::Error => "error",
    }
}

/// Hands one event from the core to the page, keeping the session watcher's
/// marks in step with the turns.
pub(crate) fn deliver(app: &AppHandle, agent_id: &str, event: ChatEvent) {
    if let Some(manager) = app.try_state::<AcpManager>() {
        match event.kind {
            ChatEventKind::TurnStarted => manager.mark(agent_id, true),
            ChatEventKind::TurnCompleted => manager.mark(agent_id, false),
            ChatEventKind::Status
                if matches!(event.payload["state"].as_str(), Some("stopped" | "error")) =>
            {
                manager.forget(agent_id)
            }
            _ => {}
        }
    }
    emit(app, agent_id, kind_name(event.kind), event.payload);
}

/// Replays a chat to the page as the events it would have heard, with
/// streamed updates sent in large batches.
fn replay(app: &AppHandle, agent_id: &str, events: Vec<ChatEvent>) {
    let mut updates = Vec::new();
    let flush = |updates: &mut Vec<Value>| {
        if !updates.is_empty() {
            emit(
                app,
                agent_id,
                "session_update",
                json!({ "updates": std::mem::take(updates) }),
            );
        }
    };
    for event in events {
        if event.kind == ChatEventKind::SessionUpdate {
            updates.push(event.payload);
            if updates.len() >= REPLAY_BATCH {
                flush(&mut updates);
            }
            continue;
        }
        flush(&mut updates);
        emit(app, agent_id, kind_name(event.kind), event.payload);
    }
    flush(&mut updates);
}

/// The core went away and came back, still running the chats it lists or
/// having taken them up again, which the page attaches to again.
pub(crate) async fn reconnected(app: &AppHandle, client: Option<&Arc<CoreClient>>) {
    let Some(manager) = app.try_state::<AcpManager>() else {
        return;
    };
    let shown = manager.take_shown();
    if shown.is_empty() {
        return;
    }
    let live: HashSet<String> = match client {
        Some(client) => client
            .acp_list()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|chat| chat.agent_id)
            .collect(),
        None => HashSet::new(),
    };
    for agent_id in shown {
        if live.contains(&agent_id) {
            emit(app, &agent_id, "reattach", json!({}));
        } else {
            emit(
                app,
                &agent_id,
                "status",
                json!({ "state": "stopped", "reason": "exited" }),
            );
        }
    }
}

/// One of the person's accounts, as the page names it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRequest {
    id: String,
    label: String,
    config_path: Option<String>,
}

fn chat_account(provider: &str, account: &AccountRequest) -> ChatAccount {
    agent_accounts::chat_account(
        provider,
        &account.id,
        &account.label,
        account.config_path.as_deref(),
    )
}

fn failure(error: ClientError) -> String {
    error.to_string()
}

async fn core(pty: &PtyManager) -> Result<Arc<CoreClient>, String> {
    pty.client().await.map_err(|error| error.to_string())
}

/// How the app starts `spec` as a chat, on `model_provider` when one is named,
/// with the skills library and, for Hermes and Grok, a private home of its own.
async fn chat_launch(
    app: &AppHandle,
    agent_id: Option<&str>,
    spec: &AgentSpec,
    model_provider: Option<String>,
    model: Option<String>,
) -> Result<AgentLaunch, String> {
    if let Some(path) = spec.config_path.as_deref() {
        bounded_text("config path", path, 4_096)?;
    }
    let program = spec.program().await?;
    let providers = crate::model_providers::wiring(&spec.provider, model_provider, model).await?;
    let mut launch = spec.chat(
        program,
        providers,
        crate::browser::agents::skills_data_dir(app).as_deref(),
    );
    if let Some(agent_id) = agent_id {
        launch
            .env
            .insert(crate::ports::AGENT_ID_ENV.into(), agent_id.to_owned());
        crate::browser::agents::chat_home(
            app,
            agent_id,
            &spec.provider,
            &launch.program,
            &mut launch.env,
        )
        .await;
    }
    Ok(launch)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn acp_start(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    agent_id: String,
    provider: String,
    cwd: String,
    resume_id: Option<String>,
    permission_mode: String,
    config_path: Option<String>,
    executable_path: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    environment_keys: Vec<String>,
    account: Option<AccountRequest>,
    model_provider: Option<String>,
) -> Result<ChatStart, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    bounded_text("provider", &provider, 64)?;
    bounded_text("working directory", &cwd, 4_096)?;
    let launch_id = uuid::Uuid::new_v4();
    manager.preparing().insert(agent_id.clone(), launch_id);
    let spec = AgentSpec {
        provider: provider.clone(),
        executable_path,
        config_path: config_path.clone(),
        environment_keys,
    };
    let prepared = chat_launch(&app, Some(&agent_id), &spec, model_provider, model.clone()).await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            manager.finish_preparing(&agent_id, launch_id);
            return Err(error);
        }
    };
    // The tools this agent can drive its own browser tabs with. A session
    // that cannot be told about them still runs.
    let mcp_servers = match crate::browser::agents::acp_browser_server(&app, &agent_id) {
        Ok(server) => serde_json::to_value(server).into_iter().collect(),
        Err(error) => {
            eprintln!("Sikemux browser tools are unavailable to this agent: {error}");
            Vec::new()
        }
    };
    let launch = ChatLaunch {
        agent_id: agent_id.clone(),
        provider: provider.clone(),
        cwd: PathBuf::from(&cwd),
        program: prepared.program,
        args: prepared.args,
        env: prepared.env,
        mcp_servers,
        resume_id: resume_id.clone(),
        permission_mode,
        model,
        effort,
        account: account
            .as_ref()
            .map(|account| chat_account(&provider, account)),
        fallbacks: Vec::new(),
    };
    let client = core(&pty).await?;
    let started = client
        .submit(
            Request::AcpStart {
                launch: Box::new(launch),
            },
            |reply| match reply {
                Ok(Reply::Response(Response::ChatStarted { start })) => Ok(start),
                Ok(_) => Err(ClientError::UnexpectedReply),
                Err(error) => Err(error),
            },
        )
        .map_err(failure)?;
    // A stop that came while the launch was prepared reaches the core after
    // the start, whichever of the two it raced.
    let stopped = !manager.finish_preparing(&agent_id, launch_id);
    if stopped {
        let _ = client.acp_stop(agent_id).await;
        return Err("ACP session stopped before initialization completed".into());
    }
    let start = started.await.map_err(failure)?.map_err(failure)?;
    manager.show(
        &agent_id,
        StreamMark {
            provider: provider.clone(),
            cwd: cwd.clone(),
            config_path: config_path.clone(),
            session_id: start.session_id.clone(),
        },
        false,
    );
    let session_id = start.session_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::activity::record_launch(
            &provider,
            &cwd,
            "chat",
            resume_id.is_some().then_some(session_id.as_str()),
            config_path.as_deref(),
        )
    });
    Ok(start)
}

/// What the page hears back from an attach. The replay itself arrives as
/// `acp_event`s before any live event.
#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Attachment {
    Live {
        start: Box<ChatStart>,
        permission_mode: String,
        running: bool,
        turned: bool,
        /// The core's run of this chat, which paging back names.
        feed: String,
        /// Set when the replay holds only the chat's last turns: the event
        /// before which `acp_history` pages back.
        older_before: Option<u64>,
    },
    Missing,
}

/// Turns from before what the page shows, and where the page before them starts.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistory {
    events: Vec<ChatEvent>,
    older_before: Option<u64>,
}

/// Takes up a chat the core already runs, if it runs one for this agent.
#[tauri::command]
pub async fn acp_attach(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    agent_id: String,
    provider: String,
    cwd: String,
    config_path: Option<String>,
) -> Result<Attachment, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let client = core(&pty).await?;
    let replaying = app.clone();
    let replay_agent = agent_id.clone();
    // The replay goes to the page on the connection's reader, before any
    // event the core sent after it.
    let replied = client
        .submit(
            Request::AcpAttach {
                agent_id: agent_id.clone(),
                since: None,
            },
            move |reply| match reply {
                Ok(Reply::Response(Response::ChatAttached { attachment })) => {
                    Ok(match attachment {
                        ChatAttachment::Live {
                            start,
                            permission_mode,
                            running,
                            turned,
                            replay: events,
                            mark,
                            older_before,
                        } => {
                            replay(&replaying, &replay_agent, events);
                            Attachment::Live {
                                start,
                                permission_mode,
                                running,
                                turned,
                                feed: mark.feed,
                                older_before,
                            }
                        }
                        ChatAttachment::Missing => Attachment::Missing,
                        // Only an attach that names a mark is resumed, and the core no longer restarts chats.
                        ChatAttachment::Restart | ChatAttachment::Resumed { .. } => {
                            return Err(ClientError::UnexpectedReply)
                        }
                    })
                }
                Ok(_) => Err(ClientError::UnexpectedReply),
                Err(error) => Err(error),
            },
        )
        .map_err(failure)?;
    let attachment = replied.await.map_err(failure)?.map_err(failure)?;
    if let Attachment::Live { start, running, .. } = &attachment {
        manager.show(
            &agent_id,
            StreamMark {
                provider,
                cwd,
                config_path,
                session_id: start.session_id.clone(),
            },
            *running,
        );
    }
    Ok(attachment)
}

/// The chat's turns before event `before` of its run `feed`, for a page shown
/// only the chat's end.
#[tauri::command]
pub async fn acp_history(
    pty: State<'_, PtyManager>,
    agent_id: String,
    feed: String,
    before: u64,
    turns: u32,
) -> Result<ChatHistory, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let (events, older_before) = core(&pty)
        .await?
        .acp_history(agent_id, feed, before, turns)
        .await
        .map_err(failure)?;
    Ok(ChatHistory {
        events,
        older_before,
    })
}

/// Moves a running chat to another account, on the same session.
#[tauri::command]
pub async fn acp_switch_account(
    pty: State<'_, PtyManager>,
    agent_id: String,
    provider: String,
    account: AccountRequest,
) -> Result<(), String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let account = chat_account(&provider, &account);
    core(&pty)
        .await?
        .acp_switch_account(agent_id, account)
        .await
        .map_err(failure)
}

/// The chats the core runs, for the page to stop the ones it no longer shows.
#[tauri::command]
pub async fn acp_list(pty: State<'_, PtyManager>) -> Result<Vec<ChatInfo>, String> {
    core(&pty).await?.acp_list().await.map_err(failure)
}

#[tauri::command]
pub async fn acp_set_permission_mode(
    pty: State<'_, PtyManager>,
    agent_id: String,
    permission_mode: String,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_set_permission_mode(agent_id, permission_mode)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_set_config(
    pty: State<'_, PtyManager>,
    agent_id: String,
    config_id: String,
    value: String,
) -> Result<Value, String> {
    core(&pty)
        .await?
        .acp_set_config(agent_id, config_id, value)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_prompt(
    pty: State<'_, PtyManager>,
    agent_id: String,
    message_id: Option<String>,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_prompt(agent_id, message_id, text, paths, context)
        .await
        .map_err(failure)
}

/// Takes the chat back to before the person's message `message_id` and sends
/// `text` in its place.
#[tauri::command]
pub async fn acp_edit(
    pty: State<'_, PtyManager>,
    agent_id: String,
    message_id: String,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_edit(agent_id, message_id, text, paths, context)
        .await
        .map_err(failure)
}

/// Puts a message into the running turn, answering `promptRequired` when the
/// turn ended first and the caller should send it as a prompt of its own.
#[tauri::command]
pub async fn acp_steer(
    pty: State<'_, PtyManager>,
    agent_id: String,
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
) -> Result<String, String> {
    core(&pty)
        .await?
        .acp_steer(agent_id, text, paths, context)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_cancel(pty: State<'_, PtyManager>, agent_id: String) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_cancel(agent_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_stop_task(
    pty: State<'_, PtyManager>,
    agent_id: String,
    task_id: String,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_stop_task(agent_id, task_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_permission_reply(
    pty: State<'_, PtyManager>,
    agent_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    core(&pty)
        .await?
        .acp_permission_reply(agent_id, request_id, option_id)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_stop(
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    agent_id: String,
) -> Result<(), String> {
    manager.preparing().remove(&agent_id);
    manager.forget(&agent_id);
    core(&pty).await?.acp_stop(agent_id).await.map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_kind_keeps_the_name_the_page_reads() {
        for (kind, name) in [
            (ChatEventKind::Status, "status"),
            (ChatEventKind::Ready, "ready"),
            (ChatEventKind::SessionUpdate, "session_update"),
            (ChatEventKind::TurnStarted, "turn_started"),
            (ChatEventKind::TurnCompleted, "turn_completed"),
            (ChatEventKind::PermissionRequest, "permission_request"),
            (ChatEventKind::Error, "error"),
        ] {
            assert_eq!(kind_name(kind), name);
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    #[test]
    fn an_attachment_reads_as_the_page_expects() {
        let live = Attachment::Live {
            start: Box::new(ChatStart {
                session_id: "s".into(),
                capabilities: json!({}),
                setup: json!({}),
            }),
            permission_mode: "bypass".into(),
            running: true,
            turned: false,
            feed: "f".into(),
            older_before: Some(7),
        };
        assert_eq!(
            serde_json::to_value(live).unwrap(),
            json!({
                "status": "live",
                "start": { "sessionId": "s", "capabilities": {}, "setup": {} },
                "permissionMode": "bypass",
                "running": true,
                "turned": false,
                "feed": "f",
                "olderBefore": 7,
            })
        );
    }
}
