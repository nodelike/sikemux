//! Chat agents run in the background core, so a turn keeps going when the
//! window reloads or the app quits. The core starts each chat from the agent
//! it keeps; the app forwards every command to it, and its events come back
//! on its connection and reach the page as `acp_event`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_core::acp::bounded_text;
use sikemux_core::agent_accounts;
use sikemux_core::client::{ClientError, CoreClient, Reply};
use sikemux_core::protocol::{
    ChatAccount, ChatAttachment, ChatEvent, ChatEventKind, ChatStart, Request, Response,
};
use sikemux_process::sync::Locked;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::hosts::{self, Hosts};
use crate::pty::PtyManager;

const MAX_AGENT_ID: usize = 200;
/// Streamed updates in one replayed event, so a long chat comes back in a
/// few script evals rather than one per update.
const REPLAY_BATCH: usize = 2_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpEvent<'a, Kind> {
    agent_id: &'a str,
    kind: Kind,
    payload: Value,
}

#[derive(Default)]
pub struct AcpManager {
    /// The chats this app shows, which hear the core's events for them, by
    /// the host each runs on; none is this one.
    shown: Mutex<HashMap<String, Option<String>>>,
}

impl AcpManager {
    fn show(&self, agent_id: &str, host: Option<String>) {
        self.shown.locked().insert(agent_id.to_owned(), host);
    }

    fn forget(&self, agent_id: &str) {
        self.shown.locked().remove(agent_id);
    }

    fn take_shown(&self, host: Option<&str>) -> Vec<String> {
        let mut shown = self.shown.locked();
        let taken: Vec<String> = shown
            .iter()
            .filter(|(_, on)| on.as_deref() == host)
            .map(|(agent_id, _)| agent_id.clone())
            .collect();
        for agent_id in &taken {
            shown.remove(agent_id);
        }
        taken
    }
}

fn emit(app: &AppHandle, agent_id: &str, kind: impl Serialize + Clone, payload: Value) {
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

/// Hands one event from the core to the page.
pub(crate) fn deliver(app: &AppHandle, agent_id: &str, event: ChatEvent) {
    let ended = event.kind == ChatEventKind::Status
        && matches!(event.payload["state"].as_str(), Some("stopped" | "error"));
    if let (true, Some(manager)) = (ended, app.try_state::<AcpManager>()) {
        manager.forget(agent_id);
    }
    emit(app, agent_id, event.kind, event.payload);
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
                ChatEventKind::SessionUpdate,
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
        emit(app, agent_id, event.kind, event.payload);
    }
    flush(&mut updates);
}

/// The core of `host`, or this host's own, went away and came back, still
/// running the chats it lists or having taken them up again, which the page
/// attaches to again.
pub(crate) async fn reconnected(
    app: &AppHandle,
    host: Option<&str>,
    client: Option<&Arc<CoreClient>>,
) {
    let Some(manager) = app.try_state::<AcpManager>() else {
        return;
    };
    let shown = manager.take_shown(host);
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
                ChatEventKind::Status,
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

async fn core(
    pty: &PtyManager,
    hosts: &Hosts,
    host: Option<&str>,
) -> Result<Arc<CoreClient>, String> {
    hosts::client(pty, hosts, host)
        .await
        .map_err(|error| error.to_string())
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
        /// before which `acpHistory` pages back.
        older_before: Option<u64>,
    },
    Missing,
}

/// Takes up a chat the core already runs, if it runs one for this agent.
#[tauri::command]
pub async fn acp_attach(
    app: AppHandle,
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    agent_id: String,
) -> Result<Attachment, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let client = core(&pty, &hosts, host.as_deref()).await?;
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
        manager.show(&agent_id, host);
    }
    Ok(attachment)
}

/// Moves a running chat to another account, on the same session.
#[tauri::command]
pub async fn acp_switch_account(
    pty: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    agent_id: String,
    provider: String,
    account: AccountRequest,
) -> Result<(), String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let account = chat_account(&provider, &account);
    core(&pty, &hosts, host.as_deref())
        .await?
        .acp_switch_account(agent_id, account)
        .await
        .map_err(failure)
}

#[tauri::command]
pub async fn acp_stop(
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    agent_id: String,
) -> Result<(), String> {
    manager.forget(&agent_id);
    core(&pty, &hosts, host.as_deref())
        .await?
        .acp_stop(agent_id)
        .await
        .map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

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
