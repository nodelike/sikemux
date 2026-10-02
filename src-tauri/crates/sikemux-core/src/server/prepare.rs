use std::collections::HashMap;
use std::path::PathBuf;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use sikemux_pty::launch::{
    apply_agent_profile, configure_interactive_command, configure_pty_environment,
    validate_direct_command, PtyContext,
};
use sikemux_pty::shell::{
    configure_shell_integration, configure_task_command, inherited_ssh_environment,
    shell_integration_requested, shell_wants_login_flag, startup_bootstrap, ShellLaunchIntegration,
};
use sikemux_pty::task::{validate_task_request, TaskSpawnRequest};
use sikemux_pty::user_shell::{configured_shell, login_shell_environment};
use sikemux_pty::validate_pty_dimensions;

use crate::protocol::{Continuation, LaunchIdentity, SessionKind, TaskSessionInfo, TerminalSpawn};

use super::CoreResult;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Owner {
    pub project: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub task_execution_id: Option<String>,
    pub task: Option<TaskSessionInfo>,
    /// The paired device that started the session, which the app leaves
    /// running although nothing in its layout names it.
    pub started_by: Option<String>,
}

impl Owner {
    fn from_context(context: Option<&PtyContext>) -> Self {
        let Some(context) = context else {
            return Self::default();
        };
        let present = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
        Self {
            project: present(&context.project),
            pane_id: present(&context.pane_id),
            agent_id: present(&context.agent_id),
            agent_type: present(&context.agent_type),
            task_execution_id: None,
            task: None,
            started_by: None,
        }
    }
}

pub(crate) struct PreparedLaunch {
    pub cols: u16,
    pub rows: u16,
    pub command: CommandBuilder,
    pub kind: SessionKind,
    pub owner: Owner,
    pub shell_integration: Option<ShellLaunchIntegration>,
    pub initial_prompt_submitted: bool,
    pub continues: Option<Continuation>,
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The command the app's `pty_spawn` builds, minus the parts only the app can
/// do: it applies the browser integration to `direct_command` and passes its
/// environment in `env`.
pub(crate) fn prepare_terminal(
    launch: &LaunchIdentity,
    spawn: TerminalSpawn,
) -> CoreResult<PreparedLaunch> {
    let TerminalSpawn {
        cols,
        rows,
        cwd,
        startup,
        direct_command,
        context,
        env,
        continues,
    } = spawn;
    validate_pty_dimensions(cols, rows)?;
    let startup = startup.filter(|value| !value.is_empty());
    if startup.is_some() && direct_command.is_some() {
        return Err(
            "invalid argument: PTY startup and direct command are mutually exclusive".into(),
        );
    }
    if let Some(command) = direct_command.as_ref() {
        validate_direct_command(command, context.as_ref())?;
    }
    let shell = configured_shell();
    let direct_profile = direct_command
        .as_ref()
        .and_then(|command| command.profile.clone());
    let mut cmd = CommandBuilder::new(&shell);
    if let Some(command) = direct_command.as_ref() {
        configure_interactive_command(&mut cmd, &shell, command);
    }
    configure_pty_environment(
        &mut cmd,
        context.as_ref(),
        &launch.version,
        launch.cli_executable.as_deref(),
        launch.cli_endpoint.as_deref(),
        &HashMap::new(),
    );
    apply_agent_profile(
        &mut cmd,
        context.as_ref(),
        direct_profile.as_ref(),
        &user_home(),
    );
    for (key, value) in env {
        cmd.env(key, value);
    }
    let shell_integration = if shell_integration_requested(
        context.as_ref(),
        startup.is_some(),
        inherited_ssh_environment(),
    ) {
        configure_shell_integration(&mut cmd, &shell).unwrap_or(None)
    } else {
        None
    };
    let cwd = cwd.unwrap_or_else(|| user_home().to_string_lossy().into_owned());
    cmd.cwd(cwd);
    let login_shell = shell_wants_login_flag(&shell);
    if login_shell && startup.is_none() && direct_command.is_none() {
        cmd.arg("-l");
    }
    if let Some(startup) = startup.as_deref() {
        cmd.env("SIKEMUX_SHELL", &shell);
        cmd.arg("-c");
        cmd.arg(startup_bootstrap(startup, login_shell));
    }
    Ok(PreparedLaunch {
        cols,
        rows,
        command: cmd,
        kind: SessionKind::Terminal,
        owner: Owner::from_context(context.as_ref()),
        shell_integration,
        initial_prompt_submitted: context
            .as_ref()
            .is_some_and(|context| context.initial_prompt_submitted),
        continues,
    })
}

/// The command the app's `task_spawn` builds. Blocks on the first call while
/// the login shell's environment is captured.
pub(crate) fn prepare_task(
    launch: &LaunchIdentity,
    request: TaskSpawnRequest,
) -> CoreResult<PreparedLaunch> {
    let paths = validate_task_request(&request)?;
    let TaskSpawnRequest {
        execution_id,
        terminal_key,
        task_id,
        label,
        project,
        source,
        command,
        cwd,
        env,
        cols,
        rows,
        agent_id,
    } = request;
    let task = TaskSessionInfo {
        execution_id: execution_id.clone(),
        terminal_key: terminal_key.clone(),
        task_id: task_id.clone(),
        label: label.clone(),
        project,
        source,
        command: command.clone(),
        cwd,
        agent_id,
    };

    let shell = configured_shell();
    let mut task_command = CommandBuilder::new(&shell);
    let context = PtyContext {
        session_id: execution_id.clone(),
        session_name: label,
        session_kind: "task".into(),
        project: Some(paths.project.to_string_lossy().into_owned()),
        window_id: None,
        pane_id: None,
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: false,
    };
    let owner = Owner {
        project: context.project.clone(),
        task_execution_id: Some(execution_id.clone()),
        task: Some(task),
        ..Owner::default()
    };
    configure_pty_environment(
        &mut task_command,
        Some(&context),
        &launch.version,
        launch.cli_executable.as_deref(),
        launch.cli_endpoint.as_deref(),
        login_shell_environment(),
    );
    task_command.env("SIKEMUX_TASK_EXECUTION_ID", execution_id);
    task_command.env("SIKEMUX_TASK_TERMINAL_KEY", terminal_key);
    task_command.env("SIKEMUX_TASK_ID", task_id);
    task_command.env("SIKEMUX_TASK_SOURCE", source.as_str());
    for (key, value) in env {
        task_command.env(key, value);
    }
    task_command.cwd(paths.cwd);
    configure_task_command(&mut task_command, &shell, &command)?;
    Ok(PreparedLaunch {
        cols,
        rows,
        command: task_command,
        kind: SessionKind::Task,
        owner,
        shell_integration: None,
        initial_prompt_submitted: false,
        continues: None,
    })
}
