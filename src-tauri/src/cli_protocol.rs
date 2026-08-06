use serde::{Deserialize, Serialize};

pub const CLI_PROTOCOL_VERSION: u16 = 1;
pub const MAX_CLI_FRAME_BYTES: u64 = 64 * 1024;
pub const MAX_CLI_TARGETS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliEndpointDescriptor {
    pub protocol: u16,
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenTarget {
    pub id: String,
    pub kind: CliTargetKind,
    pub path: String,
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CliTargetKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenRequest {
    pub id: String,
    pub cwd: String,
    pub wait: bool,
    pub targets: Vec<CliOpenTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum CliClientCommand {
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
    Pong {
        protocol: u16,
        version: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_open_requests() {
        let command = CliClientCommand::Open {
            protocol: CLI_PROTOCOL_VERSION,
            token: "secret".into(),
            request: CliOpenRequest {
                id: "request-1".into(),
                cwd: "/repo".into(),
                wait: true,
                targets: vec![CliOpenTarget {
                    id: "target-1".into(),
                    kind: CliTargetKind::File,
                    path: "/repo/src/main.rs".into(),
                    project_root: "/repo".into(),
                    line: Some(41),
                    column: Some(7),
                }],
            },
        };
        let encoded = serde_json::to_string(&command).unwrap();
        assert_eq!(
            serde_json::from_str::<CliClientCommand>(&encoded).unwrap(),
            command
        );
    }
}
