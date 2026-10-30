//! Background tasks and subagents, which core ACP has no updates for.
//!
//! An ACP agent carries both as an extension called AIR, nested inside `_meta`
//! so peers that do not know it skip it. It stays silent until a client names
//! the parts it understands in `initialize`. The Claude and Codex drivers tell
//! the chat the same updates.

use agent_client_protocol::schema::v1::{
    AuthCapabilities, ClientCapabilities, ClientSessionCapabilities, ContentBlock, Meta,
    NoticeCapabilities,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::server::chat::failure::{Failure, FailureKind};

const EXTENSION_VERSION: u8 = 1;
const EXTENSION_CAPABILITIES: [&str; 3] =
    ["asyncTasks", "nativeSubagentSessions", "sessionFailure"];

/// Asks the agent for async task and subagent session updates, and for a
/// failed turn's cause and its asides as data rather than as text in the
/// transcript. An aside sent as text reads as the agent starting a turn.
pub fn client_capabilities() -> ClientCapabilities {
    let mut meta = Meta::new();
    meta.insert(
        "jetbrains".into(),
        json!({
            "air": {
                "version": EXTENSION_VERSION,
                "capabilities": EXTENSION_CAPABILITIES,
            }
        }),
    );
    // Sikemux can run the agent's own sign-in in a terminal, so the agent may
    // offer it. Zed's earlier form of the same capability is in `_meta`.
    meta.insert("terminal-auth".into(), json!(true));
    ClientCapabilities::new()
        .session(ClientSessionCapabilities::new().notices(NoticeCapabilities::new()))
        .auth(AuthCapabilities::new().terminal(true))
        .meta(meta)
}

/// Whether the agent takes a message aimed at the turn already running.
///
/// Steering is its own extension rather than part of AIR, advertised at the top
/// level of the initialize response.
pub fn steering_supported(initialize_meta: &Value) -> bool {
    initialize_meta
        .pointer("/steering/supported")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Puts a message into the turn that is already running.
///
/// `idleBehavior` asks for the message back when the turn settled first, rather
/// than letting the agent open a turn of its own that this side never started
/// and so would never see finish. The agent answers with its `outcome`:
/// `injected` when the message reached the running turn, `promptRequired` when
/// there was no turn left to reach and the message is the client's again.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Steer {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
    #[serde(rename = "_meta")]
    pub meta: Value,
}

impl Steer {
    pub const METHOD: &str = "_session/steering";

    pub fn new(session_id: String, prompt: Vec<ContentBlock>) -> Self {
        Self {
            session_id,
            prompt,
            meta: json!({ "steering": { "idleBehavior": "promptRequired" } }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopAsyncTask {
    pub session_id: String,
    pub async_task_id: String,
}

impl StopAsyncTask {
    pub const METHOD: &str = "_session/async_task/stop";
}

/// The failure an ACP agent attached to a prompt's answer or a session update.
///
/// It comes as the `sessionFailure` part of the AIR extension, once the client
/// says it understands it. Its kind is not sent, only a category and the
/// actions that would fix it.
pub fn failure(meta: &Value) -> Option<Failure> {
    let failure = meta.pointer("/jetbrains/air/sessionFailure")?;
    if failure.get("severity").and_then(Value::as_str) == Some("warning") {
        return None;
    }
    let category = failure.get("category").and_then(Value::as_str);
    let offers = |action: &str| {
        failure
            .get("actions")
            .and_then(Value::as_array)
            .is_some_and(|actions| actions.iter().any(|item| item == action))
    };
    let kind = match category {
        Some("access") if offers("login") => FailureKind::SignIn,
        // A context or budget limit belongs to the session, not the account,
        // and its way out is a new session rather than another account.
        Some("limit") if !offers("new_session") => FailureKind::Limit,
        Some("connection") => FailureKind::Network,
        _ => FailureKind::Other,
    };
    let title = failure
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("The agent could not finish this turn")
        .to_owned();
    let details = failure
        .get("details")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if kind == FailureKind::Network {
        return Some(Failure::lost_connection(details.unwrap_or(title)));
    }
    Some(Failure {
        kind,
        title,
        details,
        resets_at: failure.get("resetsAt").and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::chat::failure::{retrying, LOST_CONNECTION};

    #[test]
    fn capabilities_offer_a_terminal_sign_in_in_both_forms() {
        let capabilities = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(capabilities.pointer("/auth/terminal"), Some(&json!(true)));
        assert_eq!(
            capabilities.pointer("/_meta/terminal-auth"),
            Some(&json!(true))
        );
    }

    #[test]
    fn capabilities_name_the_extension_parts_the_frontend_renders() {
        let capabilities = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(
            capabilities.pointer("/_meta/jetbrains/air"),
            Some(&json!({
                "version": 1,
                "capabilities": ["asyncTasks", "nativeSubagentSessions", "sessionFailure"],
            }))
        );
        assert_eq!(capabilities.pointer("/session/notices"), Some(&json!({})));
    }

    #[test]
    fn steering_is_read_from_the_initialize_meta() {
        assert!(steering_supported(
            &json!({ "steering": { "supported": true } })
        ));
        assert!(!steering_supported(
            &json!({ "steering": { "supported": false } })
        ));
        assert!(!steering_supported(&json!({ "goal": { "version": 1 } })));
        assert!(!steering_supported(&Value::Null));
    }

    #[test]
    fn a_steer_asks_for_the_message_back_when_no_turn_is_running() {
        let steer = Steer::new("session-1".into(), vec!["hello".into()]);
        let payload = serde_json::to_value(&steer).unwrap();
        assert_eq!(payload["sessionId"], json!("session-1"));
        assert_eq!(
            payload.pointer("/_meta/steering/idleBehavior"),
            Some(&json!("promptRequired"))
        );
    }

    #[test]
    fn a_connection_failure_is_the_networks_and_reads_plainly() {
        let lost = failure(&air(json!({
            "id": "t:error", "revision": 1, "category": "connection", "severity": "error",
            "title": "stream disconnected before completion", "actions": ["retry", "new_session"],
        })))
        .unwrap();
        assert_eq!(lost.kind, FailureKind::Network);
        assert_eq!(lost.title, LOST_CONNECTION);
        assert_eq!(
            lost.details.as_deref(),
            Some("stream disconnected before completion")
        );
        let payload = lost.payload(None);
        assert_eq!(payload["message"], LOST_CONNECTION);
        assert_eq!(payload["failure"]["kind"], "network");
    }

    #[test]
    fn a_retry_is_a_warning_that_no_failure_is_read_from() {
        let update = retrying("turn-1:retry", "connection", 3, "Connection error");
        assert_eq!(update["sessionUpdate"], "session_info_update");
        let notice = &update["_meta"]["jetbrains"]["air"]["sessionFailure"];
        assert_eq!(notice["severity"], "warning");
        assert_eq!(notice["attempt"], 3);
        assert!(failure(&update["_meta"]).is_none());
    }

    fn air(failure: Value) -> Value {
        json!({ "jetbrains": { "air": { "version": 1, "sessionFailure": failure } } })
    }

    #[test]
    fn a_usage_limit_is_the_accounts_and_a_context_limit_the_sessions() {
        let quota = air(json!({
            "id": "t:error", "revision": 1, "category": "limit", "severity": "error",
            "title": "You've hit your limit · resets 3pm", "actions": [],
        }));
        let failure = failure(&quota).unwrap();
        assert_eq!(failure.kind, FailureKind::Limit);
        assert_eq!(failure.title, "You've hit your limit · resets 3pm");

        let rate = air(
            json!({ "category": "limit", "severity": "error", "title": "Slow down", "actions": ["retry"] }),
        );
        assert_eq!(super::failure(&rate).unwrap().kind, FailureKind::Limit);

        let context = air(
            json!({ "category": "limit", "severity": "error", "title": "Full", "actions": ["new_session"] }),
        );
        assert_eq!(super::failure(&context).unwrap().kind, FailureKind::Other);
    }

    #[test]
    fn a_sign_in_failure_offers_a_login() {
        let signed_out = air(
            json!({ "category": "access", "severity": "error", "title": "Sign in", "actions": ["login"] }),
        );
        assert_eq!(failure(&signed_out).unwrap().kind, FailureKind::SignIn);
        let denied = air(
            json!({ "category": "access", "severity": "error", "title": "Verify", "actions": ["retry"] }),
        );
        assert_eq!(failure(&denied).unwrap().kind, FailureKind::Other);
    }

    #[test]
    fn a_retry_warning_is_no_failure() {
        let warning = air(
            json!({ "category": "limit", "severity": "warning", "title": "Retrying", "actions": [] }),
        );
        assert_eq!(failure(&warning), None);
        assert_eq!(failure(&json!({ "quota": {} })), None);
    }
}
