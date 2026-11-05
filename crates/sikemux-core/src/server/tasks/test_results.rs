//! The test results the build leaves in a project's modules, for the Tests tab.

use super::*;

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

pub(super) fn test_modules(tasks: &[Definition]) -> Vec<&Path> {
    let mut modules: Vec<&Path> = tasks
        .iter()
        .filter(|task| detect::is_test_task(&task.id))
        .map(|task| task.cwd.as_path())
        .collect();
    modules.dedup();
    modules
}

/// The latest test results each module's test task left, failures first.
pub(super) fn test_results(project: &Path, tasks: &[Definition]) -> Vec<Value> {
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
                        "message": crate::text::clipped(&failure.message, 1000),
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
