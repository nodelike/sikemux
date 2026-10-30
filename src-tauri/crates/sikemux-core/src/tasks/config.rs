//! A project's `sikemux.json`: the actions, tasks, preview and worktree hooks
//! it declares, checked before anything in it may run.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const CONFIG_FILE: &str = "sikemux.json";
pub const CONFIG_VERSION: u64 = 1;

const ROOT_FIELDS: &[&str] = &[
    "version", "$schema", "icon", "actions", "tasks", "preview", "worktree", "generate", "check",
];
const ACTION_FIELDS: &[&str] = &[
    "id",
    "label",
    "description",
    "command",
    "placement",
    "contexts",
    "keybinding",
];
const TASK_FIELDS: &[&str] = &["id", "label", "command", "cwd", "env"];
const PREVIEW_FIELDS: &[&str] = &["url", "command"];
const WORKTREE_FIELDS: &[&str] = &["onCreate"];
const HOOK_FIELDS: &[&str] = &["id", "label", "command"];
const GENERATE_FIELDS: &[&str] = &["command", "outputs"];
const MAX_OUTPUTS: usize = 50;
const MAX_CONFIG_BYTES: usize = 256 * 1024;
const MAX_ACTIONS: usize = 100;
const MAX_TASKS: usize = 100;
const MAX_HOOKS: usize = 20;
const MAX_TASK_ENV_ENTRIES: usize = 128;
const MAX_TASK_ENV_VALUE_LENGTH: usize = 8_192;
const MAX_TASK_ENV_TOTAL_LENGTH: usize = 65_536;
const UNSAFE_ENVIRONMENT_KEYS: &[&str] = &["__proto__", "constructor", "prototype"];
const KEYBINDING_MODIFIERS: &[&str] = &["Meta", "Ctrl", "Alt", "Shift"];
const KEYBINDING_PRIMARY_MODIFIERS: &[&str] = &["Meta", "Ctrl", "Alt"];
const PHYSICAL_KEY_CODES: &[&str] = &[
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "Backquote",
    "Backslash",
    "Backspace",
    "BracketLeft",
    "BracketRight",
    "Comma",
    "ContextMenu",
    "Delete",
    "End",
    "Enter",
    "Equal",
    "Escape",
    "Help",
    "Home",
    "Insert",
    "IntlBackslash",
    "IntlRo",
    "IntlYen",
    "Minus",
    "PageDown",
    "PageUp",
    "Pause",
    "Period",
    "PrintScreen",
    "Quote",
    "ScrollLock",
    "Semicolon",
    "Slash",
    "Space",
    "Tab",
];

static PATTERNED_KEY_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:Key[A-Z]|Digit[0-9]|F(?:[1-9]|1[0-9]|2[0-4])|Numpad(?:[0-9]|Add|Comma|Decimal|Divide|Enter|Equal|Multiply|Subtract))$",
    )
    .expect("key code pattern")
});

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActionPlacement {
    Background,
    Terminal,
    Split,
    Popup,
    Replace,
}

impl ActionPlacement {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "background" => Self::Background,
            "terminal" => Self::Terminal,
            "split" => Self::Split,
            "popup" => Self::Popup,
            "replace" => Self::Replace,
            _ => return None,
        })
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CommandContext {
    Project,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAction {
    pub id: String,
    pub label: String,
    pub description: String,
    pub command: String,
    pub placement: ActionPlacement,
    /// Empty makes the action available throughout its project.
    pub contexts: Vec<CommandContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTask {
    pub id: String,
    pub label: String,
    pub command: String,
    /// Relative to the project, with `/` separators; `.` is the project itself.
    pub cwd: String,
    #[serde(with = "env_map")]
    pub env: Vec<(String, String)>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeHook {
    pub id: String,
    pub label: String,
    pub command: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeConfig {
    pub on_create: Vec<WorktreeHook>,
}

/// What makes the files a project commits but builds from its other files,
/// and which files they are, so the core keeps them up to date on its own.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GenerateConfig {
    pub command: String,
    pub outputs: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub version: u64,
    /// A hint for editors; never fetched.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// An image path relative to the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub actions: Vec<ProjectAction>,
    pub tasks: Vec<ProjectTask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<ProjectPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate: Option<Box<GenerateConfig>>,
    /// What proves the project sound: run on each commit, and on a card's
    /// change when its pull request has no CI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigErrorCode {
    InvalidJson,
    InvalidType,
    MissingField,
    UnknownField,
    UnsupportedVersion,
    InvalidValue,
    DuplicateId,
    LimitExceeded,
    ReadFailed,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigError {
    /// Where in the file, as `$.tasks[0].cwd`.
    pub path: String,
    pub code: ConfigErrorCode,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrustSummary {
    pub requires_approval: bool,
    pub executable_entries: usize,
    pub reasons: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConfigLoad {
    Absent {
        path: String,
    },
    Invalid {
        path: String,
        errors: Vec<ConfigError>,
        /// Present when the file was read, so a changed invalid file can still be told apart.
        #[serde(skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    Valid {
        path: String,
        config: Box<ProjectConfig>,
        fingerprint: String,
        trust: TrustSummary,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustCommand {
    pub label: String,
    pub command: String,
}

/// Reads and checks `<project>/sikemux.json`.
pub fn load(project: &Path) -> ConfigLoad {
    let file = project.join(CONFIG_FILE);
    let path = file.to_string_lossy().into_owned();
    let source = match std::fs::read(&file) {
        Ok(source) => source,
        Err(error) if error.kind() == ErrorKind::NotFound => return ConfigLoad::Absent { path },
        Err(error) => {
            return ConfigLoad::Invalid {
                path,
                errors: vec![issue(
                    "$",
                    ConfigErrorCode::ReadFailed,
                    format!("Could not read {CONFIG_FILE}: {error}"),
                )],
                fingerprint: None,
            };
        }
    };
    let fingerprint = fingerprint(&source);
    if source.len() > MAX_CONFIG_BYTES {
        return ConfigLoad::Invalid {
            path,
            errors: vec![issue(
                "$",
                ConfigErrorCode::LimitExceeded,
                format!(
                    "Project configuration must be {} KiB or smaller.",
                    MAX_CONFIG_BYTES / 1024
                ),
            )],
            fingerprint: Some(fingerprint),
        };
    }
    let parsed = std::str::from_utf8(&source)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str::<Value>(text).map_err(|error| error.to_string()));
    let value = match parsed {
        Ok(value) => value,
        Err(detail) => {
            return ConfigLoad::Invalid {
                path,
                errors: vec![issue(
                    "$",
                    ConfigErrorCode::InvalidJson,
                    format!("Invalid JSON: {detail}"),
                )],
                fingerprint: Some(fingerprint),
            };
        }
    };
    match validate(&value) {
        Ok(config) => {
            let trust = trust_summary(&config);
            ConfigLoad::Valid {
                path,
                config: Box::new(config),
                fingerprint,
                trust,
            }
        }
        Err(errors) => ConfigLoad::Invalid {
            path,
            errors,
            fingerprint: Some(fingerprint),
        },
    }
}

/// Checks a parsed file, collecting every problem rather than stopping at the first.
pub fn validate(value: &Value) -> Result<ProjectConfig, Vec<ConfigError>> {
    let Some(root) = value.as_object() else {
        return Err(vec![issue(
            "$",
            ConfigErrorCode::InvalidType,
            "Project configuration must be a JSON object.",
        )]);
    };
    let mut errors = Vec::new();
    reject_unknown_fields(root, ROOT_FIELDS, "$", &mut errors);
    let version = root.get("version");
    if version.and_then(Value::as_f64) != Some(CONFIG_VERSION as f64) {
        errors.push(match version {
            None => issue(
                "$.version",
                ConfigErrorCode::MissingField,
                format!("Version is required. The current version is {CONFIG_VERSION}."),
            ),
            Some(version) => issue(
                "$.version",
                ConfigErrorCode::UnsupportedVersion,
                format!(
                    "Unsupported project configuration version “{}”; expected {CONFIG_VERSION}.",
                    version
                ),
            ),
        });
    }
    let schema = bounded_optional(
        root.get("$schema"),
        "$.$schema",
        "Schema URL",
        2_048,
        &mut errors,
    );
    let icon = parse_icon(root.get("icon"), &mut errors);
    let actions = parse_actions(root.get("actions"), &mut errors);
    let tasks = parse_tasks(root.get("tasks"), &mut errors);
    let preview = parse_preview(root.get("preview"), &mut errors);
    let worktree = parse_worktree(root.get("worktree"), &mut errors);
    let generate = parse_generate(root.get("generate"), &mut errors);
    let check = root
        .get("check")
        .and_then(|value| parse_command(Some(value), "$.check", &mut errors));
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ProjectConfig {
        version: CONFIG_VERSION,
        schema,
        icon,
        actions,
        tasks,
        preview,
        worktree,
        generate,
        check,
    })
}

pub fn trust_summary(config: &ProjectConfig) -> TrustSummary {
    let actions = config.actions.len();
    let tasks = config.tasks.len();
    let previews = usize::from(preview_command(config).is_some());
    let hooks = config
        .worktree
        .as_ref()
        .map_or(0, |worktree| worktree.on_create.len());
    let mut reasons = Vec::new();
    if actions > 0 {
        reasons.push(format!(
            "{actions} project {}",
            if actions == 1 { "action" } else { "actions" }
        ));
    }
    if tasks > 0 {
        reasons.push(format!(
            "{tasks} project {}",
            if tasks == 1 { "task" } else { "tasks" }
        ));
    }
    if previews > 0 {
        reasons.push("a preview command".to_owned());
    }
    if hooks > 0 {
        reasons.push(format!(
            "{hooks} worktree-create {}",
            if hooks == 1 { "hook" } else { "hooks" }
        ));
    }
    let generates = usize::from(config.generate.is_some());
    if generates > 0 {
        reasons.push("a command that regenerates committed files".to_owned());
    }
    let checks = usize::from(config.check.is_some());
    if checks > 0 {
        reasons.push("a check command".to_owned());
    }
    let executable_entries = actions + tasks + previews + hooks + generates + checks;
    TrustSummary {
        requires_approval: executable_entries > 0,
        executable_entries,
        reasons,
    }
}

/// "sha256:<hex>" of the file's bytes.
pub fn fingerprint(source: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(source)))
}

/// The one spelling the app records a shortcut in, or none when it is not a
/// shortcut an action may take.
pub fn normalize_keybinding(value: &str) -> Option<String> {
    let mut parts: Vec<&str> = value.split('+').collect();
    let code = parts.pop()?;
    if code.is_empty() || !is_physical_key_code(code) || parts.is_empty() {
        return None;
    }
    let mut modifiers = Vec::new();
    for modifier in parts {
        if !KEYBINDING_MODIFIERS.contains(&modifier) || modifiers.contains(&modifier) {
            return None;
        }
        modifiers.push(modifier);
    }
    if !KEYBINDING_PRIMARY_MODIFIERS
        .iter()
        .any(|primary| modifiers.contains(primary))
    {
        return None;
    }
    let mut ordered: Vec<&str> = KEYBINDING_MODIFIERS
        .iter()
        .copied()
        .filter(|modifier| modifiers.contains(modifier))
        .collect();
    ordered.push(code);
    Some(ordered.join("+"))
}

/// The commands a person approves when they trust the file.
pub fn trust_commands(config: &ProjectConfig) -> Vec<TrustCommand> {
    let actions = config.actions.iter().map(|action| TrustCommand {
        label: format!("Action · {}", action.label),
        command: action.command.clone(),
    });
    let tasks = config.tasks.iter().map(|task| TrustCommand {
        label: format!("Task · {}", task.label),
        command: task_command_line(task),
    });
    let preview = preview_command(config).map(|command| TrustCommand {
        label: "Preview".to_owned(),
        command: command.to_owned(),
    });
    let hooks = config
        .worktree
        .iter()
        .flat_map(|worktree| &worktree.on_create)
        .map(|hook| TrustCommand {
            label: format!("New worktree · {}", hook.label),
            command: hook.command.clone(),
        });
    actions.chain(tasks).chain(preview).chain(hooks).collect()
}

fn preview_command(config: &ProjectConfig) -> Option<&str> {
    config
        .preview
        .as_ref()?
        .command
        .as_deref()
        .filter(|command| !command.is_empty())
}

fn task_command_line(task: &ProjectTask) -> String {
    let mut words = Vec::new();
    if task.cwd != "." {
        words.push(format!("cd {} &&", task.cwd));
    }
    words.extend(
        task.env
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    words.push(task.command.clone());
    words.join(" ")
}

fn issue(
    path: impl Into<String>,
    code: ConfigErrorCode,
    message: impl Into<String>,
) -> ConfigError {
    ConfigError {
        path: path.into(),
        code,
        message: message.into(),
    }
}

fn reject_unknown_fields(
    record: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
    errors: &mut Vec<ConfigError>,
) {
    for key in record.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(issue(
                format!("{path}.{key}"),
                ConfigErrorCode::UnknownField,
                format!("Unknown field “{key}”."),
            ));
        }
    }
}

fn bounded_required(
    value: Option<&Value>,
    path: &str,
    label: &str,
    max_length: usize,
    errors: &mut Vec<ConfigError>,
) -> Option<String> {
    let Some(Value::String(text)) = value else {
        let code = if value.is_none() {
            ConfigErrorCode::MissingField
        } else {
            ConfigErrorCode::InvalidType
        };
        errors.push(issue(path, code, format!("{label} must be a string.")));
        return None;
    };
    let normalized = text.trim();
    if normalized.is_empty() {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidValue,
            format!("{label} cannot be empty."),
        ));
        return None;
    }
    if normalized.chars().count() > max_length {
        errors.push(issue(
            path,
            ConfigErrorCode::LimitExceeded,
            format!("{label} must be {max_length} characters or fewer."),
        ));
        return None;
    }
    if normalized.contains('\0') {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidValue,
            format!("{label} cannot contain a null byte."),
        ));
        return None;
    }
    Some(normalized.to_owned())
}

fn bounded_optional(
    value: Option<&Value>,
    path: &str,
    label: &str,
    max_length: usize,
    errors: &mut Vec<ConfigError>,
) -> Option<String> {
    value?;
    bounded_required(value, path, label, max_length, errors)
}

fn parse_id(value: Option<&Value>, path: &str, errors: &mut Vec<ConfigError>) -> Option<String> {
    let id = bounded_required(value, path, "ID", 64, errors)?;
    if !is_project_id(&id) {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidValue,
            "ID must start with a letter or number and contain only letters, numbers, dots, dashes, or underscores.",
        ));
        return None;
    }
    Some(id)
}

fn parse_command(
    value: Option<&Value>,
    path: &str,
    errors: &mut Vec<ConfigError>,
) -> Option<String> {
    bounded_required(value, path, "Command", 8_000, errors)
}

fn note_id(
    ids: &mut HashSet<String>,
    id: Option<&String>,
    path: &str,
    kind: &str,
    errors: &mut Vec<ConfigError>,
) {
    let Some(id) = id else { return };
    if !ids.insert(id.clone()) {
        errors.push(issue(
            format!("{path}.id"),
            ConfigErrorCode::DuplicateId,
            format!("{kind} ID “{id}” is duplicated."),
        ));
    }
}

fn parse_keybinding(
    value: Option<&Value>,
    path: &str,
    errors: &mut Vec<ConfigError>,
) -> Option<String> {
    let binding = bounded_optional(value, path, "Keybinding", 100, errors)?;
    let normalized = normalize_keybinding(&binding);
    if normalized.is_none() {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidValue,
            "Keybinding must contain Meta, Ctrl, or Alt plus one supported physical key code, with optional Shift.",
        ));
    }
    normalized
}

fn parse_actions(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Vec<ProjectAction> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(candidates) = value.as_array() else {
        errors.push(issue(
            "$.actions",
            ConfigErrorCode::InvalidType,
            "Actions must be an array.",
        ));
        return Vec::new();
    };
    if candidates.len() > MAX_ACTIONS {
        errors.push(issue(
            "$.actions",
            ConfigErrorCode::LimitExceeded,
            format!("At most {MAX_ACTIONS} actions are allowed."),
        ));
    }
    let mut actions = Vec::new();
    let mut ids = HashSet::new();
    for (index, candidate) in candidates.iter().take(MAX_ACTIONS).enumerate() {
        let path = format!("$.actions[{index}]");
        let Some(candidate) = candidate.as_object() else {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidType,
                "Action must be an object.",
            ));
            continue;
        };
        reject_unknown_fields(candidate, ACTION_FIELDS, &path, errors);
        let id = parse_id(candidate.get("id"), &format!("{path}.id"), errors);
        let label = bounded_required(
            candidate.get("label"),
            &format!("{path}.label"),
            "Label",
            120,
            errors,
        );
        let description = bounded_optional(
            candidate.get("description"),
            &format!("{path}.description"),
            "Description",
            240,
            errors,
        )
        .unwrap_or_default();
        let command = parse_command(candidate.get("command"), &format!("{path}.command"), errors);

        let mut placement = ActionPlacement::Terminal;
        if let Some(raw) = candidate.get("placement") {
            match raw.as_str().and_then(ActionPlacement::parse) {
                Some(parsed) => placement = parsed,
                None => errors.push(issue(
                    format!("{path}.placement"),
                    ConfigErrorCode::InvalidValue,
                    "Placement must be background, terminal, split, popup, or replace.",
                )),
            }
        }

        let mut contexts = Vec::new();
        if let Some(raw) = candidate.get("contexts") {
            match raw.as_array() {
                None => errors.push(issue(
                    format!("{path}.contexts"),
                    ConfigErrorCode::InvalidType,
                    "Contexts must be an array.",
                )),
                Some(list) => {
                    for (context_index, context) in list.iter().enumerate() {
                        if context.as_str() != Some("project") {
                            errors.push(issue(
                                format!("{path}.contexts[{context_index}]"),
                                ConfigErrorCode::InvalidValue,
                                "Project action context must be \"project\".",
                            ));
                        } else if !contexts.contains(&CommandContext::Project) {
                            contexts.push(CommandContext::Project);
                        }
                    }
                }
            }
        }

        let keybinding = parse_keybinding(
            candidate.get("keybinding"),
            &format!("{path}.keybinding"),
            errors,
        );
        note_id(&mut ids, id.as_ref(), &path, "Action", errors);
        if let (Some(id), Some(label), Some(command)) = (id, label, command) {
            actions.push(ProjectAction {
                id,
                label,
                description,
                command,
                placement,
                contexts,
                keybinding,
            });
        }
    }
    actions
}

fn parse_task_cwd(value: Option<&Value>, path: &str, errors: &mut Vec<ConfigError>) -> String {
    let cwd = bounded_optional(value, path, "Task working directory", 512, errors)
        .unwrap_or_else(|| ".".to_owned());
    let slashed = cwd.replace('\\', "/");
    let trimmed = slashed.trim_end_matches('/');
    let normalized = if trimmed.is_empty() { "." } else { trimmed };
    let leaves_project = normalized.chars().any(is_control_character)
        || normalized.starts_with('/')
        || has_drive_prefix(normalized)
        || normalized.starts_with("~/")
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == ".." || (part == "." && normalized != "."));
    if leaves_project {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidValue,
            "Task working directory must stay within the project and contain no dot segments.",
        ));
        return ".".to_owned();
    }
    normalized.to_owned()
}

fn parse_task_env(
    value: Option<&Value>,
    path: &str,
    errors: &mut Vec<ConfigError>,
) -> Vec<(String, String)> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(record) = value.as_object() else {
        errors.push(issue(
            path,
            ConfigErrorCode::InvalidType,
            "Task environment must be an object of string values.",
        ));
        return Vec::new();
    };
    let keys: Vec<&str> = record.keys().map(String::as_str).collect();
    if keys.len() > MAX_TASK_ENV_ENTRIES {
        errors.push(issue(
            path,
            ConfigErrorCode::LimitExceeded,
            format!("Task environment may contain at most {MAX_TASK_ENV_ENTRIES} entries."),
        ));
    }
    let mut total_length = 0;
    let mut env = Vec::new();
    for key in keys.into_iter().take(MAX_TASK_ENV_ENTRIES) {
        let entry_path = format!("{path}.{key}");
        if !is_environment_key(key) || UNSAFE_ENVIRONMENT_KEYS.contains(&key) {
            errors.push(issue(
                entry_path,
                ConfigErrorCode::InvalidValue,
                "Environment variable names must use letters, numbers, and underscores.",
            ));
            continue;
        }
        let Some(raw) = record.get(key).and_then(Value::as_str) else {
            errors.push(issue(
                entry_path,
                ConfigErrorCode::InvalidType,
                "Environment variable values must be strings.",
            ));
            continue;
        };
        if raw.contains('\0') {
            errors.push(issue(
                entry_path,
                ConfigErrorCode::InvalidValue,
                "Environment variable values cannot contain a null byte.",
            ));
            continue;
        }
        let length = raw.chars().count();
        if length > MAX_TASK_ENV_VALUE_LENGTH {
            errors.push(issue(
                entry_path,
                ConfigErrorCode::LimitExceeded,
                format!("Environment variable values must be {MAX_TASK_ENV_VALUE_LENGTH} characters or fewer."),
            ));
            continue;
        }
        total_length += key.chars().count() + length;
        if total_length > MAX_TASK_ENV_TOTAL_LENGTH {
            errors.push(issue(
                path,
                ConfigErrorCode::LimitExceeded,
                format!("Task environment must be {MAX_TASK_ENV_TOTAL_LENGTH} characters or fewer in total."),
            ));
            break;
        }
        env.push((key.to_owned(), raw.to_owned()));
    }
    env
}

fn parse_tasks(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Vec<ProjectTask> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(candidates) = value.as_array() else {
        errors.push(issue(
            "$.tasks",
            ConfigErrorCode::InvalidType,
            "Tasks must be an array.",
        ));
        return Vec::new();
    };
    if candidates.len() > MAX_TASKS {
        errors.push(issue(
            "$.tasks",
            ConfigErrorCode::LimitExceeded,
            format!("At most {MAX_TASKS} tasks are allowed."),
        ));
    }
    let mut tasks = Vec::new();
    let mut ids = HashSet::new();
    for (index, candidate) in candidates.iter().take(MAX_TASKS).enumerate() {
        let path = format!("$.tasks[{index}]");
        let Some(candidate) = candidate.as_object() else {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidType,
                "Task must be an object.",
            ));
            continue;
        };
        reject_unknown_fields(candidate, TASK_FIELDS, &path, errors);
        let id = parse_id(candidate.get("id"), &format!("{path}.id"), errors);
        let label = bounded_required(
            candidate.get("label"),
            &format!("{path}.label"),
            "Label",
            120,
            errors,
        );
        let command = parse_command(candidate.get("command"), &format!("{path}.command"), errors);
        let cwd = parse_task_cwd(candidate.get("cwd"), &format!("{path}.cwd"), errors);
        let env = parse_task_env(candidate.get("env"), &format!("{path}.env"), errors);
        note_id(&mut ids, id.as_ref(), &path, "Task", errors);
        if let (Some(id), Some(label), Some(command)) = (id, label, command) {
            tasks.push(ProjectTask {
                id,
                label,
                command,
                cwd,
                env,
            });
        }
    }
    tasks
}

fn parse_preview(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Option<ProjectPreview> {
    let Some(record) = value?.as_object() else {
        errors.push(issue(
            "$.preview",
            ConfigErrorCode::InvalidType,
            "Preview must be an object.",
        ));
        return None;
    };
    reject_unknown_fields(record, PREVIEW_FIELDS, "$.preview", errors);
    let url = bounded_optional(
        record.get("url"),
        "$.preview.url",
        "Preview URL",
        2_048,
        errors,
    );
    let command = bounded_optional(
        record.get("command"),
        "$.preview.command",
        "Command",
        8_000,
        errors,
    );
    if url.is_none() && command.is_none() {
        errors.push(issue(
            "$.preview",
            ConfigErrorCode::MissingField,
            "Preview must define at least a URL or command.",
        ));
        return None;
    }
    if let Some(url) = &url {
        match url::Url::parse(url) {
            Ok(parsed) => {
                if parsed.scheme() != "http" && parsed.scheme() != "https" {
                    errors.push(issue(
                        "$.preview.url",
                        ConfigErrorCode::InvalidValue,
                        "Preview URL must use HTTP or HTTPS.",
                    ));
                }
                if !parsed.username().is_empty()
                    || parsed
                        .password()
                        .is_some_and(|password| !password.is_empty())
                {
                    errors.push(issue(
                        "$.preview.url",
                        ConfigErrorCode::InvalidValue,
                        "Preview URL cannot contain embedded credentials.",
                    ));
                }
            }
            Err(_) => {
                errors.push(issue(
                    "$.preview.url",
                    ConfigErrorCode::InvalidValue,
                    "Preview URL must be an absolute URL.",
                ));
            }
        }
    }
    Some(ProjectPreview { url, command })
}

fn parse_worktree(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Option<WorktreeConfig> {
    let Some(record) = value?.as_object() else {
        errors.push(issue(
            "$.worktree",
            ConfigErrorCode::InvalidType,
            "Worktree configuration must be an object.",
        ));
        return None;
    };
    reject_unknown_fields(record, WORKTREE_FIELDS, "$.worktree", errors);
    let on_create = record.get("onCreate");
    let Some(candidates) = on_create.and_then(Value::as_array) else {
        let code = if on_create.is_none() {
            ConfigErrorCode::MissingField
        } else {
            ConfigErrorCode::InvalidType
        };
        errors.push(issue(
            "$.worktree.onCreate",
            code,
            "onCreate must be an array.",
        ));
        return None;
    };
    if candidates.len() > MAX_HOOKS {
        errors.push(issue(
            "$.worktree.onCreate",
            ConfigErrorCode::LimitExceeded,
            format!("At most {MAX_HOOKS} worktree hooks are allowed."),
        ));
    }
    let mut hooks = Vec::new();
    let mut ids = HashSet::new();
    for (index, candidate) in candidates.iter().take(MAX_HOOKS).enumerate() {
        let path = format!("$.worktree.onCreate[{index}]");
        let Some(candidate) = candidate.as_object() else {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidType,
                "Worktree hook must be an object.",
            ));
            continue;
        };
        reject_unknown_fields(candidate, HOOK_FIELDS, &path, errors);
        let id = parse_id(candidate.get("id"), &format!("{path}.id"), errors);
        let label = bounded_optional(
            candidate.get("label"),
            &format!("{path}.label"),
            "Label",
            120,
            errors,
        )
        .or_else(|| id.clone());
        let command = parse_command(candidate.get("command"), &format!("{path}.command"), errors);
        note_id(&mut ids, id.as_ref(), &path, "Worktree hook", errors);
        if let (Some(id), Some(label), Some(command)) = (id, label, command) {
            hooks.push(WorktreeHook { id, label, command });
        }
    }
    Some(WorktreeConfig { on_create: hooks })
}

fn parse_generate(
    value: Option<&Value>,
    errors: &mut Vec<ConfigError>,
) -> Option<Box<GenerateConfig>> {
    let Some(record) = value?.as_object() else {
        errors.push(issue(
            "$.generate",
            ConfigErrorCode::InvalidType,
            "Generate must be an object.",
        ));
        return None;
    };
    reject_unknown_fields(record, GENERATE_FIELDS, "$.generate", errors);
    let command = parse_command(record.get("command"), "$.generate.command", errors);
    let Some(listed) = record.get("outputs").and_then(Value::as_array) else {
        errors.push(issue(
            "$.generate.outputs",
            ConfigErrorCode::MissingField,
            "outputs must list the files the command writes.",
        ));
        return None;
    };
    if listed.is_empty() || listed.len() > MAX_OUTPUTS {
        errors.push(issue(
            "$.generate.outputs",
            ConfigErrorCode::LimitExceeded,
            format!("outputs must list between 1 and {MAX_OUTPUTS} files."),
        ));
        return None;
    }
    let mut outputs = Vec::new();
    for (index, output) in listed.iter().enumerate() {
        let path = format!("$.generate.outputs[{index}]");
        let Some(output) = bounded_required(Some(output), &path, "Output", 512, errors) else {
            continue;
        };
        let normalized = output.replace('\\', "/");
        if normalized.starts_with('/')
            || has_drive_prefix(&normalized)
            || normalized
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidValue,
                "Each output must be a normalized project-relative file path without dot segments.",
            ));
            continue;
        }
        outputs.push(normalized);
    }
    Some(Box::new(GenerateConfig {
        command: command?,
        outputs,
    }))
}

fn parse_icon(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Option<String> {
    let icon = bounded_optional(value, "$.icon", "Icon path", 512, errors)?;
    let normalized = icon.replace('\\', "/");
    if normalized.starts_with('/')
        || has_drive_prefix(&normalized)
        || normalized.starts_with("~/")
        || normalized
            .split('/')
            .any(|part| part == ".." || part == ".")
    {
        errors.push(issue(
            "$.icon",
            ConfigErrorCode::InvalidValue,
            "Icon must be a normalized project-relative path without dot segments.",
        ));
        return None;
    }
    Some(normalized)
}

fn is_physical_key_code(value: &str) -> bool {
    PATTERNED_KEY_CODE.is_match(value) || PHYSICAL_KEY_CODES.contains(&value)
}

fn is_project_id(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && value.len() <= 64
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn is_environment_key(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_control_character(character: char) -> bool {
    matches!(u32::from(character), 0..=31 | 127..=159)
}

fn has_drive_prefix(path: &str) -> bool {
    matches!(path.as_bytes(), [letter, b':', b'/', ..] if letter.is_ascii_alphabetic())
}

mod env_map {
    use serde::Serializer;

    pub fn serialize<S: Serializer>(
        env: &[(String, String)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(env.iter().map(|(name, value)| (name, value)))
    }
}
