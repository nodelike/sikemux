use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::config::{clean_model, yaml_model_section};
use super::executable::{apply_login_environment, apply_process_config, expand_user_path};
use super::{allowed_agent_path, home_path, AgentKind, AgentModelInfo};

/// Full model identifiers exposed by the selected CLI. Catalog lookup is lazy
/// because some providers build their list from local caches or provider data.
#[tauri::command]
pub async fn agent_models(
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<Vec<AgentModelInfo>, String> {
    let executable = executable_path
        .as_deref()
        .map(expand_user_path)
        .or_else(|| {
            crate::system::find_executable_matching(agent.as_str(), |candidate| {
                allowed_agent_path(agent.as_str(), candidate)
            })
        })
        .ok_or_else(|| format!("{} is not available", agent.as_str()))?;
    match agent {
        AgentKind::Claude => claude_models(&executable, config_path.as_deref()).await,
        AgentKind::Codex => run_model_catalog_executable(
            "codex",
            &executable,
            &["debug", "models", "--bundled"],
            None,
            config_path.as_deref(),
        )
        .await
        .and_then(|text| {
            parse_codex_models(&text)
                .ok_or_else(|| "Codex returned an unreadable model catalog".to_string())
        }),
        AgentKind::Hermes => hermes_cached_models(),
        AgentKind::Pi => run_model_catalog("pi", &["--list-models"])
            .await
            .map(|text| parse_pi_models(&text)),
        AgentKind::Opencode => run_model_catalog("opencode", &["models"])
            .await
            .map(|text| parse_line_models(&text)),
        AgentKind::Omp => {
            run_model_catalog_executable("omp", &executable, &["models", "--json"], None, None)
                .await
                .and_then(|text| {
                    parse_omp_models(&text)
                        .ok_or_else(|| "OMP returned an unreadable model catalog".to_string())
                })
        }
        AgentKind::Grok => {
            run_model_catalog_executable("grok", &executable, &["models"], None, None)
                .await
                .map(|text| parse_grok_models(&text))
        }
    }
}

const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(8);
const MODEL_CATALOG_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const MODEL_CATALOG_ERROR_DETAIL_LIMIT: usize = 240;
pub(super) const CLAUDE_MODEL_CATALOG_ARGS: &[&str] = &[
    "--print",
    "--output-format",
    "stream-json",
    "--verbose",
    "--system-prompt",
    "",
    "--tools",
    "",
    "--input-format",
    "stream-json",
];

async fn run_model_catalog(agent: &str, args: &[&str]) -> Result<String, String> {
    run_model_catalog_with_input(agent, args, None).await
}

async fn run_model_catalog_with_input(
    agent: &str,
    args: &[&str],
    input: Option<&str>,
) -> Result<String, String> {
    let executable = crate::system::find_executable_matching(agent, |candidate| {
        allowed_agent_path(agent, candidate)
    })
    .ok_or_else(|| format!("{agent} is not available on PATH"))?;
    run_model_catalog_executable(agent, &executable, args, input, None).await
}

async fn run_model_catalog_executable(
    agent: &str,
    executable: &Path,
    args: &[&str],
    input: Option<&str>,
    config_path: Option<&str>,
) -> Result<String, String> {
    run_model_catalog_executable_without_env(agent, executable, args, input, config_path, &[]).await
}

/// Clears `removed_env` after the login-shell import, so a variable the
/// captured profile set is dropped too, not just an inherited one.
pub(super) async fn run_model_catalog_executable_without_env(
    agent: &str,
    executable: &Path,
    args: &[&str],
    input: Option<&str>,
    config_path: Option<&str>,
    removed_env: &[&str],
) -> Result<String, String> {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    for key in removed_env {
        command.env_remove(key);
    }
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_process_config(&mut command, agent, config_path);
    command.envs(crate::model_providers::environment(agent).await);
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start {agent} model lookup"))?;
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("Could not open {agent} model lookup input"))?;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|_| format!("Could not write {agent} model lookup input"))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| format!("Could not finish {agent} model lookup input"))?;
    }
    let output = tokio::time::timeout(MODEL_CATALOG_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{agent} model lookup timed out"))?
        .map_err(|_| format!("Could not read {agent} model lookup output"))?;
    if !output.status.success() {
        let detail = model_catalog_error_detail(&output.stderr);
        return Err(match detail {
            Some(detail) => format!("{agent} model lookup exited unsuccessfully: {detail}"),
            None => format!("{agent} model lookup exited unsuccessfully"),
        });
    }
    if output.stdout.len() > MODEL_CATALOG_OUTPUT_LIMIT {
        return Err(format!("{agent} model catalog was too large"));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{agent} model catalog was not valid UTF-8"))
}

pub(super) fn model_catalog_error_detail(stderr: &[u8]) -> Option<String> {
    let normalized = String::from_utf8_lossy(stderr)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return None;
    }
    let mut characters = normalized.chars();
    let detail = characters
        .by_ref()
        .take(MODEL_CATALOG_ERROR_DETAIL_LIMIT)
        .collect::<String>();
    Some(if characters.next().is_some() {
        format!("{detail}…")
    } else {
        detail
    })
}

async fn claude_models(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<Vec<AgentModelInfo>, String> {
    const REQUEST_ID: &str = "sikemux-models";
    let request = format!(
        "{{\"type\":\"control_request\",\"request_id\":\"{REQUEST_ID}\",\"request\":{{\"subtype\":\"initialize\",\"hooks\":null}}}}\n"
    );
    run_model_catalog_executable(
        "claude",
        executable,
        CLAUDE_MODEL_CATALOG_ARGS,
        Some(&request),
        config_path,
    )
    .await
    .and_then(|text| {
        let models = parse_claude_models(&text, REQUEST_ID);
        (!models.is_empty())
            .then_some(models)
            .ok_or_else(|| "Claude returned an empty model catalog".to_string())
    })
}

fn parse_claude_models(text: &str, request_id: &str) -> Vec<AgentModelInfo> {
    let Some(models) = text.lines().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let response = value.get("response")?;
        if value.get("type").and_then(Value::as_str) != Some("control_response")
            || response.get("request_id").and_then(Value::as_str) != Some(request_id)
        {
            return None;
        }
        response.get("response")?.get("models")?.as_array().cloned()
    }) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    models
        .iter()
        // "default" is the inherited selection already rendered first by the UI.
        .filter(|model| model.get("value").and_then(Value::as_str) != Some("default"))
        .filter_map(|model| {
            let id = model.get("resolvedModel")?.as_str()?;
            let info = model_info(id, model.get("displayName").and_then(Value::as_str))?;
            seen.insert(info.id.clone()).then_some(info)
        })
        .collect()
}

fn parse_codex_models(text: &str) -> Option<Vec<AgentModelInfo>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let models = value.get("models")?.as_array()?;
    Some(
        models
            .iter()
            .filter(|model| model.get("visibility").and_then(Value::as_str) == Some("list"))
            .filter_map(|model| {
                model_info(
                    model.get("slug")?.as_str()?,
                    model.get("display_name").and_then(Value::as_str),
                )
            })
            .collect(),
    )
}

fn parse_pi_models(text: &str) -> Vec<AgentModelInfo> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let provider = columns.next()?;
            let model = columns.next()?;
            model_info(&format!("{provider}/{model}"), None)
        })
        .collect()
}

fn parse_line_models(text: &str) -> Vec<AgentModelInfo> {
    text.lines()
        .filter_map(|line| model_info(line, None))
        .collect()
}

fn parse_omp_models(text: &str) -> Option<Vec<AgentModelInfo>> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(
        value
            .get("models")?
            .as_array()?
            .iter()
            .filter_map(|model| {
                model_info(
                    model.get("selector")?.as_str()?,
                    model.get("name").and_then(Value::as_str),
                )
            })
            .collect(),
    )
}

fn parse_grok_models(text: &str) -> Vec<AgentModelInfo> {
    let mut in_models = false;
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line == "Available models:" {
                in_models = true;
                return None;
            }
            if !in_models {
                return None;
            }
            let id = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("- "))?
                .trim_end_matches(" (default)")
                .trim();
            model_info(id, None)
        })
        .collect()
}

fn hermes_cached_models() -> Result<Vec<AgentModelInfo>, String> {
    let Some(home) = home_path() else {
        return Ok(Vec::new());
    };
    let Ok(config) = fs::read_to_string(home.join(".hermes/config.yaml")) else {
        return Ok(Vec::new());
    };
    let (provider, _) = yaml_model_section(&config);
    let Some(provider) = provider else {
        return Ok(Vec::new());
    };
    let Ok(text) = fs::read_to_string(home.join(".hermes/provider_models_cache.json")) else {
        return Ok(Vec::new());
    };
    Ok(parse_hermes_models(&text, &provider).unwrap_or_default())
}

fn parse_hermes_models(text: &str, provider: &str) -> Option<Vec<AgentModelInfo>> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(
        value
            .get(provider)?
            .get("models")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|model| model_info(&format!("{provider}/{model}"), None))
            .collect(),
    )
}

fn model_info(id: &str, label: Option<&str>) -> Option<AgentModelInfo> {
    let id = clean_model(id)?;
    let label = label.and_then(clean_model).unwrap_or_else(|| id.clone());
    Some(AgentModelInfo { id, label })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_claude_models, parse_codex_models, parse_grok_models, parse_hermes_models,
        parse_line_models, parse_omp_models, parse_pi_models, AgentModelInfo,
    };
    #[cfg(unix)]
    use super::{
        run_model_catalog_executable, run_model_catalog_executable_without_env,
        MODEL_CATALOG_ERROR_DETAIL_LIMIT,
    };
    #[cfg(unix)]
    use std::path::Path;

    #[test]
    fn codex_catalog_keeps_only_visible_full_model_ids() {
        let models = parse_codex_models(
            r#"{"models":[
                {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol","visibility":"list"},
                {"slug":"internal-model","display_name":"Internal","visibility":"hide"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            models,
            vec![AgentModelInfo {
                id: "gpt-5.6-sol".into(),
                label: "GPT-5.6-Sol".into()
            }]
        );
    }

    #[test]
    fn claude_catalog_replaces_aliases_with_resolved_model_ids() {
        let models = parse_claude_models(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"test-models","response":{"models":[{"value":"default","resolvedModel":"claude-opus-5[1m]","displayName":"Default"},{"value":"opus[1m]","resolvedModel":"claude-opus-5[1m]","displayName":"Opus"},{"value":"sonnet","resolvedModel":"claude-sonnet-5","displayName":"Sonnet"},{"value":"haiku","resolvedModel":"claude-haiku-4-5-20251001","displayName":"Haiku"}]}}}"#,
            "test-models",
        );
        assert_eq!(
            models,
            vec![
                AgentModelInfo {
                    id: "claude-opus-5[1m]".into(),
                    label: "Opus".into()
                },
                AgentModelInfo {
                    id: "claude-sonnet-5".into(),
                    label: "Sonnet".into()
                },
                AgentModelInfo {
                    id: "claude-haiku-4-5-20251001".into(),
                    label: "Haiku".into()
                }
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_catalog_subprocess_captures_stdout() {
        let output = run_model_catalog_executable(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", "printf '%s' '{\"models\":[]}'"],
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(output, r#"{"models":[]}"#);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_catalog_subprocess_drops_removed_environment() {
        const READ_HOME: &str = "printf '%s' \"${HOME-unset}\"";
        let kept = run_model_catalog_executable_without_env(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", READ_HOME],
            None,
            None,
            &[],
        )
        .await
        .unwrap();
        let dropped = run_model_catalog_executable_without_env(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", READ_HOME],
            None,
            None,
            &["HOME"],
        )
        .await
        .unwrap();

        assert_ne!(kept, "unset");
        assert_eq!(dropped, "unset");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_catalog_subprocess_surfaces_bounded_stderr() {
        let long_detail = "x".repeat(MODEL_CATALOG_ERROR_DETAIL_LIMIT + 20);
        let script = format!("printf 'first\\nsecond {long_detail}' >&2; exit 7");
        let error = run_model_catalog_executable(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", &script],
            None,
            None,
        )
        .await
        .unwrap_err();

        assert!(error.starts_with("test-agent model lookup exited unsuccessfully: first second "));
        assert!(error.ends_with('…'));
        assert!(error.chars().count() <= MODEL_CATALOG_ERROR_DETAIL_LIMIT + 52);
    }

    #[test]
    fn line_catalogs_preserve_provider_qualified_ids() {
        assert_eq!(
            parse_pi_models(
                "provider model context\nopenai-codex gpt-5.6-sol 272K\nanthropic claude-opus-5 1M\n"
            ),
            vec![
                AgentModelInfo {
                    id: "openai-codex/gpt-5.6-sol".into(),
                    label: "openai-codex/gpt-5.6-sol".into()
                },
                AgentModelInfo {
                    id: "anthropic/claude-opus-5".into(),
                    label: "anthropic/claude-opus-5".into()
                }
            ]
        );
        assert_eq!(
            parse_line_models("opencode/big-pickle\nollama/qwen3\n"),
            vec![
                AgentModelInfo {
                    id: "opencode/big-pickle".into(),
                    label: "opencode/big-pickle".into()
                },
                AgentModelInfo {
                    id: "ollama/qwen3".into(),
                    label: "ollama/qwen3".into()
                }
            ]
        );
    }

    #[test]
    fn omp_and_grok_catalogs_keep_cli_model_ids() {
        assert_eq!(
            parse_omp_models(
                r#"{"models":[{"selector":"openai-codex/gpt-5.6-sol","name":"GPT-5.6 Sol"}]}"#
            ),
            Some(vec![AgentModelInfo {
                id: "openai-codex/gpt-5.6-sol".into(),
                label: "GPT-5.6 Sol".into()
            }])
        );
        assert_eq!(
            parse_grok_models(
                "Default model: grok-4.5\n\nAvailable models:\n  * grok-4.5 (default)\n  - custom-model\n"
            ),
            vec![
                AgentModelInfo {
                    id: "grok-4.5".into(),
                    label: "grok-4.5".into()
                },
                AgentModelInfo {
                    id: "custom-model".into(),
                    label: "custom-model".into()
                }
            ]
        );
    }

    #[test]
    fn hermes_catalog_uses_only_the_configured_provider() {
        let models = parse_hermes_models(
            r#"{
                "deepseek":{"models":["deepseek-v4-pro","deepseek-v4-flash"]},
                "anthropic":{"models":["claude-opus-5"]}
            }"#,
            "deepseek",
        )
        .unwrap();
        assert_eq!(
            models,
            vec![
                AgentModelInfo {
                    id: "deepseek/deepseek-v4-pro".into(),
                    label: "deepseek/deepseek-v4-pro".into()
                },
                AgentModelInfo {
                    id: "deepseek/deepseek-v4-flash".into(),
                    label: "deepseek/deepseek-v4-flash".into()
                }
            ]
        );
    }
}
