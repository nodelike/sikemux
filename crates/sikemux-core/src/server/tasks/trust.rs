//! Whether the person trusts a project's stated tasks and checks, and asking when not.

use super::*;
use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;

pub(super) fn config_ask(
    path: &str,
    fingerprint: &str,
    config: &config::ProjectConfig,
) -> TrustAsk {
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

/// What the person must trust before `config` runs, or none. The file counts as trusted under
/// any path that leads to it, and a checkout's copy of the project's file while it holds the
/// bytes the person trusted there.
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
    let trusted_at = |file: &Path| {
        let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let file = real(file);
        let state = core.tasks().state.locked();
        state.trust.configs.iter().any(|known| {
            known.trusted.fingerprint == *fingerprint
                && real(Path::new(&known.trusted.path)) == file
        })
    };
    let trusted = core.tasks().trusts(&ask.grant)
        || trusted_at(Path::new(path))
        || crate::server::ports::checkout::projects_own(core, Path::new(path))
            .is_some_and(|own| trusted_at(&own));
    (trust.requires_approval && !trusted).then_some(ask)
}

pub(super) fn personal_ask(project: &Path, task: &Definition) -> TrustAsk {
    let cwd = task.cwd.to_string_lossy().into_owned();
    let command = if task.cwd == project {
        task.command.clone()
    } else {
        format!("cd {cwd} && {}", task.command)
    };
    TrustAsk {
        title: format!("Run {}?", task.label),
        body: "This process comes from the project's .sikemux/tasks.json. Trust lasts until the process changes.".into(),
        commands: vec![TrustCommand {
            label: task.label.clone(),
            command,
        }],
        confirm_label: "Run process".into(),
        grant: Grant::Personal(TrustedTask {
            project: project.to_string_lossy().into_owned(),
            cwd,
            command: task.command.clone(),
        }),
    }
}

/// The person let this agent run anything, so its tasks skip the trust question.
pub(super) fn bypasses(core: &Core, agent_id: Option<&str>) -> bool {
    agent_id
        .and_then(|agent_id| core.workspaces().run_of(agent_id))
        .is_some_and(|run| run.permission_mode == PermissionMode::Bypass)
}

pub(super) fn trust_for(
    core: &Core,
    project: &Path,
    task: &Definition,
    config: &ConfigLoad,
) -> Option<TrustAsk> {
    match task.source {
        Source::Project => config_needs(core, config),
        Source::Personal => {
            let ask = personal_ask(project, task);
            (!core.tasks().trusts(&ask.grant)).then_some(ask)
        }
        Source::Detected | Source::Rerun | Source::Command => None,
    }
}

/// Puts `ask` to the person and waits for the answer. False when they
/// refused, the run waiting on it was stopped, or nobody answered in time.
pub(crate) async fn ask_person(
    core: &Arc<Core>,
    project: &str,
    ask: TrustAsk,
    execution_id: Option<&str>,
) -> bool {
    let id = uuid::Uuid::new_v4().to_string();
    let grant = ask.grant.clone();
    core.tasks().state.locked().asking.insert(
        id.clone(),
        Asked {
            id: id.clone(),
            project: project.to_owned(),
            execution_id: execution_id.map(str::to_owned),
            ask,
        },
    );
    if let Some(execution_id) = execution_id {
        core.harness().awaiting_trust(execution_id);
    }
    core.changed(What::Read(Topic::Trust));
    let until = Instant::now() + TRUST_WAIT;
    let answer = loop {
        let mut changed = core.harness().subscribe();
        if core.tasks().trusts(&grant) {
            break true;
        }
        let waiting = core.tasks().state.locked().asking.contains_key(&id)
            && execution_id.is_none_or(|execution_id| {
                core.harness()
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
    core.tasks().state.locked().asking.remove(&id);
    core.changed(What::Read(Topic::Trust));
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

pub(crate) fn asks(core: &Core) -> Vec<TrustQuestion> {
    (core.tasks().state.locked().asking.values())
        .map(|asked| TrustQuestion {
            id: asked.id.clone(),
            project: asked.project.clone(),
            execution_id: asked.execution_id.clone(),
            title: asked.ask.title.clone(),
            body: asked.ask.body.clone(),
            commands: asked.ask.commands.clone(),
            confirm_label: asked.ask.confirm_label.clone(),
        })
        .collect()
}

/// The person answers a trust question, from `answered_by` when it is another device.
pub(crate) fn answer_trust(
    core: &Core,
    id: &str,
    trust: bool,
    answered_by: Option<String>,
) -> CoreResult<()> {
    let asked = core
        .tasks()
        .state
        .locked()
        .asking
        .remove(id)
        .ok_or("Nothing waits on that answer any more")?;
    if trust {
        core.tasks().grant(asked.ask.grant, answered_by);
        core.changed(What::ReadIn(Topic::Tasks, Some(&asked.project), None));
    } else if let Some(execution_id) = &asked.execution_id {
        core.harness()
            .launch_failed(execution_id, "Project configuration was not approved");
    }
    core.harness().changed();
    Ok(())
}
