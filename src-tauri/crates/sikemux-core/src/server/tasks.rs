//! The tasks a project offers and how the core starts them, for agents and
//! for the person alike: what `sikemux.json`, the build files and the
//! person's own list hold, whether the person trusts commands someone else
//! wrote, and what stops a task from starting.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::protocol::{SessionId, SpawnTarget};
use crate::tasks::check::{self, Check, CheckItem, Checked, Listening};
use crate::tasks::config::{self, ConfigLoad, TrustCommand};
use crate::tasks::detect::{self, AddedTask, PERSONAL_PREFIX};
use crate::tasks::java::{self, JavaChoice};
use crate::tasks::results::{self, TestResults};
use crate::tasks::tools;
use sikemux_pty::task::{TaskSource, TaskSpawnRequest};

use super::connection::blocking;
use super::kept::Kept;
use super::ports::{self, PortOwner};
use super::{session, Core, CoreResult};

const TRUST_FILE: &str = "task-trust.json";
const MAX_TRUSTED: usize = 256;
const COLS: u16 = 120;
const ROWS: u16 = 30;
/// A start waits this long for the person to trust what it runs.
const TRUST_WAIT: Duration = Duration::from_secs(10 * 60);
const MAX_RERUNS: usize = 16;
const INSTALL_WAIT: Duration = Duration::from_secs(10 * 60);

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
    configs: Vec<TrustedConfig>,
    personal: Vec<TrustedTask>,
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

/// A trust question put to the person.
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
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn trusts(&self, grant: &Grant) -> bool {
        let state = self.lock();
        match grant {
            Grant::Config(config) => state.trust.configs.contains(config),
            Grant::Personal(task) => state.trust.personal.contains(task),
        }
    }

    fn grant(&self, grant: Grant) {
        let mut state = self.lock();
        let trust = &mut state.trust;
        match grant {
            Grant::Config(config) => {
                trust.configs.retain(|known| known.path != config.path);
                trust.configs.push(config);
                let over = trust.configs.len().saturating_sub(MAX_TRUSTED);
                trust.configs.drain(..over);
            }
            Grant::Personal(task) => {
                trust.personal.retain(|known| *known != task);
                trust.personal.push(task);
                let over = trust.personal.len().saturating_sub(MAX_TRUSTED);
                trust.personal.drain(..over);
            }
        }
        self.kept.save(&state.trust);
    }

    /// Forgets the person trusted the project file at `path`, so its commands ask again.
    fn revoke(&self, path: &str) {
        let mut state = self.lock();
        state.trust.configs.retain(|known| known.path != path);
        self.kept.save(&state.trust);
    }

    /// A run of a test task ended; what it left is read once.
    pub(crate) fn tests_of(&self, execution_id: &str) -> Option<TestResults> {
        self.lock().tests.get(execution_id).cloned()
    }

    /// Forgets what is kept for runs no longer known.
    fn retain(&self, known: impl Fn(&str) -> bool) {
        let mut state = self.lock();
        state.tests.retain(|id, _| known(id));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Source {
    /// From `sikemux.json`.
    Project,
    /// From the person's `.sikemux/tasks.json`.
    Personal,
    /// Suggested by the build files.
    Detected,
    /// A failed test run narrowed to its failures.
    Rerun,
    /// A shell command an agent gave.
    Command,
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
    let own_java: HashSet<PathBuf> = found
        .iter()
        .filter(|task| task.java.is_some())
        .map(|task| task.cwd.clone())
        .filter(|cwd| {
            core.workspaces
                .folder_environment(cwd, false)
                .contains_key("JAVA_HOME")
        })
        .collect();
    let project_key = project.to_string_lossy().into_owned();
    let state = core.tasks.lock();
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
                .filter(|_| !own_java.contains(&task.cwd))
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JavaView {
    required: u32,
    runs_on: Option<u32>,
    current: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskView {
    id: String,
    label: String,
    command: String,
    /// Relative to the project, `.` for its root.
    cwd: String,
    source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    java: Option<JavaView>,
}

fn view(project: &Path, task: &Definition) -> TaskView {
    TaskView {
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

fn config_ask(path: &str, fingerprint: &str, config: &config::ProjectConfig) -> TrustAsk {
    TrustAsk {
        title: "Trust this sikemux.json?".into(),
        body: "It lets Sikemux run these commands in this project. Trust lasts until the file changes.".into(),
        commands: config::trust_commands(config),
        confirm_label: "Trust project".into(),
        grant: Grant::Config(TrustedConfig {
            path: path.to_owned(),
            fingerprint: fingerprint.to_owned(),
        }),
    }
}

/// What the person must trust before `config` runs, or none.
pub(crate) fn config_needs(core: &Core, config: &ConfigLoad) -> Option<TrustAsk> {
    let ConfigLoad::Valid {
        path,
        config,
        fingerprint,
        trust,
    } = config
    else {
        return None;
    };
    let ask = config_ask(path, fingerprint, config);
    (trust.requires_approval && !core.tasks.trusts(&ask.grant)).then_some(ask)
}

fn personal_ask(project: &Path, task: &Definition) -> TrustAsk {
    let cwd = task.cwd.to_string_lossy().into_owned();
    let command = if task.cwd == project {
        task.command.clone()
    } else {
        format!("cd {cwd} && {}", task.command)
    };
    TrustAsk {
        title: format!("Run {}?", task.label),
        body: "This task comes from the project's .sikemux/tasks.json. Trust lasts until the task changes.".into(),
        commands: vec![TrustCommand {
            label: task.label.clone(),
            command,
        }],
        confirm_label: "Run task".into(),
        grant: Grant::Personal(TrustedTask {
            project: project.to_string_lossy().into_owned(),
            cwd,
            command: task.command.clone(),
        }),
    }
}

/// `sikemux.json` as read now, and whether the person trusts it on this host.
pub(crate) async fn project_config(core: &Arc<Core>, project: String) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        let config = config::load(Path::new(&project));
        let ask = config_needs(&core, &config);
        let mut value = serde_json::to_value(&config)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("trusted".into(), ask.is_none().into());
            if let Some(ask) = ask {
                object.insert("trustAsk".into(), serde_json::to_value(ask)?);
            }
        }
        Ok(value)
    })
    .await
}

/// The person trusts `sikemux.json` as it is now on this host.
pub(crate) async fn trust_project(
    core: &Arc<Core>,
    project: String,
    fingerprint: String,
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
        core.tasks
            .grant(Grant::Config(TrustedConfig { path, fingerprint }));
        core.harness.changed();
        core.changes.changed("tasks", Some(&project), None);
        Ok(())
    })
    .await
}

/// The person no longer trusts the project's `sikemux.json`; its commands ask before they run again.
pub(crate) async fn untrust_project(core: &Arc<Core>, project: String) -> CoreResult<()> {
    let core = core.clone();
    blocking(move || {
        let (ConfigLoad::Absent { path }
        | ConfigLoad::Invalid { path, .. }
        | ConfigLoad::Valid { path, .. }) = config::load(Path::new(&project));
        core.tasks.revoke(&path);
        core.harness.changed();
        core.changes.changed("tasks", Some(&project), None);
        Ok(())
    })
    .await
}

/// Everything the project can run, in the order the person picks from.
pub(crate) async fn list(core: &Arc<Core>, project: String) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        let project = PathBuf::from(project);
        let Offered { config, tasks } = offered(&core, &project);
        let trusted = config_needs(&core, &config).is_none();
        Ok(json!({
            "project": project,
            "configStatus": match &config {
                ConfigLoad::Absent { .. } => "absent",
                ConfigLoad::Invalid { .. } => "invalid",
                ConfigLoad::Valid { .. } => "valid",
            },
            "configErrors": match &config {
                ConfigLoad::Invalid { errors, .. } => serde_json::to_value(errors)?,
                _ => Value::Null,
            },
            "trusted": trusted,
            "tasks": tasks.iter().map(|task| view(&project, task)).collect::<Vec<_>>(),
        }))
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
    let ports: Vec<Value> = ports::for_project(ports::listening_now(core), project)
        .into_iter()
        .map(|port| {
            let (kind, label) = match &port.listening.owner {
                PortOwner::Terminal { .. } => ("terminal", "terminal".to_owned()),
                PortOwner::Task { label, .. } => ("task", label.clone()),
                PortOwner::Agent { label, .. } => ("agent", label.clone()),
            };
            json!({
                "port": port.listening.port,
                "address": port.listening.address,
                "process": port.listening.process,
                "owner": { "kind": kind, "label": label },
            })
        })
        .collect();
    let mut value = json!({
        "tasks": listed,
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
        let found = test_results(project, &tasks);
        if !found.is_empty() {
            object.insert("testResults".into(), found.into());
        }
    }
    value
}

/// The latest results each module's test task left in `project`, worded as
/// `workspace_inspect` gives them.
pub(crate) async fn module_tests(core: &Arc<Core>, project: String) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        let project = PathBuf::from(project);
        let Offered { tasks, .. } = offered(&core, &project);
        Ok(Value::Array(test_results(&project, &tasks)))
    })
    .await
}

fn test_modules(tasks: &[Definition]) -> Vec<&Path> {
    let mut modules: Vec<&Path> = tasks
        .iter()
        .filter(|task| detect::is_test_task(&task.id))
        .map(|task| task.cwd.as_path())
        .collect();
    modules.dedup();
    modules
}

/// The latest test results each module's test task left, failures first.
fn test_results(project: &Path, tasks: &[Definition]) -> Vec<Value> {
    test_modules(tasks)
        .into_iter()
        .filter_map(|cwd| {
            let found = results::read(cwd)?;
            Some(json!({
                "cwd": relative(project, cwd),
                "passed": found.passed,
                "failed": found.failed,
                "skipped": found.skipped,
                "failures": found.failures.iter().map(|failure| {
                    let mut entry = json!({
                        "test": if results::is_file_path(&failure.class_name) {
                            format!("{} › {}", failure.class_name, failure.name)
                        } else {
                            format!("{}.{}", failure.class_name, failure.name)
                        },
                        "message": shorten(&failure.message, 1000),
                    });
                    if let (Some(file), Some(object)) = (&failure.file, entry.as_object_mut()) {
                        object.insert("file".into(), relative(project, Path::new(file)).into());
                        object.insert("line".into(), failure.line.into());
                    }
                    entry
                }).collect::<Vec<_>>(),
            }))
        })
        .collect()
}

fn shorten(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
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
        return Err("No task with this id in the project's build files; call workspace_inspect for the current list".into());
    }
    Err(match config {
        ConfigLoad::Absent { .. } => {
            "Project has no sikemux.json; add one that defines tasks".into()
        }
        ConfigLoad::Invalid { errors, .. } => format!(
            "sikemux.json is invalid: {}",
            errors
                .iter()
                .map(|error| format!("{} {}", error.path, error.message))
                .collect::<Vec<_>>()
                .join(" · ")
        ),
        ConfigLoad::Valid { .. } => "Task is not defined in sikemux.json".into(),
    })
}

/// The person let this agent run anything, so its tasks skip the trust question.
fn bypasses(core: &Core, agent_id: Option<&str>) -> bool {
    agent_id
        .and_then(|agent_id| core.workspaces.run_of(agent_id))
        .is_some_and(|run| run.permission_mode == "bypass")
}

fn trust_for(
    core: &Core,
    project: &Path,
    task: &Definition,
    config: &ConfigLoad,
) -> Option<TrustAsk> {
    match task.source {
        Source::Project => config_needs(core, config),
        Source::Personal => {
            let ask = personal_ask(project, task);
            (!core.tasks.trusts(&ask.grant)).then_some(ask)
        }
        Source::Detected | Source::Rerun | Source::Command => None,
    }
}

/// Puts `ask` to the person and waits for the answer. False when they
/// refused, the run waiting on it was stopped, or nobody answered in time.
async fn ask_person(
    core: &Arc<Core>,
    project: &str,
    ask: TrustAsk,
    execution_id: Option<&str>,
) -> bool {
    let id = uuid::Uuid::new_v4().to_string();
    let grant = ask.grant.clone();
    core.tasks.lock().asking.insert(
        id.clone(),
        Asked {
            id: id.clone(),
            project: project.to_owned(),
            execution_id: execution_id.map(str::to_owned),
            ask,
        },
    );
    if let Some(execution_id) = execution_id {
        core.harness.awaiting_trust(execution_id);
    }
    core.changes.changed("trust", None, None);
    let until = Instant::now() + TRUST_WAIT;
    let answer = loop {
        let mut changed = core.harness.subscribe();
        if core.tasks.trusts(&grant) {
            break true;
        }
        let waiting = core.tasks.lock().asking.contains_key(&id)
            && execution_id.is_none_or(|execution_id| {
                core.harness
                    .run_of(execution_id)
                    .is_some_and(|run| run.status.is_active())
            });
        if !waiting || Instant::now() >= until {
            break false;
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(until) => {}
        }
    };
    core.tasks.lock().asking.remove(&id);
    core.changes.changed("trust", None, None);
    answer
}

/// Whether `agent_id`, or the person when there is none, may run the
/// commands in `project`'s `sikemux.json`, asking the person when they have
/// not trusted it on this host yet.
pub(crate) async fn config_trusted(
    core: &Arc<Core>,
    project: &Path,
    agent_id: Option<&str>,
) -> bool {
    if bypasses(core, agent_id) {
        return true;
    }
    let config = {
        let project = project.to_path_buf();
        blocking(move || Ok(config::load(&project))).await
    };
    let Ok(config) = config else {
        return false;
    };
    match config_needs(core, &config) {
        Some(ask) => ask_person(core, &project.to_string_lossy(), ask, None).await,
        None => true,
    }
}

/// The trust questions waiting on the person.
pub(crate) fn asks(core: &Core) -> Vec<Value> {
    core.tasks
        .lock()
        .asking
        .values()
        .map(|asked| serde_json::to_value(asked).unwrap_or_default())
        .collect()
}

/// The person answers a trust question.
pub(crate) fn answer_trust(core: &Core, id: &str, trust: bool) -> CoreResult<()> {
    let asked = core
        .tasks
        .lock()
        .asking
        .remove(id)
        .ok_or("Nothing waits on that answer any more")?;
    if trust {
        core.tasks.grant(asked.ask.grant);
        core.changes.changed("tasks", Some(&asked.project), None);
    } else if let Some(execution_id) = &asked.execution_id {
        core.harness
            .launch_failed(execution_id, "Project configuration was not approved");
    }
    core.harness.changed();
    Ok(())
}

/// The core's own identity for a session it spawns itself, as the app gives
/// it for the ones it asks for.
fn launch_identity(core: &Core) -> crate::protocol::LaunchIdentity {
    crate::protocol::LaunchIdentity {
        version: core.build.version.clone(),
        cli_executable: std::env::current_exe().ok(),
        cli_endpoint: core
            .listening
            .get()
            .and_then(|listening| listening.config.cli_endpoint.clone()),
    }
}

async fn spawn(core: &Arc<Core>, request: TaskSpawnRequest) -> Result<SessionId, String> {
    let task = super::connection::spawn(
        core.clone(),
        launch_identity(core),
        Box::new(SpawnTarget::Task { request }),
        None,
    )
    .await
    .map_err(|error| error.to_string())?;
    let id = task.id();
    let stopped = task
        .task()
        .is_some_and(|task| core.harness.session_started(id, task));
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
    let run = core.harness.run_of(execution_id).ok_or("The run is gone")?;
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
    let mut preview_url = match &config {
        ConfigLoad::Valid { config, .. } if task.source == Source::Project => config
            .preview
            .as_ref()
            .filter(|preview| preview.command.as_deref() == Some(task.command.as_str()))
            .and_then(|preview| preview.url.clone()),
        _ => None,
    };
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
        let rest: Vec<Check> = found
            .into_iter()
            .filter(|check| {
                !matches!(
                    check.item,
                    CheckItem::Port {
                        free_port: Some(_),
                        env_var: Some(_),
                        ..
                    }
                )
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
    let source = match task.source {
        Source::Project | Source::Command => TaskSource::Project,
        Source::Personal | Source::Detected | Source::Rerun => TaskSource::BuiltIn,
    };
    super::services::before_task(core, &task.cwd)
        .await
        .map_err(|error| error.to_string())?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartParams {
    project: String,
    task_id: String,
    /// Start whatever the checks found.
    #[serde(default)]
    force: bool,
    /// Install what the checks found missing and move the app off a taken
    /// port first.
    #[serde(default)]
    fix: bool,
}

/// Puts right what the checks found, so the task can start.
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
                core.tasks.lock().moved.insert(
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
    let mut child = sikemux_process::user_environment::command(program)
        .args(words)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("run {install}: {error}"))?;
    let until = std::time::Instant::now() + INSTALL_WAIT;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                }
                Err(format!("{install} failed: {}", stderr.trim()))
            };
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            return Err(format!("{install} did not finish within 10 minutes"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The person starts a task: it replaces the one their project is running.
/// Answers the checks that stopped it, or the run it started.
pub(crate) async fn start(core: &Arc<Core>, params: StartParams) -> CoreResult<Value> {
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
            return Ok(json!({ "checks": found }));
        }
    }
    let execution_id = super::harness::start_for_person(core, &params.project, &params.task_id)?;
    Ok(json!({ "executionId": execution_id }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddParams {
    project: String,
    label: String,
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    /// For the team in `sikemux.json`, or for this person only.
    team: bool,
}

/// Saves a task the person wrote; answers its id.
pub(crate) async fn add(core: &Arc<Core>, params: AddParams) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        let project = PathBuf::from(&params.project);
        let cwd = params
            .cwd
            .as_deref()
            .map(|cwd| cwd.trim().trim_start_matches("./").trim_matches('/'))
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or(".")
            .to_owned();
        let task = AddedTask {
            label: params.label.trim().to_owned(),
            command: params.command.trim().to_owned(),
            cwd: cwd.clone(),
        };
        let task_id = if params.team {
            detect::save_team(&project, &task)?
        } else {
            let task_id = detect::save_personal(&project, &task)?;
            core.tasks.grant(Grant::Personal(TrustedTask {
                project: params.project.clone(),
                cwd: if cwd == "." {
                    params.project.clone()
                } else {
                    project.join(&cwd).to_string_lossy().into_owned()
                },
                command: task.command.clone(),
            }));
            task_id
        };
        core.changes.changed("tasks", Some(&params.project), None);
        Ok(json!({ "taskId": task_id }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RerunParams {
    execution_id: String,
}

/// Adds a task that runs only the tests a finished run failed, and starts it.
pub(crate) async fn rerun_failed(core: &Arc<Core>, params: RerunParams) -> CoreResult<Value> {
    let run = core
        .harness
        .run_of(&params.execution_id)
        .ok_or("That run is gone")?;
    let failures = core
        .tasks
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
        let mut state = core.tasks.lock();
        let reruns = state.reruns.entry(run.project.clone()).or_default();
        reruns.retain(|known| known.id != task_id);
        reruns.push(rerun);
        let over = reruns.len().saturating_sub(MAX_RERUNS);
        reruns.drain(..over);
    }
    core.changes.changed("tasks", Some(&run.project), None);
    let execution_id = super::harness::start_for_person(core, &run.project, &task_id)?;
    Ok(json!({ "taskId": task_id, "executionId": execution_id }))
}

/// Every run as clients show it, with what it waits on the person for and
/// the test results it left.
pub(crate) fn runs(core: &Core) -> Vec<Value> {
    let runs = core.harness.all_runs();
    core.tasks
        .retain(|id| runs.iter().any(|run| run.execution_id == id));
    let state = core.tasks.lock();
    runs.into_iter()
        .map(|run| {
            let mut value = serde_json::to_value(&run).unwrap_or_default();
            if let Some(object) = value.as_object_mut() {
                object.insert("agentId".into(), run.agent_id.clone().into());
                object.insert(
                    "terminalKey".into(),
                    terminal_key(&run.project, &run.task_id).into(),
                );
                if let Some(tests) = state.tests.get(&run.execution_id) {
                    object.insert(
                        "tests".into(),
                        serde_json::to_value(tests).unwrap_or_default(),
                    );
                }
            }
            value
        })
        .collect()
}

/// A task session ended: a test task's results are read for the person to see.
pub(crate) fn ended(core: &Core, pty_id: SessionId) {
    let Some(run) = core.harness.run_on(pty_id) else {
        return;
    };
    let (Some(cwd), Some(core)) = (run.cwd.clone(), core.arc()) else {
        return;
    };
    if !detect::is_test_task(&run.task_id) {
        return;
    }
    tokio::task::spawn_blocking(move || {
        if let Some(found) = results::read(Path::new(&cwd)) {
            core.tasks.lock().tests.insert(run.execution_id, found);
            core.harness.changed();
        }
    });
}

/// Tells clients when a run changes, which they read again with `taskRuns`.
pub(crate) async fn publish_runs(core: Arc<Core>) {
    let mut changes = core.harness.subscribe();
    let mut last = Vec::new();
    while changes.changed().await.is_ok() {
        let now = runs(&core);
        if now != last {
            last = now;
            core.changes.changed("runs", None, None);
        }
    }
}
