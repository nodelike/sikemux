use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::DeviceAccess;

/// How an outside program reaches the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// The MCP endpoint, signed in through the browser.
    Mcp,
    /// `sikemux chat`, run outside every agent.
    Cli,
}

/// A program the person let in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideClient {
    pub id: String,
    /// What the program called itself. Nothing vouches for it.
    pub name: String,
    pub via: Via,
    /// Where its sign-in returned to, for one that signed in through the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    pub access: DeviceAccess,
    pub approved_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub last_used: Option<u64>,
    #[serde(skip_serializing)]
    #[serde(default)]
    pub token_hash: String,
}

/// A program waiting for the person to let it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideWaiting {
    pub id: String,
    pub name: String,
    pub via: Via,
    pub redirect: Option<String>,
    /// Four digits the sign-in shows where it started, to match against the app's question.
    pub check: String,
    pub expires_at: u64,
}

/// What the app shows of outside access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideView {
    /// The MCP endpoint's address, once it listens.
    pub url: Option<String>,
    /// Why it does not listen.
    pub error: Option<String>,
    pub waiting: Vec<OutsideWaiting>,
    pub clients: Vec<OutsideClient>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideAnswer {
    pub id: String,
    pub allow: bool,
    pub access: DeviceAccess,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideChange {
    pub id: String,
    /// None removes the program.
    #[serde(default)]
    pub access: Option<DeviceAccess>,
}
