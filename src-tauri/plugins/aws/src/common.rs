// Shared CLI plumbing for every AWS module.
//
//   run_aws_cli         — spawn `aws ...` with PAGER/COLOR scrubbed
//   aws_json            — run + parse stdout as JSON
//   classify_cli_err    — map stderr text → typed AwsError
//   describe_in_chunks  — run N AWS calls in parallel, splitting `arns` into
//                         chunks (AWS describe-* commands cap at 10/100/etc)

use std::process::Command;
use std::time::Duration;

use futures::future::try_join_all;
use serde::de::DeserializeOwned;
use sikemux_process::{ProcessCancellation, ProcessRunError};
use tokio::task;

use crate::error::{AwsError, AwsResult};

const DESCRIBE_CHUNK_CONCURRENCY: usize = 4;
const AWS_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const AWS_SSO_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const AWS_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn aws_bin() -> String {
    std::env::var("AWS_CLI").unwrap_or_else(|_| "aws".to_string())
}

fn run_aws_cli(
    args: &[&str],
    profile: Option<&str>,
    cancellation: &ProcessCancellation,
) -> AwsResult<(bool, String, String)> {
    let bin = aws_bin();
    let mut cmd = Command::new(&bin);
    if let Some(p) = profile {
        cmd.env("AWS_PROFILE", p);
    }
    cmd.env("AWS_PAGER", "").env("NO_COLOR", "1");
    cmd.args(args);
    let timeout = if args.starts_with(&["sso", "login"]) {
        AWS_SSO_TIMEOUT
    } else {
        AWS_COMMAND_TIMEOUT
    };
    let out = sikemux_process::run(
        &mut cmd,
        None,
        timeout,
        AWS_MAX_OUTPUT_BYTES,
        Some(cancellation),
    )
    .map_err(|error| {
        if matches!(&error, ProcessRunError::Spawn(cause) if cause.kind() == std::io::ErrorKind::NotFound)
        {
            AwsError::CliMissing(bin.clone())
        } else {
            AwsError::Aws(error.to_string())
        }
    })?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// The host stops a call or stream by dropping its future, and a blocking
/// process run does not notice that on its own.
struct CancelOnDrop(ProcessCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) async fn run_aws_cli_async(
    args: &[&str],
    profile: Option<&str>,
) -> AwsResult<(bool, String, String)> {
    let cancellation = ProcessCancellation::new();
    let _cancel_on_drop = CancelOnDrop(cancellation.clone());
    let profile = profile.map(String::from);
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    task::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_aws_cli(&refs, profile.as_deref(), &cancellation)
    })
    .await
    .map_err(|e| AwsError::Aws(format!("join error: {e}")))?
}

/// Map AWS CLI stderr text into a typed AwsError.
///
/// Prefers structured JSON when the CLI ran `--output json` and surfaced
/// an error envelope (`{"Error": {"Code": "ExpiredToken", ...}}`). Falls
/// back to substring matching the human stderr when no structured form is
/// present — that's still the common case for client-side failures (e.g.
/// "Unable to locate credentials" emitted before any API call).
pub(crate) fn classify_cli_err(stderr: &str) -> AwsError {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stderr.trim()) {
        let code = v
            .get("Error")
            .and_then(|e| e.get("Code"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_ascii_lowercase());
        if let Some(c) = code {
            if c == "expiredtoken" || c == "expiredtokenexception" {
                return AwsError::TokenExpired;
            }
            if c == "credentialsnotfound" || c.contains("nocredential") {
                return AwsError::NoCredentials;
            }
        }
    }

    let s = stderr.to_lowercase();
    if s.contains("token has expired")
        || s.contains("sso session associated with this profile has expired")
        || s.contains("expiredtoken")
    {
        AwsError::TokenExpired
    } else if s.contains("could not be found") || s.contains("unable to locate credentials") {
        AwsError::NoCredentials
    } else {
        AwsError::Aws(stderr.trim().to_string())
    }
}

pub(crate) async fn aws_json<T: DeserializeOwned>(profile: &str, args: &[&str]) -> AwsResult<T> {
    let (ok, stdout, stderr) = run_aws_cli_async(args, Some(profile)).await?;
    if !ok {
        return Err(classify_cli_err(&stderr));
    }
    Ok(serde_json::from_str::<T>(&stdout)?)
}

/// Run several `aws ... describe-X --<flag> <arns>` calls in parallel,
/// chunking `arns` into groups of `chunk_size`. Each chunk's response is
/// parsed into `R`; the per-chunk results are returned in input order.
///
/// `base_args` is the call prefix before the arns flag (e.g.
/// `["ecs", "describe-services", "--cluster", "<c>"]`).
/// `arns_flag` is the flag the arns are appended after (e.g. `"--services"`).
/// `tail_args` lands at the very end (typically `["--output", "json"]`).
pub(crate) async fn describe_in_chunks<R: DeserializeOwned>(
    profile: String,
    base_args: Vec<String>,
    arns_flag: &'static str,
    arns: Vec<String>,
    chunk_size: usize,
    tail_args: Vec<String>,
) -> AwsResult<Vec<R>> {
    if arns.is_empty() {
        return Ok(Vec::new());
    }
    let chunk_size = chunk_size.max(1);
    let chunks: Vec<Vec<String>> = arns.chunks(chunk_size).map(|c| c.to_vec()).collect();

    let mut out = Vec::with_capacity(chunks.len());
    let mut pending = chunks.into_iter().peekable();
    while pending.peek().is_some() {
        let futs = pending
            .by_ref()
            .take(DESCRIBE_CHUNK_CONCURRENCY)
            .map(|chunk| {
                let mut args = base_args.clone();
                args.push(arns_flag.to_string());
                args.extend(chunk);
                args.extend(tail_args.iter().cloned());
                let profile = profile.clone();
                async move {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    aws_json::<R>(&profile, &refs).await
                }
            });
        out.extend(try_join_all(futs).await?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_substring_expired() {
        let e = classify_cli_err("An error occurred: token has expired blah");
        assert!(matches!(e, AwsError::TokenExpired));
    }

    #[test]
    fn classify_substring_no_credentials() {
        let e = classify_cli_err("Unable to locate credentials");
        assert!(matches!(e, AwsError::NoCredentials));
    }

    #[test]
    fn classify_structured_expired() {
        let body = r#"{"Error":{"Code":"ExpiredToken","Message":"x"}}"#;
        assert!(matches!(classify_cli_err(body), AwsError::TokenExpired));
    }

    #[test]
    fn classify_fallthrough() {
        match classify_cli_err("some weird error\n") {
            AwsError::Aws(msg) => assert_eq!(msg, "some weird error"),
            _ => panic!("expected Aws variant"),
        }
    }
}
