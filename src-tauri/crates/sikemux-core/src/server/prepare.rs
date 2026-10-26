use std::collections::HashMap;
use std::path::PathBuf;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use sikemux_process::vars;
use sikemux_pty::launch::{
    account_dir, configure_interactive_command, configure_pty_environment, validate_direct_command,
    PtyContext,
};
use sikemux_pty::shell::{
    configure_shell_integration, configure_task_command, inherited_ssh_environment,
    shell_integration_requested, shell_wants_login_flag, startup_arguments, ShellLaunchIntegration,
};
use sikemux_pty::task::{validate_task_request, TaskSpawnRequest};
use sikemux_pty::user_shell::{configured_shell, login_shell_environment};
use sikemux_pty::validate_pty_dimensions;

use crate::git_guard::{homes, shim};
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
    /// The folder an agent keeps its account and sessions in, when the
    /// launch named one rather than leaving it to the CLI's default.
    pub account_home: Option<PathBuf>,
    /// The SSH config host an ssh terminal connects to.
    pub ssh_host: Option<String>,
    /// What a terminal outside every project, agent and SSH host is called
    /// in the rail, numbered from 1.
    #[serde(default)]
    pub rail_number: Option<u32>,
    /// A shell started with no command, startup or name of its own.
    #[serde(skip)]
    pub plain: bool,
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
            account_home: None,
            ssh_host: (context.session_kind == "ssh" && !context.session_name.is_empty())
                .then(|| context.session_name.clone()),
            rail_number: None,
            plain: false,
        }
    }

    /// A plain shell outside every project, agent, task and SSH host, which
    /// the rail can only tell apart by number.
    pub(crate) fn wants_rail_number(&self, kind: SessionKind) -> bool {
        kind == SessionKind::Terminal
            && self.plain
            && self.project.is_none()
            && self.agent_id.is_none()
            && self.task.is_none()
            && self.task_execution_id.is_none()
            && self.ssh_host.is_none()
    }
}

pub(crate) struct PreparedLaunch {
    pub cols: u16,
    pub rows: u16,
    pub command: CommandBuilder,
    pub kind: SessionKind,
    pub owner: Owner,
    pub shell_integration: Option<ShellLaunchIntegration>,
    pub ssh: bool,
    pub initial_prompt_submitted: bool,
    pub continues: Option<Continuation>,
    /// It sets a host up, so only the person's own app types into it.
    pub setup: bool,
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
        mut direct_command,
        context,
        env,
        continues,
    } = spawn;
    validate_pty_dimensions(cols, rows)?;
    let startup = startup.filter(|value| !value.is_empty());
    let mut owner = Owner::from_context(context.as_ref());
    let startup = startup.or_else(|| {
        owner
            .ssh_host
            .as_deref()
            .filter(|_| direct_command.is_none())
            .map(super::ssh::startup)
    });
    if startup.is_some() && direct_command.is_some() {
        return Err(
            "invalid argument: PTY startup and direct command are mutually exclusive".into(),
        );
    }
    if let Some(command) = direct_command.as_ref() {
        validate_direct_command(command, context.as_ref())?;
    }
    let shell = configured_shell();
    let mut cmd = CommandBuilder::new(&shell);
    if let Some(command) = direct_command.as_mut() {
        let agent = context
            .as_ref()
            .and_then(|context| context.agent_type.as_deref());
        let guarded = shim::arguments(agent.unwrap_or_default()).iter();
        command
            .args
            .splice(0..0, guarded.map(|arg| (*arg).to_owned()));
        configure_interactive_command(&mut cmd, &shell, command, shim::folder());
    }
    configure_pty_environment(
        &mut cmd,
        context.as_ref(),
        &launch.version,
        launch.cli_executable.as_deref(),
        launch.cli_endpoint.as_deref(),
        &HashMap::new(),
    );
    owner.plain = startup.is_none()
        && direct_command.is_none()
        && context
            .as_ref()
            .is_none_or(|context| context.session_name.is_empty());
    let variable = owner
        .agent_type
        .as_deref()
        .and_then(crate::launch::account_variable);
    owner.account_home = variable
        .and_then(|variable| env.get(variable))
        .map(|home| account_dir(home));
    let guard = match direct_command {
        Some(_) => {
            let mut guard = shim::environment(env.get("PATH").map(String::as_str));
            guard.extend(homes::environment(
                owner.agent_type.as_deref().unwrap_or_default(),
                owner.agent_id.as_deref().unwrap_or_default(),
                true,
                |key| env.get(key).map(String::as_str),
            ));
            guard
        }
        None => Vec::new(),
    };
    for (key, value) in env.into_iter().chain(guard) {
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
    let cwd = cwd.unwrap_or_else(|| {
        sikemux_process::paths::home()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    cmd.cwd(cwd);
    let login_shell = shell_wants_login_flag(&shell);
    if login_shell && startup.is_none() && direct_command.is_none() {
        cmd.arg("-l");
    }
    if let Some(startup) = startup.as_deref() {
        cmd.env(vars::SHELL, &shell);
        cmd.args(startup_arguments(&shell, startup, login_shell));
    }
    Ok(PreparedLaunch {
        cols,
        rows,
        command: cmd,
        kind: SessionKind::Terminal,
        owner,
        shell_integration,
        ssh: context
            .as_ref()
            .is_some_and(|context| context.session_kind == "ssh"),
        initial_prompt_submitted: context
            .as_ref()
            .is_some_and(|context| context.initial_prompt_submitted),
        continues,
        setup: context
            .as_ref()
            .is_some_and(|context| context.session_kind == "setup"),
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
    task_command.env(vars::TASK_EXECUTION_ID, execution_id);
    task_command.env(vars::TASK_TERMINAL_KEY, terminal_key);
    task_command.env(vars::TASK_ID, task_id);
    task_command.env(vars::TASK_SOURCE, source.as_str());
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
        ssh: false,
        initial_prompt_submitted: false,
        continues: None,
        setup: false,
    })
}
