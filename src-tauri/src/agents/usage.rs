use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_pty::user_shell::login_shell_environment;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::Command;

use super::config::clean_model;
use super::executable::{apply_login_environment, apply_process_config, expand_user_path};
use super::models::{run_model_catalog_executable_without_env, CLAUDE_MODEL_CATALOG_ARGS};
use super::{allowed_agent_path, AgentKind, AgentUsage, AgentUsageResetAt, AgentUsageWindow};

/// Account-level plan usage for providers that expose structured rolling
/// windows. This intentionally goes through each installed CLI so Sikemux uses
/// the same account, credential store, and provider semantics as the agent.
#[tauri::command]
pub async fn agent_usage(
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<AgentUsage, String> {
    let key = format!(
        "{}\0{}\0{}",
        agent.as_str(),
        executable_path.as_deref().unwrap_or(""),
        config_path.as_deref().unwrap_or("")
    );
    if let Some(usage) = cached_agent_usage(&key) {
        return Ok(usage);
    }
    let executable = executable_path
        .as_deref()
        .map(expand_user_path)
        .or_else(|| {
            crate::system::find_executable_matching(agent.as_str(), |candidate| {
                allowed_agent_path(agent.as_str(), candidate)
            })
        })
        .ok_or_else(|| format!("{} is not available", agent.as_str()))?;
    let usage = match agent {
        AgentKind::Claude => claude_usage(&executable, config_path.as_deref()).await,
        AgentKind::Codex => codex_usage(&executable, config_path.as_deref()).await,
        _ => Err(format!(
            "{} does not expose structured plan usage",
            agent.as_str()
        )),
    }?;
    remember_agent_usage(key, usage.clone());
    Ok(usage)
}

/// Every lookup boots the provider's CLI, so a rail that polls gets the answer
/// the last boot gave until it is old enough to be worth another.
const AGENT_USAGE_TTL: Duration = Duration::from_secs(120);

fn agent_usage_cache() -> &'static Mutex<HashMap<String, (Instant, AgentUsage)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, AgentUsage)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_agent_usage(key: &str) -> Option<AgentUsage> {
    let mut cache = agent_usage_cache().lock().ok()?;
    let (stored, usage) = cache.get(key)?;
    if stored.elapsed() < AGENT_USAGE_TTL {
        return Some(usage.clone());
    }
    cache.remove(key);
    None
}

fn remember_agent_usage(key: String, usage: AgentUsage) {
    if let Ok(mut cache) = agent_usage_cache().lock() {
        cache.insert(key, (Instant::now(), usage));
    }
}

const CLAUDE_USAGE_REQUEST_ID: &str = "sikemux-usage";
const CODEX_USAGE_INITIALIZE_ID: u64 = 1;
const CODEX_USAGE_REQUEST_ID: u64 = 2;
const USAGE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(12);
const USAGE_LOOKUP_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

/// Each of these makes the Claude CLI authenticate as something other than the
/// claude.ai login, and none of them reports subscription limits.
const CLAUDE_SUBSCRIPTION_OVERRIDE_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

fn claude_subscription_override_present() -> bool {
    CLAUDE_SUBSCRIPTION_OVERRIDE_ENV.iter().any(|key| {
        std::env::var_os(key).is_some_and(|value| !value.is_empty())
            || login_shell_environment()
                .get(*key)
                .is_some_and(|value| !value.is_empty())
    })
}

async fn read_claude_usage(
    executable: &Path,
    config_path: Option<&str>,
    removed_env: &[&str],
) -> Result<AgentUsage, String> {
    let request = format!(
        "{{\"type\":\"control_request\",\"request_id\":\"{CLAUDE_USAGE_REQUEST_ID}\",\"request\":{{\"subtype\":\"get_usage\"}}}}\n"
    );
    run_model_catalog_executable_without_env(
        "claude",
        executable,
        CLAUDE_MODEL_CATALOG_ARGS,
        Some(&request),
        config_path,
        removed_env,
    )
    .await
    .and_then(|text| {
        parse_claude_usage(&text, CLAUDE_USAGE_REQUEST_ID)
            .ok_or_else(|| "Claude returned an unreadable usage snapshot".to_string())
    })
}

async fn claude_usage(executable: &Path, config_path: Option<&str>) -> Result<AgentUsage, String> {
    let usage = read_claude_usage(executable, config_path, &[]).await?;
    // An API key inherited from whichever shell launched Sikemux outranks the
    // claude.ai login, so a subscriber sees no limits purely because of how the
    // app was started. A key-only login fails the retry and keeps this answer.
    if usage.unavailable_reason.is_none() || !claude_subscription_override_present() {
        return Ok(usage);
    }
    Ok(
        read_claude_usage(executable, config_path, CLAUDE_SUBSCRIPTION_OVERRIDE_ENV)
            .await
            .ok()
            .filter(|retry| retry.unavailable_reason.is_none())
            .unwrap_or(usage),
    )
}

async fn codex_usage(executable: &Path, config_path: Option<&str>) -> Result<AgentUsage, String> {
    run_codex_usage_executable(executable, config_path).await
}

async fn run_codex_usage_executable(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<AgentUsage, String> {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    command
        .args(["app-server", "--stdio"])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    apply_process_config(&mut command, "codex", config_path);
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Codex usage lookup".to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Could not open Codex usage lookup input".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read Codex usage lookup output".to_string())?;
    let mut lines = AsyncBufReader::new(stdout).lines();

    let lookup = tokio::time::timeout(USAGE_LOOKUP_TIMEOUT, async {
        let initialize = serde_json::json!({
            "id": CODEX_USAGE_INITIALIZE_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "sikemux",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": true },
            },
        });
        stdin
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .map_err(|_| "Could not initialize Codex usage lookup".to_string())?;
        stdin
            .flush()
            .await
            .map_err(|_| "Could not initialize Codex usage lookup".to_string())?;

        let mut output_bytes = 0usize;
        let mut initialized = false;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "Could not read Codex usage lookup output".to_string())?
        {
            output_bytes = output_bytes.saturating_add(line.len());
            if output_bytes > USAGE_LOOKUP_OUTPUT_LIMIT {
                return Err("Codex usage lookup output was too large".to_string());
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let response_id = value.get("id").and_then(Value::as_u64);
            if response_id == Some(CODEX_USAGE_INITIALIZE_ID) && !initialized {
                if value.get("error").is_some() {
                    return Err("Codex rejected usage lookup initialization".to_string());
                }
                let requests = format!(
                    "{{\"method\":\"initialized\"}}\n{{\"id\":{CODEX_USAGE_REQUEST_ID},\"method\":\"account/rateLimits/read\",\"params\":null}}\n"
                );
                stdin
                    .write_all(requests.as_bytes())
                    .await
                    .map_err(|_| "Could not request Codex usage".to_string())?;
                stdin
                    .flush()
                    .await
                    .map_err(|_| "Could not request Codex usage".to_string())?;
                initialized = true;
                continue;
            }
            if response_id != Some(CODEX_USAGE_REQUEST_ID) {
                continue;
            }
            if value.get("error").is_some() {
                return Err("Codex rejected the usage lookup".to_string());
            }
            let result = value
                .get("result")
                .ok_or_else(|| "Codex returned an empty usage snapshot".to_string())?;
            return Ok(parse_codex_usage_result(result));
        }
        Err("Codex usage lookup closed before responding".to_string())
    })
    .await;

    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;

    lookup.map_err(|_| "Codex usage lookup timed out".to_string())?
}

fn usage_reset_at(value: &Value) -> Option<AgentUsageResetAt> {
    value.as_u64().map(AgentUsageResetAt::Unix).or_else(|| {
        value
            .as_str()
            .map(|value| AgentUsageResetAt::Iso(value.to_string()))
    })
}

fn push_claude_usage_window(
    windows: &mut Vec<AgentUsageWindow>,
    limits: &Value,
    key: &str,
    label: &str,
    window_minutes: u64,
) {
    let Some(window) = limits.get(key) else {
        return;
    };
    let Some(used_percent) = window.get("utilization").and_then(Value::as_f64) else {
        return;
    };
    windows.push(AgentUsageWindow {
        label: label.to_string(),
        used_percent: used_percent.clamp(0.0, 100.0),
        resets_at: window.get("resets_at").and_then(usage_reset_at),
        window_minutes: Some(window_minutes),
    });
}

fn parse_claude_usage(text: &str, request_id: &str) -> Option<AgentUsage> {
    let payload = text.lines().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let response = value.get("response")?;
        if value.get("type").and_then(Value::as_str) != Some("control_response")
            || response.get("request_id").and_then(Value::as_str) != Some(request_id)
            || response.get("subtype").and_then(Value::as_str) != Some("success")
        {
            return None;
        }
        response.get("response").cloned()
    })?;

    if !payload
        .get("rate_limits")
        .is_some_and(|value| value.is_object())
        && payload
            .get("rate_limits_available")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return None;
    }
    let unavailable_reason = (payload.get("rate_limits_available").and_then(Value::as_bool) == Some(false))
        .then(|| "Plan limits are unavailable for this login. API keys and third-party providers do not report subscription limits; OAuth access also requires profile permission.".to_string());
    let plan = payload
        .get("subscription_type")
        .and_then(Value::as_str)
        .and_then(clean_model);
    let mut windows = Vec::new();
    if let Some(limits) = payload.get("rate_limits").filter(|value| value.is_object()) {
        push_claude_usage_window(&mut windows, limits, "five_hour", "5h", 300);
        push_claude_usage_window(&mut windows, limits, "seven_day", "7d", 10_080);
        push_claude_usage_window(&mut windows, limits, "seven_day_opus", "Opus · 7d", 10_080);
        push_claude_usage_window(
            &mut windows,
            limits,
            "seven_day_sonnet",
            "Sonnet · 7d",
            10_080,
        );
        push_claude_usage_window(
            &mut windows,
            limits,
            "seven_day_oauth_apps",
            "OAuth apps · 7d",
            10_080,
        );
        if let Some(model_scoped) = limits.get("model_scoped").and_then(Value::as_array) {
            for window in model_scoped {
                let Some(display_name) = window
                    .get("display_name")
                    .and_then(Value::as_str)
                    .and_then(clean_model)
                else {
                    continue;
                };
                let Some(used_percent) = window.get("utilization").and_then(Value::as_f64) else {
                    continue;
                };
                windows.push(AgentUsageWindow {
                    label: format!("{display_name} · 7d"),
                    used_percent: used_percent.clamp(0.0, 100.0),
                    resets_at: window.get("resets_at").and_then(usage_reset_at),
                    window_minutes: Some(10_080),
                });
            }
        }
    }

    Some(AgentUsage {
        provider: "claude",
        plan,
        windows,
        unavailable_reason,
    })
}

fn usage_duration_label(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(300) => "5h".to_string(),
        Some(10_080) => "7d".to_string(),
        Some(minutes) if minutes >= 1_440 && minutes % 1_440 == 0 => {
            format!("{}d", minutes / 1_440)
        }
        Some(minutes) if minutes >= 60 && minutes % 60 == 0 => {
            format!("{}h", minutes / 60)
        }
        Some(minutes) => format!("{minutes}m"),
        None => fallback.to_string(),
    }
}

fn push_codex_usage_window(
    windows: &mut Vec<AgentUsageWindow>,
    snapshot: &Value,
    key: &str,
    fallback: &str,
    scope: Option<&str>,
) {
    let Some(window) = snapshot.get(key).filter(|value| value.is_object()) else {
        return;
    };
    let Some(used_percent) = window.get("usedPercent").and_then(Value::as_f64) else {
        return;
    };
    let window_minutes = window.get("windowDurationMins").and_then(Value::as_u64);
    let duration = usage_duration_label(window_minutes, fallback);
    windows.push(AgentUsageWindow {
        label: scope
            .map(|scope| format!("{scope} · {duration}"))
            .unwrap_or(duration),
        used_percent: used_percent.clamp(0.0, 100.0),
        resets_at: window.get("resetsAt").and_then(usage_reset_at),
        window_minutes,
    });
}

fn parse_codex_usage_result(result: &Value) -> AgentUsage {
    let mut snapshots: Vec<(&str, &Value)> = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|values| {
            let mut rows = values
                .iter()
                .map(|(key, value)| (key.as_str(), value))
                .collect::<Vec<_>>();
            rows.sort_by(|(left, _), (right, _)| {
                (left != &"codex", *left).cmp(&(right != &"codex", *right))
            });
            rows
        })
        .unwrap_or_default();
    if snapshots.is_empty() {
        if let Some(snapshot) = result.get("rateLimits").filter(|value| value.is_object()) {
            snapshots.push(("codex", snapshot));
        }
    }

    let plan = snapshots.iter().find_map(|(_, snapshot)| {
        snapshot
            .get("planType")
            .and_then(Value::as_str)
            .and_then(clean_model)
    });
    let mut windows = Vec::new();
    for (key, snapshot) in snapshots {
        let limit_id = snapshot
            .get("limitId")
            .and_then(Value::as_str)
            .unwrap_or(key);
        let scope = (limit_id != "codex")
            .then(|| snapshot.get("limitName").and_then(Value::as_str))
            .flatten()
            .and_then(clean_model);
        push_codex_usage_window(
            &mut windows,
            snapshot,
            "primary",
            "Primary",
            scope.as_deref(),
        );
        push_codex_usage_window(
            &mut windows,
            snapshot,
            "secondary",
            "Secondary",
            scope.as_deref(),
        );
    }

    AgentUsage {
        provider: "codex",
        plan,
        unavailable_reason: windows.is_empty().then(|| "This account did not report subscription limits. API-key accounts do not provide plan usage.".to_string()),
        windows,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_claude_usage, parse_codex_usage_result, AgentUsageResetAt};

    #[test]
    fn claude_usage_normalizes_plan_windows_and_model_scopes() {
        let usage = parse_claude_usage(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"test-usage","response":{"subscription_type":"max","rate_limits":{"five_hour":{"utilization":21.5,"resets_at":"2026-08-13T12:00:00Z"},"seven_day":{"utilization":64,"resets_at":"2026-08-18T09:30:00Z"},"seven_day_opus":null,"model_scoped":[{"display_name":"Fable","utilization":9,"resets_at":"2026-08-20T00:00:00Z"}]}}}}"#,
            "test-usage",
        )
        .unwrap();

        assert_eq!(usage.provider, "claude");
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert_eq!(
            usage
                .windows
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            vec!["5h", "7d", "Fable · 7d"]
        );
        assert_eq!(usage.windows[0].used_percent, 21.5);
        assert_eq!(usage.windows[0].window_minutes, Some(300));
        assert_eq!(
            usage.windows[0].resets_at,
            Some(AgentUsageResetAt::Iso("2026-08-13T12:00:00Z".to_string()))
        );
    }

    #[test]
    fn claude_usage_explains_unavailable_limits_and_rejects_unknown_payloads() {
        let usage = parse_claude_usage(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"usage","response":{"subscription_type":null,"rate_limits_available":false,"rate_limits":null}}}"#,
            "usage",
        ).unwrap();
        assert!(usage.windows.is_empty());
        assert!(usage.unavailable_reason.unwrap().contains("API keys"));
        assert!(parse_claude_usage(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"usage","response":{}}}"#,
            "usage",
        ).is_none());
    }

    #[test]
    fn codex_usage_prefers_multi_bucket_data_and_preserves_named_limits() {
        let usage = parse_codex_usage_result(&serde_json::json!({
            "rateLimits": {
                "limitId": "stale",
                "planType": "free",
                "primary": { "usedPercent": 99, "windowDurationMins": 60, "resetsAt": 1 }
            },
            "rateLimitsByLimitId": {
                "codex_spark": {
                    "limitId": "codex_spark",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "planType": "pro",
                    "primary": { "usedPercent": 3, "windowDurationMins": 10080, "resetsAt": 1787220418 }
                },
                "codex": {
                    "limitId": "codex",
                    "planType": "pro",
                    "primary": { "usedPercent": 4, "windowDurationMins": 300, "resetsAt": 1786998646 },
                    "secondary": { "usedPercent": 17, "windowDurationMins": 10080, "resetsAt": 1787157619 }
                }
            }
        }));

        assert_eq!(usage.provider, "codex");
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert_eq!(
            usage
                .windows
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            vec!["5h", "7d", "GPT-5.3-Codex-Spark · 7d"]
        );
        assert_eq!(usage.windows[1].used_percent, 17.0);
        assert_eq!(
            usage.windows[1].resets_at,
            Some(AgentUsageResetAt::Unix(1787157619))
        );
    }
}
