pub mod agent_detection;
pub mod error;
pub mod launch;
pub mod output_log;
pub mod process;
pub mod screen;
pub mod shell;
pub mod shell_protocol;
pub mod task;
pub mod user_shell;

use error::{PtyError, PtyResult};

pub const MAX_PTY_DIMENSION: u16 = 1_000;

pub fn validate_pty_dimensions(cols: u16, rows: u16) -> PtyResult<()> {
    if cols == 0 || cols > MAX_PTY_DIMENSION || rows == 0 || rows > MAX_PTY_DIMENSION {
        return Err(PtyError::BadArg("invalid pty terminal dimensions"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::launch::PtyContext;
    use portable_pty::CommandBuilder;

    pub(crate) fn env(command: &CommandBuilder, key: &str) -> Option<String> {
        command
            .get_env(key)
            .map(|value| value.to_string_lossy().into_owned())
    }

    pub(crate) fn local_shell_context() -> PtyContext {
        PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: Some("window-1".into()),
            pane_id: Some("pane-1".into()),
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: true,
        }
    }
}
