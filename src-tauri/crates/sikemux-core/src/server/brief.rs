//! What an agent working in a project is told about it: how each folder is
//! built and run with the person's settings, what must pass before its work
//! is done, and the person's own notes. Secret values never go in it.

use std::fmt::Write;
use std::path::Path;

use serde_json::Value;

use std::collections::BTreeMap;

use super::workspace::{FolderSettings, ProjectRules};
use super::Core;
use crate::tasks::folders::{self, Folder};

/// Variables whose values say how the code is built and run, shown as they are.
const SHOWN: [(&str, &str); 12] = [
    ("MAVEN_ARGS", "Maven"),
    ("SPRING_PROFILES_ACTIVE", "Spring profiles"),
    ("JDK_JAVA_OPTIONS", "JVM options"),
    ("NODE_ENV", "NODE_ENV"),
    ("NODE_OPTIONS", "NODE_OPTIONS"),
    ("GOFLAGS", "GOFLAGS"),
    ("GOPRIVATE", "GOPRIVATE"),
    ("RUSTUP_TOOLCHAIN", "toolchain"),
    ("RUST_LOG", "RUST_LOG"),
    ("RUST_BACKTRACE", "RUST_BACKTRACE"),
    ("PYTHONPATH", "PYTHONPATH"),
    ("PYTHONUNBUFFERED", "PYTHONUNBUFFERED"),
];

pub(crate) fn write(core: &Core, project: &Path) -> String {
    let inspected = super::tasks::inspect(core, project);
    let tasks = inspected
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    of(core, project, &tasks)
}

/// The brief, given the tasks the project offers as `workspace_inspect` lists them.
pub(crate) fn of(core: &Core, project: &Path, tasks: &[Value]) -> String {
    let key = project.to_string_lossy();
    let settings = core.workspaces.folder_settings(&key);
    let rules = core.workspaces.project_rules(&key);
    compose(&folders::folders(project), &settings, &rules, tasks)
}

fn compose(
    found: &[Folder],
    settings: &BTreeMap<String, FolderSettings>,
    rules: &ProjectRules,
    tasks: &[Value],
) -> String {
    let mut brief = String::new();
    for folder in found {
        let chosen = settings.get(&folder.path);
        let line = folder_line(folder, chosen.map(|chosen| &chosen.env));
        let loaded = chosen.and_then(|chosen| chosen.environment.as_deref());
        if line.is_empty() && loaded.is_none() {
            continue;
        }
        let _ = writeln!(brief, "{}: {line}", where_(&folder.path));
        if let Some(environment) = loaded {
            let name = if environment.is_empty() {
                "the shared env files"
            } else {
                environment
            };
            let _ = writeln!(brief, "  environment: {name} loaded");
        }
        let set: Vec<&str> = chosen
            .map(|chosen| {
                chosen
                    .env
                    .keys()
                    .map(String::as_str)
                    .filter(|key| {
                        *key != "JAVA_HOME" && !SHOWN.iter().any(|(shown, _)| shown == key)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !set.is_empty() {
            let _ = writeln!(brief, "  also set: {} (values not shown)", set.join(", "));
        }
    }
    let label = |id: &str| {
        tasks
            .iter()
            .find(|task| task.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|task| task.get("label").and_then(Value::as_str))
            .map(|label| format!("{label} ({id})"))
    };
    if !tasks.is_empty() {
        let listed: Vec<String> = tasks
            .iter()
            .filter_map(|task| task.get("id").and_then(Value::as_str))
            .filter_map(label)
            .collect();
        let _ = writeln!(
            brief,
            "Tasks, started with task_start: {}",
            listed.join(", ")
        );
    }
    let done: Vec<String> = settings
        .iter()
        .filter(|(_, chosen)| !chosen.done.is_empty())
        .map(|(folder, chosen)| {
            let names: Vec<String> = chosen
                .done
                .iter()
                .map(|id| label(id).unwrap_or_else(|| id.clone()))
                .collect();
            format!("{} → {}", where_(folder), names.join(", "))
        })
        .collect();
    if !done.is_empty() {
        let scope = if rules.check_every_folder {
            "run every folder's checks below"
        } else {
            "run the checks below for each folder you changed"
        };
        let failing = if rules.stop_on_failure {
            "If one fails, stop and tell the person what failed."
        } else {
            "If one fails, fix the cause and run it again; tell the person if you cannot."
        };
        let _ = writeln!(
            brief,
            "Done means: before you say your work is finished, {scope} with task_start and wait for them to pass. {failing}\n  {}",
            done.join("\n  ")
        );
    }
    let notes = rules.notes.trim();
    if !notes.is_empty() {
        let _ = writeln!(brief, "From the person: {notes}");
    }
    brief.trim_end().to_owned()
}

fn where_(folder: &str) -> String {
    if folder == "." {
        "root".to_owned()
    } else {
        format!("{folder}/")
    }
}

/// `Java 17 (pom.xml asks 21) · Maven -Pdev · Spring profiles local` for one folder.
fn folder_line(
    folder: &Folder,
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for language in &folder.languages {
        let mut part = match language.id {
            "java" => "Java".to_owned(),
            "node" => "Node".to_owned(),
            "go" => "Go".to_owned(),
            "rust" => "Rust".to_owned(),
            "python" => "Python".to_owned(),
            other => other.to_owned(),
        };
        let chosen_java = env
            .and_then(|env| env.get("JAVA_HOME"))
            .filter(|_| language.id == "java")
            .and_then(|home| crate::tasks::java::java_major(Path::new(home)));
        match (chosen_java, &language.version) {
            (Some(major), Some(asked)) if major.to_string() != *asked => {
                let _ = write!(part, " {major} (the build asks for {asked})");
            }
            (Some(major), _) => {
                let _ = write!(part, " {major}");
            }
            (None, Some(asked)) => {
                let _ = write!(part, " {asked}");
            }
            (None, None) => {}
        }
        if language.markers.iter().any(|marker| marker == "mvnw") {
            part.push_str(", build with ./mvnw");
        } else if language.markers.iter().any(|marker| marker == "gradlew") {
            part.push_str(", build with ./gradlew");
        }
        parts.push(part);
    }
    if let Some(env) = env {
        for (variable, name) in SHOWN {
            if let Some(value) = env.get(variable) {
                parts.push(format!("{name} {value}"));
            }
        }
    }
    parts.join(" · ")
}
