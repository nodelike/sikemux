//! Proving a project's setup on fresh checkouts: two throwaway checkouts cut at once run the
//! steps a new checkout runs, start every process that has a port and wait for it to answer,
//! run the project's check, then run the removal steps, go, and leave nothing listening. The
//! last result is kept with the `sikemux.json` it proved.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::changed::ChangedCore;
use super::connection::blocking;
use super::kept::Kept;
use super::parts::Parts;
use super::ports::{self, blocks::BLOCK, checkout};
use super::worktree;
use super::{tasks, Core, CoreResult};
use crate::git::Git;
use crate::tasks::config::{self, ConfigLoad, ProjectCheck, ProjectConfig, Stated};
pub use sikemux_calls::setup::{Outcome, SetupParams, SetupRun, SetupState, SetupStep, StepKind};

const FILE: &str = "setup-checks.json";
/// How long one step may take, such as installing a project's packages.
const STEP_TIME: Duration = Duration::from_secs(15 * 60);
/// How long a process has to answer on its port.
const ANSWER_TIME: Duration = Duration::from_secs(120);
/// How long ports may take to close once everything is stopped.
const CLOSE_TIME: Duration = Duration::from_secs(5);

#[derive(Default, Serialize, Deserialize)]
struct Book {
    runs: HashMap<String, SetupRun>,
}

pub(crate) struct Setups {
    book: Mutex<Book>,
    checking: Mutex<Vec<String>>,
    kept: Kept,
    logs: Option<PathBuf>,
}

impl Setups {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            book: Mutex::new(kept.load()),
            checking: Mutex::default(),
            kept,
            logs: data_dir.map(|dir| dir.join("setup-checks")),
        }
    }

    fn keep(&self, project: &str, run: SetupRun) {
        let mut book = self.book.locked();
        book.runs.insert(project.to_owned(), run);
        self.kept.save(&*book);
    }
}

/// The project's setup and its last check.
pub(crate) async fn state(core: &Arc<Core>, params: SetupParams) -> CoreResult<SetupState> {
    let core = core.clone();
    blocking(move || Ok(state_of(&core, &params.project))).await
}

pub(crate) fn state_of(core: &Core, project: &str) -> SetupState {
    let load = config::load(Path::new(project));
    let (config, fingerprint) = match load {
        ConfigLoad::Valid {
            config,
            fingerprint,
            ..
        } => (Some(*config), Some(fingerprint)),
        ConfigLoad::Invalid { fingerprint, .. } => (None, fingerprint),
        ConfigLoad::Absent { .. } => (None, None),
    };
    let setups = &core.tasks().setups;
    let key = key(project);
    let last = setups.book.locked().runs.get(&key).cloned();
    let stale = last
        .as_ref()
        .is_some_and(|last| Some(&last.fingerprint) != fingerprint.as_ref());
    SetupState {
        project: project.to_owned(),
        config,
        last,
        stale,
        checking: setups.checking.locked().contains(&key),
    }
}

/// The name a project's checks are kept under: its folder, wherever links lead.
fn key(project: &str) -> String {
    std::fs::canonicalize(project).map_or_else(
        |_| project.to_owned(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Whether the project's setup was proved on the file as it is now.
pub(crate) fn proved(core: &Core, project: &str) -> bool {
    state_of(core, project).proved()
}

/// Checks the project's setup on two fresh checkouts, asking the person to trust its
/// `sikemux.json` first when they have not, on behalf of `agent_id`.
pub(crate) async fn check(
    core: &Arc<Core>,
    project: &str,
    agent_id: Option<&str>,
) -> CoreResult<SetupRun> {
    let folder = PathBuf::from(project);
    let load = {
        let folder = folder.clone();
        blocking(move || Ok(config::load(&folder))).await?
    };
    let (config, fingerprint) = match load {
        ConfigLoad::Valid {
            config,
            fingerprint,
            ..
        } => (*config, fingerprint),
        ConfigLoad::Invalid { errors, .. } => return Err(config::invalid_words(&errors).into()),
        ConfigLoad::Absent { .. } => {
            return Err("the project has no sikemux.json to check yet".into())
        }
    };
    if !tasks::config_trusted(core, &folder, agent_id).await {
        return Err("sikemux.json was not trusted, so its steps did not run".into());
    }
    let project = key(project);
    let project = project.as_str();
    {
        let mut checking = core.tasks().setups.checking.locked();
        if checking.iter().any(|known| known == project) {
            return Err("a check of this project's setup is already running".into());
        }
        checking.push(project.to_owned());
    }
    core.changed(super::changed::What::Read(super::changed::Topic::Tasks));
    let ran = {
        let (core, folder) = (core.clone(), folder.clone());
        blocking(move || Ok(run(&core, &folder, &config, &fingerprint))).await
    };
    core.tasks()
        .setups
        .checking
        .locked()
        .retain(|known| known != project);
    let ran = ran.and_then(|ran| ran.map_err(Into::into));
    if let Ok(run) = &ran {
        core.tasks().setups.keep(project, run.clone());
    }
    core.changed(super::changed::What::Read(super::changed::Topic::Tasks));
    ran
}

fn run(
    core: &Arc<Core>,
    project: &Path,
    config: &ProjectConfig,
    fingerprint: &str,
) -> Result<SetupRun, String> {
    let repo = PathBuf::from(
        Git::quick(project)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .map_err(|_| "the project is not in a git repository")?,
    );
    let inside = worktree::inside(project, &repo);
    let started = epoch_ms();
    let logs = (core.tasks().setups.logs.clone())
        .unwrap_or_else(std::env::temp_dir)
        .join(started.to_string());
    std::fs::create_dir_all(&logs).map_err(|error| error.to_string())?;
    let cut = |which: &str| {
        let branch = format!("sikemux/setup-check-{started}-{which}");
        let (branch, path) = worktree::cut(&repo, &branch, "HEAD")?;
        worktree::copy_included(&repo, &path);
        let own = project.join(config::CONFIG_FILE);
        let _ = std::fs::copy(&own, path.join(&inside).join(config::CONFIG_FILE));
        Ok::<_, String>((branch, path))
    };
    let trees = [cut("a")?, cut("b")?];
    let repo = &repo;
    let results: Vec<Vec<SetupStep>> = std::thread::scope(|scope| {
        let running: Vec<_> = (trees.iter())
            .zip(["a", "b"])
            .map(|((branch, path), which)| {
                let logs = logs.join(which);
                let cwd = path.join(&inside);
                scope.spawn(move || {
                    let _ = std::fs::create_dir_all(&logs);
                    let steps = in_checkout(core, config, &cwd, &logs);
                    let gone = remove(core, repo, path, branch);
                    let mut steps = steps;
                    steps.push(gone);
                    steps
                })
            })
            .collect();
        running
            .into_iter()
            .map(|thread| thread.join().unwrap_or_default())
            .collect()
    });
    let steps = merged(&results);
    Ok(SetupRun {
        fingerprint: fingerprint.to_owned(),
        at: epoch_ms(),
        passed: steps.iter().all(|step| step.outcome != Outcome::Failed),
        steps,
    })
}

/// The steps of both checkouts as one list: a step fails when it failed in either.
fn merged(results: &[Vec<SetupStep>]) -> Vec<SetupStep> {
    let mut steps = results.first().cloned().unwrap_or_default();
    for other in &results[1.min(results.len())..] {
        for (step, theirs) in steps.iter_mut().zip(other) {
            if step.outcome != Outcome::Failed && theirs.outcome == Outcome::Failed {
                *step = theirs.clone();
            }
        }
    }
    steps
}

/// The steps in the fresh checkout at `cwd`, with `onRemove` last, before its folder goes.
fn in_checkout(core: &Core, config: &ProjectConfig, cwd: &Path, logs: &Path) -> Vec<SetupStep> {
    let here = checkout::here(core, cwd);
    let env: Vec<(String, String)> = here
        .as_ref()
        .map(|here| {
            let mut env = here.environment();
            env.push((
                "SIKEMUX_PROJECT".to_owned(),
                cwd.to_string_lossy().into_owned(),
            ));
            env
        })
        .unwrap_or_default();
    let worktree = config
        .worktree
        .clone()
        .unwrap_or_else(|| config::WorktreeConfig {
            on_create: Vec::new(),
            on_settle: Vec::new(),
            on_remove: Vec::new(),
        });
    let mut steps = Vec::new();
    let mut failed = false;
    for (at, hook) in worktree.on_create.iter().enumerate() {
        let step = step(StepKind::Create, &hook.label, &hook.command);
        if failed {
            steps.push(step);
            continue;
        }
        let log = logs.join(format!("create-{at}.log"));
        let step = finished(step, shell(&hook.command, cwd, &env, &log), log);
        failed |= step.outcome == Outcome::Failed;
        steps.push(step);
    }
    let mut serving: Vec<Child> = Vec::new();
    for task in config.tasks.iter().filter(|task| task.port.is_some()) {
        let step = step(StepKind::Process, &task.label, &task.command);
        let port = here.as_ref().and_then(|here| here.task_port(&task.id));
        let (Some(port), false) = (port, failed) else {
            steps.push(step);
            continue;
        };
        let mut task_env = env.clone();
        task_env.extend(
            here.as_ref()
                .map(|here| here.task_environment(&task.id))
                .unwrap_or_default(),
        );
        let task_cwd = if task.cwd == "." {
            cwd.to_owned()
        } else {
            cwd.join(&task.cwd)
        };
        let log = logs.join(format!("process-{}.log", task.id));
        let answered = serve(
            &task.command,
            &task_cwd,
            &task_env,
            &log,
            port,
            &mut serving,
        );
        let step = finished(step, answered, log);
        failed |= step.outcome == Outcome::Failed;
        steps.push(step);
    }
    let check = match &config.check {
        Some(Stated::Given(ProjectCheck::Command(command))) => Some(command.clone()),
        _ => None,
    };
    if let Some(command) = check {
        let step = step(StepKind::Check, "The project's check", &command);
        if failed {
            steps.push(step);
        } else {
            let log = logs.join("check.log");
            let (ran, left) = checked(&command, cwd, &env, &log);
            steps.push(finished(step, ran, log));
            let heavy: Vec<&str> = (config.heavy.iter().flatten())
                .map(|heavy| heavy.command.as_str())
                .collect();
            if !heavy.is_empty() {
                let still = left
                    .iter()
                    .find(|line| heavy.iter().any(|heavy| line.contains(heavy)));
                steps.push(SetupStep {
                    outcome: if still.is_some() { Outcome::Failed } else { Outcome::Passed },
                    last_line: still.map(|line| {
                        format!("still running after the check ended, so it is not a heavy command: {line}")
                    }),
                    ..self::step(StepKind::Heavy, "No heavy command left running", "")
                });
            }
        }
    }
    for child in &mut serving {
        sikemux_process::kill_and_reap(child);
    }
    for (at, hook) in worktree.on_remove.iter().enumerate() {
        let log = logs.join(format!("remove-{at}.log"));
        let step = finished(
            step(StepKind::Remove, &hook.label, &hook.command),
            shell(&hook.command, cwd, &env, &log),
            log,
        );
        steps.push(step);
    }
    steps
}

/// Removes the checkout, frees its ports and says whether any of them still listens.
fn remove(core: &Core, repo: &Path, path: &Path, branch: &str) -> SetupStep {
    let block = core.ports().blocks.of(path).map(|held| held.first_port);
    worktree::remove(repo, path, branch);
    core.ports().blocks.free(path);
    let step = step(StepKind::Ports, "Nothing left listening", "");
    let Some(first) = block else {
        return SetupStep {
            outcome: Outcome::Passed,
            ..step
        };
    };
    let until = Instant::now() + CLOSE_TIME;
    loop {
        let open: Vec<u16> = (first..first + BLOCK)
            .filter(|port| ports::in_use(*port))
            .collect();
        if open.is_empty() {
            return SetupStep {
                outcome: Outcome::Passed,
                ..step
            };
        }
        if Instant::now() > until {
            let ports: Vec<String> = open.iter().map(u16::to_string).collect();
            return SetupStep {
                outcome: Outcome::Failed,
                last_line: Some(format!("still listening on {}", ports.join(", "))),
                ..step
            };
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn step(kind: StepKind, label: &str, command: &str) -> SetupStep {
    SetupStep {
        kind,
        label: label.to_owned(),
        command: command.to_owned(),
        outcome: Outcome::NotRun,
        last_line: None,
        log: None,
    }
}

fn finished(step: SetupStep, ran: Result<(), String>, log: PathBuf) -> SetupStep {
    match ran {
        Ok(()) => SetupStep {
            outcome: Outcome::Passed,
            log: Some(log),
            ..step
        },
        Err(why) => SetupStep {
            outcome: Outcome::Failed,
            last_line: Some(why),
            log: Some(log),
            ..step
        },
    }
}

/// The person's login shell running `command` in `cwd` with `env`, its group its own.
pub(crate) fn command_in(command: &str, cwd: &Path, env: &[(String, String)]) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut process = sikemux_process::user_environment::agent_command_in_own_cgroup(shell);
    process.args(["-lc", command]).current_dir(cwd);
    process.envs(env.iter().map(|(key, value)| (key, value)));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    process
}

/// Runs `command` to its end, writing what it printed to `log`. Why it failed, by the last
/// line it printed, when it did.
pub(crate) fn shell(
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
    log: &Path,
) -> Result<(), String> {
    let mut process = command_in(command, cwd, env);
    let ran = sikemux_process::run(&mut process, None, STEP_TIME, 4 << 20, None);
    let output = match ran {
        Ok(output) => output,
        Err(error) => return Err(error.to_string()),
    };
    let mut printed = output.stdout;
    printed.extend(output.stderr);
    if let Ok(mut file) = std::fs::File::create(log) {
        let _ = file.write_all(&printed);
    }
    match output.status.success() {
        true => Ok(()),
        false => Err(last_line(&printed)
            .unwrap_or_else(|| format!("exit {}", output.status.code().unwrap_or(-1)))),
    }
}

fn last_line(printed: &[u8]) -> Option<String> {
    String::from_utf8_lossy(printed)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Starts `command` and waits for `port` to answer, keeping it running in `serving`.
fn serve(
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
    log: &Path,
    port: u16,
    serving: &mut Vec<Child>,
) -> Result<(), String> {
    let file = std::fs::File::create(log).map_err(|error| error.to_string())?;
    let mut process = command_in(command, cwd, env);
    process
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(|error| error.to_string())?)
        .stderr(file);
    let mut child = process.spawn().map_err(|error| error.to_string())?;
    let until = Instant::now() + ANSWER_TIME;
    let answered = loop {
        if ports::in_use(port) {
            break Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            let printed = std::fs::read(log).unwrap_or_default();
            break Err(last_line(&printed)
                .unwrap_or_else(|| format!("stopped ({status}) before port {port} answered")));
        }
        if Instant::now() > until {
            break Err(format!("port {port} did not answer within two minutes"));
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    serving.push(child);
    answered
}

/// Runs the check to its end, then stops and names what it left running in its group.
fn checked(
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
    log: &Path,
) -> (Result<(), String>, Vec<String>) {
    let mut process = command_in(command, cwd, env);
    let file = match std::fs::File::create(log) {
        Ok(file) => file,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let Ok(copy) = file.try_clone() else {
        return (Err("could not write the check's log".into()), Vec::new());
    };
    process.stdin(Stdio::null()).stdout(copy).stderr(file);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let group = child.id();
    let until = Instant::now() + STEP_TIME;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() > until => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => break None,
        }
    };
    let left = left_in_group(group);
    sikemux_process::terminate_tree(group, true);
    let printed = std::fs::read(log).unwrap_or_default();
    let ran = match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(last_line(&printed).unwrap_or_else(|| format!("{status}"))),
        None => {
            sikemux_process::kill_and_reap(&mut child);
            Err("took longer than 15 minutes".into())
        }
    };
    (ran, left)
}

/// The command lines of what still runs in the process group `group`.
fn left_in_group(group: u32) -> Vec<String> {
    let listed = sikemux_process::user_environment::command("pgrep")
        .args(["-g", &group.to_string()])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    listed
        .lines()
        .filter_map(|pid| pid.trim().parse::<u32>().ok())
        .filter_map(sikemux_process::procs::command_line)
        .map(|words| words.join(" "))
        .collect()
}
