//! One tool call, one connection to the app's CLI broker.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_core::cli::client::{attempt, Failure};
pub use sikemux_core::cli::protocol::TOOLS_REPLY_TIMEOUT;
use sikemux_host::cli_client::current_project;
use sikemux_host::cli_paths::handed_endpoint;

/// How long a call waits for Sikemux to come back while it restarts, such as
/// after an update.
pub const RESTART_WAIT: Duration = Duration::from_secs(15);
const RETRY_GAP: Duration = Duration::from_millis(250);

pub fn call(agent_id: &str, method: &str, params: &Value) -> Result<Value, String> {
    call_within(agent_id, method, params, TOOLS_REPLY_TIMEOUT, RESTART_WAIT)
}

/// As [`call`], giving up on an answer after `timeout`, and on reaching
/// Sikemux after `patience`.
pub fn call_within(
    agent_id: &str,
    method: &str,
    params: &Value,
    timeout: Duration,
    patience: Duration,
) -> Result<Value, String> {
    relay(
        &endpoint()?,
        &current_project()?,
        agent_id,
        method,
        params,
        timeout,
        patience,
    )
}

/// Only the endpoint Sikemux handed over, so a tools server it did not start
/// never reaches whichever Sikemux happens to be running.
fn endpoint() -> Result<PathBuf, String> {
    handed_endpoint()
        .ok_or_else(|| "Missing SIKEMUX_CLI_ENDPOINT; launch this MCP from Sikemux".into())
}

/// What the endpoint file says now. It changes when another Sikemux, or a
/// newer one, takes over the endpoint.
pub fn instance() -> Option<Vec<u8>> {
    std::fs::read(endpoint().ok()?).ok()
}

/// Tries again, re-reading the endpoint file, while Sikemux cannot be reached
/// for up to `patience`. A request Sikemux may have received is never sent twice.
pub fn relay(
    endpoint: &Path,
    project: &str,
    agent_id: &str,
    method: &str,
    params: &Value,
    timeout: Duration,
    patience: Duration,
) -> Result<Value, String> {
    let give_up = Instant::now() + patience;
    loop {
        match attempt(endpoint, project, Some(agent_id), method, params, timeout) {
            Ok(value) => return Ok(value),
            Err(Failure::NotSent(_)) if Instant::now() + RETRY_GAP < give_up => {
                std::thread::sleep(RETRY_GAP);
            }
            Err(Failure::NotSent(message) | Failure::Sent(message)) => return Err(message),
        }
    }
}
