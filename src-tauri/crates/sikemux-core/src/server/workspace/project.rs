//! A project's own settings, kept in the saved workspace by the project's real
//! path: each folder's environment, Done means and services, the rules every
//! agent there follows, and what agents suggested. They change one named change
//! at a time, so two changes made together never undo each other, and none of
//! them is ever sent to a device, since they can hold secrets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::{too_long, Workspaces, MAX_FIELD_CHARS, MAX_PROJECTS, MAX_TITLE_CHARS};
use crate::server::suggestions::{Status, Suggestion};
use crate::server::{CoreError, CoreResult};

const MAX_FOLDER_VARIABLES: usize = 256;
const MAX_VARIABLE_CHARS: usize = 8192;
const MAX_LIST: usize = 32;
const MAX_NOTES_CHARS: usize = 4000;
const MAX_GUARDRAILS: usize = 64;
const MAX_SUGGESTIONS: usize = 30;
const MAX_PENDING_SUGGESTIONS: usize = 10;

/// The environment processes started in one folder of a project get, and what
/// an agent's work there must pass.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderSettings {
    /// Whose environment files are read: absent reads none, empty only the ones every
    /// environment shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Whose environment files agents get, when not the same as the person's. A
    /// production-looking environment never reaches an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_environment: Option<String>,
    /// Set by the person, over what the files say.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The tasks an agent's work in the folder must pass before it is done, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<String>,
    /// docker-compose services the folder's tasks need running first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

/// What the person tells every agent working in a project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRules {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Agents run every folder's checks, not only those of the folders they changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check_every_folder: bool,
    /// An agent whose check fails stops and says so, instead of fixing it first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_on_failure: bool,
    /// Paths, as globs, agents must not change without asking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub never_edit: Vec<String>,
    /// Commands, as patterns, an agent asks the person before running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask_before: Vec<String>,
    /// Commands, as patterns, agents never run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub never_run: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Guardrail {
    NeverEdit,
    AskBefore,
    NeverRun,
}

/// One change to a project's settings. The folder ones name the folder they change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum Change {
    /// Sets a variable, or with no value removes it.
    Variable {
        key: String,
        value: Option<String>,
    },
    Environment {
        value: Option<String>,
    },
    AgentEnvironment {
        value: Option<String>,
    },
    Done {
        task: String,
        on: bool,
    },
    Service {
        name: String,
        on: bool,
    },
    Notes {
        text: String,
    },
    AddNote {
        text: String,
    },
    CheckEveryFolder {
        on: bool,
    },
    StopOnFailure {
        on: bool,
    },
    Guardrail {
        list: Guardrail,
        pattern: String,
        on: bool,
    },
}

impl Change {
    fn of_folder(&self) -> bool {
        matches!(
            self,
            Change::Variable { .. }
                | Change::Environment { .. }
                | Change::AgentEnvironment { .. }
                | Change::Done { .. }
                | Change::Service { .. }
        )
    }
}

/// The key a project's settings are kept under: its real path, so a symlink to
/// it, an agent's view of it and the window's all find the same settings.
pub(crate) fn project_key(project: impl AsRef<Path>) -> String {
    let project = project.as_ref();
    std::fs::canonicalize(project)
        .unwrap_or_else(|_| project.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// `.` for the project root, else the name of a folder directly inside it.
pub(crate) fn is_folder_name(folder: &str) -> bool {
    folder == "." || (!folder.is_empty() && !folder.contains('/') && !folder.starts_with('.'))
}

fn is_variable_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

fn toggle(list: &mut Vec<String>, item: String, on: bool) {
    list.retain(|kept| *kept != item);
    if on {
        list.push(item);
    }
}

fn change_folder(settings: &mut FolderSettings, change: Change) -> CoreResult<()> {
    match change {
        Change::Variable { key, value } => {
            if !is_variable_name(&key) || too_long(&key, MAX_TITLE_CHARS) {
                return Err(format!("{key} cannot name an environment variable").into());
            }
            match value {
                Some(value) if too_long(&value, MAX_VARIABLE_CHARS) => {
                    return Err(format!("{key}'s value is longer than the core keeps").into())
                }
                Some(value) => {
                    settings.env.insert(key, value);
                }
                None => {
                    settings.env.remove(&key);
                }
            }
            if settings.env.len() > MAX_FOLDER_VARIABLES {
                return Err("more variables than the core keeps for one folder".into());
            }
        }
        Change::Environment { value } | Change::AgentEnvironment { value }
            if value.as_ref().is_some_and(|name| too_long(name, 64)) =>
        {
            return Err("that environment's name is longer than the core keeps".into())
        }
        Change::Environment { value } => settings.environment = value,
        Change::AgentEnvironment { value } => settings.agent_environment = value,
        Change::Done { task, .. } | Change::Service { name: task, .. }
            if too_long(&task, MAX_FIELD_CHARS) =>
        {
            return Err("that name is longer than the core keeps".into())
        }
        Change::Done { task, on } => toggle(&mut settings.done, task, on),
        Change::Service { name, on } => toggle(&mut settings.services, name, on),
        _ => {}
    }
    if settings.done.len() > MAX_LIST || settings.services.len() > MAX_LIST {
        return Err("more than the core keeps for one folder".into());
    }
    Ok(())
}

fn change_rules(rules: &mut ProjectRules, change: Change) -> CoreResult<()> {
    match change {
        Change::Notes { text } => rules.notes = text,
        Change::AddNote { text } => {
            rules.notes = [rules.notes.trim(), text.trim()]
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
        }
        Change::CheckEveryFolder { on } => rules.check_every_folder = on,
        Change::StopOnFailure { on } => rules.stop_on_failure = on,
        Change::Guardrail { list, pattern, on } => {
            let pattern = pattern.trim().to_owned();
            if pattern.is_empty() || too_long(&pattern, MAX_FIELD_CHARS) {
                return Err("a guardrail needs a pattern the core can keep".into());
            }
            let list = match list {
                Guardrail::NeverEdit => &mut rules.never_edit,
                Guardrail::AskBefore => &mut rules.ask_before,
                Guardrail::NeverRun => &mut rules.never_run,
            };
            toggle(list, pattern, on);
        }
        _ => {}
    }
    let guardrails = rules.never_edit.len() + rules.ask_before.len() + rules.never_run.len();
    if guardrails > MAX_GUARDRAILS || too_long(&rules.notes, MAX_NOTES_CHARS) {
        return Err("more rules than the core keeps for one project".into());
    }
    Ok(())
}

impl Workspaces {
    pub(crate) fn folder_settings(&self, project: &str) -> BTreeMap<String, FolderSettings> {
        let key = project_key(project);
        self.published
            .locked()
            .saved
            .folder_settings
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn project_rules(&self, project: &str) -> ProjectRules {
        let key = project_key(project);
        self.published
            .locked()
            .saved
            .project_rules
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// Applies one change to the project's settings as they are now; `folder`
    /// names the folder a folder change is for.
    pub(crate) fn change_project(
        &self,
        project: &str,
        folder: &str,
        change: Change,
    ) -> CoreResult<()> {
        let key = project_key(project);
        if too_long(&key, MAX_FIELD_CHARS) || !is_folder_name(folder) {
            return Err(format!(
                "{folder} is not the project root (.) or a folder directly inside it"
            )
            .into());
        }
        self.change(|saved| {
            let known =
                saved.folder_settings.contains_key(&key) || saved.project_rules.contains_key(&key);
            let kept = saved.folder_settings.len().max(saved.project_rules.len());
            if !known && kept >= MAX_PROJECTS {
                return Err("settings for more projects than the core keeps".into());
            }
            if change.of_folder() {
                let folders = saved.folder_settings.entry(key.clone()).or_default();
                let mut settings = folders.get(folder).cloned().unwrap_or_default();
                change_folder(&mut settings, change)?;
                if settings == FolderSettings::default() {
                    folders.remove(folder);
                } else {
                    folders.insert(folder.to_owned(), settings);
                }
                if folders.is_empty() {
                    saved.folder_settings.remove(&key);
                }
            } else {
                let mut rules = saved.project_rules.get(&key).cloned().unwrap_or_default();
                change_rules(&mut rules, change)?;
                if rules == ProjectRules::default() {
                    saved.project_rules.remove(&key);
                } else {
                    saved.project_rules.insert(key.clone(), rules);
                }
            }
            Ok(())
        })
    }

    pub(crate) fn suggestions(&self, project: &str) -> Vec<Suggestion> {
        let key = project_key(project);
        self.published
            .locked()
            .saved
            .suggestions
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// Keeps a new suggestion, forgetting the oldest settled ones past the limit.
    pub(crate) fn keep_suggestion(&self, project: &str, suggestion: Suggestion) -> CoreResult<()> {
        let key = project_key(project);
        self.change(|saved| {
            let kept = saved.suggestions.entry(key).or_default();
            if kept.iter().filter(|known| known.pending()).count() >= MAX_PENDING_SUGGESTIONS {
                return Err(
                    "this project already has as many suggestions waiting as it keeps".into(),
                );
            }
            kept.push(suggestion);
            while kept.len() > MAX_SUGGESTIONS {
                match kept.iter().position(|known| !known.pending()) {
                    Some(oldest) => kept.remove(oldest),
                    None => break,
                };
            }
            Ok(())
        })
    }

    /// Marks the changes in `applied` applied and the rest of the suggestion discarded.
    pub(crate) fn settle_suggestion(
        &self,
        project: &str,
        id: &str,
        applied: &[String],
    ) -> CoreResult<()> {
        let key = project_key(project);
        self.change(|saved| {
            let suggestion = saved
                .suggestions
                .get_mut(&key)
                .and_then(|kept| kept.iter_mut().find(|known| known.id == id))
                .ok_or_else(|| CoreError::from("that suggestion is gone"))?;
            for change in &mut suggestion.changes {
                if change.status == Status::Pending {
                    change.status = if applied.contains(&change.id) {
                        Status::Applied
                    } else {
                        Status::Discarded
                    };
                }
            }
            Ok(())
        })
    }

    /// The project holding `cwd`, a worktree counting as its main checkout, and the
    /// folder directly inside it that `cwd` is in.
    fn project_of(&self, cwd: &Path) -> Option<(PathBuf, Option<String>)> {
        let path = crate::server::names::in_main_checkout_or_self(cwd);
        let published = self.published.locked();
        let saved = &published.saved;
        let project = saved
            .folder_settings
            .keys()
            .chain(saved.project_rules.keys())
            .map(PathBuf::from)
            .filter(|project| path.starts_with(project))
            .max_by_key(|project| project.as_os_str().len())?;
        let inner = path
            .strip_prefix(&project)
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|first| first.as_os_str().to_string_lossy().into_owned());
        Some((project, inner))
    }

    /// What a process started in `cwd` gets from its project's folder settings: the root's,
    /// then those of the folder it is in. An agent gets the environment chosen for agents,
    /// and never the files of one that looks like production.
    pub(crate) fn folder_environment(
        &self,
        cwd: &Path,
        for_agent: bool,
    ) -> BTreeMap<String, String> {
        let Some((project, inner)) = self.project_of(cwd) else {
            return BTreeMap::new();
        };
        let folders = self.folder_settings(&project.to_string_lossy());
        let mut env = BTreeMap::new();
        for folder in std::iter::once(".".to_owned()).chain(inner) {
            let Some(settings) = folders.get(&folder) else {
                continue;
            };
            let environment = if for_agent {
                settings
                    .agent_environment
                    .as_ref()
                    .or(settings.environment.as_ref())
                    .filter(|name| !crate::tasks::folders::is_production(name))
            } else {
                settings.environment.as_ref()
            };
            if let Some(environment) = environment {
                env.extend(crate::tasks::folders::variables(
                    &project,
                    &folder,
                    environment,
                ));
            }
            env.extend(settings.env.clone());
        }
        env
    }

    /// What a run in `cwd` gets when it asks for `environment` in place of its folder's:
    /// that environment's files in the project root and in the folder, each followed by the
    /// person's own variables there. Fails when neither has the environment.
    pub(crate) fn run_environment(
        &self,
        project: &Path,
        cwd: &Path,
        environment: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        let inner = cwd
            .strip_prefix(project)
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|first| first.as_os_str().to_string_lossy().into_owned());
        let chain: Vec<String> = std::iter::once(".".to_owned()).chain(inner).collect();
        let known: BTreeSet<String> = chain
            .iter()
            .flat_map(|folder| crate::tasks::folders::folder_at(project, folder).environments)
            .map(|found| found.name)
            .collect();
        if !known.contains(environment) {
            let place = chain.last().map_or(".", String::as_str);
            return Err(if known.is_empty() {
                format!("{place} has no environment files, so it cannot run on {environment}")
            } else {
                format!(
                    "{place} has no {environment} environment; it has {}",
                    known.into_iter().collect::<Vec<_>>().join(", ")
                )
            });
        }
        let settings = self.folder_settings(&project.to_string_lossy());
        let mut env = BTreeMap::new();
        for folder in &chain {
            env.extend(crate::tasks::folders::variables(
                project,
                folder,
                environment,
            ));
            if let Some(chosen) = settings.get(folder) {
                env.extend(chosen.env.clone());
            }
        }
        Ok(env)
    }

    /// The guardrails of the project holding `cwd`.
    pub(crate) fn rules_for(&self, cwd: &Path) -> ProjectRules {
        self.project_of(cwd)
            .map(|(project, _)| self.project_rules(&project.to_string_lossy()))
            .unwrap_or_default()
    }

    /// The compose folder and the services the folder of `cwd` needs running, its own
    /// choice or else the project root's.
    pub(crate) fn services_for(&self, cwd: &Path) -> Option<(PathBuf, Vec<String>)> {
        let (project, inner) = self.project_of(cwd)?;
        let folders = self.folder_settings(&project.to_string_lossy());
        let folder = inner
            .filter(|inner| {
                folders
                    .get(inner)
                    .is_some_and(|settings| !settings.services.is_empty())
            })
            .unwrap_or_else(|| ".".to_owned());
        let services = folders.get(&folder)?.services.clone();
        if services.is_empty() {
            return None;
        }
        let dir = crate::server::services::compose_dir_for(&project, &folder)?;
        Some((dir, services))
    }
}
