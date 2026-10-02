//! Agents' tool calls. The core answers the task tools and `events.wait` from
//! its own runs and journal, so they work while the window is closed, and
//! hands everything that needs the window to it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_pty::output_log::{OutputPage, OutputQuery};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::cli::protocol::{is_browser_method, is_plugin_method, HarnessRequest};
use crate::harness::command::{command_cwd, command_label, command_task_id, COMMAND_TASK_PREFIX};
use crate::harness::journal::{JournalRecord, Journals};
use crate::harness::runs::{CommandLaunch, Launch, Run, RunStatus, Runs, RunsRecord};
use crate::protocol::{RunSelector, SessionId, TaskSessionInfo, WindowCall};

use super::connection::blocking;
use super::window::{self, Call};
use super::{session, Core};

/// MCP hosts give up on a tool call after about a minute, so a start answers
/// well before that with whatever state it reached.
const START_BUDGET: Duration = Duration::from_secs(30);
/// A little longer than the app gives its page, so the app's own timeout,
/// which names the tool, is the one an agent sees.
const WINDOW_REPLY_TIMEOUT: Duration = Duration::from_secs(66);
/// A launch may wait on the person trusting `sikemux.json`.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A chatty task adds at most one output event per this long.
const OUTPUT_EVENT_GAP: Duration = Duration::from_millis(250);
const READY_POLL: Duration = Duration::from_millis(250);
const MAX_WAITS: usize = 32;
const MAX_WAIT_MS: u64 = 30_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const TOOL_CALLS_FILE: &str = "agent-tool-calls.json";
const WINDOW_GONE: &str = "Sikemux's window closed before it answered";

const WINDOW_CLOSED_NOTE: &str = "Sikemux's window is closed, so this lists only the runs you started and the event cursor. Panes, the tasks in sikemux.json and listening ports need the window open.";
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
    waits: AtomicUsize,
}

impl Harness {
    /// Journals and the tool-call tally go under `data_dir`; without one they
    /// live in memory, or not at all.
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            state: Mutex::new(State {
                runs: Runs::default(),
                journals: Journals::new(data_dir.map(|dir| dir.join("journal"))),
                output_pending: HashSet::new(),
            }),
            changes: watch::channel(0).0,
            tool_calls: data_dir.map(|dir| dir.join(TOOL_CALLS_FILE)),
            waits: AtomicUsize::new(0),
        }
    }

    /// Runs `change` on the state and wakes every waiter.
    fn change<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let result = change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.changes.send_modify(|version| *version += 1);
        result
    }

    fn read<T>(&self, read: impl FnOnce(&mut State) -> T) -> T {
        read(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
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
            if matches!(run.status, RunStatus::Starting | RunStatus::AwaitingTrust) {
                state.set_status(&task.execution_id, RunStatus::Running);
            }
            false
        })
    }

    /// A missing `code` means the status could not be read, which counts as a
    /// failure.
    pub(crate) fn session_exited(&self, id: SessionId, code: Option<u32>, signal: Option<String>) {
        self.change(|state| {
            let Some(run) = state.runs.by_pty_mut(id) else {
                return;
            };
            let code = code.unwrap_or(1);
            run.exit_code = Some(code);
            run.signal = signal;
            let next = match run.status {
                RunStatus::Stopped => return,
                RunStatus::Stopping => RunStatus::Stopped,
                _ if code == 0 => RunStatus::Completed,
                _ => RunStatus::Failed,
            };
            let execution_id = run.execution_id.clone();
            state.set_status(&execution_id, next);
        });
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
        "task.start" | "task.restart" => "start tasks",
        method if is_browser_method(method) => "use the browser",
        method if is_plugin_method(method) => "use plugin tools",
        _ => "use this tool",
    }
}

fn no_answer(method: &str) -> String {
    format!(
        "{} got no answer from Sikemux within {} s. Check that the Sikemux window is open and responsive, then retry.",
        method.replace('.', "_"),
        WINDOW_REPLY_TIMEOUT.as_secs()
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
    let method = request.method.clone();
    let call = core
        .window
        .call(WindowCall::Harness { request }, purpose(&method), false)?;
    answer_of(core, call, WINDOW_REPLY_TIMEOUT, no_answer(&method)).await
}

/// Answers one tool call. The call is counted whether or not it succeeds.
pub(crate) async fn call(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    request.validate()?;
    let tool = tool_name(&request);
    let result = dispatch(core, request).await;
    if let (Some(tool), Some(path)) = (tool, core.harness.tool_calls.clone()) {
        let succeeded = result.is_ok();
        tokio::task::spawn_blocking(move || record_tool_call(&path, &tool, succeeded));
    }
    result
}

async fn dispatch(core: &Arc<Core>, mut request: HarnessRequest) -> Result<Value, String> {
    let project = request.project.clone();
    request.project = tokio::task::spawn_blocking(move || std::fs::canonicalize(project))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
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
        "task.start" => task_start(core, &request, false).await,
        "task.restart" => task_start(core, &request, true).await,
        "workspace.inspect" => {
            let project = request.project.clone();
            let agent_id = request.agent_id.clone();
            let closed = || {
                json!({
                    "project": project,
                    "agentId": agent_id,
                    "window": null,
                    "note": WINDOW_CLOSED_NOTE,
                })
            };
            let mut value = if core.window.is_open() {
                match forward(core, request).await {
                    Ok(value) => value,
                    Err(_) if !core.window.is_open() => closed(),
                    Err(message) => return Err(message),
                }
            } else {
                closed()
            };
            if let Some(object) = value.as_object_mut() {
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
        None => false,
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

struct WaitSlot<'a>(&'a AtomicUsize);

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
    if harness.waits.fetch_add(1, Ordering::AcqRel) >= MAX_WAITS {
        harness.waits.fetch_sub(1, Ordering::AcqRel);
        return Err("Too many event waits".into());
    }
    let _slot = WaitSlot(&harness.waits);
    let deadline = Instant::now() + Duration::from_millis(timeout);
    loop {
        let mut changed = harness.changes.subscribe();
        let page =
            harness.read(|state| state.journals.since(&request.project, after, execution_id))?;
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

/// Stops one run and waits for its process to go.
async fn stop_run(core: &Arc<Core>, execution_id: &str) -> Result<Run, String> {
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

/// Stops the active runs a closed project or agent leaves behind, or one the
/// window replaces.
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
        forwarded: HarnessRequest,
    },
}

fn begin_start(
    core: &Core,
    request: &HarnessRequest,
    key: &str,
    restart: bool,
) -> Result<Begun, String> {
    let project = request.project.as_str();
    core.harness.change(|state| {
        let (task_id, mut spec) = launch_target(state, project, &request.params, restart)?;
        if !restart {
            if let Some(existing) = state.runs.keyed(project, key, &task_id)? {
                return Ok(Begun::Existing(existing));
            }
            if task_id.starts_with(COMMAND_TASK_PREFIX) && spec.is_none() {
                spec = Some(state.earlier_command(project, &task_id)?);
            }
            if let Some(active) = state.runs.active(project, &task_id) {
                let execution_id = active.execution_id.clone();
                state
                    .runs
                    .remember_key(project, key, &task_id, &execution_id)?;
                return Ok(Begun::Existing(execution_id));
            }
        }
        if !core.window.is_open() {
            return Err(window::not_open("start tasks"));
        }
        let previous = restart
            .then(|| state.runs.latest(project, &task_id))
            .flatten()
            .map(|run| run.execution_id.clone());
        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut run = Run::new(execution_id.clone(), task_id.clone(), project.to_owned());
        run.agent_id = request.agent_id.clone();
        let mut params = json!({ "executionId": execution_id, "taskId": task_id });
        if let (Some(spec), Some(fields)) = (spec.as_ref(), params.as_object_mut()) {
            let cwd = if spec.cwd.is_empty() {
                PathBuf::from(project)
            } else {
                Path::new(project).join(&spec.cwd)
            };
            fields.insert("command".into(), spec.command.clone().into());
            fields.insert("cwd".into(), cwd.to_string_lossy().into_owned().into());
            fields.insert("label".into(), spec.label.clone().into());
            run.label = Some(spec.label.clone());
            run.command = Some(spec.command.clone());
        }
        if let (Some(previous), Some(fields)) = (previous, params.as_object_mut()) {
            fields.insert("previousExecutionId".into(), previous.into());
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
            forwarded: HarnessRequest {
                id: request.id.clone(),
                project: project.to_owned(),
                agent_id: request.agent_id.clone(),
                method: "task.start".into(),
                params,
            },
        })
    })
}

/// Records how the window's launch ended. A window that went away after the
/// task started leaves it running.
fn launch_finished(core: &Core, execution_id: &str, result: Result<Value, String>) {
    core.harness.change(|state| {
        let Some(run) = state.runs.by_execution_mut(execution_id) else {
            return;
        };
        let message = match result {
            Ok(value) => {
                run.launch = Launch::Done;
                run.preview_url = value
                    .get("previewUrl")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                return;
            }
            Err(message) => message,
        };
        if run.pty_id.is_some() && run.status == RunStatus::Running && message == WINDOW_GONE {
            run.launch = Launch::Done;
            return;
        }
        run.launch = Launch::Failed(message.clone());
        run.error.get_or_insert(message);
        if run.pty_id.is_none() && run.status.is_active() {
            state.set_status(execution_id, RunStatus::Failed);
        }
    });
}

async fn task_start(
    core: &Arc<Core>,
    request: &HarnessRequest,
    restart: bool,
) -> Result<Value, String> {
    let until = Instant::now() + START_BUDGET;
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
    let execution_id = match begin_start(core, request, &key, restart)? {
        Begun::Existing(execution_id) => execution_id,
        Begun::New {
            execution_id,
            forwarded,
        } => {
            match core.window.call(
                WindowCall::Harness { request: forwarded },
                "start tasks",
                false,
            ) {
                Ok(call) => {
                    let following = core.clone();
                    let id = execution_id.clone();
                    tokio::spawn(async move {
                        let result = answer_of(
                            &following,
                            call,
                            LAUNCH_TIMEOUT,
                            "Sikemux's window did not finish starting the task within 10 minutes"
                                .into(),
                        )
                        .await;
                        launch_finished(&following, &id, result);
                    });
                }
                Err(message) => launch_finished(core, &execution_id, Err(message)),
            }
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
        "plugins.tools" => None,
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
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let harness = Harness::new(None);
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
        let harness = Harness::new(None);
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
