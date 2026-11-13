use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An app as a client draws it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpApp {
    pub server: String,
    pub tool: Value,
    /// The page, with the policy that keeps it to the domains it named.
    pub html: String,
    pub prefers_border: bool,
    pub input: Value,
    pub result: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppParams {
    pub agent_id: String,
    pub id: String,
}

/// A request the page made of its server.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppRequestParams {
    pub agent_id: String,
    pub id: String,
    /// `tools/call` or `resources/read`.
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
