//! Running the person's own kubectl, so every way their clusters sign in (cloud
//! plugins, tokens, certificates) keeps working, and turning its errors into words.

use std::time::Duration;

use serde::de::DeserializeOwned;
use sikemux_process::{ProcessCancellation, ProcessRunError};

use crate::error::{KubeError, KubeResult};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the cluster itself gets to answer, inside the command's own limit.
const REQUEST_TIMEOUT: &str = "--request-timeout=30s";
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

/// Which cluster and namespace a call is for; none leaves kubectl's own current ones.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct Scope {
    pub context: Option<String>,
    pub namespace: Option<String>,
}

pub fn kubectl_bin() -> String {
    sikemux_process::user_environment::var("KUBECTL").unwrap_or_else(|| "kubectl".to_string())
}

/// kubectl reads anything starting with a dash as an option, so a name like
/// `--kubeconfig=...` would change what the call does instead of naming something.
pub fn plain(what: &str, value: &str) -> KubeResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KubeError::BadArg(format!("no {what} was named")));
    }
    if value.starts_with('-') || value.contains(char::is_whitespace) {
        return Err(KubeError::BadArg(format!("`{value}` is not a {what}")));
    }
    Ok(value.to_string())
}

/// A Kubernetes object's name: lowercase letters, digits, dots and dashes.
pub fn object_name(what: &str, value: &str) -> KubeResult<String> {
    let value = plain(what, value)?;
    let fits = value.len() <= 253
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'));
    if !fits {
        return Err(KubeError::BadArg(format!("`{value}` is not a {what}")));
    }
    Ok(value)
}

/// The scope as kubectl's own flags.
pub fn scope_args(scope: &Scope) -> KubeResult<Vec<String>> {
    let mut args = Vec::new();
    if let Some(context) = scope
        .context
        .as_deref()
        .filter(|context| !context.trim().is_empty())
    {
        args.push(format!("--context={}", plain("context", context)?));
    }
    if let Some(namespace) = scope
        .namespace
        .as_deref()
        .filter(|namespace| !namespace.trim().is_empty())
    {
        args.push(format!(
            "--namespace={}",
            object_name("namespace", namespace)?
        ));
    }
    Ok(args)
}

/// kubectl's stderr in the person's words.
pub fn classify(stderr: &str) -> KubeError {
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("kubectl failed")
        .trim_start_matches("error: ")
        .trim_start_matches("Error from server ")
        .to_string();
    let lower = stderr.to_lowercase();
    if lower.contains("current-context is not set")
        || lower.contains("no configuration has been provided")
        || lower.contains("context was not found")
        || (lower.contains("context") && lower.contains("does not exist"))
    {
        return KubeError::NoConfig(first);
    }
    if lower.contains("unable to connect to the server")
        || lower.contains("connection refused")
        || lower.contains("was refused")
        || lower.contains("no such host")
        || lower.contains("i/o timeout")
        || lower.contains("context deadline exceeded")
        || lower.contains("timeout awaiting response")
    {
        return KubeError::Unreachable(format!(
            "the cluster didn't answer. Check it's running and reachable ({first})"
        ));
    }
    if lower.contains("unauthorized")
        || lower.contains("you must be logged in")
        || lower.contains("token has expired")
        || lower.contains("expired")
        || lower.contains("getting credentials")
        || lower.contains("auth-plugin")
    {
        return KubeError::Auth(format!(
            "the cluster turned the sign-in away. Sign in again with your cloud's CLI ({first})"
        ));
    }
    if lower.contains("(forbidden)") || lower.contains("is forbidden") {
        return KubeError::Forbidden(first);
    }
    if lower.contains("(notfound)") || lower.contains("not found") {
        return KubeError::NotFound(first);
    }
    KubeError::Kubectl(first)
}

/// The host stops a call by dropping its future, which a blocking run doesn't notice on its own.
struct CancelOnDrop(ProcessCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn run_blocking(args: &[String], cancellation: &ProcessCancellation) -> KubeResult<String> {
    let mut command = sikemux_process::user_environment::command(kubectl_bin());
    command.args(args).env("KUBECTL_EXTERNAL_DIFF", "");
    let output = sikemux_process::run(
        &mut command,
        None,
        COMMAND_TIMEOUT,
        MAX_OUTPUT_BYTES,
        Some(cancellation),
    )
    .map_err(|error| match &error {
        ProcessRunError::Spawn(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            KubeError::CliMissing
        }
        _ => KubeError::Kubectl(error.to_string()),
    })?;
    if !output.status.success() {
        return Err(classify(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs kubectl with the scope's flags in front of `args`, answering with what it printed.
pub async fn run(scope: &Scope, args: &[&str]) -> KubeResult<String> {
    let mut all = scope_args(scope)?;
    all.push(REQUEST_TIMEOUT.to_string());
    all.extend(args.iter().map(|arg| (*arg).to_string()));
    let cancellation = ProcessCancellation::new();
    let _cancel_on_drop = CancelOnDrop(cancellation.clone());
    tokio::task::spawn_blocking(move || run_blocking(&all, &cancellation))
        .await
        .map_err(|error| KubeError::Kubectl(error.to_string()))?
}

/// Runs kubectl with `-o json` and reads what it printed.
pub async fn json<T: DeserializeOwned>(scope: &Scope, args: &[&str]) -> KubeResult<T> {
    let mut with_output = args.to_vec();
    with_output.extend(["-o", "json"]);
    let text = run(scope, &with_output).await?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_would_read_as_flags_are_turned_away() {
        assert_eq!(plain("context", " kind-dev ").unwrap(), "kind-dev");
        assert!(plain("context", "--kubeconfig=/tmp/x").is_err());
        assert!(plain("context", "a b").is_err());
        assert_eq!(
            object_name("pod", "web-56fd9f5c8b-fgprx").unwrap(),
            "web-56fd9f5c8b-fgprx"
        );
        assert!(object_name("pod", "Web").is_err());
        assert!(object_name("pod", "web/../x").is_err());
        let scope = Scope {
            context: Some("arn:aws:eks:eu-west-1:123:cluster/prod".into()),
            namespace: Some("shop".into()),
        };
        assert_eq!(
            scope_args(&scope).unwrap(),
            [
                "--context=arn:aws:eks:eu-west-1:123:cluster/prod",
                "--namespace=shop"
            ]
        );
        assert!(scope_args(&Scope::default()).unwrap().is_empty());
    }

    #[test]
    fn kubectls_errors_read_in_words() {
        assert!(matches!(
            classify("The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?"),
            KubeError::Unreachable(_)
        ));
        assert!(matches!(
            classify("Unable to connect to the server: dial tcp: lookup api.prod: no such host"),
            KubeError::Unreachable(_)
        ));
        assert!(matches!(
            classify("error: You must be logged in to the server (Unauthorized)"),
            KubeError::Auth(_)
        ));
        assert!(matches!(
            classify("Error from server (Forbidden): pods is forbidden: User \"dev\" cannot list resource \"pods\""),
            KubeError::Forbidden(_)
        ));
        assert!(matches!(
            classify("Error from server (NotFound): pods \"web-1\" not found"),
            KubeError::NotFound(message) if message == "(NotFound): pods \"web-1\" not found"
        ));
        assert!(matches!(
            classify("error: context \"gone\" does not exist"),
            KubeError::NoConfig(_)
        ));
        assert!(
            matches!(classify("error: something else"), KubeError::Kubectl(message) if message == "something else")
        );
    }
}
