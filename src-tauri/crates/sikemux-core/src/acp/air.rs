//! Background tasks and subagents, which core ACP has no updates for.
//!
//! The Claude adapter carries both as an extension it calls AIR, nested inside
//! `_meta` so peers that do not know it skip it. It stays silent until a client
//! names the parts it understands in `initialize`.

use agent_client_protocol::schema::v1::{ClientCapabilities, ContentBlock, Meta};
use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const EXTENSION_VERSION: u8 = 1;
const EXTENSION_CAPABILITIES: [&str; 2] = ["asyncTasks", "nativeSubagentSessions"];

/// Asks the agent for async task and subagent session updates.
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
    ClientCapabilities::new().meta(meta)
}

/// A `session/update` kept as raw JSON.
///
/// The extension adds update kinds the schema crate has never heard of, and a
/// typed notification rejects the whole message when it meets one. Parsing is
/// left to the frontend, which ignores what it does not recognise.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "session/update")]
#[serde(transparent)]
pub struct SessionUpdate(pub Value);

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
/// and so would never see finish.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteerResponse)]
#[serde(rename_all = "camelCase")]
pub struct Steer {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
    #[serde(rename = "_meta")]
    pub meta: Value,
}

impl Steer {
    pub fn new(session_id: String, prompt: Vec<ContentBlock>) -> Self {
        Self {
            session_id,
            prompt,
            meta: json!({ "steering": { "idleBehavior": "promptRequired" } }),
        }
    }
}

/// `injected` when the message reached the running turn, `promptRequired` when
/// there was no turn left to reach and the message is the client's again.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct SteerResponse {
    #[serde(default)]
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/async_task/stop", response = StopAsyncTaskResponse)]
#[serde(rename_all = "camelCase")]
pub struct StopAsyncTask {
    pub session_id: String,
    pub async_task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct StopAsyncTaskResponse {
    #[serde(default)]
    pub stopped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_name_the_extension_parts_the_frontend_renders() {
        let capabilities = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(
            capabilities.pointer("/_meta/jetbrains/air"),
            Some(
                &json!({ "version": 1, "capabilities": ["asyncTasks", "nativeSubagentSessions"] })
            )
        );
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
    fn session_updates_survive_kinds_the_schema_does_not_know() {
        let payload = json!({
            "sessionId": "session-1",
            "update": { "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1" },
        });
        let parsed: SessionUpdate = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(parsed.0, payload);
    }
}
