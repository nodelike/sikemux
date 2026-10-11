use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::executable::{apply_login_environment, apply_process_config};

const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(8);
const MODEL_CATALOG_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const MODEL_CATALOG_ERROR_DETAIL_LIMIT: usize = 240;
pub(super) const CLAUDE_MODEL_CATALOG_ARGS: &[&str] = &[
    "--print",
    "--output-format",
    "stream-json",
    "--verbose",
    "--system-prompt",
    "",
    "--tools",
    "",
    "--input-format",
    "stream-json",
];

/// Clears `removed_env` after the login-shell import, so a variable the
/// captured profile set is dropped too, not just an inherited one.
pub(super) async fn run_model_catalog_executable_without_env(
    agent: &str,
    executable: &Path,
    args: &[&str],
    input: Option<&str>,
    config_path: Option<&str>,
    removed_env: &[&str],
) -> Result<String, String> {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    for key in removed_env {
        command.env_remove(key);
    }
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_process_config(&mut command, agent, config_path);
    command.envs(crate::model_providers::environment(agent).await);
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start {agent} model lookup"))?;
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("Could not open {agent} model lookup input"))?;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|_| format!("Could not write {agent} model lookup input"))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| format!("Could not finish {agent} model lookup input"))?;
    }
    let output = tokio::time::timeout(MODEL_CATALOG_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{agent} model lookup timed out"))?
        .map_err(|_| format!("Could not read {agent} model lookup output"))?;
    if !output.status.success() {
        let detail = model_catalog_error_detail(&output.stderr);
        return Err(match detail {
            Some(detail) => format!("{agent} model lookup exited unsuccessfully: {detail}"),
            None => format!("{agent} model lookup exited unsuccessfully"),
        });
    }
    if output.stdout.len() > MODEL_CATALOG_OUTPUT_LIMIT {
        return Err(format!("{agent} model catalog was too large"));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{agent} model catalog was not valid UTF-8"))
}

pub(super) fn model_catalog_error_detail(stderr: &[u8]) -> Option<String> {
    let normalized = String::from_utf8_lossy(stderr)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return None;
    }
    let mut characters = normalized.chars();
    let detail = characters
        .by_ref()
        .take(MODEL_CATALOG_ERROR_DETAIL_LIMIT)
        .collect::<String>();
    Some(if characters.next().is_some() {
        format!("{detail}…")
    } else {
        detail
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::{run_model_catalog_executable_without_env, MODEL_CATALOG_ERROR_DETAIL_LIMIT};
    use std::path::Path;

    #[tokio::test]
    async fn model_catalog_subprocess_drops_removed_environment() {
        const READ_HOME: &str = "printf '%s' \"${HOME-unset}\"";
        let kept = run_model_catalog_executable_without_env(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", READ_HOME],
            None,
            None,
            &[],
        )
        .await
        .unwrap();
        let dropped = run_model_catalog_executable_without_env(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", READ_HOME],
            None,
            None,
            &["HOME"],
        )
        .await
        .unwrap();

        assert_ne!(kept, "unset");
        assert_eq!(dropped, "unset");
    }

    #[tokio::test]
    async fn model_catalog_subprocess_surfaces_bounded_stderr() {
        let long_detail = "x".repeat(MODEL_CATALOG_ERROR_DETAIL_LIMIT + 20);
        let script = format!("printf 'first\\nsecond {long_detail}' >&2; exit 7");
        let error = run_model_catalog_executable_without_env(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", &script],
            None,
            None,
            &[],
        )
        .await
        .unwrap_err();

        assert!(error.starts_with("test-agent model lookup exited unsuccessfully: first second "));
        assert!(error.ends_with('…'));
        assert!(error.chars().count() <= MODEL_CATALOG_ERROR_DETAIL_LIMIT + 52);
    }
}
