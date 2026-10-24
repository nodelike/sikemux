//! The model and effort an account's agent starts with when Sikemux names none, read
//! from the agent's own configuration.

use std::fs;
use std::path::Path;

use serde_json::Value;
use sikemux_process::paths::home;
use sikemux_process::user_environment::shell_var;

use crate::agent_catalog::AgentKind;
use crate::sessions::roots::{agent_config_root, environment_path, omp_agent_root};

const MAX_MODEL_LENGTH: usize = 256;

pub fn configured_default_model(kind: &str, config_path: Option<&str>) -> Option<String> {
    let home = home()?;
    match kind {
        "claude" => shell_var("ANTHROPIC_MODEL")
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
        "hermes" => shell_var("HERMES_INFERENCE_MODEL")
            .and_then(|value| clean_model(&value))
            .or_else(|| {
                let text =
                    fs::read_to_string(AgentKind::Hermes.home_root()?.join("config.yaml")).ok()?;
                let (provider, model) = yaml_model_section(&text);
                qualify_model(provider.as_deref(), model.as_deref())
            }),
        "pi" => {
            let settings =
                fs::read_to_string(AgentKind::Pi.home_root()?.join("settings.json")).ok()?;
            let value: Value = serde_json::from_str(&settings).ok()?;
            qualify_model(
                value.get("defaultProvider").and_then(Value::as_str),
                value.get("defaultModel").and_then(Value::as_str),
            )
        }
        "opencode" => {
            let path = environment_path("OPENCODE_CONFIG").unwrap_or_else(|| {
                environment_path("XDG_CONFIG_HOME")
                    .unwrap_or_else(|| home.join(".config"))
                    .join("opencode/opencode.json")
            });
            json_model(&path, "model")
        }
        "omp" => fs::read_to_string(omp_agent_root()?.join("config.yml"))
            .ok()
            .and_then(|text| yaml_top_level_scalar(&text, "defaultModel"))
            .and_then(|value| clean_model(&value)),
        "grok" => fs::read_to_string(AgentKind::Grok.home_root()?.join("config.toml"))
            .ok()
            .and_then(|text| toml_section_string(&text, "models", "default"))
            .and_then(|value| clean_model(&value))
            .or_else(|| Some("grok-build".to_string())),
        "muse" => shell_var("MUSE_MODEL")
            .and_then(|value| clean_model(&value))
            .or_else(|| json_model(&AgentKind::Muse.home_root()?.join("settings.json"), "model")),
        _ => None,
    }
}

pub fn configured_default_effort(kind: &str, config_path: Option<&str>) -> Option<String> {
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
        "hermes" => fs::read_to_string(AgentKind::Hermes.home_root()?.join("config.yaml"))
            .ok()
            .and_then(|text| yaml_agent_reasoning_effort(&text))
            .and_then(|value| clean_effort(kind, &value)),
        "pi" => json_effort(
            &AgentKind::Pi.home_root()?.join("settings.json"),
            "defaultThinkingLevel",
            kind,
        ),
        "omp" => fs::read_to_string(omp_agent_root()?.join("config.yml"))
            .ok()
            .and_then(|text| yaml_top_level_scalar(&text, "defaultThinkingLevel"))
            .and_then(|value| clean_effort(kind, &value)),
        "grok" => fs::read_to_string(AgentKind::Grok.home_root()?.join("config.toml"))
            .ok()
            .and_then(|text| toml_section_string(&text, "models", "default_reasoning_effort"))
            .and_then(|value| clean_effort(kind, &value)),
        "muse" => json_effort(
            &AgentKind::Muse.home_root()?.join("settings.json"),
            "reasoning_effort",
            kind,
        ),
        _ => None,
    }
}

fn clean_model(value: &str) -> Option<String> {
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
        "muse" => &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
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
fn yaml_model_section(text: &str) -> (Option<String>, Option<String>) {
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
