//! What the `sikemux` CLI and the agents' tool server send to Sikemux's tool
//! endpoint: one JSON object per line over a loopback TCP connection.

use serde::{Deserialize, Serialize};

use super::methods::{BROWSER_METHODS, HARNESS_METHODS, SIM_METHODS};

pub use sikemux_wire::cli::{
    CliOpenRequest, CliOpenTarget, CliTargetKind, HarnessRequest, MAX_CLI_TARGETS,
};

pub const CLI_PROTOCOL_VERSION: u16 = 2;
pub const MAX_CLI_FRAME_BYTES: u64 = 64 * 1024;
/// Harness answers carry page text and screenshots, so they get more room
/// than a request frame.
pub const MAX_CLI_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliEndpointDescriptor {
    pub protocol: u16,
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub version: String,
}

/// The first frame on every connection; see `cli_auth`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum CliClientHello {
    Hello { protocol: u16, nonce: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum CliClientCommand {
    Harness {
        protocol: u16,
        token: String,
        request: HarnessRequest,
    },
    Ping {
        protocol: u16,
        token: String,
    },
    Open {
        protocol: u16,
        token: String,
        request: CliOpenRequest,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenFailure {
    pub target_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum CliServerResponse {
    Hello {
        proof: String,
    },
    Result {
        value: serde_json::Value,
    },
    Pong {
        protocol: u16,
        version: String,
        /// Whether the app's window is open. Without it, tools that act on
        /// the window fail and the CLI's `open` starts the app first.
        window: bool,
    },
    Accepted {
        request_id: String,
        opened: Vec<String>,
        failed: Vec<CliOpenFailure>,
    },
    Closed {
        request_id: String,
        reason: CliCloseReason,
    },
    Error {
        message: String,
    },
}

/// What the window opened of a CLI `open`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenOutcome {
    pub opened: Vec<String>,
    pub failed: Vec<CliOpenFailure>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CliCloseReason {
    TabsClosed,
    AppExit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliFrontendRequest {
    pub request: CliOpenRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenResult {
    pub request_id: String,
    pub target_id: String,
    pub pane_id: Option<String>,
    pub path: String,
    pub error: Option<String>,
}

pub const PLUGIN_METHODS: &[&str] = &["plugins.tools", "plugins.call"];
/// The tools server waits on this to hear that plugins may offer its agent
/// different tools, such as after the person signs in to one.
pub const PLUGINS_CHANGED_METHOD: &str = "plugins.changed";

pub fn is_browser_method(method: &str) -> bool {
    BROWSER_METHODS.contains(&method)
}

/// The tools server asks this as it starts: whether its agent gets the simulator tools.
pub const SIM_OFFERED_METHOD: &str = "sim.offered";
/// The tools server sends this when the agent cancels a simulator call it is waiting on.
pub const SIM_CANCEL_METHOD: &str = "sim.cancel";

pub fn is_sim_method(method: &str) -> bool {
    SIM_METHODS.contains(&method) || method == SIM_OFFERED_METHOD || method == SIM_CANCEL_METHOD
}

pub fn is_plugin_method(method: &str) -> bool {
    PLUGIN_METHODS.contains(&method)
}

pub fn validate_harness(request: &HarnessRequest) -> Result<(), String> {
    if request.id.is_empty() || request.id.len() > 128 {
        return Err("request ID must contain 1 to 128 bytes".into());
    }
    if request.project.len() > 4096 || !std::path::Path::new(&request.project).is_absolute() {
        return Err("project must be an absolute path".into());
    }
    if request
        .agent_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err("invalid agent ID".into());
    }
    if !HARNESS_METHODS.contains(&request.method.as_str())
        && !is_browser_method(&request.method)
        && !is_sim_method(&request.method)
        && !is_plugin_method(&request.method)
        && request.method != PLUGINS_CHANGED_METHOD
    {
        return Err("unknown harness method".into());
    }
    if !request.params.is_object() {
        return Err("params must be an object".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str) -> HarnessRequest {
        HarnessRequest {
            id: "one".into(),
            project: "/tmp".into(),
            agent_id: None,
            method: method.into(),
            params: serde_json::json!({}),
        }
    }

    #[test]
    fn open_requests_must_be_absolute_and_bounded() {
        let request = CliOpenRequest {
            id: "request".into(),
            cwd: "/repo".into(),
            wait: true,
            targets: vec![CliOpenTarget {
                id: "target".into(),
                kind: CliTargetKind::File,
                path: "/repo/file.rs".into(),
                project_root: "/repo".into(),
                line: Some(0),
                column: Some(0),
            }],
        };
        assert!(request.validate().is_ok());
        let mut relative = request.clone();
        relative.targets[0].path = "file.rs".into();
        assert!(relative.validate().is_err());
        let mut directory = request.clone();
        directory.targets[0].kind = CliTargetKind::Directory;
        assert!(directory.validate().is_err());
    }

    #[test]
    fn only_declared_methods_with_object_params_are_valid() {
        assert!(validate_harness(&request("workspace.inspect")).is_ok());
        assert!(validate_harness(&request("browser.click")).is_ok());
        assert!(validate_harness(&request("plugins.call")).is_ok());
        assert!(validate_harness(&request("pty_kill")).is_err());
        let mut relative = request("task.read");
        relative.project = "project".into();
        assert!(validate_harness(&relative).is_err());
        let mut list = request("task.read");
        list.params = serde_json::json!([]);
        assert!(validate_harness(&list).is_err());
    }
}
