//! Plan limits of agents whose CLI keeps them to itself: Grok's and OpenCode Go's are
//! asked of the vendor's own usage endpoint with the sign-in their CLI saved. The token
//! is read afresh for each lookup and goes nowhere else. A sign-in these endpoints cannot
//! answer for, such as an API key, has no limits rather than a failure.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use super::usage::{Usage, UsageResetAt, UsageWindow};
use crate::agent_catalog::AgentKind;
use sikemux_process::user_environment::shell_var;

/// What a lookup answers when the account's sign-in has no plan limits to read.
pub const NO_LIMITS: &str = "This sign-in has no plan limits to read";

const SESSION: u64 = 300;
const WEEK: u64 = 7 * 24 * 60;
const MONTH: u64 = 30 * 24 * 60;

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    crate::http::shared(&CLIENT, || {
        crate::http::builder().timeout(Duration::from_secs(10))
    })
    .map_err(|_| "Could not reach the usage service".to_owned())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

async fn ask(url: &str, token: &str) -> Result<Option<Value>, String> {
    let answer = client()?
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not reach the usage service".to_owned())?;
    if matches!(answer.status().as_u16(), 401 | 403 | 404) {
        return Ok(None);
    }
    let answer = answer.error_for_status().map_err(|error| {
        format!(
            "The usage service said {}",
            error.status().unwrap_or_default()
        )
    })?;
    answer
        .json()
        .await
        .map(Some)
        .map_err(|_| "The usage service's answer did not read".to_owned())
}

fn usage(provider: &str, windows: Vec<UsageWindow>) -> Usage {
    Usage {
        provider: provider.to_owned(),
        plan: None,
        tier: None,
        windows,
        unavailable_reason: None,
        resets: None,
        light: true,
    }
}

fn window(label: &str, minutes: u64, used: f64, resets_at: Option<&str>) -> UsageWindow {
    UsageWindow {
        label: label.to_owned(),
        scope: None,
        used_percent: used.clamp(0.0, 100.0),
        resets_at: resets_at.map(|at| UsageResetAt::Iso(at.to_owned())),
        window_minutes: Some(minutes),
    }
}

/// Settings that point Grok at another account or service, whose limits these are not.
const GROK_ELSEWHERE: &[&str] = &[
    "XAI_API_KEY",
    "GROK_AUTH",
    "GROK_CONFIG",
    "GROK_CONFIG_PATH",
    "GROK_LOCAL_AUTH",
    "GROK_AUTH_PROVIDER_COMMAND",
    "GROK_CLI_CHAT_PROXY_BASE_URL",
    "GROK_OIDC_ISSUER",
    "GROK_OAUTH2_ISSUER",
];
/// Where Grok keeps the sign-in to grok.com, among others it may keep.
const GROK_SIGN_INS: &[&str] = &[
    "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828",
    "https://accounts.x.ai/sign-in",
];

fn grok_token(home: &Path) -> Option<String> {
    let points_elsewhere = |config: &str| {
        config.lines().any(|line| {
            let line = line.trim_start().trim_start_matches('[').trim_start();
            ["auth", "grok_com_config", "endpoints"]
                .iter()
                .any(|section| {
                    line.strip_prefix(section)
                        .is_some_and(|rest| rest.starts_with([']', '.', '=', ' ']))
                })
        })
    };
    let configs = ["config.toml", "managed_config.toml", "requirements.toml"];
    if configs.iter().any(|name| {
        std::fs::read_to_string(home.join(name)).is_ok_and(|config| points_elsewhere(&config))
    }) {
        return None;
    }
    let saved = read_json(&home.join("auth.json"))?;
    let sign_in = GROK_SIGN_INS.iter().find_map(|key| saved.get(*key))?;
    if sign_in.get("auth_mode").and_then(Value::as_str) == Some("api_key") {
        return None;
    }
    let token = sign_in.get("key")?.as_str()?.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Grok's allowance for its billing period, with the sign-in `grok login` saved.
pub async fn grok(config_path: Option<&str>) -> Result<Usage, String> {
    if GROK_ELSEWHERE.iter().any(|name| shell_var(name).is_some()) {
        return Err(NO_LIMITS.into());
    }
    let home = match config_path {
        Some(folder) => sikemux_process::paths::account_dir(folder),
        None => AgentKind::Grok.home_root().ok_or(NO_LIMITS)?,
    };
    let token = grok_token(&home).ok_or(NO_LIMITS)?;
    let answer = ask(
        "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
        &token,
    )
    .await?
    .ok_or(NO_LIMITS)?;
    grok_usage(&answer).ok_or_else(|| NO_LIMITS.to_owned())
}

fn grok_usage(answer: &Value) -> Option<Usage> {
    let config = answer.get("config")?;
    let used = config.get("creditUsagePercent")?.as_f64()?;
    let period = config.get("currentPeriod");
    let minutes = match period.and_then(|period| period.get("type")?.as_str()) {
        Some(kind) if kind.ends_with("WEEKLY") => WEEK,
        _ => MONTH,
    };
    let label = if minutes == WEEK { "7d" } else { "30d" };
    let resets = period.and_then(|period| period.get("end")?.as_str());
    Some(usage("grok", vec![window(label, minutes, used, resets)]))
}

fn opencode_go_key() -> Option<String> {
    let data = shell_var("XDG_DATA_HOME")
        .map(|data| Path::new(&data).join("opencode"))
        .or_else(|| {
            sikemux_process::paths::home().map(|home| home.join(".local/share/opencode"))
        })?;
    let saved = read_json(&data.join("auth.json"));
    let stored = saved
        .as_ref()
        .and_then(|saved| saved.get("opencode-go"))
        .filter(|go| go.get("type").and_then(Value::as_str) == Some("api"))
        .and_then(|go| go.get("key")?.as_str().map(str::to_owned));
    let key = stored.or_else(|| shell_var("OPENCODE_API_KEY"))?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// OpenCode Go's session, week and month, with the key OpenCode keeps for it.
pub async fn opencode() -> Result<Usage, String> {
    let key = opencode_go_key().ok_or(NO_LIMITS)?;
    let answer = ask("https://opencode.ai/zen/go/v1/usage", &key)
        .await?
        .ok_or(NO_LIMITS)?;
    opencode_usage(&answer).ok_or_else(|| NO_LIMITS.to_owned())
}

fn opencode_usage(answer: &Value) -> Option<Usage> {
    let usage_of = answer.get("usage")?;
    let read = |name: &str, label: &str, minutes: u64| {
        let window_of = usage_of.get(name)?;
        Some(window(
            label,
            minutes,
            window_of.get("percent")?.as_f64()?,
            window_of.get("resetsAt").and_then(Value::as_str),
        ))
    };
    let windows: Vec<UsageWindow> = [
        read("rolling", "5h", SESSION),
        read("weekly", "7d", WEEK),
        read("monthly", "30d", MONTH),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!windows.is_empty()).then(|| usage("opencode", windows))
}
