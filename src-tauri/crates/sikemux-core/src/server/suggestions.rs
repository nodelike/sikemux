//! Changes agents suggest to a project's settings. An agent never changes the
//! settings itself: what it sends waits here until the person applies or
//! discards each change.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};
use crate::cli::protocol::HarnessRequest;

pub(crate) const KINDS: [&str; 8] = [
    "variable",
    "environment",
    "agentEnvironment",
    "done",
    "task",
    "note",
    "guardrail",
    "service",
];
pub(crate) const GUARDRAILS: [&str; 3] = ["neverEdit", "askBefore", "neverRun"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Suggestion {
    pub id: String,
    /// The agent's name as the person knows it.
    pub by: String,
    /// Milliseconds since the epoch.
    pub at: u64,
    pub title: String,
    pub changes: Vec<Change>,
}

impl Suggestion {
    pub(crate) fn pending(&self) -> bool {
        self.changes
            .iter()
            .any(|change| change.status == Status::Pending)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Change {
    pub id: String,
    pub kind: String,
    /// `.` for the project root, else a folder directly inside it.
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default)]
    pub status: Status,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Status {
    #[default]
    Pending,
    Applied,
    Discarded,
}

#[derive(Deserialize)]
struct Sent {
    title: String,
    changes: Vec<SentChange>,
}

#[derive(Deserialize)]
struct SentChange {
    kind: String,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<String>,
    why: String,
    #[serde(default)]
    evidence: Option<String>,
}

/// `project_suggest`: keeps what the agent sends for the person to look at.
pub(crate) fn suggest(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let sent: Sent = serde_json::from_value(request.params.clone())
        .map_err(|error| format!("project_suggest: {error}"))?;
    let changes = sent
        .changes
        .into_iter()
        .map(|change| {
            let folder = change.folder.unwrap_or_else(|| ".".to_owned());
            check(
                &change.kind,
                &folder,
                change.key.as_deref(),
                change.value.as_deref(),
            )?;
            Ok(Change {
                id: uuid::Uuid::new_v4().simple().to_string(),
                kind: change.kind,
                folder,
                key: change.key,
                value: change.value,
                why: change.why,
                evidence: change.evidence,
                status: Status::Pending,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let by = request
        .agent_id
        .as_deref()
        .and_then(|agent_id| core.names.name(agent_id))
        .unwrap_or_else(|| "An agent".to_owned());
    let suggestion = Suggestion {
        id: uuid::Uuid::new_v4().simple().to_string(),
        by,
        at: now(),
        title: sent.title,
        changes,
    };
    let count = suggestion.changes.len();
    core.workspaces
        .keep_suggestion(&request.project, suggestion)
        .map_err(|error| error.to_string())?;
    core.changes
        .changed("suggestions", Some(&request.project), None);
    Ok(json!({
        "kept": count,
        "note": "The person sees these in the project's settings and applies the ones they want; nothing changed yet.",
    }))
}

fn check(kind: &str, folder: &str, key: Option<&str>, value: Option<&str>) -> Result<(), String> {
    if !KINDS.contains(&kind) {
        return Err(format!("{kind} is not a kind of change"));
    }
    let one_folder =
        folder == "." || (!folder.is_empty() && !folder.contains('/') && !folder.starts_with('.'));
    if !one_folder {
        return Err(format!(
            "{folder} is not the project root (.) or a folder directly inside it"
        ));
    }
    let needs = |what: &str, given: Option<&str>| {
        given
            .filter(|given| !given.trim().is_empty())
            .map(|_| ())
            .ok_or_else(|| format!("a {kind} change needs a {what}"))
    };
    match kind {
        "variable" => needs("key", key),
        "task" => needs("key", key).and(needs("value", value)),
        "guardrail" => match key {
            Some(key) if GUARDRAILS.contains(&key) => needs("value", value),
            _ => Err("a guardrail's key is neverEdit, askBefore or neverRun".to_owned()),
        },
        "agentEnvironment" | "environment" | "done" | "note" | "service" => needs("value", value),
        _ => Ok(()),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnswerParams {
    project: String,
    id: String,
    /// The changes to apply, by id; the rest are discarded.
    apply: Vec<String>,
}

/// Applies the chosen changes of a suggestion and discards the others.
pub(crate) async fn answer(core: &Arc<Core>, params: AnswerParams) -> CoreResult<()> {
    let suggestion = core
        .workspaces
        .suggestions(&canonical(&params.project))
        .into_iter()
        .find(|suggestion| suggestion.id == params.id)
        .ok_or_else(|| CoreError::from("that suggestion is gone"))?;
    let mut applied = Vec::new();
    for change in suggestion
        .changes
        .iter()
        .filter(|change| change.status == Status::Pending && params.apply.contains(&change.id))
    {
        apply(core, &params.project, change).await?;
        applied.push(change.id.clone());
    }
    core.workspaces
        .settle_suggestion(&canonical(&params.project), &params.id, &applied)?;
    core.changes
        .changed("suggestions", Some(&params.project), None);
    Ok(())
}

async fn apply(core: &Arc<Core>, project: &str, change: &Change) -> CoreResult<()> {
    let value = change.value.clone().unwrap_or_default();
    if change.kind == "task" {
        let added = serde_json::from_value(json!({
            "project": project,
            "label": change.key.clone().unwrap_or_default(),
            "command": value,
            "cwd": change.folder,
            "team": false,
        }))
        .map_err(|error| CoreError::from(error.to_string()))?;
        super::tasks::add(core, added).await?;
        return Ok(());
    }
    if matches!(change.kind.as_str(), "note" | "guardrail") {
        let mut rules = core.workspaces.project_rules(project);
        match (change.kind.as_str(), change.key.as_deref()) {
            ("note", _) => {
                rules.notes = [rules.notes.trim(), value.trim()]
                    .into_iter()
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            (_, Some("neverEdit")) => push_new(&mut rules.never_edit, value),
            (_, Some("askBefore")) => push_new(&mut rules.ask_before, value),
            (_, Some("neverRun")) => push_new(&mut rules.never_run, value),
            _ => {}
        }
        return core.workspaces.set_project_rules(project.to_owned(), rules);
    }
    let mut settings = core
        .workspaces
        .folder_settings(project)
        .remove(&change.folder)
        .unwrap_or_default();
    match change.kind.as_str() {
        "variable" => {
            let key = change.key.clone().unwrap_or_default();
            if value.is_empty() {
                settings.env.remove(&key);
            } else {
                settings.env.insert(key, value);
            }
        }
        "environment" => settings.environment = Some(value),
        "agentEnvironment" => settings.agent_environment = Some(value),
        "done" => push_new(&mut settings.done, value),
        "service" => push_new(&mut settings.services, value),
        _ => {}
    }
    core.workspaces
        .set_folder_settings(project.to_owned(), change.folder.clone(), settings)
}

fn push_new(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

#[derive(Deserialize)]
pub(crate) struct ListParams {
    project: String,
}

pub(crate) async fn list(core: &Arc<Core>, params: ListParams) -> CoreResult<Vec<Suggestion>> {
    let core = core.clone();
    blocking(move || Ok(core.workspaces.suggestions(&canonical(&params.project)))).await
}

/// Agents name the project by its real path; the window by the folder it opened.
fn canonical(project: &str) -> String {
    std::fs::canonicalize(project)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| project.to_owned())
}
