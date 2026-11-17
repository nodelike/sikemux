//! What a checkout's processes are given: its name, its folder, a port per name the project's
//! `sikemux.json` lists, and the project's own environment with those filled in.

use std::path::Path;

use serde_json::{Map, Value};

use super::config::{ConfigError, ConfigErrorCode, ProjectConfig};

/// The name of the project's own folder, which keeps the ports `sikemux.json` states.
pub const MAIN: &str = "main";
/// The most names a project may give ports, which is how many ports each checkout has.
pub const MAX_PORT_NAMES: usize = 10;
pub const MAX_NAME_CHARS: usize = 32;

pub const CHECKOUT_VAR: &str = "SIKEMUX_CHECKOUT";
pub const ROOT_VAR: &str = "SIKEMUX_ROOT";
pub const PORT_VAR: &str = "SIKEMUX_PORT";

/// A checkout as its processes see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    /// The first port of its own ten; none in the project's own folder.
    pub first_port: Option<u16>,
}

/// Each port name `config` lists, with the port it has in `place`: the stated one in the
/// project's own folder, else the next of the checkout's own, in the order they are listed.
pub fn ports(config: &ProjectConfig, place: &Place) -> Vec<(String, u16)> {
    config
        .port_names()
        .into_iter()
        .enumerate()
        .map(|(index, (name, stated))| {
            let port = place
                .first_port
                .map_or(stated, |first| first + index as u16);
            (name.to_owned(), port)
        })
        .collect()
}

/// `text` with `{port.<name>}`, `{checkout}` and `{root}` filled in. Any other braces stay.
pub fn fill(text: &str, place: &Place, ports: &[(String, u16)]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let value = match &after[..close] {
                "checkout" => place.name.to_owned(),
                "root" => place.root.to_string_lossy().into_owned(),
                inside => {
                    let name = inside.strip_prefix("port.")?;
                    let (_, port) = ports.iter().find(|(known, _)| known == name)?;
                    port.to_string()
                }
            };
            Some((value, close))
        });
        match value {
            Some((value, close)) => {
                filled.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                filled.push('{');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

/// `text` filled with stand-ins, to check what is left is well formed.
pub fn sample(text: &str) -> String {
    let names: Vec<(String, u16)> = template_port_names(text)
        .map(|name| (name.to_owned(), 1))
        .collect();
    let place = Place {
        name: MAIN,
        root: Path::new("/root"),
        first_port: None,
    };
    fill(text, &place, &names)
}

/// What everything Sikemux starts in `place` carries: who it is, its ports and the project's
/// `env`, filled in.
pub fn environment(config: Option<&ProjectConfig>, place: &Place) -> Vec<(String, String)> {
    let mut env = vec![
        (CHECKOUT_VAR.to_owned(), place.name.to_owned()),
        (
            ROOT_VAR.to_owned(),
            place.root.to_string_lossy().into_owned(),
        ),
    ];
    if let Some(first) = place.first_port {
        env.push((PORT_VAR.to_owned(), first.to_string()));
    }
    let Some(config) = config else {
        return env;
    };
    let ports = ports(config, place);
    env.extend(
        ports
            .iter()
            .map(|(name, port)| (port_variable(name), port.to_string())),
    );
    env.extend(
        config
            .env
            .iter()
            .map(|(key, value)| (key.clone(), fill(value, place, &ports))),
    );
    env
}

/// `SIKEMUX_PORT_WEB` for `web`.
pub fn port_variable(name: &str) -> String {
    let upper: String = name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' => character.to_ascii_uppercase(),
            _ => '_',
        })
        .collect();
    format!("{PORT_VAR}_{upper}")
}

/// A checkout's name made from its branch: its last part, in lowercase letters, digits and
/// underscores, starting with a letter and at most 32 characters, and none of `taken`.
pub fn name_for(branch: &str, taken: &[&str]) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let mut name = String::new();
    for character in last.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            name.push(character);
        } else if !name.is_empty() && !name.ends_with('_') {
            name.push('_');
        }
    }
    let mut name = name.trim_end_matches('_').to_owned();
    if !name.starts_with(|first: char| first.is_ascii_lowercase()) {
        name.insert_str(0, "c_");
    }
    if name == "c_" {
        name = "checkout".to_owned();
    }
    let base = shortened(&name, MAX_NAME_CHARS);
    if base != MAIN && !taken.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|number| {
            let suffix = format!("_{number}");
            format!(
                "{}{suffix}",
                shortened(&base, MAX_NAME_CHARS - suffix.len())
            )
        })
        .find(|name| !taken.contains(&name.as_str()))
        .unwrap_or(base)
}

fn shortened(name: &str, most: usize) -> String {
    name.chars()
        .take(most)
        .collect::<String>()
        .trim_end_matches('_')
        .to_owned()
}

/// Root `ports`: names to the port the project's own folder uses.
pub(super) fn parse_ports(
    value: Option<&Value>,
    errors: &mut Vec<ConfigError>,
) -> Vec<(String, u16)> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(record) = value.as_object() else {
        errors.push(issue(
            "$.ports",
            ConfigErrorCode::InvalidType,
            "ports must be an object of names to port numbers.",
        ));
        return Vec::new();
    };
    let mut ports = Vec::new();
    for (name, port) in record {
        let path = format!("$.ports.{name}");
        if !is_port_name(name) {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidValue,
                "Port names use letters, numbers, dots, dashes and underscores, starting with a letter or number.",
            ));
            continue;
        }
        match parse_port(Some(port), &path, errors) {
            Some(port) => ports.push((name.clone(), port)),
            None => continue,
        }
    }
    ports
}

/// A task's `port`.
pub(super) fn parse_port(
    value: Option<&Value>,
    path: &str,
    errors: &mut Vec<ConfigError>,
) -> Option<u16> {
    let value = value?;
    match value.as_u64().and_then(|port| u16::try_from(port).ok()) {
        Some(port) if port > 0 => Some(port),
        _ => {
            errors.push(issue(
                path,
                ConfigErrorCode::InvalidValue,
                "A port must be a whole number from 1 to 65535.",
            ));
            None
        }
    }
}

/// Every port name is listed once, there are at most ten, and every `{port.<name>}` in `env`,
/// a task's `env` and `preview.url` names one of them.
pub(super) fn check_names(
    root: &Map<String, Value>,
    config: &ProjectConfig,
    errors: &mut Vec<ConfigError>,
) {
    let names = config.port_names();
    if names.len() > MAX_PORT_NAMES {
        errors.push(issue(
            "$.ports",
            ConfigErrorCode::LimitExceeded,
            format!("At most {MAX_PORT_NAMES} ports may be named, counting processes with a port."),
        ));
    }
    for (index, (name, _)) in names.iter().enumerate() {
        if names[..index].iter().any(|(earlier, _)| earlier == name) {
            errors.push(issue(
                format!("$.ports.{name}"),
                ConfigErrorCode::DuplicateId,
                format!("The port name “{name}” is used twice."),
            ));
        }
    }
    let known = |name: &str| names.iter().any(|(known, _)| *known == name);
    let mut unknown = |text: &str, path: String| {
        for name in template_port_names(text) {
            if !known(name) {
                errors.push(issue(
                    path.clone(),
                    ConfigErrorCode::InvalidValue,
                    format!("“{{port.{name}}}” names no port; name it in ports, or give a process that port."),
                ));
            }
        }
    };
    for (key, value) in &config.env {
        unknown(value, format!("$.env.{key}"));
    }
    for (index, task) in config.tasks.iter().enumerate() {
        for (key, value) in &task.env {
            unknown(value, format!("$.tasks[{index}].env.{key}"));
        }
    }
    let url = root
        .get("preview")
        .and_then(|preview| preview.get("url"))
        .and_then(Value::as_str);
    if let Some(url) = url {
        unknown(url, "$.preview.url".to_owned());
    }
}

fn template_port_names(text: &str) -> impl Iterator<Item = &str> {
    text.split("{port.")
        .skip(1)
        .filter_map(|after| after.split_once('}').map(|(name, _)| name))
}

fn is_port_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && value.len() <= 64
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
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
