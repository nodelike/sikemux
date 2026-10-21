// The gcloud CLI, which holds the person's sign-in. Sikemux never stores a
// Google credential: it asks gcloud for an access token and calls the REST
// APIs with it, so every account, configuration and key gcloud knows works here.

use std::time::Duration;

use serde_json::Value;
use sikemux_process::{ProcessCancellation, ProcessRunError};
use tokio::task;

use crate::error::{GcpError, GcpResult};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// gcloud reads anything starting with a dash as a flag, so an account or
/// project name like `--impersonate-service-account=…` would change the call.
pub fn value(value: &str) -> GcpResult<&str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GcpError::BadArg("an empty name".into()));
    }
    if value.starts_with('-') {
        return Err(GcpError::BadArg("names cannot start with a dash".into()));
    }
    Ok(value)
}

fn binary() -> String {
    sikemux_process::user_environment::var("GCLOUD").unwrap_or_else(|| "gcloud".to_string())
}

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn run_blocking(
    args: &[String],
    timeout: Duration,
    cancellation: &ProcessCancellation,
) -> GcpResult<Output> {
    let mut command = sikemux_process::user_environment::command(binary());
    command
        .args(args)
        .env("CLOUDSDK_CORE_DISABLE_PROMPTS", "1")
        .env("CLOUDSDK_CORE_DISABLE_USAGE_REPORTING", "1")
        .env("NO_COLOR", "1");
    let out = sikemux_process::run(
        &mut command,
        None,
        timeout,
        MAX_OUTPUT_BYTES,
        Some(cancellation),
    )
    .map_err(|error| match &error {
        ProcessRunError::Spawn(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            GcpError::NoGcloud
        }
        _ => GcpError::Gcloud(error.to_string()),
    })?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// The host stops a call by dropping its future, which a blocking process run does not notice.
struct CancelOnDrop(ProcessCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn run_for(args: &[&str], timeout: Duration) -> GcpResult<Output> {
    let cancellation = ProcessCancellation::new();
    let _cancel_on_drop = CancelOnDrop(cancellation.clone());
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    task::spawn_blocking(move || run_blocking(&args, timeout, &cancellation))
        .await
        .map_err(|error| GcpError::Gcloud(format!("join error: {error}")))?
}

pub async fn run(args: &[&str]) -> GcpResult<Output> {
    run_for(args, COMMAND_TIMEOUT).await
}

/// Runs gcloud and reads what it printed as JSON.
pub async fn json(args: &[&str]) -> GcpResult<Value> {
    let mut args = args.to_vec();
    args.push("--format=json");
    let out = run(&args).await?;
    if !out.ok {
        return Err(classify(&out.stderr));
    }
    if out.stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&out.stdout)?)
}

/// Runs a sign-in that opens the browser and waits for the person to finish it.
pub async fn sign_in(args: &[&str]) -> GcpResult<()> {
    let out = run_for(args, SIGN_IN_TIMEOUT).await?;
    if out.ok {
        Ok(())
    } else {
        Err(classify(&out.stderr))
    }
}

/// gcloud's own words for a failure, as one of ours.
pub fn classify(stderr: &str) -> GcpError {
    let lower = stderr.to_lowercase();
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("ERROR:"))
        .map(|line| line.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| stderr.trim().to_string());
    if lower.contains("you do not currently have an active account")
        || lower.contains("no credentialed accounts")
    {
        GcpError::Unconfigured
    } else if lower.contains("reauthentication")
        || lower.contains("there was a problem refreshing your current auth tokens")
        || lower.contains("invalid_grant")
        || lower.contains("please run:\n\n  $ gcloud auth login")
    {
        GcpError::Auth(message)
    } else {
        GcpError::Gcloud(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_pass_as_a_flag() {
        assert_eq!(value(" my-project ").ok(), Some("my-project"));
        assert!(value("--impersonate-service-account=x").is_err());
        assert!(value("").is_err());
    }

    #[test]
    fn gcloud_failures_read_in_words() {
        assert!(matches!(
            classify("ERROR: (gcloud.auth.print-access-token) You do not currently have an active account selected."),
            GcpError::Unconfigured
        ));
        assert!(matches!(
            classify("ERROR: (gcloud.auth.print-access-token) There was a problem refreshing your current auth tokens: Reauthentication failed."),
            GcpError::Auth(_)
        ));
        assert!(
            matches!(classify("WARNING: x\nERROR: (gcloud.config) Bad value"), GcpError::Gcloud(message) if message == "(gcloud.config) Bad value")
        );
    }
}
