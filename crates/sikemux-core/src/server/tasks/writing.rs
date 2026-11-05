//! Writing tasks and the check into a project's `sikemux.json`, as the person asks.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddParams {
    project: String,
    label: String,
    command: String,
    cwd: Option<String>,
    /// For the team in `sikemux.json`, or for this person only.
    team: bool,
    /// The task this one is an edit of, which goes when its id or place changes.
    replaces: Option<String>,
}

/// Saves a task the person wrote; answers its id.
pub(crate) async fn add(
    core: &Arc<Core>,
    params: AddParams,
    answered_by: Option<String>,
) -> CoreResult<Value> {
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
            core.tasks.grant(
                Grant::Personal(TrustedTask {
                    project: params.project.clone(),
                    cwd: if cwd == "." {
                        params.project.clone()
                    } else {
                        project.join(&cwd).to_string_lossy().into_owned()
                    },
                    command: task.command.clone(),
                }),
                answered_by,
            );
            task_id
        };
        if let Some(old) = params.replaces.filter(|old| *old != task_id) {
            detect::remove_saved(&project, &old)?;
        }
        core.changed(What::ReadIn(Topic::Tasks, Some(&params.project), None));
        Ok(json!({ "taskId": task_id }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckParams {
    project: String,
    command: String,
}

/// Names the project's check command in its `sikemux.json`. A file the person
/// trusted stays trusted, since they wrote this change themselves.
pub(crate) async fn set_check(core: &Arc<Core>, params: CheckParams) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        let project = PathBuf::from(&params.project);
        let trusted = match config::load(&project) {
            ConfigLoad::Absent { .. } => true,
            load @ ConfigLoad::Valid { .. } => config_needs(&core, &load).is_none(),
            ConfigLoad::Invalid { .. } => false,
        };
        detect::save_check(&project, &params.command)?;
        if let (
            true,
            ConfigLoad::Valid {
                path, fingerprint, ..
            },
        ) = (trusted, config::load(&project))
        {
            core.tasks
                .grant(Grant::Config(TrustedConfig { path, fingerprint }), None);
        }
        core.harness.changed();
        core.changed(What::ReadIn(Topic::Tasks, Some(&params.project), None));
        Ok(Value::Null)
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoveParams {
    project: String,
    task_id: String,
}

/// Takes a task the person added off the project.
pub(crate) async fn remove(core: &Arc<Core>, params: RemoveParams) -> CoreResult<Value> {
    let core = core.clone();
    blocking(move || {
        detect::remove_saved(Path::new(&params.project), &params.task_id)?;
        core.changed(What::ReadIn(Topic::Tasks, Some(&params.project), None));
        Ok(Value::Null)
    })
    .await
}
