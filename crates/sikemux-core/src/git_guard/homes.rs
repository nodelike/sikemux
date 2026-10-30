//! Hermes, and Grok in a terminal, take the hooks they run before a shell
//! command only from their home's config. So each runs in a private home whose
//! config runs the git hook: the one its launch already made, or a copy of the
//! person's own, which stays as they wrote it.

use std::path::{Path, PathBuf};

use crate::agent_catalog::AgentKind;
use crate::launch::homes;

use super::shim;

/// How an agent that takes hooks only from its home's config is given the git hook.
pub(crate) struct GitHook {
    /// Its config, as text, with the hook running `command` added.
    pub(crate) config: fn(config: &str, command: &str) -> Result<String, String>,
    /// What else its process needs for the hook to run.
    pub(crate) environment: &'static [(&'static str, &'static str)],
}

/// The variables that start `provider` in a private home running the git
/// hook, where `given` reads the variables its launch already has. A Grok chat
/// loads the hook as a plugin instead, so only Grok in a terminal needs one.
pub(crate) fn environment<'a>(
    provider: &str,
    agent_id: &str,
    terminal: bool,
    given: impl Fn(&str) -> Option<&'a str>,
) -> Vec<(String, String)> {
    let Some(kind) = AgentKind::from_name(provider) else {
        return Vec::new();
    };
    let Some(hook) = kind.ops().git_hook(terminal) else {
        return Vec::new();
    };
    let (Some((data_dir, command)), Some((home, _))) = (shim::hook(), homes::private(kind)) else {
        return Vec::new();
    };
    let given = given(home.variable).map(PathBuf::from);
    match guarded_home(kind, data_dir, &command, hook.config, agent_id, given) {
        Ok(path) => {
            let mut env = vec![(
                home.variable.to_owned(),
                path.to_string_lossy().into_owned(),
            )];
            env.extend(
                hook.environment
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
            env
        }
        Err(error) => {
            eprintln!("sikemux core: {provider}'s git commands go unchecked: {error}");
            Vec::new()
        }
    }
}

fn guarded_home(
    kind: AgentKind,
    data_dir: &Path,
    command: &str,
    with_hook: fn(&str, &str) -> Result<String, String>,
    agent_id: &str,
    given: Option<PathBuf>,
) -> Result<PathBuf, String> {
    let path = match given {
        Some(given) if given.starts_with(data_dir) => given,
        given => {
            if agent_id.is_empty() || agent_id.contains('/') || agent_id.starts_with('.') {
                return Err(format!("{agent_id:?} cannot name a folder"));
            }
            let private = data_dir
                .join("agent-home")
                .join(kind.as_str())
                .join(agent_id);
            homes::private_home(kind, &private, given)?;
            private
        }
    };
    let config_path = path.join(homes::private(kind).ok_or("no private home")?.1.config);
    let config = std::fs::read_to_string(&config_path).unwrap_or_default();
    let hooked = with_hook(&config, command)?;
    if hooked != config {
        sikemux_process::files::write_atomic(&config_path, hooked.as_bytes(), true)
            .map_err(|error| error.to_string())?;
    }
    Ok(path)
}

/// `config_yaml` with the hook added to the person's own `pre_tool_call`
/// hooks, edited as text so the rest of the file stays as Hermes wrote it.
pub(crate) fn hermes_config(config_yaml: &str, command: &str) -> Result<String, String> {
    let quoted = serde_json::to_string(command).map_err(|error| error.to_string())?;
    let entry = format!("{{matcher: terminal, command: {quoted}}}");
    if config_yaml.contains(&entry) {
        return Ok(config_yaml.to_owned());
    }
    let mut lines: Vec<String> = config_yaml.lines().map(str::to_owned).collect();
    let empty_value = |value: &str| matches!(value, "" | "{}" | "[]" | "null" | "~");
    let value = |line: &str, key: &str| {
        let value = line.trim_start()[key.len()..].trim();
        value
            .split_once('#')
            .map_or(value, |(value, _)| value.trim())
            .to_owned()
    };
    let Some(hooks) = lines.iter().position(|line| line.starts_with("hooks:")) else {
        lines.extend([
            "hooks:".into(),
            "  pre_tool_call:".into(),
            format!("  - {entry}"),
        ]);
        return Ok(lines.join("\n") + "\n");
    };
    if !empty_value(&value(&lines[hooks], "hooks:")) {
        return Err("its hooks in config.yaml are not written as a block".into());
    }
    lines[hooks] = "hooks:".into();
    let body_end = lines[hooks + 1..]
        .iter()
        .position(|line| !(line.trim().is_empty() || line.starts_with([' ', '#'])))
        .map_or(lines.len(), |end| hooks + 1 + end);
    let indent = |line: &str| line.len() - line.trim_start().len();
    let child_indent = lines[hooks + 1..body_end]
        .iter()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map_or(2, |line| indent(line));
    let pad = " ".repeat(child_indent);
    let key = "pre_tool_call:";
    let Some(at) = (hooks + 1..body_end)
        .find(|&at| indent(&lines[at]) == child_indent && lines[at].trim_start().starts_with(key))
    else {
        lines.splice(
            hooks + 1..hooks + 1,
            [format!("{pad}{key}"), format!("{pad}- {entry}")],
        );
        return Ok(lines.join("\n") + "\n");
    };
    if !empty_value(&value(&lines[at], key)) {
        return Err("its pre_tool_call hooks in config.yaml are not written as a block".into());
    }
    lines[at] = format!("{pad}{key}");
    let item_indent = lines[at + 1..body_end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .filter(|line| line.trim_start().starts_with('-') && indent(line) >= child_indent)
        .map_or(child_indent, |line| indent(line));
    lines.insert(at + 1, format!("{}- {entry}", " ".repeat(item_indent)));
    Ok(lines.join("\n") + "\n")
}

/// `config_toml` with the hook added beside the person's own hooks.
pub(crate) fn grok_config(config_toml: &str, command: &str) -> Result<String, String> {
    let mut config: toml::Table = config_toml.parse().map_err(|error| format!("{error}"))?;
    let hooks = config
        .entry("hooks")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or("[hooks] in config.toml is not a table")?;
    let ours =
        toml::Value::try_from(shim::grok_hooks(command)).map_err(|error| error.to_string())?;
    let mut changed = false;
    for (event, groups) in ours.as_table().into_iter().flatten() {
        let list = hooks
            .entry(event)
            .or_insert_with(|| toml::Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} in config.toml is not a list"))?;
        for group in groups.as_array().into_iter().flatten() {
            if !list.contains(group) {
                list.push(group.clone());
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(config_toml.to_owned());
    }
    toml::to_string(&config).map_err(|error| error.to_string())
}
