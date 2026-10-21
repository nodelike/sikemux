//! Agents' tool calls. The core answers the task tools and `events.wait` from
//! its own runs and journal, so they work while the window is closed, and
//! hands everything that needs the window to it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_pty::output_log::{OutputPage, OutputQuery};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::cli::protocol::{
    is_browser_method, is_sim_method, validate_harness, HarnessRequest, COMMIT_FOOTER_METHOD,
    CORE_REPLY_TIMEOUT, GIT_GUARD_METHOD, PLUGINS_CHANGED_METHOD, SIM_CANCEL_METHOD,
    SIM_OFFERED_METHOD, TOOLS_MANIFEST_METHOD,
};
use crate::harness::command::{command_cwd, command_label, command_task_id, COMMAND_TASK_PREFIX};
use crate::harness::journal::{EventsPage, JournalRecord, Journals};
use crate::harness::runs::{CommandLaunch, Launch, Run, RunStatus, Runs, RunsRecord};
use crate::protocol::{SessionId, TaskSessionInfo, WindowCall};

use super::connection::blocking;
use super::messages::{self, Mailbox};
use super::questions::{self, Questions};
use super::window::Call;
use super::{board, follow, saved, spawn};
use super::{session, Core};
use crate::skills;

/// MCP hosts give up on a tool call after about a minute, so a start answers
/// well before that with whatever state it reached.
const START_BUDGET: Duration = Duration::from_secs(30);
/// A chatty task adds at most one output event per this long.
const OUTPUT_EVENT_GAP: Duration = Duration::from_millis(250);
const READY_POLL: Duration = Duration::from_millis(250);
const MAX_WAITS: usize = 32;
const MAX_WAIT_MS: u64 = 30_000;
/// Every running agent's tools server holds one of these open.
const MAX_PLUGIN_WAITS: usize = 512;
/// Shorter than the tools server waits for any answer.
const PLUGIN_WAIT: Duration = Duration::from_secs(55);
/// A pane that makes several calls in a row wakes each tools server once.
const PLUGIN_SETTLE: Duration = Duration::from_secs(1);
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const TOOL_CALLS_FILE: &str = "agent-tool-calls.json";
const WINDOW_GONE: &str = "Sikemux's window closed before it answered";
/// Tools that act as the agent calling them, or with a sign-in or a view
/// the caller does not hold itself, so a call from outside every agent's
/// processes is refused rather than answered for no one.
const AGENT_METHODS: [&str; 8] = [
    "agent.",
    "board.",
    "git.",
    "review.",
    "browser.",
    "sim.",
    "plugins.call",
    "app.console",
];
/// What a reviewer Auto started blind may not do: start agents, read saved
/// sessions, who wrote what, or the board, any of which could show it the
/// builder's side.
const BLIND_REFUSED: [&str; 4] = ["agent.spawn", "agent.sessions", "agent.wrote", "board."];
/// What agents do goes in the same journal as tasks, for `agent_wait` alone.
pub(crate) const AGENT_EVENT: &str = "agent.";

/// What `workspace_inspect` takes from the window: the panes it shows.
const LAYOUT_FIELDS: [&str; 4] = ["sessionId", "active", "activeWindowId", "windows"];
const NO_WINDOW_NOTE: &str =
    "No Sikemux window is open on this host, so there are no panes to list.";
const AWAITING_TRUST_NOTE: &str = "Waiting for the person to trust this project's sikemux.json in Sikemux. Call task_start again with the same idempotencyKey, or events_wait with this executionId, to see when it starts.";
const STARTING_NOTE: &str = "Still starting. Call task_start again with the same idempotencyKey, or events_wait with this executionId, to see when it runs.";
const NOT_READY_NOTE: &str = "The task is running but readyWhen has not appeared yet. Wait with events_wait on this executionId, or task_read with search.";

const LAUNCH_CUT_SHORT: &str = "Sikemux's background process was updated while the window was starting this task; start it again";

/// The harness as a replacement core takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessRecord {
    runs: RunsRecord,
    journals: Vec<JournalRecord>,
}

struct State {
    runs: Runs,
    journals: Journals,
    output_pending: HashSet<String>,
}

impl State {
    fn set_status(&mut self, execution_id: &str, status: RunStatus) {
        let Some(run) = self.runs.by_execution_mut(execution_id) else {
            return;
        };
        if run.status == status {
            return;
        }
        run.status = status;
        let (project, task_id) = (run.project.clone(), run.task_id.clone());
        self.journals.append(
            &project,
            status.event_kind(),
            Some(execution_id),
            Some(&task_id),
        );
    }

    fn run(&self, project: &str, execution_id: &str) -> Result<Run, String> {
        self.runs.get(project, execution_id).cloned()
    }

    /// The run a call names by `executionId`, or by `taskId` for its latest.
    fn execution_for(&mut self, project: &str, params: &Value) -> Result<String, String> {
        let execution_id = text(params, "executionId")?;
        let task_id = text(params, "taskId")?;
        match (execution_id, task_id) {
            (Some(_), Some(_)) => Err("Pass either executionId or taskId, not both".into()),
            (Some(execution_id), None) => Ok(execution_id.to_owned()),
            (None, None) => Err("executionId or taskId is required".into()),
            (None, Some(task_id)) => {
                if let Some(run) = self.runs.latest(project, task_id) {
                    return Ok(run.execution_id.clone());
                }
                if self.journals.mentions_task(project, task_id) {
                    return Err(format!("Task {task_id} was started earlier, but its run is gone: it ended a while ago or was stopped with Quit and Stop Everything. Start it again with task_start."));
                }
                Err(format!("Task {task_id} has not been started; call workspace_inspect to see the runs Sikemux knows about, or start it with task_start"))
            }
        }
    }

    fn earlier_command(&mut self, project: &str, task_id: &str) -> Result<CommandLaunch, String> {
        if let Some(spec) = self.runs.launch_spec(project, task_id) {
            return Ok(spec.clone());
        }
        if self.journals.mentions_task(project, task_id) {
            return Err(format!("Task {task_id} was started earlier, but its run and command are gone; start it again with command"));
        }
        Err(format!(
            "Task {task_id} has not been started; start it with command"
        ))
    }
}

pub(crate) struct Harness {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    tool_calls: Option<PathBuf>,
    /// The app data folder, which holds the skills library.
    pub(crate) data_dir: Option<PathBuf>,
    waits: AtomicUsize,
    plugins: watch::Sender<u64>,
    plugin_waits: AtomicUsize,
    /// Messages between agents waiting for their turn to end.
    pub(crate) mailbox: Mailbox,
    /// Questions agents asked that wait on an answer.
    pub(crate) questions: Questions,
    pub(crate) spawning: spawn::Spawning,
}

impl Harness {
    /// Journals, the tool-call tally, and the messages and questions between
    /// agents go under `data_dir`; without one they live in memory, or not at
    /// all. Messages and questions of agents `known` does not hold are dropped.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        Self {
            state: Mutex::new(State {
                runs: Runs::default(),
                journals: Journals::new(data_dir.map(|dir| dir.join("journal"))),
                output_pending: HashSet::new(),
            }),
            changes: watch::channel(0).0,
            tool_calls: data_dir.map(|dir| dir.join(TOOL_CALLS_FILE)),
            data_dir: data_dir.map(Path::to_path_buf),
            waits: AtomicUsize::new(0),
            plugins: watch::channel(0).0,
            plugin_waits: AtomicUsize::new(0),
            mailbox: Mailbox::new(data_dir, &known),
            questions: Questions::new(data_dir, &known),
            spawning: spawn::Spawning::default(),
        }
    }

    pub(crate) fn plugins_changed(&self) {
        self.plugins.send_modify(|version| *version += 1);
    }

    /// Runs `change` on the state and wakes every waiter.
    fn change<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let result = change(&mut self.state.locked());
        self.changes.send_modify(|version| *version += 1);
        result
    }

    fn read<T>(&self, read: impl FnOnce(&mut State) -> T) -> T {
        read(&mut self.state.locked())
    }

    /// A task session just spawned. True when its run was stopped before it
    /// got here, so the session must go.
    pub(crate) fn session_started(&self, id: SessionId, task: &TaskSessionInfo) -> bool {
        self.change(|state| {
            let Some(run) = state.runs.by_execution_mut(&task.execution_id) else {
                return false;
            };
            if !run.status.is_active() {
                return true;
            }
            run.pty_id = Some(id);
            run.label = Some(task.label.clone());
            run.command = Some(task.command.clone());
            run.cwd = Some(task.cwd.clone());
            run.started_at = Some(sikemux_process::clock::epoch_ms());
            if matches!(run.status, RunStatus::Starting | RunStatus::AwaitingTrust) {
                state.set_status(&task.execution_id, RunStatus::Running);
            }
            false
        })
    }

    /// A missing `code` means the status could not be read, which counts as a
    /// failure. Returns the run when it failed on its own and an agent started it.
    pub(crate) fn session_exited(
        &self,
        id: SessionId,
        code: Option<u32>,
        signal: Option<String>,
    ) -> Option<FailedRun> {
        self.change(|state| {
            let run = state.runs.by_pty_mut(id)?;
            let code = code.unwrap_or(1);
            run.exit_code = Some(code);
            run.signal = signal.clone();
            let next = match run.status {
                RunStatus::Stopped => return None,
                RunStatus::Stopping => RunStatus::Stopped,
                _ if code == 0 => RunStatus::Completed,
                _ => RunStatus::Failed,
            };
            let failed = (next == RunStatus::Failed)
                .then(|| run.agent_id.clone())
                .flatten()
                .map(|agent_id| FailedRun {
                    agent_id,
                    execution_id: run.execution_id.clone(),
                    label: run.label.clone().unwrap_or_else(|| run.task_id.clone()),
                    reason: match &signal {
                        Some(signal) => format!("it stopped from signal {signal}"),
                        None => format!("it exited with code {code}"),
                    },
                });
            let execution_id = run.execution_id.clone();
            state.set_status(&execution_id, next);
            failed
        })
    }

    pub(crate) fn record(&self) -> HarnessRecord {
        self.read(|state| HarnessRecord {
            runs: state.runs.record(),
            journals: state.journals.record(),
        })
    }

    /// Takes over an earlier core's runs, keys and journals. A launch the
    /// window was in the middle of answered the earlier core, so it is over.
    pub(crate) fn restore(&self, record: HarnessRecord) {
        self.change(|state| {
            state.runs = Runs::restored(record.runs);
            state.journals.restore(record.journals);
            let mut cut_short = Vec::new();
            for run in state.runs.all_mut() {
                if run.launch != Launch::Pending {
                    continue;
                }
                if run.pty_id.is_some() {
                    run.launch = Launch::Done;
                    continue;
                }
                run.launch = Launch::Failed(LAUNCH_CUT_SHORT.into());
                run.error.get_or_insert_with(|| LAUNCH_CUT_SHORT.into());
                if run.status.is_active() {
                    cut_short.push(run.execution_id.clone());
                }
            }
            for execution_id in cut_short {
                state.set_status(&execution_id, RunStatus::Failed);
            }
        });
    }

    /// One of the few waits the core holds open at once, or an error when
    /// they are all taken.
    pub(crate) fn wait_slot(&self) -> Result<WaitSlot<'_>, String> {
        if self.waits.fetch_add(1, Ordering::AcqRel) >= MAX_WAITS {
            self.waits.fetch_sub(1, Ordering::AcqRel);
            return Err("Too many event waits".into());
        }
        Ok(WaitSlot(&self.waits))
    }

    /// Changes whenever a project's journal does.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub(crate) fn journal_cursor(&self, project: &str) -> u64 {
        self.read(|state| state.journals.cursor(project))
    }

    pub(crate) fn journal_since(&self, project: &str, after: u64) -> Result<EventsPage, String> {
        self.read(|state| state.journals.since(project, after, None))
    }

    /// Something happened to a project's repository, such as a commit landing
    /// or its check ending, which `events_wait` hears of.
    pub(crate) fn note_repository(&self, project: &str, kind: &str) {
        self.change(|state| {
            state.journals.append(project, kind, None, None);
        });
    }

    /// Something happened to an agent, which `agent_wait` hears of.
    pub(crate) fn note_agent(&self, project: &str, kind: &str, agent_id: &str) {
        self.change(|state| {
            state.journals.append_agent(project, kind, agent_id);
        });
    }

    /// Wakes whatever waits on the harness, for a change kept elsewhere.
    pub(crate) fn changed(&self) {
        self.change(|_| ());
    }

    pub(crate) fn run_of(&self, execution_id: &str) -> Option<Run> {
        self.read(|state| state.runs.by_execution(execution_id).cloned())
    }

    pub(crate) fn run_on(&self, pty_id: SessionId) -> Option<Run> {
        self.read(|state| state.runs.by_pty(pty_id).cloned())
    }

    pub(crate) fn all_runs(&self) -> Vec<Run> {
        self.read(|state| state.runs.all().to_vec())
    }

    /// A run that has not spawned yet will not.
    pub(crate) fn launch_failed(&self, execution_id: &str, message: &str) {
        self.change(|state| {
            let Some(run) = state.runs.by_execution_mut(execution_id) else {
                return;
            };
            if run.pty_id.is_some() || !run.status.is_active() {
                return;
            }
            run.error.get_or_insert_with(|| message.to_owned());
            state.set_status(execution_id, RunStatus::Failed);
        });
    }

    pub(crate) fn awaiting_trust(&self, execution_id: &str) {
        self.change(|state| {
            if state
                .runs
                .by_execution(execution_id)
                .is_some_and(|run| run.status == RunStatus::Starting)
            {
                state.set_status(execution_id, RunStatus::AwaitingTrust);
            }
        });
    }
}

/// New output from a task session. Several arrivals close together make one
/// event.
pub(crate) fn note_output(core: &Arc<Core>, id: SessionId) {
    let pending = core.harness.read(|state| {
        let run = state.runs.by_pty_mut(id)?;
        let execution_id = run.execution_id.clone();
        let project = run.project.clone();
        state
            .output_pending
            .insert(execution_id.clone())
            .then_some((execution_id, project))
    });
    let Some((execution_id, project)) = pending else {
        return;
    };
    let core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(OUTPUT_EVENT_GAP).await;
        core.harness.change(|state| {
            state.output_pending.remove(&execution_id);
            state
                .journals
                .append(&project, "task.output", Some(&execution_id), None);
        });
    });
}

fn text<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() && value.chars().count() <= 4096 => {
            Ok(Some(value))
        }
        Some(_) => Err(format!(
            "{key} must be nonempty text of at most 4096 characters"
        )),
    }
}

fn required_text<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    text(params, key)?
        .ok_or_else(|| format!("{key} must be nonempty text of at most 4096 characters"))
}

fn integer(params: &Value, key: &str, fallback: u64, max: u64, min: u64) -> Result<u64, String> {
    let value = match params.get(key) {
        None | Some(Value::Null) => return Ok(fallback),
        Some(value) => value,
    };
    value
        .as_u64()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{key} must be an integer between {min} and {max}"))
}

fn with_fields(run: &Run, fields: Value) -> Value {
    let mut value = serde_json::to_value(run).unwrap_or_else(|_| json!({}));
    if let (Some(object), Value::Object(fields)) = (value.as_object_mut(), fields) {
        object.extend(fields);
    }
    value
}

fn purpose(method: &str) -> &'static str {
    match method {
        "workspace.inspect" => "inspect the workspace",
        "ui.open" => "open things in it",
        "app.console" => "read its console",
        method if is_browser_method(method) => "use the browser",
        method if is_sim_method(method) => "drive the iOS Simulator",
        _ => "use this tool",
    }
}

fn no_answer(method: &str) -> String {
    format!(
        "{} got no answer from Sikemux within {} s. Check that the Sikemux window is open and responsive, then retry.",
        method.replace('.', "_"),
        CORE_REPLY_TIMEOUT.as_secs()
    )
}

async fn answer_of(
    core: &Core,
    call: Call,
    limit: Duration,
    late: String,
) -> Result<Value, String> {
    match tokio::time::timeout(limit, call.answer).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(WINDOW_GONE.into()),
        Err(_) => {
            core.window.forget(call.id);
            Err(late)
        }
    }
}

async fn forward(core: &Core, request: HarnessRequest) -> Result<Value, String> {
    let call = core.window.call(
        WindowCall::Harness {
            request: serde_json::to_value(&request).map_err(|error| error.to_string())?,
        },
        purpose(&request.method),
        false,
    )?;
    answer_of(core, call, CORE_REPLY_TIMEOUT, no_answer(&request.method)).await
}

/// Answers one tool call from the client at `peer` on the tool endpoint's
/// `port`, as the agent whose processes hold that connection, whatever agent
/// it names, or as a commit the core runs. The call is counted whether or not
/// it succeeds.
pub(crate) async fn call(
    core: &Arc<Core>,
    mut request: HarnessRequest,
    peer: std::net::SocketAddr,
    port: u16,
) -> Result<Value, String> {
    validate_harness(&request)?;
    let grant = if request.method == COMMIT_FOOTER_METHOD {
        tokio::task::spawn_blocking(move || super::callers::take_commit_grant(peer, port))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    request.agent_id = match grant {
        Some(_) => None,
        None => tokio::task::spawn_blocking(move || super::callers::identify(peer, port))
            .await
            .ok()
            .flatten(),
    };
    if request.agent_id.is_none()
        && grant.is_none()
        && AGENT_METHODS
            .iter()
            .any(|prefix| request.method.starts_with(prefix))
    {
        return Err(format!(
            "{} works only for an agent Sikemux started, and this call came from outside every agent's processes; start this agent from Sikemux again",
            request.method.replace('.', "_")
        ));
    }
    let tool = tool_name(&request);
    let result = dispatch(core, request, grant).await;
    if let (Some(tool), Some(path)) = (tool, core.harness.tool_calls.clone()) {
        let succeeded = result.is_ok();
        tokio::task::spawn_blocking(move || record_tool_call(&path, &tool, succeeded));
    }
    result
}

async fn dispatch(
    core: &Arc<Core>,
    mut request: HarnessRequest,
    grant: Option<super::callers::CommitGrant>,
) -> Result<Value, String> {
    if request.method == TOOLS_MANIFEST_METHOD {
        return crate::cli::manifest::served();
    }
    let project = request.project.clone();
    request.project = tokio::task::spawn_blocking(move || std::fs::canonicalize(project))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    if let Some(agent_id) = &request.agent_id {
        messages::used_tool(core, agent_id, &request.method.replace('.', "_"));
        if core.names.is_blind(agent_id)
            && BLIND_REFUSED
                .iter()
                .any(|refused| request.method.starts_with(refused))
        {
            return Err(format!(
                "You review a card blind for Auto, so {} is not yours to use; judge the change from the card, the diff and your folder",
                request.method.replace('.', "_")
            ));
        }
    }
    match request.method.as_str() {
        "task.read" => task_read(core, &request).await,
        "task.stop" => {
            let execution_id = core
                .harness
                .read(|state| state.execution_for(&request.project, &request.params))?;
            core.harness
                .read(|state| state.run(&request.project, &execution_id))?;
            stop_run(core, &execution_id)
                .await
                .map(|run| with_fields(&run, json!({})))
        }
        "events.wait" => events_wait(core, &request).await,
        "agent.list" => messages::list(core, &request).await,
        "agent.send" => messages::send(core, &request).await,
        "agent.spawn" => spawn::spawn(core, &request).await,
        "agent.sessions" => saved::find(core, &request).await,
        "agent.ask" => questions::ask(core, &request).await,
        "agent.read" => follow::read(core, &request).await,
        "agent.wait" => follow::wait(core, &request).await,
        "agent.wrote" => super::who_wrote::tool(core, &request).await,
        "board.add" => board::add(core, &request).await,
        "board.list" => board::list(core, &request).await,
        "board.claim" => board::claim(core, &request).await,
        "board.update" => board::update(core, &request).await,
        "review.verdict" => board::verdict(core, &request).await,
        "git.commit" => super::edits::commit::call(core, &request).await,
        COMMIT_FOOTER_METHOD => super::edits::footer::call(core, &request, grant).await,
        GIT_GUARD_METHOD => super::edits::guard::call(core, &request).await,
        "browser.record" => {
            let agent_id = request.agent_id.clone();
            let answer = forward(core, request).await?;
            if let Some(agent_id) = agent_id {
                super::inbox::recorded(core, &agent_id, &answer);
            }
            Ok(answer)
        }
        PLUGINS_CHANGED_METHOD => plugins_wait(&core.harness, &request.params).await,
        "plugins.tools" => super::plugins::agent_tools(&request.project).await,
        "plugins.call" => super::plugins::call_agent_tool(&request.params).await,
        "skills.manage" => skills_call(core, &request).await,
        "task.start" => task_start(core, &request, false).await,
        "task.restart" => task_start(core, &request, true).await,
        "workspace.inspect" => inspect(core, request).await,
        "project.suggest" => super::suggestions::suggest(core, &request),
        "page.show" => show_page(core, request).await,
        "ui.open" => {
            let kind = text(&request.params, "kind")?.map(str::to_owned);
            if kind.as_deref() == Some("terminal") {
                let execution_id = required_text(&request.params, "executionId")?;
                core.harness
                    .read(|state| state.run(&request.project, execution_id))?;
            }
            let project = request.project.clone();
            let value = forward(core, request).await?;
            if kind.as_deref() != Some("preview") {
                core.harness.change(|state| {
                    state.journals.append(&project, "ui.opened", None, None);
                });
            }
            Ok(value)
        }
        _ => forward(core, request).await,
    }
}

/// The project as the core knows it, with the panes the window shows when
/// one is open.
const PAGE_IN_REPLY: &str = "The person sees the page in your reply, above the text you write next. Do not describe or restate it; write only what it does not already say.";
const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
const PAGE_KEPT: &str = "The page is kept, but no Sikemux window is open to show it on your desk.";

/// Keeps the page an agent wrote. A chat shows it in its reply; for an agent
/// in a terminal the window opens it on the agent's desk.
async fn show_page(core: &Arc<Core>, mut request: HarnessRequest) -> Result<Value, String> {
    let path = PathBuf::from(required_text(&request.params, "path")?);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let title = required_text(&request.params, "title")?.to_owned();
    let height = match request.params.get("height") {
        None | Some(Value::Null) => None,
        Some(_) => Some(integer(&request.params, "height", 0, 4000, 1)? as u32),
    };
    let dir = core
        .harness
        .data_dir
        .clone()
        .ok_or("Pages need Sikemux's data folder, and this core was started without one")?
        .join(crate::pages::FOLDER);
    let page = blocking(move || Ok(crate::pages::publish(&dir, &path, &title, height)))
        .await
        .map_err(|error| error.to_string())??;
    let in_chat = request
        .agent_id
        .as_deref()
        .is_none_or(|agent_id| core.chats.get(agent_id).is_some());
    let mut on_desk = false;
    if core.window.is_open() {
        request.params = json!({ "id": page.id, "desk": !in_chat });
        on_desk = forward(core, request).await.is_ok() && !in_chat;
    }
    let note = if in_chat {
        PAGE_IN_REPLY
    } else if on_desk {
        PAGE_ON_DESK
    } else {
        PAGE_KEPT
    };
    Ok(json!({ "page": page, "note": note }))
}

async fn inspect(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    let project = request.project.clone();
    let agent_id = request.agent_id.clone();
    let layout = if core.window.is_open() {
        forward(core, request).await.ok()
    } else {
        None
    };
    let mut value = {
        let (core, project) = (core.clone(), project.clone());
        blocking(move || {
            let mut inspected = super::tasks::inspect(&core, Path::new(&project));
            let tasks = inspected
                .get("tasks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let brief = super::brief::of(&core, Path::new(&project), &tasks);
            if let Some(object) = inspected.as_object_mut() {
                object.insert("projectBrief".into(), brief.into());
            }
            Ok(inspected)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("project".into(), project.clone().into());
        object.insert("agentId".into(), agent_id.into());
        match layout {
            Some(Value::Object(mut layout)) => {
                for key in LAYOUT_FIELDS {
                    if let Some(value) = layout.remove(key) {
                        object.insert(key.into(), value);
                    }
                }
            }
            _ => {
                object.insert("note".into(), NO_WINDOW_NOTE.into());
            }
        }
        core.harness.read(|state| {
            object.insert(
                "runs".into(),
                serde_json::to_value(state.runs.list(&project)).unwrap_or_default(),
            );
            object.insert(
                "cursor".into(),
                state.journals.cursor(&project).to_string().into(),
            );
        });
    }
    Ok(value)
}

const SKILL_LOADS: &str =
    "Start a new agent session to load it. Claude lists it as /sikemux:<name>.";

async fn skills_call(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let data_dir = core
        .harness
        .data_dir
        .clone()
        .ok_or("Skills need Sikemux's data folder, and this core was started without one")?;
    let params = request.params.clone();
    blocking(move || Ok(skills_answer(&data_dir, &params)))
        .await
        .map_err(|error| error.to_string())?
}

fn skills_answer(data_dir: &Path, params: &Value) -> Result<Value, String> {
    match required_text(params, "action")? {
        "list" => Ok(json!({
            "library": skills::library(data_dir),
            "skills": skills::list(data_dir),
        })),
        "create" => {
            let body = match params.get("body") {
                None | Some(Value::Null) => "",
                Some(Value::String(body)) => body.as_str(),
                Some(_) => return Err("body must be text".into()),
            };
            let skill = skills::create(
                data_dir,
                required_text(params, "name")?,
                required_text(params, "description")?,
                body,
            )?;
            Ok(json!({ "skill": skill, "note": SKILL_LOADS }))
        }
        "install" => {
            let source = required_text(params, "source")?;
            let skill = if source.starts_with('/') {
                skills::install_folder(data_dir, Path::new(source))?
            } else {
                skills::install_git(data_dir, source, text(params, "subfolder")?)?
            };
            Ok(json!({ "skill": skill, "note": SKILL_LOADS }))
        }
        "remove" => {
            let name = required_text(params, "name")?;
            skills::remove(data_dir, name)?;
            Ok(json!({ "removed": name }))
        }
        other => Err(format!(
            "action must be list, create, install or remove, not {other}"
        )),
    }
}

async fn read_output(core: &Core, id: SessionId, query: OutputQuery) -> Result<OutputPage, String> {
    let session = core
        .session(id)
        .ok_or("Task output expired or task no longer exists")?;
    blocking(move || session::task_output(&session, &query))
        .await
        .map_err(|error| error.to_string())
}

async fn task_read(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    let run = core.harness.read(|state| {
        let execution_id = state.execution_for(&request.project, params)?;
        state.run(&request.project, &execution_id)
    })?;
    let plain = match params.get("plain") {
        None => true,
        Some(Value::Bool(plain)) => *plain,
        Some(_) => return Err("plain must be a boolean".into()),
    };
    let query = OutputQuery {
        cursor: integer(params, "cursor", 0, MAX_SAFE_INTEGER, 0)?,
        limit: integer(params, "limit", 8192, 8192, 4)? as usize,
        tail: match params.get("tail") {
            None => None,
            Some(_) => Some(integer(params, "tail", 1, 10_000, 1)? as usize),
        },
        search: text(params, "search")?.map(str::to_owned),
        context: integer(params, "context", 3, 20, 0)? as usize,
        plain,
    };
    let Some(pty_id) = run.pty_id else {
        return Ok(with_fields(
            &run,
            json!({ "output": "", "cursor": 0, "end": 0, "hasMore": false, "truncated": false }),
        ));
    };
    let page = read_output(core, pty_id, query).await?;
    let mut fields = json!({
        "output": String::from_utf8_lossy(&page.bytes),
        "cursor": page.cursor,
        "end": page.end,
        "hasMore": page.has_more,
        "truncated": page.truncated,
    });
    if let (Some(matches), Some(object)) = (page.matches, fields.as_object_mut()) {
        object.insert("matches".into(), matches.into());
    }
    Ok(with_fields(&run, fields))
}

pub(crate) struct WaitSlot<'a>(&'a AtomicUsize);

impl Drop for WaitSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn events_wait(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    let cursor = required_text(params, "cursor")?;
    let after = cursor
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| cursor.parse::<u64>().ok())
        .flatten()
        .ok_or("Event cursor is invalid; take a fresh one from workspace_inspect")?;
    let timeout = integer(params, "timeoutMs", MAX_WAIT_MS, MAX_WAIT_MS, 0)?;
    let execution_id = text(params, "executionId")?;
    let harness = &core.harness;
    let _slot = harness.wait_slot()?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    loop {
        let mut changed = harness.changes.subscribe();
        let mut page =
            harness.read(|state| state.journals.since(&request.project, after, execution_id))?;
        page.events
            .retain(|entry| !entry.kind.starts_with(AGENT_EVENT));
        if !page.events.is_empty() || page.truncated || Instant::now() >= deadline {
            let events: Vec<Value> = page
                .events
                .iter()
                .map(|entry| {
                    let mut event = json!({ "cursor": entry.seq.to_string(), "kind": entry.kind });
                    if let (Some(id), Some(object)) = (&entry.execution_id, event.as_object_mut()) {
                        object.insert("executionId".into(), id.clone().into());
                    }
                    event
                })
                .collect();
            return Ok(json!({
                "events": events,
                "cursor": page.cursor.to_string(),
                "truncated": page.truncated,
            }));
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

/// Answers with how many times plugins have changed, once that is no longer
/// `seen` or the wait runs out.
async fn plugins_wait(harness: &Harness, params: &Value) -> Result<Value, String> {
    let seen = params.get("seen").and_then(Value::as_u64);
    if harness.plugin_waits.fetch_add(1, Ordering::AcqRel) >= MAX_PLUGIN_WAITS {
        harness.plugin_waits.fetch_sub(1, Ordering::AcqRel);
        return Err("Too many plugin waits".into());
    }
    let _slot = WaitSlot(&harness.plugin_waits);
    let mut changes = harness.plugins.subscribe();
    if seen == Some(*changes.borrow_and_update()) {
        let changed = tokio::time::timeout(PLUGIN_WAIT, changes.changed()).await;
        if matches!(changed, Ok(Ok(()))) {
            tokio::time::sleep(PLUGIN_SETTLE).await;
        }
    }
    let version = *harness.plugins.borrow();
    Ok(json!({ "version": version }))
}

/// Stops one run and waits for its process to go.
pub(crate) async fn stop_run(core: &Arc<Core>, execution_id: &str) -> Result<Run, String> {
    let kill = core
        .harness
        .change(|state| -> Result<Option<SessionId>, String> {
            let run = state
                .runs
                .by_execution(execution_id)
                .ok_or_else(|| String::from(crate::harness::runs::NOT_IN_PROJECT))?;
            let exited = run.exit_code.is_some();
            match (run.pty_id, run.status) {
                (None, RunStatus::AwaitingTrust | RunStatus::Starting) => {
                    state.set_status(execution_id, RunStatus::Stopped);
                    Ok(None)
                }
                (None, _) | (Some(_), RunStatus::Completed | RunStatus::Stopped) => Ok(None),
                (Some(_), RunStatus::Failed) if exited => Ok(None),
                (Some(pty_id), _) => {
                    state.set_status(execution_id, RunStatus::Stopping);
                    Ok(Some(pty_id))
                }
            }
        })?;
    if let Some(pty_id) = kill {
        if let Some(target) = core.session(pty_id) {
            let killing = core.clone();
            let _ = blocking(move || {
                session::kill(&killing, &target);
                Ok(())
            })
            .await;
        }
        core.harness.change(|state| {
            if state
                .runs
                .by_execution(execution_id)
                .is_some_and(|run| run.status == RunStatus::Stopping)
            {
                state.set_status(execution_id, RunStatus::Stopped);
            }
        });
    }
    core.harness.read(|state| {
        state
            .runs
            .by_execution(execution_id)
            .cloned()
            .ok_or_else(|| crate::harness::runs::NOT_IN_PROJECT.into())
    })
}

/// The runs that match every field given.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSelector {
    execution_id: Option<String>,
    project: Option<String>,
    agent_id: Option<String>,
}

/// Stops the active runs that match, such as the ones a closed project or
/// agent leaves behind.
pub(crate) async fn stop_runs(core: &Arc<Core>, selector: RunSelector) {
    let matching = core.harness.read(|state| {
        state.runs.matching(|run| {
            run.status.is_active()
                && selector
                    .execution_id
                    .as_ref()
                    .is_none_or(|id| *id == run.execution_id)
                && selector
                    .project
                    .as_ref()
                    .is_none_or(|project| *project == run.project)
                && selector
                    .agent_id
                    .as_ref()
                    .is_none_or(|agent| run.agent_id.as_ref() == Some(agent))
        })
    });
    for execution_id in matching {
        let _ = stop_run(core, &execution_id).await;
    }
}

/// What a start names: the task id, and for a command task what it runs.
fn launch_target(
    state: &mut State,
    project: &str,
    params: &Value,
    restart: bool,
) -> Result<(String, Option<CommandLaunch>), String> {
    if restart {
        let task_id = required_text(params, "taskId")?;
        let spec = if task_id.starts_with(COMMAND_TASK_PREFIX) {
            Some(state.earlier_command(project, task_id)?)
        } else {
            None
        };
        return Ok((task_id.to_owned(), spec));
    }
    let task_id = text(params, "taskId")?;
    let has_command = params.get("command").is_some();
    match task_id {
        Some(_) if has_command => Err("Pass either taskId or command, not both".into()),
        None if !has_command => Err("taskId or command is required".into()),
        None => {
            let command = required_text(params, "command")?;
            let label = text(params, "label")?;
            if label.is_some_and(|label| label.chars().count() > 80) {
                return Err("label must be at most 80 characters".into());
            }
            let cwd = command_cwd(text(params, "cwd")?)?;
            let task_id = command_task_id(command, &cwd, label);
            Ok((
                task_id,
                Some(CommandLaunch {
                    command: command.to_owned(),
                    cwd,
                    label: label.map_or_else(|| command_label(command), str::to_owned),
                }),
            ))
        }
        Some(task_id) => {
            if params.get("cwd").is_some() || params.get("label").is_some() {
                return Err(
                    "cwd and label go with command; a sikemux.json task sets its own".into(),
                );
            }
            Ok((task_id.to_owned(), None))
        }
    }
}

enum Begun {
    Existing(String),
    New {
        execution_id: String,
        /// The run a restart replaces.
        previous: Option<String>,
    },
}

fn begin_start(
    core: &Core,
    project: &str,
    agent_id: Option<&str>,
    params: &Value,
    key: &str,
    restart: bool,
) -> Result<Begun, String> {
    let asked = asked_environment(params)?;
    core.harness.change(|state| {
        let (task_id, mut spec) = launch_target(state, project, params, restart)?;
        if !restart {
            if let Some(existing) = state.runs.keyed(project, key, &task_id)? {
                return Ok(Begun::Existing(existing));
            }
            if task_id.starts_with(COMMAND_TASK_PREFIX) && spec.is_none() {
                spec = Some(state.earlier_command(project, &task_id)?);
            }
            if let Some(active) = state.runs.active(project, &task_id) {
                if let Some(Some(environment)) = &asked {
                    if active.environment.as_ref() != Some(environment) {
                        return Err(format!(
                            "{task_id} is already running on {}; call task_restart with environment {environment} to switch",
                            active.environment.as_deref().unwrap_or("its folder's environment")
                        ));
                    }
                }
                let execution_id = active.execution_id.clone();
                state
                    .runs
                    .remember_key(project, key, &task_id, &execution_id)?;
                return Ok(Begun::Existing(execution_id));
            }
        }
        let latest = restart
            .then(|| state.runs.latest(project, &task_id))
            .flatten();
        let previous = latest.map(|run| run.execution_id.clone());
        let environment = match &asked {
            Some(environment) => environment.clone(),
            None => latest.and_then(|run| run.environment.clone()),
        };
        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut run = Run::new(execution_id.clone(), task_id.clone(), project.to_owned());
        run.agent_id = agent_id.map(str::to_owned);
        run.environment = environment;
        if let Some(spec) = &spec {
            run.label = Some(spec.label.clone());
            run.command = Some(spec.command.clone());
        }
        run.launch_spec = spec;
        state.runs.insert(run)?;
        if let Err(error) = state
            .runs
            .remember_key(project, key, &task_id, &execution_id)
        {
            state.runs.remove(&execution_id);
            return Err(error);
        }
        state.journals.append(
            project,
            RunStatus::Starting.event_kind(),
            Some(&execution_id),
            Some(&task_id),
        );
        Ok(Begun::New {
            execution_id,
            previous,
        })
    })
}

/// The environment a start names: none when it names none, so a restart keeps
/// the last run's, and `Some(None)` when it asks for the folder's own.
fn asked_environment(params: &Value) -> Result<Option<Option<String>>, String> {
    match params.get("environment") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(name))
            if !name.trim().is_empty()
                && name.chars().count() <= 64
                && !name.contains(['/', '\\']) =>
        {
            Ok(Some(Some(name.trim().to_owned())))
        }
        Some(_) => Err("environment must be the name of one of the folder's environments, such as staging, or null for the folder's own".into()),
    }
}

/// Records how a launch ended.
fn launch_finished(core: &Core, execution_id: &str, result: Result<Option<String>, String>) {
    core.harness.change(|state| {
        let Some(run) = state.runs.by_execution_mut(execution_id) else {
            return;
        };
        let message = match result {
            Ok(preview_url) => {
                run.launch = Launch::Done;
                run.preview_url = preview_url;
                return;
            }
            Err(message) => message,
        };
        run.launch = Launch::Failed(message.clone());
        run.error.get_or_insert(message);
        if run.pty_id.is_none() && run.status.is_active() {
            state.set_status(execution_id, RunStatus::Failed);
        }
    });
}

/// Launches a run in the background. `checked` says the person already saw
/// what could stop it.
fn launch(core: &Arc<Core>, execution_id: String, previous: Option<String>, checked: bool) {
    let core = core.clone();
    tokio::spawn(async move {
        let result = super::tasks::launch(&core, &execution_id, previous, checked).await;
        launch_finished(&core, &execution_id, result);
    });
}

/// The person starts a task, in place of whatever else they run in that
/// project. Answers the new run's id.
pub(crate) fn start_for_person(
    core: &Arc<Core>,
    project: &str,
    task_id: &str,
    environment: Option<&str>,
) -> Result<String, String> {
    let params = json!({ "taskId": task_id, "environment": environment });
    let key = uuid::Uuid::new_v4().to_string();
    let Begun::New {
        execution_id,
        previous,
    } = begin_start(core, project, None, &params, &key, true)?
    else {
        return Err("The task did not start".into());
    };
    let others = core.harness.read(|state| {
        state.runs.matching(|run| {
            run.project == project
                && run.agent_id.is_none()
                && run.task_id != task_id
                && run.status.is_active()
        })
    });
    let stopping = core.clone();
    tokio::spawn(async move {
        for other in others {
            let _ = stop_run(&stopping, &other).await;
        }
    });
    launch(core, execution_id.clone(), previous, true);
    Ok(execution_id)
}

async fn task_start(
    core: &Arc<Core>,
    request: &HarnessRequest,
    restart: bool,
) -> Result<Value, String> {
    let until = Instant::now() + START_BUDGET;
    if request.agent_id.is_some() {
        if let Some(command) = text(&request.params, "command")? {
            super::guardrails::check_command(core, Path::new(&request.project), command)?;
        }
    }
    let (key, ready_when) = if restart {
        (uuid::Uuid::new_v4().to_string(), None)
    } else {
        let key = required_text(&request.params, "idempotencyKey")?;
        if key.chars().count() > 128 {
            return Err("idempotencyKey must be at most 128 characters".into());
        }
        (
            key.to_owned(),
            text(&request.params, "readyWhen")?.map(str::to_owned),
        )
    };
    let begun = begin_start(
        core,
        &request.project,
        request.agent_id.as_deref(),
        &request.params,
        &key,
        restart,
    )?;
    let execution_id = match begun {
        Begun::Existing(execution_id) => execution_id,
        Begun::New {
            execution_id,
            previous,
        } => {
            launch(core, execution_id.clone(), previous, false);
            execution_id
        }
    };
    answer_start(core, &request.project, &execution_id, until, ready_when).await
}

async fn answer_start(
    core: &Core,
    project: &str,
    execution_id: &str,
    until: Instant,
    ready_when: Option<String>,
) -> Result<Value, String> {
    let harness = &core.harness;
    loop {
        let mut changed = harness.changes.subscribe();
        let run = harness.read(|state| state.run(project, execution_id))?;
        match &run.launch {
            Launch::Failed(message) => return Err(message.clone()),
            Launch::Done => break,
            Launch::Pending => {}
        }
        if run.status == RunStatus::AwaitingTrust
            || !run.status.is_active()
            || Instant::now() >= until
        {
            break;
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(until) => {}
        }
    }
    let run = harness.read(|state| state.run(project, execution_id))?;
    let note = match run.status {
        RunStatus::AwaitingTrust => Some(AWAITING_TRUST_NOTE),
        RunStatus::Starting => Some(STARTING_NOTE),
        _ => None,
    };
    if let Some(note) = note {
        return Ok(match ready_when {
            Some(_) => with_fields(&run, json!({ "ready": false, "note": note })),
            None => with_fields(&run, json!({ "note": note })),
        });
    }
    let Some(pattern) = ready_when else {
        return Ok(with_fields(&run, json!({})));
    };
    let ready = output_appears(core, project, execution_id, &pattern, until).await?;
    let latest = harness.read(|state| state.run(project, execution_id))?;
    Ok(if ready || latest.status != RunStatus::Running {
        with_fields(&latest, json!({ "ready": ready }))
    } else {
        with_fields(&latest, json!({ "ready": false, "note": NOT_READY_NOTE }))
    })
}

async fn output_appears(
    core: &Core,
    project: &str,
    execution_id: &str,
    pattern: &str,
    until: Instant,
) -> Result<bool, String> {
    loop {
        let mut changed = core.harness.changes.subscribe();
        let run = core
            .harness
            .read(|state| state.run(project, execution_id))?;
        if let Some(pty_id) = run.pty_id {
            let query = OutputQuery {
                cursor: 0,
                limit: 4096,
                tail: Some(1),
                search: Some(pattern.to_owned()),
                context: 0,
                plain: false,
            };
            if read_output(core, pty_id, query)
                .await
                .is_ok_and(|page| page.matches.unwrap_or(0) > 0)
            {
                return Ok(true);
            }
        }
        if !matches!(run.status, RunStatus::Starting | RunStatus::Running)
            || Instant::now() >= until
        {
            return Ok(false);
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(until) => {}
        }
        tokio::time::sleep_until((Instant::now() + READY_POLL).min(until)).await;
    }
}

/// A plugin tool is named by the agent's tool name, anything else by its
/// method. Listing plugin tools is not a call.
fn tool_name(request: &HarnessRequest) -> Option<String> {
    match request.method.as_str() {
        "plugins.tools"
        | PLUGINS_CHANGED_METHOD
        | TOOLS_MANIFEST_METHOD
        | COMMIT_FOOTER_METHOD
        | GIT_GUARD_METHOD
        | SIM_OFFERED_METHOD
        | SIM_CANCEL_METHOD => None,
        "plugins.call" => request
            .params
            .get("tool")
            .and_then(Value::as_str)
            .map(str::to_owned),
        method => Some(method.to_owned()),
    }
}

#[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
struct Tally {
    calls: u64,
    failures: u64,
}

/// How often agents call each tool and how often a call fails, so the tool
/// surface can be trimmed by what is used. The tally stays on this machine.
fn record_tool_call(path: &Path, tool: &str, succeeded: bool) {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _held = ONE_AT_A_TIME.locked();
    if let Err(error) = add_tool_call(path, tool, succeeded) {
        eprintln!("sikemux core: could not count a call to {tool}: {error}");
    }
}

fn add_tool_call(path: &Path, tool: &str, succeeded: bool) -> std::io::Result<()> {
    let mut tallies: BTreeMap<String, Tally> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let tally = tallies.entry(tool.to_owned()).or_default();
    tally.calls += 1;
    if !succeeded {
        tally.failures += 1;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&tallies)?)
}

/// A run an agent started that ended with an error nobody asked for.
pub(crate) struct FailedRun {
    agent_id: String,
    execution_id: String,
    label: String,
    reason: String,
}

/// Tells the agent its run died, so a crashed dev server does not go unnoticed
/// while the agent works on something else.
pub(crate) fn tell_agent_run_failed(core: &Core, run: FailedRun) {
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    let text = format!(
        "Your task \"{}\" (executionId {}) stopped: {}. Read its output with task_read, fix the cause, and start it again with task_start.",
        run.label, run.execution_id, run.reason
    );
    runtime.spawn(async move {
        let _ = super::messages::from_sikemux(&core, run.agent_id, text).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skills_tool_creates_lists_and_removes_in_the_library() {
        let data =
            std::env::temp_dir().join(format!("sikemux-harness-skills-{}", uuid::Uuid::new_v4()));
        let answer = |params: Value| skills_answer(&data, &params);
        let created = answer(json!({
            "action": "create",
            "name": "deploy-preview",
            "description": "Deploys a preview",
            "body": "Run make preview.",
        }))
        .unwrap();
        assert_eq!(created["skill"]["name"], "deploy-preview");
        assert!(created["note"].as_str().unwrap().contains("/sikemux:"));

        let listed = answer(json!({ "action": "list" })).unwrap();
        assert_eq!(listed["skills"][0]["description"], "Deploys a preview");

        let refused = answer(json!({ "action": "create", "name": "Bad Name", "description": "x" }));
        assert!(refused.unwrap_err().contains("not a skill name"));
        assert!(answer(json!({ "action": "install", "source": "relative/path" })).is_err());
        assert!(answer(json!({ "action": "rename" })).is_err());

        answer(json!({ "action": "remove", "name": "deploy-preview" })).unwrap();
        assert_eq!(
            answer(json!({ "action": "list" })).unwrap()["skills"],
            json!([])
        );
        std::fs::remove_dir_all(&data).unwrap();
    }

    fn request(method: &str, params: Value) -> HarnessRequest {
        HarnessRequest {
            id: "1".into(),
            project: "/project".into(),
            agent_id: Some("agent".into()),
            method: method.into(),
            params,
        }
    }

    #[test]
    fn a_plugin_call_is_counted_under_the_tool_the_agent_named() {
        assert_eq!(
            tool_name(&request("plugins.call", json!({ "tool": "signoz_logs" }))).as_deref(),
            Some("signoz_logs")
        );
        assert_eq!(
            tool_name(&request("browser.navigate", json!({}))).as_deref(),
            Some("browser.navigate")
        );
        assert_eq!(tool_name(&request("plugins.tools", json!({}))), None);
        assert_eq!(tool_name(&request(PLUGINS_CHANGED_METHOD, json!({}))), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_plugin_wait_answers_at_once_unless_it_has_seen_the_latest_change() {
        let harness = Harness::new(None, |_| true);
        assert_eq!(
            plugins_wait(&harness, &json!({})).await,
            Ok(json!({ "version": 0 }))
        );
        harness.plugins_changed();
        assert_eq!(
            plugins_wait(&harness, &json!({ "seen": 0 })).await,
            Ok(json!({ "version": 1 }))
        );

        let seen = json!({ "seen": 1 });
        let waiting = plugins_wait(&harness, &seen);
        let change = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            harness.plugins_changed();
            harness.plugins_changed();
        };
        let started = Instant::now();
        let (answer, ()) = tokio::join!(waiting, change);
        assert_eq!(answer, Ok(json!({ "version": 3 })));
        assert_eq!(started.elapsed(), Duration::from_secs(5) + PLUGIN_SETTLE);

        let started = Instant::now();
        assert_eq!(
            plugins_wait(&harness, &json!({ "seen": 3 })).await,
            Ok(json!({ "version": 3 }))
        );
        assert_eq!(started.elapsed(), PLUGIN_WAIT);
        assert_eq!(harness.plugin_waits.load(Ordering::Acquire), 0);
    }

    #[test]
    fn calls_and_failures_add_up_across_writes() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("nested").join(TOOL_CALLS_FILE);
        add_tool_call(&path, "browser.click", true).expect("counts");
        add_tool_call(&path, "browser.click", false).expect("counts");
        add_tool_call(&path, "task.start", true).expect("counts");
        let tallies: BTreeMap<String, Tally> =
            serde_json::from_slice(&std::fs::read(&path).expect("written")).expect("parses");
        assert_eq!(
            tallies.get("browser.click"),
            Some(&Tally {
                calls: 2,
                failures: 1
            })
        );
        assert_eq!(tallies.get("task.start").map(|tally| tally.calls), Some(1));
    }

    #[test]
    fn task_ids_and_commands_are_resolved_before_anything_starts() {
        let harness = Harness::new(None, |_| true);
        let resolve = |params: Value, restart: bool| {
            harness.read(|state| launch_target(state, "/one", &params, restart))
        };
        let (task_id, spec) = resolve(
            json!({ "command": "pnpm dev", "cwd": "./web/", "label": "Web" }),
            false,
        )
        .unwrap();
        assert!(task_id.starts_with("sh:web-"));
        assert_eq!(spec.unwrap().cwd, "web");
        assert!(resolve(json!({ "taskId": "test", "command": "ls" }), false)
            .unwrap_err()
            .contains("not both"));
        assert!(resolve(json!({}), false)
            .unwrap_err()
            .contains("taskId or command"));
        assert!(resolve(json!({ "taskId": "test", "cwd": "web" }), false)
            .unwrap_err()
            .contains("go with command"));
        assert!(resolve(json!({ "command": "ls", "cwd": "../x" }), false)
            .unwrap_err()
            .contains("inside the project"));
        assert!(resolve(json!({ "taskId": "sh:never-abcdef" }), true)
            .unwrap_err()
            .contains("has not been started"));
        assert_eq!(
            resolve(json!({ "taskId": "dev" }), true).unwrap(),
            ("dev".into(), None)
        );
    }

    #[test]
    fn a_run_follows_its_session_and_says_how_it_ended() {
        let harness = Harness::new(None, |_| true);
        harness.change(|state| {
            state
                .runs
                .insert(Run::new("a".into(), "dev".into(), "/one".into()))
                .unwrap();
        });
        harness.awaiting_trust("a");
        let task = TaskSessionInfo {
            execution_id: "a".into(),
            terminal_key: "key".into(),
            task_id: "dev".into(),
            label: "Dev".into(),
            project: "/one".into(),
            source: sikemux_pty::task::TaskSource::Project,
            command: "pnpm dev".into(),
            cwd: "/one".into(),
            agent_id: None,
        };
        assert!(!harness.session_started(7, &task));
        harness.session_exited(7, Some(3), None);
        let (run, kinds) = harness.read(|state| {
            let run = state.run("/one", "a").unwrap();
            let kinds: Vec<String> = state
                .journals
                .since("/one", 0, Some("a"))
                .unwrap()
                .events
                .into_iter()
                .map(|entry| entry.kind)
                .collect();
            (run, kinds)
        });
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.exit_code, Some(3));
        assert_eq!(run.pty_id, Some(7));
        assert_eq!(
            kinds,
            ["task.awaiting-trust", "task.running", "task.failed"]
        );

        harness.change(|state| {
            state
                .runs
                .insert(Run::new("b".into(), "dev".into(), "/one".into()))
                .unwrap();
            state.set_status("b", RunStatus::Stopped);
        });
        let late = TaskSessionInfo {
            execution_id: "b".into(),
            ..task
        };
        assert!(
            harness.session_started(8, &late),
            "a stopped run's late session is killed"
        );
    }
}
