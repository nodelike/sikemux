pub(crate) mod context;
pub(crate) mod delete;
pub(crate) mod recent;
pub(crate) mod relocate;
pub(crate) mod rename;

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sikemux_core::sessions::{apply_login_environment, apply_process_config, AgentSession};
use tauri::async_runtime::spawn_blocking;
use tokio::process::Command;

use super::AgentKind;
use sikemux_process::paths::expand_home;

const LIVE_SESSION_TIMEOUT: Duration = Duration::from_secs(6);
const LIVE_SESSION_OUTPUT_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgentSession {
    pub session_id: String,
    pub status: String,
}

fn parse_live_sessions(text: &str) -> Vec<LiveAgentSession> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let session_id = row.get("sessionId")?.as_str()?.to_string();
            let status = row.get("status")?.as_str()?.to_string();
            Some(LiveAgentSession { session_id, status })
        })
        .collect()
}

/// What Claude Code says about the sessions it is running right now. A session
/// reads `idle` only when nothing of its own is left going: a turn in flight,
/// a prompt waiting on the user, or a background shell each report something
/// else, so `idle` is the one status that is safe to end.
#[tauri::command]
pub async fn live_agent_sessions(
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<Vec<LiveAgentSession>, String> {
    let executable = executable_path
        .as_deref()
        .map(expand_home)
        .or_else(|| sikemux_process::user_environment::find_executable("claude"))
        .ok_or_else(|| "claude is not available".to_string())?;
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    command
        .args(["agents", "--json"])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    apply_process_config(&mut command, "claude", config_path.as_deref());
    let output = tokio::time::timeout(LIVE_SESSION_TIMEOUT, command.output())
        .await
        .map_err(|_| "claude session lookup timed out".to_string())?
        .map_err(|_| "Could not start claude session lookup".to_string())?;
    if !output.status.success() {
        return Err("claude session lookup exited unsuccessfully".to_string());
    }
    if output.stdout.len() > LIVE_SESSION_OUTPUT_LIMIT {
        return Err("claude listed too many sessions".to_string());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "claude session list was not valid UTF-8".to_string())?;
    Ok(parse_live_sessions(&text))
}

/// Existing on-disk conversations for an agent. The scan reads directories and
/// transcripts, so it runs on a blocking thread rather than the IPC thread.
#[tauri::command]
pub async fn agent_sessions(
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Vec<AgentSession> {
    spawn_blocking(move || {
        sikemux_core::sessions::agent_sessions(agent, &cwd, config_path.as_deref())
    })
    .await
    .unwrap_or_default()
}
