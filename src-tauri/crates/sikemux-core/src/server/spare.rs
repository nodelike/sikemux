//! New terminal tabs, and one shell started ahead of time for the project in
//! front, at the size terminals were last given, so a new terminal there
//! opens on a prompt instead of waiting on the person's rc files. At most one
//! runs, and one nobody takes is stopped after a while.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::protocol::{SessionId, SpawnTarget, TerminalSpawn};
use sikemux_wire::pty::launch::PtyContext;

use super::layout::At;
use super::wake::Wake;
use super::{session, Core, CoreError, CoreResult};

/// How long the front project and the terminal size stay put before a shell
/// starts, so a burst of changes starts one.
const SETTLE_MS: u64 = 1_000;
/// A shell nobody takes in this long is stopped until something changes.
const IDLE_LIMIT_MS: u64 = 10 * 60 * 1_000;

#[derive(Default)]
pub(crate) struct Spares {
    pub(crate) wake: Wake,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    size: Option<(u16, u16)>,
    spare: Option<Spare>,
    /// The pane id of a shell being started, hidden like a ready one.
    starting: Option<String>,
    changed_at: u64,
}

struct Spare {
    project: String,
    size: (u16, u16),
    id: SessionId,
    window_id: String,
    pane_id: String,
    started_at: u64,
}

/// A shell handed to a new terminal, with the ids it was started under for
/// the terminal to take as its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Taken {
    id: SessionId,
    window_id: String,
    pane_id: String,
}

#[derive(Deserialize)]
pub(crate) struct TakeParams {
    project: String,
}

impl Spares {
    /// Whether a session's pane is a shell started ahead, which no list shows.
    pub(crate) fn hides(&self, pane_id: Option<&str>) -> bool {
        let held = self.held.locked();
        pane_id.is_some_and(|pane| {
            held.starting.as_deref() == Some(pane)
                || held
                    .spare
                    .as_ref()
                    .is_some_and(|spare| spare.pane_id == pane)
        })
    }

    pub(crate) fn is_spare(&self, id: SessionId) -> bool {
        self.held
            .locked()
            .spare
            .as_ref()
            .is_some_and(|spare| spare.id == id)
    }

    /// A terminal was given this size; the next shell starts at it.
    pub(crate) fn note_size(&self, cols: u16, rows: u16) {
        let mut held = self.held.locked();
        if held.size != Some((cols, rows)) {
            held.size = Some((cols, rows));
            self.changed(&mut held);
        }
    }

    /// The project in front may have changed.
    pub(crate) fn front_changed(&self) {
        self.changed(&mut self.held.locked());
    }

    fn changed(&self, held: &mut Held) {
        held.changed_at = epoch_ms();
        self.wake.at(held.changed_at + SETTLE_MS);
    }
}

/// The shell started for `project`, for a new terminal there to take over.
pub(crate) fn take(core: &Core, params: TakeParams) -> Option<Taken> {
    let mut held = core.spares.held.locked();
    let fits = held.spare.as_ref()?.project == params.project
        && Some(held.spare.as_ref()?.size) == held.size;
    let spare = held.spare.take().filter(|_| fits)?;
    core.spares.changed(&mut held);
    Some(Taken {
        id: spare.id,
        window_id: spare.window_id,
        pane_id: spare.pane_id,
    })
}

/// Keeps one shell for the project in front at the last size, starting,
/// replacing or stopping it as they change and when it has gone untaken too long.
pub(crate) fn settle(core: &Arc<Core>) {
    let now = epoch_ms();
    let front = front_project(core);
    let mut held = core.spares.held.locked();
    if now < held.changed_at + SETTLE_MS {
        core.spares.wake.at(held.changed_at + SETTLE_MS);
        return;
    }
    let wanted = front.zip(held.size);
    if let Some(spare) = &held.spare {
        let fits = wanted.as_ref().is_some_and(|((project, _, _), size)| {
            *project == spare.project && *size == spare.size
        });
        let stale = now >= spare.started_at + IDLE_LIMIT_MS && spare.started_at >= held.changed_at;
        if fits && !stale {
            core.spares.wake.at(spare.started_at + IDLE_LIMIT_MS);
            return;
        }
        let id = spare.id;
        held.spare = None;
        stop(core, id);
        if stale {
            return;
        }
    }
    let (Some(((project, path, name), size)), None) = (wanted, &held.starting) else {
        return;
    };
    let pane_id = format!("pane-spare-{}", uuid::Uuid::new_v4().simple());
    held.starting = Some(pane_id.clone());
    drop(held);
    tokio::spawn(start(core.clone(), project, path, name, size, pane_id));
}

/// The project in front, by id, with its folder and name.
fn front_project(core: &Core) -> Option<(String, String, String)> {
    let id = super::layout::front_project(core)?;
    let (path, name) = project_folder(core, &id)?;
    Some((id, path, name))
}

/// A project's folder and name.
fn project_folder(core: &Core, id: &str) -> Option<(String, String)> {
    let project = core
        .workspace_view()
        .projects
        .into_iter()
        .find(|project| project.id == id)?;
    Some((project.path.to_string_lossy().into_owned(), project.name))
}

async fn start(
    core: Arc<Core>,
    project: String,
    path: String,
    name: String,
    size: (u16, u16),
    pane_id: String,
) {
    let window_id = format!("win-spare-{}", uuid::Uuid::new_v4().simple());
    let started = shell(
        &core,
        &project,
        path,
        name,
        size,
        (&window_id, &pane_id),
        None,
    )
    .await;
    let id = match started {
        Ok(id) => id,
        Err(error) => {
            eprintln!("sikemux core: a spare shell did not start: {error}");
            core.spares.held.locked().starting = None;
            return;
        }
    };
    let mut held = core.spares.held.locked();
    held.starting = None;
    held.spare = Some(Spare {
        project,
        size,
        id,
        window_id,
        pane_id,
        started_at: epoch_ms(),
    });
    drop(held);
    core.spares.wake.now();
}

/// Starts a shell in a project's folder, as a terminal of that project, and
/// types `startup` into it.
async fn shell(
    core: &Arc<Core>,
    project: &str,
    path: String,
    name: String,
    size: (u16, u16),
    (window_id, pane_id): (&str, &str),
    startup: Option<String>,
) -> CoreResult<SessionId> {
    let context = PtyContext {
        session_id: project.to_owned(),
        session_name: name,
        session_kind: "project".into(),
        project: Some(path.clone()),
        window_id: Some(window_id.to_owned()),
        pane_id: Some(pane_id.to_owned()),
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: true,
    };
    terminal(core, size, Some(path), context, startup).await
}

/// Starts a terminal at `size` in `cwd`, as `context` describes it, typing
/// `startup` into its shell.
async fn terminal(
    core: &Arc<Core>,
    size: (u16, u16),
    cwd: Option<String>,
    context: PtyContext,
    startup: Option<String>,
) -> CoreResult<SessionId> {
    let spawn = TerminalSpawn {
        cols: size.0,
        rows: size.1,
        cwd,
        startup,
        context: Some(context),
        ..TerminalSpawn::default()
    };
    let target = Box::new(SpawnTarget::Terminal(spawn));
    let pending =
        super::connection::spawn(core.clone(), core.launch_identity(), target, None).await?;
    let id = pending.id();
    pending.start(core);
    Ok(id)
}

/// The size a terminal opened by the core starts at: the last one a client gave.
pub(crate) fn opening_size(core: &Core) -> (u16, u16) {
    core.spares.held.locked().size.unwrap_or((80, 24))
}

#[derive(Deserialize)]
pub(crate) struct NewTerminal {
    #[serde(default)]
    project: String,
    /// Opens it among the terminals of no project, in the home folder.
    #[serde(default)]
    terminals: bool,
}

/// Opens a terminal tab in `project` on the shell started ahead when it fits,
/// or on a new one, or among the terminals of no project. Answers the tab's id.
pub(crate) async fn new_terminal(core: &Arc<Core>, params: NewTerminal) -> CoreResult<String> {
    if params.terminals {
        let id = home_shell(core).await?;
        return super::layout::show_terminal(core, At::terminals(), id, None);
    }
    let id = project_shell(core, &params.project).await?;
    super::layout::show_terminal(core, At::project(&params.project), id, None)
}

/// Brings the terminals of no project to the front, opening one when there are none.
pub(crate) async fn show_terminals(core: &Arc<Core>) -> CoreResult<()> {
    if !super::layout::show_terminals(core)? {
        let id = home_shell(core).await?;
        super::layout::show_terminal(core, At::terminals(), id, None)?;
    }
    Ok(())
}

async fn home_shell(core: &Arc<Core>) -> CoreResult<SessionId> {
    let context = PtyContext {
        session_id: "terminals".into(),
        session_name: "Terminals".into(),
        session_kind: "command".into(),
        project: None,
        window_id: Some(format!("win-{}", uuid::Uuid::new_v4().simple())),
        pane_id: Some(format!("pane-{}", uuid::Uuid::new_v4().simple())),
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: true,
    };
    let home = sikemux_process::user_environment::var_os("HOME")
        .map(|home| home.to_string_lossy().into_owned());
    terminal(core, opening_size(core), home, context, None).await
}

/// A shell in `project`'s folder for a new terminal: the one started ahead
/// when it fits, or a new one.
pub(crate) async fn project_shell(core: &Arc<Core>, project: &str) -> CoreResult<SessionId> {
    if let Some(taken) = take(
        core,
        TakeParams {
            project: project.to_owned(),
        },
    ) {
        return Ok(taken.id);
    }
    let (path, name) = project_folder(core, project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))?;
    let size = opening_size(core);
    let window_id = format!("win-{}", uuid::Uuid::new_v4().simple());
    let pane_id = format!("pane-{}", uuid::Uuid::new_v4().simple());
    shell(
        core,
        project,
        path,
        name,
        size,
        (&window_id, &pane_id),
        None,
    )
    .await
}

/// Opens a terminal tab titled `title` in `project`, running `startup` in a
/// new shell. Answers the tab's id.
pub(crate) async fn command_terminal(
    core: &Arc<Core>,
    project: String,
    title: String,
    startup: String,
) -> CoreResult<String> {
    let id = command_shell(core, &project, startup).await?;
    super::layout::show_terminal(core, At::project(&project), id, Some(title))
}

/// A new shell in `project`'s folder running `startup`.
pub(crate) async fn command_shell(
    core: &Arc<Core>,
    project: &str,
    startup: String,
) -> CoreResult<SessionId> {
    let (path, name) = project_folder(core, project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))?;
    let window_id = format!("win-{}", uuid::Uuid::new_v4().simple());
    let pane_id = format!("pane-{}", uuid::Uuid::new_v4().simple());
    let size = opening_size(core);
    shell(
        core,
        project,
        path,
        name,
        size,
        (&window_id, &pane_id),
        Some(startup),
    )
    .await
}

#[derive(Deserialize)]
pub(crate) struct SshParams {
    host: String,
}

impl SshParams {
    pub(crate) fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
        }
    }
}

/// Opens a terminal tab connected to `host`, by its alias in the person's
/// SSH config, and brings the host to the front. Answers the tab's id.
pub(crate) async fn open_ssh_terminal(core: &Arc<Core>, params: SshParams) -> CoreResult<String> {
    let id = ssh_shell(core, &params.host).await?;
    super::layout::show_ssh_terminal(core, params.host, id)
}

/// A new shell for a view beside the others of the strip `at` names: in the project's
/// folder, on the SSH host, or in the home folder for the terminals of no project.
pub(crate) async fn strip_shell(core: &Arc<Core>, at: At) -> CoreResult<SessionId> {
    match (at.host, at.terminals) {
        (_, true) => home_shell(core).await,
        (Some(host), false) => ssh_shell(core, &host).await,
        (None, false) => project_shell(core, &at.project).await,
    }
}

/// A shell in the home folder for the SSH host `host`'s strip, running `startup`, which sets
/// that host up over ssh. It is not one of the host's ssh terminals, so nothing reconnects it.
pub(crate) async fn setup_shell(
    core: &Arc<Core>,
    host: &str,
    startup: String,
) -> CoreResult<SessionId> {
    let context = PtyContext {
        session_id: format!("ssh:{host}"),
        session_name: host.to_owned(),
        session_kind: "setup".into(),
        project: None,
        window_id: Some(format!("win-{}", uuid::Uuid::new_v4().simple())),
        pane_id: Some(format!("pane-{}", uuid::Uuid::new_v4().simple())),
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: false,
    };
    terminal(core, opening_size(core), None, context, Some(startup)).await
}

async fn ssh_shell(core: &Arc<Core>, host: &str) -> CoreResult<SessionId> {
    let context = PtyContext {
        session_id: format!("ssh:{host}"),
        session_name: host.to_owned(),
        session_kind: "ssh".into(),
        project: None,
        window_id: Some(format!("win-{}", uuid::Uuid::new_v4().simple())),
        pane_id: Some(format!("pane-{}", uuid::Uuid::new_v4().simple())),
        agent_id: None,
        agent_type: None,
        initial_prompt_submitted: false,
        shell_integration: false,
    };
    terminal(core, opening_size(core), None, context, None).await
}

/// Stops the shell started ahead, as before the core hands itself over.
pub(crate) fn discard(core: &Arc<Core>) {
    let spare = core.spares.held.locked().spare.take();
    if let Some(spare) = spare {
        stop(core, spare.id);
    }
}

fn stop(core: &Arc<Core>, id: SessionId) {
    if let Some(target) = core.take_session_for_kill(id) {
        let core = core.clone();
        tokio::task::spawn_blocking(move || session::kill(&core, &target));
    }
}
