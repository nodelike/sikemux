mod config;
pub(crate) mod executable;
pub(crate) mod models;
pub(crate) mod sessions;
pub(crate) mod usage;
pub(crate) mod watch;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) use executable::resolve_agent_executable;
pub(crate) use sessions::context::session_transcript_path;
pub use watch::{note_streaming_session, watch_count};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSession {
    id: String,
    title: String,
    mtime: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(rename = "type")]
    kind: &'static str,
    label: &'static str,
    command: String,
    available: bool,
    error: Option<String>,
    warning: Option<String>,
    profile_id: Option<String>,
    config_path: Option<String>,
    #[serde(rename = "defaultModel")]
    default_model: Option<String>,
    #[serde(rename = "defaultEffort")]
    default_effort: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileRequest {
    #[serde(rename = "type")]
    kind: AgentKind,
    profile_id: Option<String>,
    executable_path: Option<String>,
    config_path: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct AgentModelInfo {
    id: String,
    label: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum AgentUsageResetAt {
    Unix(u64),
    Iso(String),
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageWindow {
    label: String,
    used_percent: f64,
    resets_at: Option<AgentUsageResetAt>,
    window_minutes: Option<u64>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    provider: &'static str,
    plan: Option<String>,
    windows: Vec<AgentUsageWindow>,
    unavailable_reason: Option<String>,
}

struct AgentDef {
    kind: &'static str,
    label: &'static str,
    command: &'static str,
}

const AGENT_DEFS: &[AgentDef] = &[
    AgentDef {
        kind: "claude",
        label: "Claude",
        command: "claude",
    },
    AgentDef {
        kind: "codex",
        label: "Codex",
        command: "codex",
    },
    AgentDef {
        kind: "hermes",
        label: "Hermes",
        command: "hermes",
    },
    AgentDef {
        kind: "pi",
        label: "Pi",
        command: "pi",
    },
    AgentDef {
        kind: "opencode",
        label: "OpenCode",
        command: "opencode",
    },
    AgentDef {
        kind: "omp",
        label: "OMP",
        command: "omp",
    },
    AgentDef {
        kind: "grok",
        label: "Grok",
        command: "grok",
    },
];

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Hermes,
    Pi,
    Opencode,
    Omp,
    Grok,
}

impl AgentKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Hermes => "hermes",
            AgentKind::Pi => "pi",
            AgentKind::Opencode => "opencode",
            AgentKind::Omp => "omp",
            AgentKind::Grok => "grok",
        }
    }
}

fn home_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn allowed_agent_path(_agent: &str, _path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::allowed_agent_path;
    use std::path::Path;

    #[test]
    fn opencode_installer_directory_is_a_valid_agent_location() {
        let home = Path::new("/home/tester");
        for name in ["opencode", "opencode.exe", "opencode.cmd"] {
            assert!(allowed_agent_path(
                "opencode",
                &home.join(".opencode").join("bin").join(name)
            ));
        }
    }
}
