//! Tasks an agent starts, reads and stops, and telling it when one fails.

use super::*;

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
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSelector {
    execution_id: Option<String>,
    project: Option<String>,
    agent_id: Option<String>,
}

impl RunSelector {
    /// The one run `execution_id` names.
    pub fn of_execution(execution_id: &str) -> Self {
        Self {
            execution_id: Some(execution_id.to_owned()),
            project: None,
            agent_id: None,
        }
    }

    pub(crate) fn of_agent(agent_id: &str) -> Self {
        Self {
            execution_id: None,
            project: None,
            agent_id: Some(agent_id.to_owned()),
        }
    }
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

/// An absolute `cwd` inside the project, as the relative one tasks take.
pub(super) fn within_project(project: &str, cwd: &str) -> String {
    let real = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    match real(cwd).strip_prefix(real(project)) {
        Ok(inside) if Path::new(cwd).is_absolute() => match inside.to_string_lossy() {
            inside if inside.is_empty() => ".".to_owned(),
            inside => inside.into_owned(),
        },
        _ => cwd.to_owned(),
    }
}

pub(super) fn launch_target(
    state: &mut State,
    project: &str,
    params: &Value,
    restart: bool,
) -> Result<(String, Option<CommandLaunch>), String> {
    if restart {
        let task_id = required_text(params, "processId")?;
        let spec = if task_id.starts_with(COMMAND_TASK_PREFIX) {
            Some(state.earlier_command(project, task_id)?)
        } else {
            None
        };
        return Ok((task_id.to_owned(), spec));
    }
    let task_id = text(params, "processId")?;
    let has_command = params.get("command").is_some();
    match task_id {
        Some(_) if has_command => Err("Pass either processId or command, not both".into()),
        None if !has_command => Err("processId or command is required".into()),
        None => {
            let command = required_text(params, "command")?;
            let label = text(params, "label")?;
            if label.is_some_and(|label| label.chars().count() > 80) {
                return Err("label must be at most 80 characters".into());
            }
            let cwd = text(params, "cwd")?.map(|cwd| within_project(project, cwd));
            let cwd = command_cwd(cwd.as_deref())?;
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
                    "cwd and label go with command; a sikemux.json process sets its own".into(),
                );
            }
            Ok((task_id.to_owned(), None))
        }
    }
}

pub(super) enum Begun {
    Existing(String),
    New {
        execution_id: String,
        /// The run a restart replaces.
        previous: Option<String>,
    },
}

pub(super) fn begin_start(
    core: &Core,
    project: &str,
    agent_id: Option<&str>,
    params: &Value,
    key: &str,
    restart: bool,
) -> Result<Begun, String> {
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
                let execution_id = active.execution_id.clone();
                state
                    .runs
                    .remember_key(project, key, &task_id, &execution_id)?;
                return Ok(Begun::Existing(execution_id));
            }
        }
        let previous = if restart {
            state.runs.latest(project, &task_id)
        } else {
            spec.as_ref().zip(agent_id).and_then(|(spec, agent_id)| {
                state
                    .runs
                    .same_label(project, agent_id, &spec.label, &task_id)
            })
        }
        .map(|run| run.execution_id.clone());
        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut run = Run::new(execution_id.clone(), task_id.clone(), project.to_owned());
        run.agent_id = agent_id.map(str::to_owned);
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

pub(super) fn launch_finished(
    core: &Core,
    execution_id: &str,
    result: Result<Option<String>, String>,
) {
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
pub(super) fn launch(
    core: &Arc<Core>,
    execution_id: String,
    previous: Option<String>,
    checked: bool,
) {
    let core = core.clone();
    tokio::spawn(async move {
        let result = super::super::tasks::launch(&core, &execution_id, previous, checked).await;
        launch_finished(&core, &execution_id, result);
    });
}

/// The person starts a task, in place of whatever else they run in that
/// project. Answers the new run's id.
pub(crate) fn start_for_person(
    core: &Arc<Core>,
    project: &str,
    task_id: &str,
) -> Result<String, String> {
    let params = json!({ "processId": task_id });
    let key = uuid::Uuid::new_v4().to_string();
    let Begun::New {
        execution_id,
        previous,
    } = begin_start(core, project, None, &params, &key, true)?
    else {
        return Err("The process did not start".into());
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

pub(super) async fn process_start(
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

pub(super) async fn answer_start(
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

pub(super) async fn output_appears(
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

/// A run an agent started that ended with an error nobody asked for.
pub(crate) struct FailedRun {
    pub(crate) agent_id: String,
    pub(crate) project: String,
    pub(crate) pty_id: SessionId,
    pub(crate) execution_id: String,
    pub(crate) label: String,
    pub(crate) reason: String,
}

/// Tells the agent its run died, so a crashed dev server does not go unnoticed
/// while the agent works on something else.
pub(crate) fn tell_agent_run_failed(core: &Core, run: FailedRun) {
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    runtime.spawn(async move {
        let mut text = format!(
            "Your process \"{}\" (executionId {}) stopped: {}. Read its output with process_read, fix the cause, and start it again with process_start under the same label.",
            run.label, run.execution_id, run.reason
        );
        if let Some(held) = port_holders(&core, &run).await {
            text.push(' ');
            text.push_str(&held);
        }
        let _ = super::super::messages::from_sikemux(&core, run.agent_id, text).await;
    });
}

/// When a run died because its port was taken, says what in the project listens on which
/// port, so the agent uses that server or stops it rather than guessing another port.
pub(super) async fn port_holders(core: &Arc<Core>, run: &FailedRun) -> Option<String> {
    let query = OutputQuery {
        cursor: 0,
        limit: 8192,
        tail: Some(40),
        search: None,
        context: 0,
        plain: true,
    };
    let tail = read_output(core, run.pty_id, query).await.ok()?;
    let tail = String::from_utf8_lossy(&tail.bytes).to_lowercase();
    if !tail.contains("address already in use") && !tail.contains("eaddrinuse") {
        return None;
    }
    let listening = {
        let core = core.clone();
        blocking(move || Ok(super::super::ports::listening_now(&core)))
            .await
            .ok()?
    };
    let held: Vec<String> = super::super::ports::for_project(listening, Path::new(&run.project))
        .into_iter()
        .filter_map(|port| {
            let holder = match &port.listening.owner {
                super::super::ports::PortOwner::Task { label, .. } => {
                    format!("process \"{label}\"")
                }
                super::super::ports::PortOwner::Agent { label, .. } => format!("agent {label}"),
                super::super::ports::PortOwner::Terminal { .. } => return None,
            };
            Some(format!("{} is held by {holder}", port.listening.port))
        })
        .collect();
    Some(match held.as_slice() {
        [] => "Its port is taken by a program outside this project's processes and agents; workspace_inspect lists the ports in use.".to_owned(),
        _ => format!(
            "Its port was taken. In this project, {}. Use that server, or stop it with process_stop, rather than picking another port.",
            held.join(", ")
        ),
    })
}
