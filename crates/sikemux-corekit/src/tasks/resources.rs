//! What a project's `sikemux.json` names as used by one agent at a time, such as the app on a
//! fixed port, and what its processes and heavy commands need of them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::config::{ConfigError, ConfigErrorCode};

const FIELDS: &[&str] = &["label", "count", "port"];
const MAX_RESOURCES: usize = 32;
const MAX_COUNT: u64 = 64;
const MAX_NEEDS: usize = 8;
const MAX_LABEL_CHARS: usize = 120;
/// What every project has without naming it.
pub const SCREEN: &str = "screen";
/// Held through the simulator tools, never named in `needs`.
pub const SIMULATOR: &str = "simulator";

/// A thing only so many agents use at once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
    pub label: String,
    /// How many may hold it at once.
    pub count: u32,
    /// The port it listens on, which must be free before it is handed out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// The built-in `screen`: the front of the person's screen.
pub fn screen() -> Resource {
    Resource {
        name: SCREEN.to_owned(),
        label: "Your screen".to_owned(),
        count: 1,
        port: None,
    }
}

pub(super) fn parse(value: Option<&Value>, errors: &mut Vec<ConfigError>) -> Vec<Resource> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(named) = value.as_object() else {
        errors.push(issue(
            "$.resources",
            ConfigErrorCode::InvalidType,
            "resources must be an object of names, each with a label.",
        ));
        return Vec::new();
    };
    if named.len() > MAX_RESOURCES {
        errors.push(issue(
            "$.resources",
            ConfigErrorCode::LimitExceeded,
            format!("At most {MAX_RESOURCES} resources are allowed."),
        ));
    }
    let mut resources: Vec<Resource> = named
        .iter()
        .take(MAX_RESOURCES)
        .filter_map(|(name, entry)| one(name, entry, errors))
        .collect();
    resources.sort_by(|left, right| left.name.cmp(&right.name));
    resources
}

fn one(name: &str, entry: &Value, errors: &mut Vec<ConfigError>) -> Option<Resource> {
    let path = format!("$.resources.{name}");
    if !is_name(name) {
        errors.push(issue(
            &path,
            ConfigErrorCode::InvalidValue,
            "A resource's name must start with a letter or number and contain only letters, numbers, dots, dashes, or underscores.",
        ));
        return None;
    }
    if name == SCREEN || name == SIMULATOR {
        errors.push(issue(
            &path,
            ConfigErrorCode::InvalidValue,
            format!("{name} is built in; name it in needs without listing it here."),
        ));
        return None;
    }
    let Some(entry) = entry.as_object() else {
        errors.push(issue(
            &path,
            ConfigErrorCode::InvalidType,
            "A resource must be an object with a label.",
        ));
        return None;
    };
    reject_unknown(entry, &path, errors);
    let label = match entry.get("label").and_then(Value::as_str).map(str::trim) {
        Some(label) if !label.is_empty() && label.chars().count() <= MAX_LABEL_CHARS => label,
        _ => {
            errors.push(issue(
                format!("{path}.label"),
                ConfigErrorCode::InvalidValue,
                format!("A resource needs a label of 1 to {MAX_LABEL_CHARS} characters."),
            ));
            return None;
        }
    };
    let count = match entry.get("count") {
        None => 1,
        Some(count) => match count
            .as_u64()
            .filter(|count| (1..=MAX_COUNT).contains(count))
        {
            Some(count) => count as u32,
            None => {
                errors.push(issue(
                    format!("{path}.count"),
                    ConfigErrorCode::InvalidValue,
                    format!("count must be a whole number from 1 to {MAX_COUNT}."),
                ));
                return None;
            }
        },
    };
    let port = super::checkout::parse_port(entry.get("port"), &format!("{path}.port"), errors);
    Some(Resource {
        name: name.to_owned(),
        label: label.to_owned(),
        count,
        port,
    })
}

/// The `needs` list at `path`, each a resource the project names or a built-in one.
pub(super) fn parse_needs(
    value: Option<&Value>,
    path: &str,
    resources: &[Resource],
    errors: &mut Vec<ConfigError>,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let listed = match value.as_array() {
        Some(listed) if listed.len() <= MAX_NEEDS => listed,
        _ => {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidValue,
                format!("needs must list at most {MAX_NEEDS} resource names."),
            ));
            return Vec::new();
        }
    };
    let mut needs: Vec<String> = Vec::new();
    for (index, name) in listed.iter().enumerate() {
        let at = format!("{path}[{index}]");
        let Some(name) = name.as_str() else {
            errors.push(issue(
                at,
                ConfigErrorCode::InvalidType,
                "A need must be a resource's name.",
            ));
            continue;
        };
        if name == SIMULATOR {
            errors.push(issue(
                at,
                ConfigErrorCode::InvalidValue,
                "simulator is held through the simulator tools, so it is not named in needs.",
            ));
            continue;
        }
        if name != SCREEN && !resources.iter().any(|resource| resource.name == name) {
            errors.push(issue(
                at,
                ConfigErrorCode::InvalidValue,
                format!("{name} is not in resources."),
            ));
            continue;
        }
        if !needs.iter().any(|need| need == name) {
            needs.push(name.to_owned());
        }
    }
    needs
}

fn is_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn reject_unknown(record: &Map<String, Value>, path: &str, errors: &mut Vec<ConfigError>) {
    for key in record.keys().filter(|key| !FIELDS.contains(&key.as_str())) {
        errors.push(issue(
            format!("{path}.{key}"),
            ConfigErrorCode::UnknownField,
            format!("Unknown field “{key}”."),
        ));
    }
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
