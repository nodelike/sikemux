//! A custom command the person runs, on the host rather than on whichever
//! device asked for it: in the background, in a terminal tab of its own, beside
//! or in place of the view in focus, or in a terminal floating over the window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::connection::blocking;
use super::layout::At;
use super::{Core, CoreError, CoreResult};
use crate::protocol::{CustomCommand, CustomCommandPlacement, SessionId};
use crate::tasks::config::{self, ActionPlacement, ConfigLoad};

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
        let mut process = sikemux_process::user_environment::agent_command(shell);
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

#[derive(Deserialize)]
pub(crate) struct RunCustomParams {
    /// The strip in front it runs for: a project, the terminals of no project or an SSH host's.
    #[serde(flatten)]
    at: At,
    /// The command, by its id in the person's preferences.
    #[serde(default)]
    id: Option<String>,
    /// Or the project's `sikemux.json` action, by its id, which runs once the person trusts
    /// the file.
    #[serde(default)]
    action: Option<String>,
    /// The tab in front and its view in focus, which a split or replacing command goes
    /// beside or in place of.
    tab: Option<String>,
    pane: Option<String>,
}

/// What running a custom command did: the tab it opened or changed, the terminal to float
/// over the window, or what it printed in the background.
#[derive(Serialize, Default)]
pub(crate) struct Ran {
    #[serde(skip_serializing_if = "Option::is_none")]
    tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    popup: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<CommandResult>,
}

/// The project's `sikemux.json` action `id` as a command to run, once the person trusts the
/// file, asked as a task's start asks.
async fn project_action(core: &Arc<Core>, project: &Path, id: &str) -> CoreResult<CustomCommand> {
    let config = {
        let project = project.to_path_buf();
        blocking(move || Ok(config::load(&project))).await?
    };
    let ConfigLoad::Valid { config, .. } = config else {
        return Err("sikemux.json is not valid".into());
    };
    let action = config
        .actions
        .into_iter()
        .find(|action| action.id == id)
        .ok_or("sikemux.json has no such action")?;
    if !super::tasks::config_trusted(core, project, None).await {
        return Err("sikemux.json was not trusted".into());
    }
    Ok(CustomCommand {
        id: format!("project.{}", action.id),
        title: action.label,
        detail: action.description,
        command: action.command,
        contexts: Vec::new(),
        placement: match action.placement {
            ActionPlacement::Background => CustomCommandPlacement::Background,
            ActionPlacement::Terminal => CustomCommandPlacement::Terminal,
            ActionPlacement::Split => CustomCommandPlacement::Split,
            ActionPlacement::Popup => CustomCommandPlacement::Popup,
            ActionPlacement::Replace => CustomCommandPlacement::Replace,
        },
    })
}

/// Where a command runs for the strip it was run in, and what it is told of that strip: its
/// folder, and the session id, name and kind and project folder its environment carries.
struct Running {
    cwd: Option<PathBuf>,
    session: (String, String, &'static str),
    project: Option<PathBuf>,
}

fn running(core: &Core, at: &At) -> CoreResult<Running> {
    let home = || sikemux_process::user_environment::var_os("HOME").map(PathBuf::from);
    match (&at.host, at.terminals) {
        (_, true) => Ok(Running {
            cwd: home(),
            session: ("terminals".into(), "Terminals".into(), "command"),
            project: None,
        }),
        (Some(host), false) => Ok(Running {
            cwd: home(),
            session: (format!("ssh:{host}"), host.clone(), "ssh"),
            project: None,
        }),
        (None, false) => {
            let project = core
                .workspaces
                .project_info(&at.project)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            Ok(Running {
                cwd: Some(project.path.clone()),
                session: (project.id, project.name, "project"),
                project: Some(project.path),
            })
        }
    }
}

/// Runs one of the person's custom commands for the strip in front, as its placement says: a
/// project's in its folder, the others in the home folder of this host. A split or replacing
/// command with no view to go beside opens a tab.
pub(crate) async fn run_custom(core: &Arc<Core>, params: RunCustomParams) -> CoreResult<Ran> {
    let running = running(core, &params.at)?;
    let command = match (&params.action, &params.id) {
        (Some(action), _) => {
            let project = running
                .project
                .as_deref()
                .ok_or("a sikemux.json action runs in its project")?;
            project_action(core, project, action).await?
        }
        (None, Some(id)) => core
            .preferences
            .get()
            .custom_commands
            .into_iter()
            .find(|command| command.id == *id)
            .ok_or_else(|| CoreError::from("that custom command is no longer in Settings"))?,
        (None, None) => return Err("name a custom command or a sikemux.json action".into()),
    };
    let at = params.at;
    let beside = params.tab.zip(params.pane);
    match (command.placement, beside) {
        (CustomCommandPlacement::Background, _) => {}
        (CustomCommandPlacement::Popup, _) => {
            let id = super::spare::strip_command_shell(core, &at, command.command).await?;
            return Ok(Ran {
                popup: Some(id),
                ..Ran::default()
            });
        }
        (
            placement @ (CustomCommandPlacement::Split | CustomCommandPlacement::Replace),
            Some((tab, pane)),
        ) => {
            let id = super::spare::strip_command_shell(core, &at, command.command).await?;
            let replace = placement == CustomCommandPlacement::Replace;
            let tab = super::layout::place_terminal(core, at, tab, &pane, id, replace)?;
            return Ok(Ran {
                tab: Some(tab),
                ..Ran::default()
            });
        }
        _ => {
            let id = super::spare::strip_command_shell(core, &at, command.command).await?;
            let tab = super::layout::show_terminal(core, at, id, Some(command.title))?;
            return Ok(Ran {
                tab: Some(tab),
                ..Ran::default()
            });
        }
    }
    let (id, name, kind) = running.session;
    let folder = running
        .project
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let env = HashMap::from([
        ("SIKEMUX_SESSION_ID".to_owned(), id),
        ("SIKEMUX_SESSION_NAME".to_owned(), name),
        ("SIKEMUX_SESSION_KIND".to_owned(), kind.to_owned()),
        ("SIKEMUX_PROJECT".to_owned(), folder),
    ]);
    let result = run(CommandParams {
        command: command.command,
        cwd: running.cwd,
        env,
    })
    .await?;
    Ok(Ran {
        result: Some(result),
        ..Ran::default()
    })
}
