//! A custom command the person runs in the background, on the host rather
//! than on whichever device asked for it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::connection::blocking;
use super::{CoreError, CoreResult};

const TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_LIMIT: usize = 16_000;
const MAX_COMMAND_CHARS: usize = 8_000;

#[derive(Deserialize)]
pub(crate) struct CommandParams {
    pub command: String,
    pub cwd: Option<PathBuf>,
    /// Only `SIKEMUX_` variables are passed on.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Serialize)]
pub(crate) struct CommandResult {
    pub code: i32,
    pub output: String,
}

/// Runs `command` in the person's login shell, answering its exit code and
/// what it printed, cut short after two minutes or 16 KB.
pub(crate) async fn run(params: CommandParams) -> CoreResult<CommandResult> {
    if params.command.trim().is_empty() || params.command.len() > MAX_COMMAND_CHARS {
        return Err("a background command must be 1 to 8000 characters".into());
    }
    blocking(move || {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut process = sikemux_process::user_environment::command(shell);
        process.args(["-lc", &params.command]);
        if let Some(cwd) = params.cwd.filter(|cwd| !cwd.as_os_str().is_empty()) {
            process.current_dir(cwd);
        }
        let sikemux_variable = |key: &String| {
            key.starts_with("SIKEMUX_")
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        };
        for (key, value) in params.env.iter().filter(|(key, _)| sikemux_variable(key)) {
            process.env(key, value);
        }
        let output = sikemux_process::run(&mut process, None, TIMEOUT, OUTPUT_LIMIT, None)
            .map_err(|error| CoreError::from(format!("the background command failed: {error}")))?;
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        Ok(CommandResult {
            code: output.status.code().unwrap_or(-1),
            output: String::from_utf8_lossy(&bytes).into_owned(),
        })
    })
    .await
}
