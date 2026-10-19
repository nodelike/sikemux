//! Chat agents run in the background core, so a turn keeps going when the
//! window reloads or the app quits. The core starts each chat from the agent
//! it keeps; the app forwards every command to it, and its events come back
//! on its connection and reach the page as `acp_event`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_core::acp::bounded_text;
use sikemux_core::agent_accounts;
use sikemux_core::client::{ClientError, CoreClient, Reply};
use sikemux_core::protocol::{
    ChatAccount, ChatAttachment, ChatContext, ChatEvent, ChatEventKind, ChatInfo, ChatStart,
    Request, Response,
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

#[derive(Default)]
pub struct AcpManager {
    /// The chats this app shows, which hear the core's events for them.
    shown: Mutex<HashSet<String>>,
}

impl AcpManager {
    fn show(&self, agent_id: &str) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.insert(agent_id.to_owned());
        }
    }

    fn forget(&self, agent_id: &str) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.remove(agent_id);
        }
    }

    fn take_shown(&self) -> Vec<String> {
        self.shown
            .lock()
            .map(|mut shown| std::mem::take(&mut *shown).into_iter().collect())
            .unwrap_or_default()
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

/// Hands one event from the core to the page.
pub(crate) fn deliver(app: &AppHandle, agent_id: &str, event: ChatEvent) {
    let ended = event.kind == ChatEventKind::Status
        && matches!(event.payload["state"].as_str(), Some("stopped" | "error"));
    if let (true, Some(manager)) = (ended, app.try_state::<AcpManager>()) {
        manager.forget(agent_id);
    }
    emit(app, agent_id, kind_name(event.kind), event.payload);
}

/// A permission request was answered or withdrawn, wherever that happened.
pub(crate) fn permission_cleared(app: &AppHandle, agent_id: &str, request_id: String) {
    emit(
        app,
        agent_id,
        "permission_cleared",
        json!({ "requestId": request_id }),
    );
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
    if let Attachment::Live { .. } = &attachment {
        manager.show(&agent_id);
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
