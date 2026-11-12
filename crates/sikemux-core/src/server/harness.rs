//! Agents' tool calls. The core answers the process tools and `events.wait` from
//! its own runs and journal, so they work while the window is closed, and
//! hands everything that needs the window to it.

use super::{CoreError, CoreResult};
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
    validate_harness, HarnessRequest, COMMIT_FOOTER_METHOD, COMMIT_MADE_METHOD, CORE_REPLY_TIMEOUT,
    GIT_GUARD_METHOD, PLUGINS_CHANGED_METHOD, SHIP_CHECK_METHOD, SHIP_RAN_METHOD,
    TOOLS_MANIFEST_METHOD,
};
use crate::harness::command::{command_cwd, command_label, command_task_id, COMMAND_TASK_PREFIX};
use crate::harness::journal::{EventsPage, JournalRecord, Journals};
use crate::harness::runs::{CommandLaunch, Launch, Run, RunStatus, Runs, RunsRecord};
use sikemux_view::layout::{Pane, PaneKind};
use sikemux_wire::protocol::{SessionId, TaskSessionInfo, WindowCall};

use super::connection::blocking;
use super::messages::{self, Mailbox};
use super::questions::{self, Questions};
use super::window::Call;
use super::{board, follow, saved, spawn};
use super::{session, Core};
use crate::skills;

mod inspect;
mod methods;
pub mod pages;
mod processes;
mod skills_tool;
mod tally;
mod waits;

use inspect::*;
use methods::tool_word;
pub(crate) use pages::read_page;
use pages::*;
pub use processes::RunSelector;
use processes::*;
pub(crate) use processes::{
    start_for_person, start_served, stop_run, stop_runs, tell_agent_run_failed, FailedRun,
};
use skills_tool::*;
use tally::*;
pub(crate) use waits::WaitSlot;
use waits::*;

/// MCP hosts give up on a tool call after about a minute, so a start answers
/// well before that with whatever state it reached.
const START_BUDGET: Duration = Duration::from_secs(30);
/// A chatty task adds at most one output event per this long.
const OUTPUT_EVENT_GAP: Duration = Duration::from_millis(250);
const READY_POLL: Duration = Duration::from_millis(250);
const MAX_WAITS: usize = 32;
const MAX_WAIT_MS: u64 = 50_000;
const RUN_TAIL_LINES: usize = 20;
/// Every running agent's tools server holds one of these open.
const MAX_PLUGIN_WAITS: usize = 512;
/// Shorter than the tools server waits for any answer.
const PLUGIN_WAIT: Duration = Duration::from_secs(55);
/// A pane that makes several calls in a row wakes each tools server once.
const PLUGIN_SETTLE: Duration = Duration::from_secs(1);
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const TOOL_CALLS_FILE: &str = "agent-tool-calls.json";
const WINDOW_GONE: &str = "Sikemux's window closed before it answered";
/// What agents do goes in the same journal as tasks, for `agent_wait` alone.
pub(crate) const AGENT_EVENT: &str = "agent.";

/// What `workspace_inspect` takes from the window: the panes it shows.
const LAYOUT_FIELDS: [&str; 4] = ["sessionId", "active", "activeWindowId", "windows"];
const NO_WINDOW_NOTE: &str =
    "No Sikemux window is open on this host; any tabs listed are the ones it shows when it opens.";
const AWAITING_TRUST_NOTE: &str = "Waiting for the person to trust this project's sikemux.json in Sikemux. Call process_start again with the same idempotencyKey, or events_wait with this executionId, to see when it starts.";
const STARTING_NOTE: &str = "Still starting. Call process_start again with the same idempotencyKey, or events_wait with this executionId, to see when it runs.";
const NOT_READY_NOTE: &str = "The process is running but readyWhen has not appeared yet. Wait with events_wait on this executionId and until ready.";

const LAUNCH_CUT_SHORT: &str = "Sikemux's background process was updated while the window was starting this process; start it again";

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

    /// The run a call names by `executionId`, or by `processId` for its latest.
    fn execution_for(&mut self, project: &str, params: &Value) -> Result<String, String> {
        let execution_id = text(params, "executionId")?;
        let task_id = text(params, "processId")?;
        match (execution_id, task_id) {
            (Some(_), Some(_)) => Err("Pass either executionId or processId, not both".into()),
            (Some(execution_id), None) => Ok(execution_id.to_owned()),
            (None, None) => Err("executionId or processId is required".into()),
            (None, Some(task_id)) => {
                if let Some(run) = self.runs.latest(project, task_id) {
                    return Ok(run.execution_id.clone());
                }
                if self.journals.mentions_task(project, task_id) {
                    return Err(format!("Process {task_id} was started earlier, but its run is gone: it ended a while ago or was stopped with Quit and Stop Everything. Start it again with process_start."));
                }
                Err(format!("Process {task_id} has not been started; call workspace_inspect to see the runs Sikemux knows about, or start it with process_start"))
            }
        }
    }

    fn earlier_command(&mut self, project: &str, task_id: &str) -> Result<CommandLaunch, String> {
        if let Some(spec) = self.runs.launch_spec(project, task_id) {
            return Ok(spec.clone());
        }
        if self.journals.mentions_task(project, task_id) {
            return Err(format!("Process {task_id} was started earlier, but its run and command are gone; start it again with command"));
        }
        Err(format!(
            "Process {task_id} has not been started; start it with command"
        ))
    }
}

pub(crate) struct Harness {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    tool_calls: Option<PathBuf>,
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
                    project: run.project.clone(),
                    pty_id: id,
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
                .append(&project, "process.output", Some(&execution_id), None);
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

/// A run as agents see it: clients call its process a task, agents a process.
fn for_agent(run: &Run) -> Value {
    let mut value = serde_json::to_value(run).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.remove("taskId");
        object.insert("processId".into(), run.task_id.clone().into());
    }
    value
}

fn with_fields(run: &Run, fields: Value) -> Value {
    let mut value = for_agent(run);
    if let (Some(object), Value::Object(fields)) = (value.as_object_mut(), fields) {
        object.extend(fields);
    }
    value
}

fn no_answer(method: &str) -> String {
    format!(
        "{} got no answer from Sikemux within {} s. Check that the Sikemux window is open and responsive, then retry.",
        tool_word(method),
        CORE_REPLY_TIMEOUT.as_secs()
    )
}

pub(crate) async fn answer_of(
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

pub(crate) async fn forward(core: &Core, request: HarnessRequest) -> Result<Value, String> {
    let method = methods::of(&request.method);
    let pinned = request.agent_id.as_deref().filter(|_| method.pinned);
    let call = core.window.call(
        WindowCall::Harness {
            request: serde_json::to_value(&request).map_err(|error| error.to_string())?,
        },
        method.purpose,
        false,
        pinned,
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
    if request.agent_id.is_none() && grant.is_none() && methods::of(&request.method).as_agent {
        return Err(format!(
            "{} works only for an agent Sikemux started, and this call came from outside every agent's processes; start this agent from Sikemux again",
            tool_word(&request.method)
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
        messages::used_tool(core, agent_id, &tool_word(&request.method));
        if core.names.is_blind(agent_id) && methods::of(&request.method).blind_refused {
            return Err(format!(
                "You review a card blind for Auto, so {} is not yours to use; judge the change from the card, the diff and your folder",
                tool_word(&request.method)
            ));
        }
    }
    match request.method.as_str() {
        "process.read" => process_read(core, &request).await,
        "process.stop" => {
            let execution_id = core
                .harness
                .read(|state| state.execution_for(&request.project, &request.params))?;
            core.harness
                .read(|state| state.run(&request.project, &execution_id))?;
            stop_run(core, &execution_id)
                .await
                .map(|run| with_fields(&run, json!({})))
        }
        "events.wait" if super::wait_for::asked(&request.params) => {
            let _slot = core.harness.wait_slot()?;
            super::wait_for::call(&request).await
        }
        "events.wait" => events_wait(core, &request).await,
        "agent.list" => super::roster::list(core, &request).await,
        "agent.send" => messages::send(core, &request).await,
        "agent.spawn" => spawn::spawn(core, &request).await,
        "agent.sessions" => saved::find(core, &request).await,
        "agent.ask" => questions::ask(core, &request).await,
        "agent.read" => follow::read(core, &request).await,
        "agent.wait" => follow::wait(core, &request).await,
        "agent.close" => super::roster::close(core, &request).await,
        "agent.wrote" => super::who_wrote::tool(core, &request).await,
        "board.add" => board::add(core, &request).await,
        "board.list" => board::list(core, &request).await,
        "board.claim" => board::claim(core, &request).await,
        "board.update" => board::update(core, &request).await,
        "board.decide" => board::goal::tool(core, &request).await,
        "board.assert" => board::goal::tool_assert(core, &request).await,
        "review.verdict" => board::verdict(core, &request).await,
        "memory.add" => super::memory::tool_add(core, &request).await,
        "memory.update" => super::memory::tool_update(core, &request).await,
        "memory.list" => super::memory::tool_list(core, &request).await,
        "chat.gif" => super::gestures::gif(core, &request).await,
        "trigger.add" => super::triggers::add(core, &request).await,
        "trigger.list" => super::triggers::list(core, &request),
        "trigger.remove" => super::triggers::remove_by_agent(core, &request),
        "git.commit" => super::edits::commit::call(core, &request).await,
        COMMIT_FOOTER_METHOD => super::edits::footer::call(core, &request, grant).await,
        COMMIT_MADE_METHOD => super::edits::footer::made(core, &request).await,
        GIT_GUARD_METHOD => super::edits::guard::call(core, &request).await,
        SHIP_CHECK_METHOD => super::ship::call(core, &request).await,
        SHIP_RAN_METHOD => super::ship::ran_call(core, &request),
        "browser.screenshot" => {
            let answer = forward(core, request.clone()).await?;
            if let Some(agent_id) = request.agent_id.clone() {
                let shots = board::slice_screenshots(core, &request, &agent_id, &answer).await;
                let core = core.clone();
                tokio::task::spawn_blocking(move || {
                    board::screenshot_taken(&core, &agent_id, &shots);
                });
            }
            Ok(answer)
        }
        "browser.evaluate" => {
            let project = request.project.clone();
            let knowing = core.clone();
            let sites = tokio::task::spawn_blocking(move || {
                let project = knowing.workspaces.project_of(Path::new(&project));
                super::places::allowed_sites(&knowing, &project)
            })
            .await
            .map_err(|error| error.to_string())?;
            if let Value::Object(params) = &mut request.params {
                params.insert("allowedSites".to_owned(), json!(sites));
            }
            forward(core, request).await
        }
        "browser.record" => {
            let agent_id = request.agent_id.clone();
            let answer = forward(core, request).await?;
            if let Some(agent_id) = agent_id {
                super::inbox::recorded(core, &agent_id, &answer);
            }
            Ok(answer)
        }
        PLUGINS_CHANGED_METHOD => plugins_wait(&core.harness, &request.params).await,
        "plugins.tools" => super::plugins::agent_tools(core, &request.project).await,
        "plugins.call" => super::plugins::call_agent_tool(core, &request).await,
        "skills.manage" => skills_call(core, &request).await,
        "process.start" => process_start(core, &request, false).await,
        "process.restart" => process_start(core, &request, true).await,
        "workspace.inspect" => inspect(core, request).await,
        "page.show" => show_page(core, request).await,
        "ui.open" => ui_open(core, request).await,
        _ => forward(core, request).await,
    }
}

async fn read_output(core: &Core, id: SessionId, query: OutputQuery) -> Result<OutputPage, String> {
    let session = core
        .session(id)
        .ok_or("Process output expired or the process no longer exists")?;
    blocking(move || session::task_output(&session, &query))
        .await
        .map_err(|error| error.to_string())
}

async fn process_read(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
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

impl super::part::Part for Harness {
    fn flush(&self) {
        self.mailbox.flush();
    }

    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        use super::part::on_wake;
        vec![
            on_wake(
                core,
                |core| &core.harness.mailbox.wake,
                super::messages::settle,
            ),
            on_wake(
                core,
                |core| &core.harness.questions.wake,
                super::questions::settle,
            ),
        ]
    }
}

/// Opens what the tool names on the agent's desk, each kind with what the window needs of it.
async fn ui_open(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    let mut request = request;
    let kind = text(&request.params, "kind")?.map(str::to_owned);
    if kind.as_deref() == Some("file") {
        let path = required_text(&request.params, "path")?.to_owned();
        let project = request.project.clone();
        let resolved =
            blocking(move || Ok(sikemux_process::files::confine_to_project(&project, &path)))
                .await
                .map_err(|error| error.to_string())?;
        match resolved {
            Ok(Some(path)) if path.is_file() => {
                request.params["path"] = path.to_string_lossy().into_owned().into();
            }
            _ => return Err("File must be inside the project".into()),
        }
    }
    if kind.as_deref() == Some("terminal") {
        let execution_id = required_text(&request.params, "executionId")?;
        let run = core
            .harness
            .read(|state| state.run(&request.project, execution_id))?;
        if let Some(session) = run.pty_id {
            request.params["session"] = session.into();
            request.params["title"] = run.label.unwrap_or(run.task_id).into();
        }
    }
    if kind.as_deref() == Some("mod") {
        let id = required_text(&request.params, "mod")?.to_owned();
        request.params["title"] = super::mods::title(core, &id).await?.into();
    }
    if kind.as_deref() == Some("preview") {
        if request.agent_id.is_none() {
            return Err("Opening a preview requires a Sikemux agent session".into());
        }
        let project = PathBuf::from(&request.project);
        let config = blocking(move || Ok(crate::tasks::config::load(&project)))
            .await
            .map_err(|error| error.to_string())?;
        let url = match config {
            crate::tasks::config::ConfigLoad::Valid { config, .. } => config
                .preview
                .as_ref()
                .and_then(crate::tasks::config::Stated::given)
                .and_then(|preview| preview.url.clone()),
            _ => None,
        };
        let url = url.ok_or("No preview URL is configured in sikemux.json")?;
        request.params["url"] = url.into();
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
        add_tool_call(&path, "process.start", true).expect("counts");
        let tallies: BTreeMap<String, Tally> =
            serde_json::from_slice(&std::fs::read(&path).expect("written")).expect("parses");
        assert_eq!(
            tallies.get("browser.click"),
            Some(&Tally {
                calls: 2,
                failures: 1
            })
        );
        assert_eq!(
            tallies.get("process.start").map(|tally| tally.calls),
            Some(1)
        );
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
        assert!(
            resolve(json!({ "processId": "test", "command": "ls" }), false)
                .unwrap_err()
                .contains("not both")
        );
        assert!(resolve(json!({}), false)
            .unwrap_err()
            .contains("processId or command"));
        assert!(resolve(json!({ "processId": "test", "cwd": "web" }), false)
            .unwrap_err()
            .contains("go with command"));
        assert!(resolve(json!({ "command": "ls", "cwd": "../x" }), false)
            .unwrap_err()
            .contains("inside the project"));
        assert!(resolve(json!({ "processId": "sh:never-abcdef" }), true)
            .unwrap_err()
            .contains("has not been started"));
        assert_eq!(
            resolve(json!({ "processId": "dev" }), true).unwrap(),
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
            [
                "process.awaiting-trust",
                "process.running",
                "process.failed"
            ]
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
