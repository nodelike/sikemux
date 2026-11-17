//! An MCP app as a driver hands it to the chat.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What an app is shown from: its server's page, the call it came from and that call's result.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// The MCP server, by the name the agent knows it by.
    pub server: String,
    /// The tool as the server described it, the name under `name`.
    pub tool: Value,
    pub uri: String,
    pub html: String,
    /// The domains the page asked to reach, as `_meta.ui.csp` names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<Value>,
    #[serde(default)]
    pub prefers_border: bool,
    pub input: Value,
    /// The call's result, as MCP's `CallToolResult`.
    pub result: Value,
}

/// A request an app made of its server, for the agent that called it to pass on.
pub struct ServerRequest {
    pub server: String,
    pub method: String,
    pub params: Value,
}
