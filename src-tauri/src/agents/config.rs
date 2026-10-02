use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sikemux_pty::user_shell::login_shell_environment;

use super::executable::expand_user_path;
use super::home_path;

const MAX_MODEL_LENGTH: usize = 256;

fn environment_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .or_else(|| login_shell_environment().get(key).map(PathBuf::from))
}

fn environment_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| login_shell_environment().get(key).cloned())
}

fn omp_profile_name() -> Option<String> {
    let raw = environment_string("OMP_PROFILE").or_else(|| environment_string("PI_PROFILE"))?;
    let name = raw.trim();
    (!name.is_empty()
        && name != "default"
        && name != "."
        && name != ".."
        && name.len() <= 64
        && !name.contains(['/', '\\']))
    .then(|| name.to_string())
}

fn omp_config_root() -> Option<PathBuf> {
    let home = home_path()?;
    let config = environment_string("PI_CONFIG_DIR").unwrap_or_else(|| ".omp".to_string());
    Some(home.join(config.trim_start_matches(['/', '\\'])))
}

pub(super) fn omp_agent_root() -> Option<PathBuf> {
    if let Some(profile) = omp_profile_name() {
        return Some(
            omp_config_root()?
                .join("profiles")
                .join(profile)
                .join("agent"),
        );
    }
    environment_path("PI_CODING_AGENT_DIR").or_else(|| Some(omp_config_root()?.join("agent")))
}

pub(super) fn omp_session_dirs() -> Vec<PathBuf> {
    if let Some(path) = environment_path("PI_CODING_AGENT_SESSION_DIR") {
        return vec![path];
    }
    let mut dirs = Vec::new();
    if let Some(root) = omp_agent_root() {
        dirs.push(root.join("sessions"));
    }
    if let Some(xdg) = environment_path("XDG_DATA_HOME") {
        let root = xdg.join("omp");
        let sessions = omp_profile_name()
            .map(|profile| root.join("profiles").join(profile).join("sessions"))
            .unwrap_or_else(|| root.join("sessions"));
        if sessions.exists() && !dirs.contains(&sessions) {
            dirs.push(sessions);
        }
    }
    dirs
}

pub(super) fn grok_root() -> Option<PathBuf> {
    environment_path("GROK_HOME").or_else(|| Some(home_path()?.join(".grok")))
}

pub(super) fn agent_config_root(kind: &str, configured: Option<&str>) -> Option<PathBuf> {
    let home = home_path()?;
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let path = expand_user_path(configured);
        let is_config_file = matches!(
            path.file_name().and_then(|value| value.to_str()),
            Some("config.toml" | "config.yml" | "settings.json" | "settings.local.json")
        );
        return if is_config_file {
            path.parent().map(Path::to_path_buf)
        } else {
            Some(path)
        };
    }
    match kind {
        "claude" => {
            Some(environment_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")))
        }
        "codex" => Some(environment_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"))),
        "omp" => omp_agent_root(),
        "grok" => grok_root(),
        _ => Some(home),
    }
}

pub(super) fn configured_default_model(kind: &str, config_path: Option<&str>) -> Option<String> {
    let home = home_path()?;
    match kind {
        "claude" => std::env::var("ANTHROPIC_MODEL")
            .ok()
            .and_then(|value| clean_model(&value))
            .or_else(|| {
                json_model(
                    &agent_config_root(kind, config_path)?.join("settings.local.json"),
                    "model",
                )
            })
            .or_else(|| {
                json_model(
                    &agent_config_root(kind, config_path)?.join("settings.json"),
                    "model",
                )
            }),
        "codex" => {
            let root = agent_config_root(kind, config_path)?;
            fs::read_to_string(root.join("config.toml"))
                .ok()
                .and_then(|text| toml_model(&text))
        }
        "hermes" => std::env::var("HERMES_INFERENCE_MODEL")
            .ok()
            .and_then(|value| clean_model(&value))
            .or_else(|| {
                let text = fs::read_to_string(home.join(".hermes/config.yaml")).ok()?;
                let (provider, model) = yaml_model_section(&text);
                qualify_model(provider.as_deref(), model.as_deref())
            }),
        "pi" => {
            let root = std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".pi/agent"));
            let value: Value =
                serde_json::from_str(&fs::read_to_string(root.join("settings.json")).ok()?).ok()?;
            qualify_model(
                value.get("defaultProvider").and_then(Value::as_str),
                value.get("defaultModel").and_then(Value::as_str),
            )
        }
        "opencode" => {
            let path = std::env::var_os("OPENCODE_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let config = std::env::var_os("XDG_CONFIG_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join(".config"));
                    config.join("opencode/opencode.json")
                });
            json_model(&path, "model")
        }
        "omp" => fs::read_to_string(omp_agent_root()?.join("config.yml"))
            .ok()
            .and_then(|text| yaml_top_level_scalar(&text, "defaultModel"))
            .and_then(|value| clean_model(&value)),
        "grok" => fs::read_to_string(grok_root()?.join("config.toml"))
            .ok()
            .and_then(|text| toml_section_string(&text, "models", "default"))
            .and_then(|value| clean_model(&value))
            .or_else(|| Some("grok-build".to_string())),
        _ => None,
    }
}

pub(super) fn configured_default_effort(kind: &str, config_path: Option<&str>) -> Option<String> {
    let home = home_path()?;
    match kind {
        "claude" => json_effort(
            &agent_config_root(kind, config_path)?.join("settings.local.json"),
            "effortLevel",
            kind,
        )
        .or_else(|| {
            json_effort(
                &agent_config_root(kind, config_path)?.join("settings.json"),
                "effortLevel",
                kind,
            )
        }),
        "codex" => {
            let root = agent_config_root(kind, config_path)?;
            fs::read_to_string(root.join("config.toml"))
                .ok()
                .and_then(|text| toml_effort(&text))
        }
        "hermes" => fs::read_to_string(home.join(".hermes/config.yaml"))
            .ok()
            .and_then(|text| yaml_agent_reasoning_effort(&text))
            .and_then(|value| clean_effort(kind, &value)),
        "pi" => {
            let root = std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".pi/agent"));
            json_effort(&root.join("settings.json"), "defaultThinkingLevel", kind)
        }
        "omp" => fs::read_to_string(omp_agent_root()?.join("config.yml"))
            .ok()
            .and_then(|text| yaml_top_level_scalar(&text, "defaultThinkingLevel"))
            .and_then(|value| clean_effort(kind, &value)),
        "grok" => fs::read_to_string(grok_root()?.join("config.toml"))
            .ok()
            .and_then(|text| toml_section_string(&text, "models", "default_reasoning_effort"))
            .and_then(|value| clean_effort(kind, &value)),
        _ => None,
    }
}

pub(super) fn clean_model(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= MAX_MODEL_LENGTH).then(|| value.to_string())
}

fn clean_effort(kind: &str, value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let allowed: &[&str] = match kind {
        "claude" => &["low", "medium", "high", "xhigh", "max"],
        "codex" => &["minimal", "low", "medium", "high", "xhigh", "max"],
        "hermes" => &[
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ],
        "pi" | "omp" => &["off", "minimal", "low", "medium", "high", "xhigh", "max"],
        "grok" => &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
        _ => &[],
    };
    allowed.contains(&value.as_str()).then_some(value)
}

fn json_model(path: &Path, key: &str) -> Option<String> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    value.get(key).and_then(Value::as_str).and_then(clean_model)
}

fn json_effort(path: &Path, key: &str, kind: &str) -> Option<String> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| clean_effort(kind, value))
}

fn qualify_model(provider: Option<&str>, model: Option<&str>) -> Option<String> {
    let model = clean_model(model?)?;
    if model.contains('/') {
        return Some(model);
    }
    let provider = provider.and_then(clean_model);
    provider
        .and_then(|provider| clean_model(&format!("{provider}/{model}")))
        .or(Some(model))
}

fn toml_model(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    value
        .get("model")
        .and_then(toml::Value::as_str)
        .and_then(clean_model)
}

fn toml_effort(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    value
        .get("model_reasoning_effort")
        .and_then(toml::Value::as_str)
        .and_then(|value| clean_effort("codex", value))
}

fn toml_section_string(text: &str, section: &str, key: &str) -> Option<String> {
    toml::from_str::<toml::Value>(text)
        .ok()?
        .get(section)?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn yaml_top_level_scalar(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || line.len() != line.trim_start().len() {
            return None;
        }
        let (candidate, raw) = trimmed.split_once(':')?;
        (candidate.trim() == key)
            .then(|| yaml_scalar(raw))
            .flatten()
    })
}

/// Extract only `provider` and `default` from Hermes' top-level `model` map.
/// This deliberately avoids deserializing or exposing the rest of config.yaml.
pub(super) fn yaml_model_section(text: &str) -> (Option<String>, Option<String>) {
    let mut model_indent = None;
    let mut provider = None;
    let mut model = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let Some(section_indent) = model_indent else {
            if trimmed == "model:" {
                model_indent = Some(indent);
            }
            continue;
        };
        if indent <= section_indent {
            break;
        }
        if trimmed.ends_with(':') {
            continue;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            continue;
        };
        let value = yaml_scalar(raw);
        match key.trim() {
            "provider" => provider = value,
            "default" => model = value,
            _ => {}
        }
    }
    (provider, model)
}

fn yaml_agent_reasoning_effort(text: &str) -> Option<String> {
    let mut agent_indent = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let Some(section_indent) = agent_indent else {
            if trimmed == "agent:" {
                agent_indent = Some(indent);
            }
            continue;
        };
        if indent <= section_indent {
            break;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            continue;
        };
        if key.trim() == "reasoning_effort" {
            return yaml_scalar(raw);
        }
    }
    None
}

fn yaml_scalar(raw: &str) -> Option<String> {
    let raw = raw.split(" #").next()?.trim();
    let raw = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    clean_model(raw)
}

#[cfg(test)]
mod tests {
    use super::{
        agent_config_root, json_effort, qualify_model, toml_effort, toml_model,
        toml_section_string, yaml_agent_reasoning_effort, yaml_model_section,
        yaml_top_level_scalar,
    };
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn profile_config_files_resolve_to_their_provider_root() {
        assert_eq!(
            agent_config_root("codex", Some("/profiles/work/config.toml")),
            Some(Path::new("/profiles/work").to_path_buf())
        );
        assert_eq!(
            agent_config_root("claude", Some("/profiles/personal/settings.json")),
            Some(Path::new("/profiles/personal").to_path_buf())
        );
    }

    #[test]
    fn codex_defaults_come_only_from_the_root_config() {
        let config = r#"
            model = "gpt-5.6-sol" # the CLI default
            model_reasoning_effort = "high"
            [profiles.review]
            model = "gpt-5.6-terra"
            model_reasoning_effort = "xhigh"
        "#;
        assert_eq!(toml_model(config).as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(toml_effort(config).as_deref(), Some("high"));
        assert_eq!(
            toml_model(
                r#"
                [profiles.review]
                model = "gpt-5.6-terra"
                "#,
            ),
            None
        );
        assert_eq!(
            toml_effort(
                r#"
                [profiles.review]
                model_reasoning_effort = "xhigh"
                "#,
            ),
            None
        );
    }

    #[test]
    fn provider_models_are_qualified_without_rewriting_full_ids() {
        assert_eq!(
            qualify_model(Some("openai-codex"), Some("gpt-5.6-terra")).as_deref(),
            Some("openai-codex/gpt-5.6-terra")
        );
        assert_eq!(
            qualify_model(Some("anthropic"), Some("openrouter/claude-opus-5")).as_deref(),
            Some("openrouter/claude-opus-5")
        );
    }

    #[test]
    fn json_cli_effort_defaults_use_each_providers_config_key() {
        let mut config = tempfile::NamedTempFile::new().unwrap();
        write!(
            config,
            r#"{{"effortLevel":"xhigh","defaultThinkingLevel":"minimal"}}"#
        )
        .unwrap();

        assert_eq!(
            json_effort(config.path(), "effortLevel", "claude").as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            json_effort(config.path(), "defaultThinkingLevel", "pi").as_deref(),
            Some("minimal")
        );
        assert_eq!(
            json_effort(config.path(), "defaultThinkingLevel", "claude"),
            None,
            "provider-invalid defaults must not leak into launch choices"
        );
    }

    #[test]
    fn hermes_default_model_comes_from_its_model_section() {
        let config = r#"
            model:
              provider: deepseek
              default: deepseek-v4-flash
            agent:
              reasoning_effort: high
            compression:
              reasoning_effort: low
            "#;
        let (provider, model) = yaml_model_section(config);
        assert_eq!(provider.as_deref(), Some("deepseek"));
        assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(yaml_agent_reasoning_effort(config).as_deref(), Some("high"));
    }

    #[test]
    fn omp_and_grok_defaults_use_their_native_config_sections() {
        assert_eq!(
            yaml_top_level_scalar(
                "defaultModel: openai/gpt-5\ndefaultThinkingLevel: high\n",
                "defaultModel"
            )
            .as_deref(),
            Some("openai/gpt-5")
        );
        assert_eq!(
            toml_section_string(
                "[models]\ndefault = \"grok-build\"\ndefault_reasoning_effort = \"xhigh\"\n",
                "models",
                "default_reasoning_effort"
            )
            .as_deref(),
            Some("xhigh")
        );
    }
}
