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

#[cfg(test)]
mod tests {
    use super::*;

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
}
