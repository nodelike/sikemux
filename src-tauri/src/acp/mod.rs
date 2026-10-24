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
use sikemux_core::client::{ClientError, CoreClient};
use sikemux_core::protocol::{ChatAccount, ChatAttachment};
use sikemux_process::sync::Locked;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::hosts::{self, Hosts};
use crate::pty::PtyManager;

const MAX_AGENT_ID: usize = 200;

/// What the page hears of a chat: `changes` the core made to it, numbered by
/// `seq`; `reattach` when its core came back still running it; `stopped`
/// when it came back without it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpEvent<'a> {
    agent_id: &'a str,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
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

fn emit(app: &AppHandle, agent_id: &str, kind: &'static str, seq: Option<u64>, payload: Value) {
    let _ = app.emit_to(
        "main",
        "acp_event",
        AcpEvent {
            agent_id,
            kind,
            seq,
            payload,
        },
    );
}

/// Whether `changes` say the chat's agent stopped.
pub(crate) fn ended(changes: &Value) -> bool {
    changes.as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["op"] == "head"
                && matches!(
                    change["set"]["connection"].as_str(),
                    Some("stopped" | "error")
                )
        })
    })
}

/// Hands what changed in a chat to the page.
pub(crate) fn deliver(app: &AppHandle, agent_id: &str, seq: u64, changes: Value) {
    if let (true, Some(manager)) = (ended(&changes), app.try_state::<AcpManager>()) {
        manager.forget(agent_id);
    }
    emit(app, agent_id, "changes", Some(seq), changes);
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
        let kind = if live.contains(&agent_id) {
            "reattach"
        } else {
            "stopped"
        };
        emit(app, &agent_id, kind, None, json!({}));
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

/// What the page hears back from an attach.
#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Attachment {
    /// The chat as the core folded it, a `ChatView`; changes numbered past
    /// `seq` follow as `acp_event`s.
    Live {
        view: Value,
        /// The core's run of this chat, which paging back names.
        feed: String,
        seq: u64,
    },
    Missing,
}

/// Takes up a chat the core already runs, if it runs one for this agent.
#[tauri::command]
pub async fn acp_attach(
    manager: State<'_, AcpManager>,
    pty: State<'_, PtyManager>,
    hosts: State<'_, Hosts>,
    host: Option<String>,
    agent_id: String,
) -> Result<Attachment, String> {
    bounded_text("agent id", &agent_id, MAX_AGENT_ID)?;
    let client = core(&pty, &hosts, host.as_deref()).await?;
    let attachment = match client.acp_attach(agent_id.clone()).await.map_err(failure)? {
        ChatAttachment::Live { view, mark } => Attachment::Live {
            view,
            feed: mark.feed,
            seq: mark.seq,
        },
        ChatAttachment::Missing => Attachment::Missing,
        // Only an attach that names a mark is resumed.
        ChatAttachment::Resumed { .. } => return Err(failure(ClientError::UnexpectedReply)),
    };
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
            view: json!({ "rows": [] }),
            feed: "f".into(),
            seq: 7,
        };
        assert_eq!(
            serde_json::to_value(live).unwrap(),
            json!({ "status": "live", "view": { "rows": [] }, "feed": "f", "seq": 7 })
        );
    }

    #[test]
    fn a_head_change_to_stopped_ends_the_chat() {
        assert!(ended(
            &json!([{ "op": "head", "set": { "connection": "stopped" } }])
        ));
        assert!(!ended(
            &json!([{ "op": "head", "set": { "running": false } }])
        ));
    }
}
