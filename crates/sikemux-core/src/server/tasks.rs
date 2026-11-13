//! The tasks a project offers and how the core starts them, for agents and
//! for the person alike: what `sikemux.json`, the build files and the
//! person's own list hold, whether the person trusts commands someone else
//! wrote, and what stops a task from starting.

use super::access::Peer;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::ServerCore;
pub use sikemux_calls::tasks::{ConfigStatus, JavaView, Source, TaskList, TaskView};
pub use sikemux_calls::tasks::{
    ProjectConfigRead, ProjectParams, RerunParams, RerunStarted, RunView, TrustAnswerParams,
    TrustProjectParams, TrustQuestion,
};
pub use sikemux_calls::tasks::{TaskStartParams, TaskStarted};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::time::Instant;

use super::changed::Topic;
use crate::harness::runs::Run;
use crate::launch::PermissionMode;
use crate::tasks::check::{self, Check, CheckItem, Checked, Listening};
use crate::tasks::config::{self, ConfigLoad, Stated, TrustCommand};
use crate::tasks::detect::{self, AddedTask, PERSONAL_PREFIX};
use crate::tasks::java::{self, JavaChoice};
use crate::tasks::results::{self, TestResults};
use crate::tasks::tools;
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};
use sikemux_wire::protocol::{SessionId, SpawnTarget};

use super::connection::blocking;
use super::kept::Kept;
use super::ports::{self, PortOwner};
use super::{session, Core, CoreResult};

mod build_errors;
mod test_results;
mod trust;
mod writing;

pub(crate) use test_results::module_tests;
use test_results::*;
use trust::*;
pub(crate) use trust::{answer_trust, ask_person, asks, config_needs, config_trusted};
pub use writing::*;

const TRUST_FILE: &str = "task-trust.json";
const MAX_TRUSTED: usize = 256;
const COLS: u16 = 120;
const ROWS: u16 = 30;
/// A start waits this long for the person to trust what it runs.
const TRUST_WAIT: Duration = Duration::from_secs(10 * 60);
const MAX_RERUNS: usize = 16;
const INSTALL_WAIT: Duration = Duration::from_secs(10 * 60);
const INSTALL_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// What an agent asked to find the project's processes is told, after the line naming the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedConfig {
    path: String,
    fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedTask {
    project: String,
    cwd: String,
    command: String,
}

/// Commands the person trusts on this host, newest last.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustRecord {
    configs: Vec<Granted<TrustedConfig>>,
    personal: Vec<Granted<TrustedTask>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Granted<T> {
    #[serde(flatten)]
    trusted: T,
    /// The device the person answered from, when it was not this host's own window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answered_by: Option<String>,
}

/// What a start asks the person before it runs commands someone else wrote.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustAsk {
    title: String,
    body: String,
    commands: Vec<TrustCommand>,
    confirm_label: String,
    #[serde(skip)]
    grant: Grant,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Asked {
    id: String,
    project: String,
    /// The run that waits on the answer, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<String>,
    #[serde(flatten)]
    ask: TrustAsk,
}

#[derive(Clone, Debug)]
enum Grant {
    Config(TrustedConfig),
    Personal(TrustedTask),
}

#[derive(Default)]
struct State {
    trust: TrustRecord,
    /// Questions waiting on the person, by id.
    asking: HashMap<String, Asked>,
    /// Results a finished test run left, by execution id.
    tests: HashMap<String, TestResults>,
    /// Ports a detected task's app moved to, by project and task id.
    moved: HashMap<(String, String), (String, u16)>,
    /// One-off reruns of failed tests, by project.
    reruns: HashMap<String, Vec<Definition>>,
}

pub(crate) struct Tasks {
    state: Mutex<State>,
    kept: Kept,
    pub(crate) setups: super::setup_check::Setups,
}

impl Tasks {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, TRUST_FILE);
        Self {
            state: Mutex::new(State {
                trust: kept.load(),
                ..State::default()
            }),
            kept,
            setups: super::setup_check::Setups::new(data_dir),
        }
    }

    fn trusts(&self, grant: &Grant) -> bool {
        let state = self.state.locked();
        match grant {
            Grant::Config(config) => state
                .trust
                .configs
                .iter()
                .any(|known| known.trusted == *config),
            Grant::Personal(task) => state
                .trust
                .personal
                .iter()
                .any(|known| known.trusted == *task),
        }
    }

    fn grant(&self, grant: Grant, answered_by: Option<String>) {
        if let Some(device) = &answered_by {
            match &grant {
                Grant::Config(config) => {
                    eprintln!("sikemux core: trusted {}, from {device}", config.path)
                }
                Grant::Personal(task) => eprintln!(
                    "sikemux core: trusted {} in {}, from {device}",
                    task.command, task.cwd
                ),
            }
        }
        let mut state = self.state.locked();
        let trust = &mut state.trust;
        match grant {
            Grant::Config(config) => {
                trust
                    .configs
                    .retain(|known| known.trusted.path != config.path);
                trust.configs.push(Granted {
                    trusted: config,
                    answered_by,
                });
                let over = trust.configs.len().saturating_sub(MAX_TRUSTED);
                trust.configs.drain(..over);
            }
            Grant::Personal(task) => {
                trust.personal.retain(|known| known.trusted != task);
                trust.personal.push(Granted {
                    trusted: task,
                    answered_by,
                });
                let over = trust.personal.len().saturating_sub(MAX_TRUSTED);
                trust.personal.drain(..over);
            }
        }
        self.kept.save(&state.trust);
    }

    /// A run of a test task ended; what it left is read once.
    pub(crate) fn tests_of(&self, execution_id: &str) -> Option<TestResults> {
        self.state.locked().tests.get(execution_id).cloned()
    }

    fn retain(&self, known: impl Fn(&str) -> bool) {
        let mut state = self.state.locked();
        state.tests.retain(|id, _| known(id));
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    id: String,
    label: String,
    command: String,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    source: Source,
    java: Option<JavaChoice>,
    /// The project's things only so many use at once that it holds while it runs.
    needs: Vec<String>,
}

impl Definition {
    fn checked(&self) -> Checked<'_> {
        Checked {
            command: &self.command,
            cwd: &self.cwd,
            env: &self.env,
        }
    }
}

/// Everything a project offers, configured tasks first.
struct Offered {
    config: ConfigLoad,
    tasks: Vec<Definition>,
}

fn offered(core: &Core, project: &Path) -> Offered {
    let config = config::load(project);
    let mut tasks: Vec<Definition> = match &config {
        ConfigLoad::Valid { config, .. } => config
            .tasks
            .iter()
            .map(|task| Definition {
                id: task.id.clone(),
                label: task.label.clone(),
                command: task.command.clone(),
                cwd: if task.cwd == "." {
                    project.to_path_buf()
                } else {
                    project.join(&task.cwd)
                },
                env: task.env.clone(),
                source: Source::Project,
                java: None,
                needs: task.needs.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let found: Vec<_> = detect::personal(project)
        .into_iter()
        .chain(detect::detected(project))
        .collect();
    let runtimes = found
        .iter()
        .any(|task| task.java.is_some())
        .then(java::runtimes);
    let project_key = project.to_string_lossy().into_owned();
    let state = core.tasks().state.locked();
    let mut found: Vec<Definition> = found
        .into_iter()
        .map(|task| {
            let source = if task.id.starts_with(PERSONAL_PREFIX) {
                Source::Personal
            } else {
                Source::Detected
            };
            let java = task
                .java
                .zip(runtimes.as_ref())
                .map(|(required, runtimes)| java::choose(required, runtimes));
            let mut env = Vec::new();
            if let Some(choice) = &java {
                if let Some(runtime) = &choice.runtime {
                    if choice.current.as_ref().map(|current| &current.home) != Some(&runtime.home) {
                        env.push(("JAVA_HOME".to_owned(), runtime.home.clone()));
                    }
                }
            }
            Definition {
                id: task.id,
                label: task.label,
                command: task.command,
                cwd: task.cwd,
                env,
                source,
                java,
                needs: Vec::new(),
            }
        })
        .chain(
            state
                .reruns
                .get(&project_key)
                .into_iter()
                .flatten()
                .cloned(),
        )
        .collect();
    for task in &mut found {
        if let Some((variable, port)) = state.moved.get(&(project_key.clone(), task.id.clone())) {
            task.env.retain(|(name, _)| name != variable);
            task.env.push((variable.clone(), port.to_string()));
        }
    }
    drop(state);
    found.sort_by_key(|task| detect::rank(&task.id));
    tasks.extend(found);
    Offered { config, tasks }
}

fn relative(project: &Path, path: &Path) -> String {
    match path.strip_prefix(project) {
        Ok(inside) if inside.as_os_str().is_empty() => ".".into(),
        Ok(inside) => inside.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

fn view(project: &Path, task: &Definition, run: Option<&Run>) -> TaskView {
    TaskView {
        running: run.is_some(),
        session_id: run.and_then(|run| run.pty_id),
        id: task.id.clone(),
        label: task.label.clone(),
        command: task.command.clone(),
        cwd: relative(project, &task.cwd),
        source: task.source,
        java: task.java.as_ref().map(|choice| JavaView {
            required: choice.required,
            runs_on: choice.runtime.as_ref().map(|runtime| runtime.major),
            current: choice.current.as_ref().map(|runtime| runtime.major),
        }),
    }
}

pub(crate) async fn project_config(
    core: &Arc<Core>,
    project: String,
) -> CoreResult<ProjectConfigRead> {
    let core = core.clone();
    blocking(move || {
        let load = config::load(Path::new(&project));
        let ask = config_needs(&core, &load);
        Ok(ProjectConfigRead {
            load,
            trusted: ask.is_none(),
            trust_ask: ask.map(serde_json::to_value).transpose()?,
        })
    })
    .await
}

/// Starts the preview command `project`'s `sikemux.json` checks in, in a terminal tab of its
/// own, once the person trusts the file. Answers the tab.
pub(crate) async fn start_preview(core: &Arc<Core>, project: String) -> CoreResult<String> {
    let info = core
        .workspaces()
        .project_info(&project)
        .ok_or("that project is not on this host")?;
    let config = {
        let path = info.path.clone();
        blocking(move || Ok(config::load(&path))).await?
    };
    let ConfigLoad::Valid { config, .. } = config else {
        return Err("sikemux.json is not valid".into());
    };
    let command = config
        .preview
        .as_ref()
        .and_then(Stated::given)
        .and_then(|preview| preview.command.clone())
        .filter(|command| !command.is_empty())
        .ok_or("sikemux.json has no preview command")?;
    if !config_trusted(core, &info.path, None).await {
        return Err("sikemux.json was not trusted".into());
    }
    super::spare::command_terminal(core, info.id, "Project preview".into(), command).await
}

/// The person trusts `sikemux.json` as it is now on this host.
pub(crate) async fn trust_project(
    core: &Arc<Core>,
    project: String,
    fingerprint: String,
    answered_by: Option<String>,
) -> CoreResult<()> {
    let core = core.clone();
    blocking(move || {
        let config = config::load(Path::new(&project));
        let ConfigLoad::Valid {
            path,
            fingerprint: now,
            ..
        } = config
        else {
            return Err("sikemux.json is not valid".into());
        };
        if now != fingerprint {
            return Err("sikemux.json changed; look at it again".into());
        }
        core.tasks().grant(
            Grant::Config(TrustedConfig { path, fingerprint }),
            answered_by,
        );
        core.harness().changed();
        core.changed(What::ReadIn(Topic::Tasks, Some(&project), None));
        Ok(())
    })
    .await
}

/// Everything the project can run, in the order the person picks from.
pub(crate) async fn list(core: &Arc<Core>, project: String) -> CoreResult<TaskList> {
    let core = core.clone();
    blocking(move || {
        let project = PathBuf::from(project);
        let runs = core.harness().all_runs();
        let person_run = |task: &Definition| {
            runs.iter().rev().find(|run| {
                run.agent_id.is_none()
                    && Path::new(&run.project) == project
                    && run.task_id == task.id
                    && run.status.is_active()
            })
        };
        let Offered { config, tasks } = offered(&core, &project);
        let trusted = config_needs(&core, &config).is_none();
        let shown = tasks
            .iter()
            .find(|task| person_run(task).is_some())
            .or(tasks.first())
            .map(|task| task.id.clone());
        Ok(TaskList {
            config_status: match &config {
                ConfigLoad::Absent { .. } => ConfigStatus::Absent,
                ConfigLoad::Invalid { .. } => ConfigStatus::Invalid,
                ConfigLoad::Valid { .. } => ConfigStatus::Valid,
            },
            config_errors: match &config {
                ConfigLoad::Invalid { errors, .. } => Some(errors.clone()),
                _ => None,
            },
            trusted,
            tasks: (tasks.iter())
                .map(|task| view(&project, task, person_run(task)))
                .collect(),
            default: shown,
            project,
        })
    })
    .await
}

/// What `workspace_inspect` tells an agent of the project from the core: its
/// tasks, the test results they left and what listens on its ports.
pub(crate) fn inspect(core: &Core, project: &Path) -> Value {
    let Offered { config, tasks } = offered(core, project);
    let listed: Vec<Value> = tasks
        .iter()
        .map(|task| {
            let mut entry = json!({
                "id": task.id,
                "label": task.label,
                "command": task.command,
                "cwd": relative(project, &task.cwd),
            });
            if let Some(object) = entry.as_object_mut() {
                let found_in = match task.source {
                    Source::Personal => Some(detect::PERSONAL_TASKS),
                    Source::Detected | Source::Rerun => Some("build files"),
                    Source::Project | Source::Command => None,
                };
                if let Some(found_in) = found_in {
                    object.insert("foundIn".into(), found_in.into());
                }
                if let Some(choice) = &task.java {
                    object.insert(
                        "java".into(),
                        json!({
                            "required": choice.required,
                            "runsOn": choice.runtime.as_ref().map(|runtime| runtime.major),
                        }),
                    );
                }
            }
            entry
        })
        .collect();
    let listening = ports::listening_now(core);
    let checkout =
        ports::checkout::here(core, project).map(|here| ports::checkout::about(&here, &listening));
    let ports: Vec<Value> = ports::for_project(core, listening, project)
        .into_iter()
        .map(|port| {
            let (kind, label) = match &port.listening.owner {
                PortOwner::Terminal { .. } => ("terminal", "terminal".to_owned()),
                PortOwner::Task { label, .. } => ("process", label.clone()),
                PortOwner::Agent { label, .. } => ("agent", label.clone()),
            };
            json!({
                "port": port.listening.port,
                "address": port.listening.address,
                "process": port.listening.process,
                "owner": { "kind": kind, "label": label },
                "checkout": port.checkout,
            })
        })
        .collect();
    let mut value = json!({
        "processes": listed,
        "configStatus": match &config {
            ConfigLoad::Absent { .. } => "absent",
            ConfigLoad::Invalid { .. } => "invalid",
            ConfigLoad::Valid { .. } => "valid",
        },
        "ports": ports,
    });
    if let Some(object) = value.as_object_mut() {
        if let ConfigLoad::Invalid { errors, .. } = &config {
            object.insert(
                "configErrors".into(),
                serde_json::to_value(errors).unwrap_or_default(),
            );
        }
        if let Some(checkout) = checkout {
            object.insert(
                "checkout".into(),
                serde_json::to_value(checkout).unwrap_or_default(),
            );
        }
        let found = test_results(project, &tasks);
        if !found.is_empty() {
            object.insert("testResults".into(), found.into());
        }
        let errors = build_errors::for_agent(project);
        if !errors.is_empty() {
            object.insert("buildErrors".into(), errors.into());
        }
    }
    value
}

/// The terminal a task's runs share, so a rerun shows in the same one.
pub(crate) fn terminal_key(project: &str, task_id: &str) -> String {
    json!(["task", project, task_id]).to_string()
}

/// Who listens on `port`, with runs of `task_id` counting as its own.
fn listening(listeners: &[ports::ListeningPort], task_id: &str, port: u16) -> Listening {
    if let Some(listener) = listeners.iter().find(|listener| listener.port == port) {
        return match &listener.owner {
            PortOwner::Task { task_id: owner, .. } if owner == task_id => Listening::Ours,
            _ => Listening::Held(format!(
                "{} (pid {})",
                if listener.process.is_empty() {
                    "a process"
                } else {
                    &listener.process
                },
                listener.pid
            )),
        };
    }
    if ports::in_use(port) {
        Listening::Held("another program on this computer".into())
    } else {
        Listening::Free
    }
}

fn checks(core: &Core, task: &Definition) -> Vec<Check> {
    let listeners = ports::listening_now(core);
    check::check(&task.checked(), task.java.as_ref(), |port| {
        listening(&listeners, &task.id, port)
    })
}

/// What a run starts, found again from what its project offers now.
fn resolve(
    core: &Core,
    project: &Path,
    task_id: &str,
    spec: Option<&crate::harness::runs::CommandLaunch>,
) -> Result<(Definition, ConfigLoad), String> {
    if let Some(spec) = spec {
        let cwd = if spec.cwd.is_empty() {
            project.to_path_buf()
        } else {
            project.join(&spec.cwd)
        };
        let definition = Definition {
            id: task_id.to_owned(),
            label: spec.label.clone(),
            command: spec.command.clone(),
            cwd,
            env: Vec::new(),
            source: Source::Command,
            java: None,
            needs: spec.needs.clone(),
        };
        return Ok((
            definition,
            ConfigLoad::Absent {
                path: String::new(),
            },
        ));
    }
    let Offered { config, tasks } = offered(core, project);
    if let Some(task) = tasks.into_iter().find(|task| task.id == task_id) {
        return Ok((task, config));
    }
    if task_id.starts_with(detect::DETECTED_PREFIX) || task_id.starts_with(PERSONAL_PREFIX) {
        return Err("No process with this id in the project's build files; call workspace_inspect for the current list".into());
    }
    Err(match config {
        ConfigLoad::Absent { .. } => {
            "Project has no sikemux.json; add one that defines processes".into()
        }
        ConfigLoad::Invalid { errors, .. } => config::invalid_words(&errors),
        ConfigLoad::Valid { .. } => "Process is not defined in sikemux.json".into(),
    })
}

async fn spawn(core: &Arc<Core>, request: TaskSpawnRequest) -> Result<SessionId, String> {
    let task = super::launching::spawn(
        core.clone(),
        core.launch_identity(),
        Box::new(SpawnTarget::Task { request }),
        None,
    )
    .await
    .map_err(|error| error.to_string())?;
    let id = task.id();
    let stopped = task
        .task()
        .is_some_and(|task| core.harness().session_started(id, task));
    task.start(core);
    if stopped {
        if let Some(target) = core.take_session_for_kill(id) {
            let killing = core.clone();
            let _ = blocking(move || {
                session::kill(&killing, &target);
                Ok(())
            })
            .await;
        }
    }
    Ok(id)
}

/// Starts the run the harness just made: finds what it runs, asks the person
/// to trust it when it must, checks it can start, and spawns it. `checked`
/// is set when the person already saw the checks pass or chose to run anyway.
/// Answers the run's preview URL.
pub(crate) async fn launch(
    core: &Arc<Core>,
    execution_id: &str,
    previous: Option<String>,
    checked: bool,
) -> Result<Option<String>, String> {
    let run = core
        .harness()
        .run_of(execution_id)
        .ok_or("The run is gone")?;
    let project = PathBuf::from(&run.project);
    let resolved = {
        let (core, project, task_id, spec) = (
            core.clone(),
            project.clone(),
            run.task_id.clone(),
            run.launch_spec.clone(),
        );
        blocking(move || Ok(resolve(&core, &project, &task_id, spec.as_ref())))
            .await
            .map_err(|error| error.to_string())??
    };
    let (mut task, config) = resolved;
    if !bypasses(core, run.agent_id.as_deref()) {
        if let Some(ask) = trust_for(core, &project, &task, &config) {
            if !ask_person(core, &run.project, ask, Some(execution_id)).await {
                return Err("Project configuration was not approved".into());
            }
            if let ConfigLoad::Valid { fingerprint, .. } = &config {
                let now = config::load(&project);
                if !matches!(&now, ConfigLoad::Valid { fingerprint: latest, .. } if latest == fingerprint)
                {
                    return Err("Project configuration changed; inspect and retry".into());
                }
            }
        }
    }
    if let Some(agent_id) = &run.agent_id {
        super::ship::check_process(core, agent_id, &task.command, &task.env, &task.cwd).await?;
    }
    let here = {
        let (core, cwd) = (core.clone(), task.cwd.clone());
        blocking(move || Ok(ports::checkout::here(&core, &cwd)))
            .await
            .map_err(|error| error.to_string())?
    };
    let mut preview_url = match &config {
        ConfigLoad::Valid { config, .. } if task.source == Source::Project => config
            .preview
            .as_ref()
            .and_then(Stated::given)
            .filter(|preview| preview.command.as_deref() == Some(task.command.as_str()))
            .and_then(|preview| preview.url.as_deref())
            .map(|url| {
                here.as_ref()
                    .map_or_else(|| url.to_owned(), |here| here.fill(url))
            }),
        _ => None,
    };
    if let Some(here) = here.as_ref().filter(|_| task.source == Source::Project) {
        task.env = here.task_environment(&task.id);
    }
    let own_port = (here.as_ref())
        .filter(|here| here.first_port.is_some() && task.source == Source::Project)
        .and_then(|here| here.task_port(&task.id));
    if let Some(port) = own_port {
        let (core, task_id) = (core.clone(), task.id.clone());
        let held = blocking(move || Ok(listening(&ports::listening_now(&core), &task_id, port)))
            .await
            .map_err(|error| error.to_string())?;
        if let Listening::Held(by) = held {
            return Err(format!(
                "{} can't start: this checkout's port {port} is taken by {by}",
                task.label
            ));
        }
    }
    let in_own_checkout = here.as_ref().is_some_and(|here| here.first_port.is_some());
    if !checked && task.source != Source::Command {
        let found = {
            let (core, task) = (core.clone(), task.clone());
            blocking(move || Ok(checks(&core, &task)))
                .await
                .map_err(|error| error.to_string())?
        };
        let moved = found.iter().find_map(|check| match &check.item {
            CheckItem::Port {
                ok: false,
                free_port: Some(port),
                env_var: Some(variable),
                ..
            } => Some((variable.clone(), *port)),
            _ => None,
        });
        let moved = moved.filter(|_| !in_own_checkout);
        let rest: Vec<Check> = found
            .into_iter()
            .filter(|check| match check.item {
                CheckItem::Port { .. } if in_own_checkout => false,
                CheckItem::Port {
                    free_port: Some(_),
                    env_var: Some(_),
                    ..
                } => false,
                _ => true,
            })
            .collect();
        if let Some(problems) = check::problems(&rest) {
            return Err(format!("{} can't start yet:\n{problems}", task.label));
        }
        if let Some((variable, port)) = moved {
            task.env.retain(|(name, _)| *name != variable);
            task.env.push((variable, port.to_string()));
            preview_url = Some(format!("http://localhost:{port}"));
        }
    }
    if let Some(previous) = previous {
        let _ = super::harness::stop_run(core, &previous).await;
    }
    if !task.needs.is_empty() {
        let needs = super::line::resolve(&run.project, &task.cwd, &task.needs)?;
        let agent_id = run.agent_id.clone();
        (core.line())
            .wait_for_needs(
                core,
                execution_id,
                agent_id,
                &run.project,
                &task.label,
                needs,
            )
            .await?;
    }
    let source = match task.source {
        Source::Project | Source::Command => TaskSource::Project,
        Source::Personal | Source::Detected | Source::Rerun => TaskSource::BuiltIn,
    };
    let mut env: std::collections::HashMap<String, String> = task.env.into_iter().collect();
    if let Some(path) = sikemux_process::user_environment::var_os("PATH") {
        env.entry("PATH".into())
            .or_insert_with(|| path.to_string_lossy().into_owned());
    }
    spawn(
        core,
        TaskSpawnRequest {
            execution_id: execution_id.to_owned(),
            terminal_key: terminal_key(&run.project, &run.task_id),
            task_id: run.task_id.clone(),
            label: task.label.clone(),
            project: run.project.clone(),
            source,
            command: task.command.clone(),
            cwd: task.cwd.to_string_lossy().into_owned(),
            env,
            cols: COLS,
            rows: ROWS,
            agent_id: run.agent_id.clone(),
        },
    )
    .await?;
    Ok(preview_url)
}

fn fix(core: &Core, project: &Path, task: &Definition, found: &[Check]) -> Result<(), String> {
    for check in found.iter().filter(|check| !check.item.ok()) {
        match &check.item {
            CheckItem::BuildTool { program, .. } => {
                tools::install(program)?;
            }
            CheckItem::Java { required, .. } => {
                java::install_java(*required)?;
            }
            CheckItem::Deps { install, .. } => {
                install_dependencies(&task.cwd, install)?;
            }
            CheckItem::Port {
                free_port: Some(port),
                env_var: Some(variable),
                ..
            } => {
                core.tasks().state.locked().moved.insert(
                    (project.to_string_lossy().into_owned(), task.id.clone()),
                    (variable.clone(), *port),
                );
            }
            CheckItem::Port { .. } => {}
        }
    }
    Ok(())
}

fn install_dependencies(cwd: &Path, install: &str) -> Result<(), String> {
    let mut words = install.split(' ');
    let program = words.next().ok_or("nothing to install with")?;
    let mut command = sikemux_process::user_environment::agent_command_in_own_cgroup(program);
    command
        .args(words)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null());
    let output = sikemux_process::run(&mut command, None, INSTALL_WAIT, INSTALL_OUTPUT_LIMIT, None)
        .map_err(|error| format!("{install} did not finish: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{install} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

pub(crate) async fn start(core: &Arc<Core>, params: TaskStartParams) -> CoreResult<TaskStarted> {
    let project = PathBuf::from(&params.project);
    if !params.force {
        let (core, project, task_id, fixing) = (
            core.clone(),
            project.clone(),
            params.task_id.clone(),
            params.fix,
        );
        let found = blocking(move || {
            let (task, _) = resolve(&core, &project, &task_id, None)?;
            if fixing {
                fix(&core, &project, &task, &checks(&core, &task))?;
            }
            let (task, _) = resolve(&core, &project, &task_id, None)?;
            Ok(checks(&core, &task))
        })
        .await?;
        if found.iter().any(|check| !check.item.ok()) {
            return Ok(TaskStarted::Held { checks: found });
        }
    }
    let execution_id = super::harness::start_for_person(core, &params.project, &params.task_id)?;
    Ok(TaskStarted::Running { execution_id })
}

/// Adds a task that runs only the tests a finished run failed, and starts it.
pub(crate) async fn rerun_failed(
    core: &Arc<Core>,
    params: RerunParams,
) -> CoreResult<RerunStarted> {
    let run = core
        .harness()
        .run_of(&params.execution_id)
        .ok_or("That run is gone")?;
    let failures = core
        .tasks()
        .tests_of(&params.execution_id)
        .map(|tests| tests.failures)
        .unwrap_or_default();
    let (core_ref, project, task_id) = (
        core.clone(),
        PathBuf::from(&run.project),
        run.task_id.clone(),
    );
    let rerun = blocking(move || {
        let (task, _) = resolve(&core_ref, &project, &task_id, None)?;
        let command = results::rerun_failed_command(&task.command, &failures)
            .ok_or("Sikemux cannot narrow this test command to the failed tests")?;
        let (base, folder) = match task.id.split_once(':') {
            Some((base, folder)) => (base, Some(folder)),
            None => (task.id.as_str(), None),
        };
        let id = match (base.ends_with(".failed"), folder) {
            (true, _) => task.id.clone(),
            (false, Some(folder)) => format!("{base}.failed:{folder}"),
            (false, None) => format!("{base}.failed"),
        };
        let label = if task.label.ends_with(" (failed only)") {
            task.label.clone()
        } else {
            format!("{} (failed only)", task.label)
        };
        Ok(Definition {
            id,
            label,
            command,
            source: Source::Rerun,
            ..task
        })
    })
    .await?;
    let task_id = rerun.id.clone();
    {
        let mut state = core.tasks().state.locked();
        let reruns = state.reruns.entry(run.project.clone()).or_default();
        reruns.retain(|known| known.id != task_id);
        reruns.push(rerun);
        let over = reruns.len().saturating_sub(MAX_RERUNS);
        reruns.drain(..over);
    }
    core.changed(What::ReadIn(Topic::Tasks, Some(&run.project), None));
    let execution_id = super::harness::start_for_person(core, &run.project, &task_id)?;
    Ok(RerunStarted {
        task_id,
        execution_id,
    })
}

/// Every run as clients show it, with what it waits on the person for and
/// the test results it left.
pub(crate) fn runs(core: &Core) -> Vec<RunView> {
    let runs = core.harness().all_runs();
    core.tasks()
        .retain(|id| runs.iter().any(|run| run.execution_id == id));
    let state = core.tasks().state.locked();
    runs.into_iter()
        .map(|run| RunView {
            agent_id: run.agent_id.clone(),
            terminal_key: terminal_key(&run.project, &run.task_id),
            tests: state.tests.get(&run.execution_id).cloned(),
            run,
        })
        .collect()
}

/// A task session ended: a Gradle build's errors go beside the language
/// servers' diagnostics, and a test task's results are read for the person to see.
pub(crate) fn ended(core: &Core, pty_id: SessionId) {
    let Some(run) = core.harness().run_on(pty_id) else {
        return;
    };
    let (Some(cwd), Some(core)) = (run.cwd.clone(), core.arc()) else {
        return;
    };
    let gradle = run.command.as_deref().is_some_and(detect::runs_gradle) && run.signal.is_none();
    let lines = if gradle {
        core.session(pty_id)
            .and_then(|session| session::task_lines(&session))
    } else {
        None
    };
    let tests = detect::is_test_task(&run.task_id);
    if lines.is_none() && !tests {
        return;
    }
    tokio::task::spawn_blocking(move || {
        if let Some(lines) = lines {
            build_errors::publish(&core, &run.project, Path::new(&cwd), &lines);
        }
        if !tests {
            return;
        }
        let started = run
            .started_at
            .map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms));
        if let Some(found) = results::read(Path::new(&cwd), started) {
            core.tasks()
                .state
                .locked()
                .tests
                .insert(run.execution_id, found);
            core.harness().changed();
        }
    });
}

/// Tells clients when a run changes, which they read again with `taskRuns`.
pub(crate) async fn publish_runs(core: Arc<Core>) {
    let mut changes = core.harness().subscribe();
    let mut last = Value::Array(Vec::new());
    while changes.changed().await.is_ok() {
        let now = serde_json::to_value(runs(&core)).unwrap_or_default();
        if now != last {
            last = now;
            core.changed(What::Read(Topic::Runs));
        }
    }
}

/// The name of the device the person acted from, when it was not this host's own window.
pub(crate) fn answered_by(core: &Core, peer: &Peer) -> Option<String> {
    let device = peer.device_id()?;
    Some(core.remote().device_name(&device).unwrap_or(device))
}

pub(crate) async fn trust_project_for(
    core: &Arc<Core>,
    peer: &Peer,
    params: TrustProjectParams,
) -> CoreResult<()> {
    trust_project(
        core,
        params.project,
        params.fingerprint,
        answered_by(core, peer),
    )
    .await
}

pub(crate) fn answer_trust_for(
    core: &Core,
    peer: &Peer,
    params: TrustAnswerParams,
) -> CoreResult<()> {
    answer_trust(core, &params.id, params.trust, answered_by(core, peer))
}

impl super::part::Part for Tasks {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(publish_runs(core.clone()))]
    }
}
