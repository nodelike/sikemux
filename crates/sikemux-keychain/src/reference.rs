//! Secrets kept in 1Password and named by an `op://` reference, such as
//! `op://Work/GitHub/token`. Sikemux stores only the reference and reads the secret
//! through 1Password's `op` CLI each time it is used. `op` runs in the person's own
//! environment, so a service account or a Connect server set there is used as `op` would.

use std::process::Stdio;
use std::time::Duration;

const PREFIX: &str = "op://";
/// Long enough for the person to reach the fingerprint reader when 1Password asks.
const READ_TIMEOUT: Duration = Duration::from_secs(45);
const MOST_BYTES: usize = 1 << 20;

pub fn is_reference(value: &str) -> bool {
    value.trim().starts_with(PREFIX)
}

/// `value` itself, or the secret it names when it is a reference. The error names the
/// reference and why it could not be read, never a secret.
pub fn resolve(value: &str) -> Result<String, String> {
    if !is_reference(value) {
        return Ok(value.to_owned());
    }
    let reference = value.trim();
    let mut command = sikemux_process::user_environment::command("op");
    command
        .args(["--cache=false", "read", "--no-newline", reference])
        .stdin(Stdio::null());
    let output = sikemux_process::run(&mut command, None, READ_TIMEOUT, MOST_BYTES, None).map_err(
        |error| match error {
            sikemux_process::ProcessRunError::Spawn(_) => {
                format!("1Password's CLI, op, is not installed, so {reference} could not be read")
            }
            error => format!("1Password could not read {reference}: {error}"),
        },
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let why = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it is locked or signed out");
        return Err(format!("1Password could not read {reference}: {why}"));
    }
    match String::from_utf8(output.stdout) {
        Ok(secret) if !secret.trim().is_empty() => Ok(secret),
        _ => Err(format!("1Password has nothing at {reference}")),
    }
}
