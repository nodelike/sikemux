// The Azure CLI, which holds the person's sign-in. Sikemux never stores an Azure
// credential: it asks `az` for an access token and calls the REST APIs with it, so
// every tenant, subscription and sign-in the CLI knows works here.

use std::time::Duration;

use serde_json::Value;
use sikemux_process::{ProcessCancellation, ProcessRunError};
use tokio::task;

use crate::error::{AzureError, AzureResult};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// `az` reads anything starting with a dash as a flag, so a subscription or group
/// named `--query=…` would change the call.
pub fn value(value: &str) -> AzureResult<&str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AzureError::BadArg("an empty name".into()));
    }
    if value.starts_with('-') {
        return Err(AzureError::BadArg("names cannot start with a dash".into()));
    }
    Ok(value)
}

fn binary() -> String {
    sikemux_process::user_environment::var("AZ").unwrap_or_else(|| "az".to_string())
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
) -> AzureResult<Output> {
    let mut command = sikemux_process::user_environment::command(binary());
    command
        .args(args)
        .env("AZURE_CORE_ONLY_SHOW_ERRORS", "true")
        .env("AZURE_CORE_COLLECT_TELEMETRY", "false")
        .env("AZURE_CORE_NO_COLOR", "true")
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
            AzureError::NoCli
        }
        _ => AzureError::Cli(error.to_string()),
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

async fn run_for(args: &[&str], timeout: Duration) -> AzureResult<Output> {
    let cancellation = ProcessCancellation::new();
    let _cancel_on_drop = CancelOnDrop(cancellation.clone());
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    task::spawn_blocking(move || run_blocking(&args, timeout, &cancellation))
        .await
        .map_err(|error| AzureError::Cli(format!("join error: {error}")))?
}

pub async fn run(args: &[&str]) -> AzureResult<Output> {
    run_for(args, COMMAND_TIMEOUT).await
}

/// Runs `az` and reads what it printed as JSON.
pub async fn json(args: &[&str]) -> AzureResult<Value> {
    let mut args = args.to_vec();
    args.push("--output=json");
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
pub async fn sign_in(args: &[&str]) -> AzureResult<()> {
    let out = run_for(args, SIGN_IN_TIMEOUT).await?;
    if out.ok {
        Ok(())
    } else {
        Err(classify(&out.stderr))
    }
}

/// The CLI's own words for a failure, as one of ours.
pub fn classify(stderr: &str) -> AzureError {
    let lower = stderr.to_lowercase();
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("WARNING"))
        .map(|line| line.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| stderr.trim().to_string());
    if lower.contains("please run 'az login'")
        || lower.contains("please run \"az login\"")
        || lower.contains("az login' to setup account")
        || lower.contains("no subscription found")
    {
        AzureError::Unconfigured
    } else if lower.contains("refresh token has expired")
        || lower.contains("aadsts")
        || lower.contains("re-authentication")
        || lower.contains("interaction_required")
    {
        AzureError::Auth(message)
    } else if lower.contains("does not have authorization")
        || lower.contains("authorizationfailed")
        || lower.contains("forbidden")
    {
        AzureError::Forbidden(message)
    } else {
        AzureError::Cli(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_pass_as_a_flag() {
        assert_eq!(value(" nine-mills ").ok(), Some("nine-mills"));
        assert!(value("--query=[].id").is_err());
        assert!(value("  ").is_err());
    }

    #[test]
    fn the_clis_words_are_read_as_our_failures() {
        assert!(matches!(
            classify("Please run 'az login' to setup account."),
            AzureError::Unconfigured
        ));
        assert!(matches!(
            classify("AADSTS700082: The refresh token has expired"),
            AzureError::Auth(_)
        ));
        assert!(matches!(
            classify(
                "(AuthorizationFailed) The client does not have authorization to perform action"
            ),
            AzureError::Forbidden(_)
        ));
        assert!(
            matches!(classify("WARNING: something\nERROR: the resource group was not found"), AzureError::Cli(message) if message == "the resource group was not found")
        );
    }
}
