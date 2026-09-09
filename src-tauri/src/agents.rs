use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::Command;

#[derive(Serialize, Deserialize)]
pub struct AgentSession {
    id: String,
    title: String,
    mtime: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(rename = "type")]
    kind: &'static str,
    label: &'static str,
    command: String,
    available: bool,
    error: Option<String>,
    warning: Option<String>,
    profile_id: Option<String>,
    config_path: Option<String>,
    #[serde(rename = "defaultModel")]
    default_model: Option<String>,
    #[serde(rename = "defaultEffort")]
    default_effort: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileRequest {
    #[serde(rename = "type")]
    kind: AgentKind,
    profile_id: Option<String>,
    executable_path: Option<String>,
    config_path: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct AgentModelInfo {
    id: String,
    label: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(untagged)]
pub enum AgentUsageResetAt {
    Unix(u64),
    Iso(String),
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageWindow {
    label: String,
    used_percent: f64,
    resets_at: Option<AgentUsageResetAt>,
    window_minutes: Option<u64>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    provider: &'static str,
    plan: Option<String>,
    windows: Vec<AgentUsageWindow>,
    unavailable_reason: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentSessionsChanged {
    agent: &'static str,
    cwd: String,
    config_path: Option<String>,
}

struct AgentDef {
    kind: &'static str,
    label: &'static str,
    command: &'static str,
}

const AGENT_DEFS: &[AgentDef] = &[
    AgentDef {
        kind: "claude",
        label: "Claude",
        command: "claude",
    },
    AgentDef {
        kind: "codex",
        label: "Codex",
        command: "codex",
    },
    AgentDef {
        kind: "hermes",
        label: "Hermes",
        command: "hermes",
    },
    AgentDef {
        kind: "pi",
        label: "Pi",
        command: "pi",
    },
    AgentDef {
        kind: "opencode",
        label: "OpenCode",
        command: "opencode",
    },
    AgentDef {
        kind: "omp",
        label: "OMP",
        command: "omp",
    },
    AgentDef {
        kind: "grok",
        label: "Grok",
        command: "grok",
    },
];

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Hermes,
    Pi,
    Opencode,
    Omp,
    Grok,
}

impl AgentKind {
    fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Hermes => "hermes",
            AgentKind::Pi => "pi",
            AgentKind::Opencode => "opencode",
            AgentKind::Omp => "omp",
            AgentKind::Grok => "grok",
        }
    }
}

struct AgentWatchTarget {
    dir: PathBuf,
    mode: RecursiveMode,
}

struct AgentWatchHandle {
    _watchers: Vec<RecommendedWatcher>,
}

fn watch_registry() -> &'static Mutex<HashMap<u32, Arc<AgentWatchHandle>>> {
    static R: OnceLock<Mutex<HashMap<u32, Arc<AgentWatchHandle>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn watch_count() -> usize {
    watch_registry().lock().map(|r| r.len()).unwrap_or(0)
}

static NEXT_WATCH_ID: AtomicU32 = AtomicU32::new(1);

const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const AGENT_UPDATE_RETRY_TIMEOUT: Duration = Duration::from_secs(8);
const AGENT_UPDATE_SETTLE_DELAY: Duration = Duration::from_millis(350);

fn healthy_agent_executables() -> &'static Mutex<HashMap<String, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return crate::system::user_home();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return crate::system::user_home().join(rest);
    }
    PathBuf::from(value)
}

fn apply_process_config(command: &mut Command, agent: &str, config_path: Option<&str>) {
    let Some(root) = agent_config_root(agent, config_path) else {
        return;
    };
    if config_path.is_none() {
        return;
    }
    match agent {
        "codex" => {
            command.env("CODEX_HOME", root);
        }
        "claude" => {
            command.env("CLAUDE_CONFIG_DIR", root);
        }
        _ => {}
    }
}

fn apply_login_environment(command: &mut Command) {
    for (key, value) in crate::system::login_shell_environment() {
        command.env(key, value);
    }
}

fn push_agent_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn automatic_agent_candidates(def: &AgentDef) -> Vec<PathBuf> {
    let mut candidates = crate::system::find_executables_matching(def.command, |candidate| {
        allowed_agent_path(def.kind, candidate)
    });
    let home = crate::system::user_home();
    for candidate in [
        home.join(".local/bin").join(def.command),
        home.join("Library/pnpm").join(def.command),
        home.join(".npm/bin").join(def.command),
        home.join(".bun/bin").join(def.command),
    ] {
        push_agent_candidate(&mut candidates, candidate);
    }
    if def.kind == "claude" {
        push_agent_candidate(&mut candidates, home.join(".claude/local/claude"));
        push_agent_candidate(&mut candidates, home.join(".claude/bin/claude"));
    }
    if def.kind == "opencode" {
        push_agent_candidate(&mut candidates, home.join(".opencode/bin/opencode"));
    }
    if def.kind == "grok" {
        push_agent_candidate(&mut candidates, home.join(".grok/bin/grok"));
    }
    #[cfg(target_os = "macos")]
    if def.kind == "codex" {
        push_agent_candidate(
            &mut candidates,
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        );
    }
    candidates
}

fn explicit_agent_candidates(value: &str) -> Vec<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    if value.contains('/') || value.contains('\\') || value.starts_with('~') {
        return vec![expand_user_path(value)];
    }
    crate::system::find_executables_matching(value, |_| true)
}

/// Arguments that answer "does this CLI run?" without side effects. `--version`
/// suits most agents, but Hermes bundles a `git fetch` update check into it
/// (bounded by its own 10s network timeout, cached for six hours), so every
/// cache miss outran our probe and reported an installed CLI as missing.
/// `--help` exercises the same interpreter and venv without the network.
fn agent_probe_args(agent: &str) -> &'static [&'static str] {
    match agent {
        "hermes" => &["--help"],
        _ => &["--version"],
    }
}

async fn probe_agent_executable_with_timeout(
    agent: &str,
    executable: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let probe_args = agent_probe_args(agent);
    #[cfg(windows)]
    let mut command = if matches!(
        executable.extension().and_then(|value| value.to_str()),
        Some(value) if value.eq_ignore_ascii_case("cmd") || value.eq_ignore_ascii_case("bat")
    ) {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/S", "/C"])
            .arg(executable)
            .args(probe_args);
        command
    } else {
        let mut command = Command::new(executable);
        command.args(probe_args);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(executable);
        command.args(probe_args);
        command
    };
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_login_environment(&mut command);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "version check timed out".to_string())?
        .map_err(|error| format!("could not start: {error}"))?;
    if !output.status.success() {
        let detail = model_catalog_error_detail(&output.stderr)
            .or_else(|| model_catalog_error_detail(&output.stdout))
            .unwrap_or_else(|| format!("exited with {}", output.status));
        return Err(detail);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

#[derive(Clone)]
struct AgentProbe {
    checked_at: std::time::Instant,
    modified: std::time::SystemTime,
    size: u64,
    version: String,
}

async fn probe_agent_executable(agent: &str, executable: &Path) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, AgentProbe>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let metadata = fs::metadata(executable).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    let size = metadata.as_ref().map_or(0, |metadata| metadata.len());
    if let Some(cached) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(executable).cloned())
    {
        if cached.checked_at.elapsed() < Duration::from_secs(60)
            && Some(cached.modified) == modified
            && cached.size == size
        {
            return Ok(cached.version);
        }
    }
    let version =
        probe_agent_executable_with_timeout(agent, executable, AGENT_PROBE_TIMEOUT).await?;
    if let (Some(modified), Ok(mut cache)) = (modified, cache.lock()) {
        if cache.len() >= 128 {
            cache.retain(|_, probe| probe.checked_at.elapsed() < Duration::from_secs(60));
        }
        cache.insert(
            executable.to_path_buf(),
            AgentProbe {
                checked_at: std::time::Instant::now(),
                modified,
                size,
                version: version.clone(),
            },
        );
    }
    Ok(version)
}

async fn first_healthy_agent_candidate(
    agent: &str,
    candidates: Vec<PathBuf>,
) -> (Option<PathBuf>, Vec<String>) {
    let mut failures = Vec::new();
    let cached = healthy_agent_executables()
        .lock()
        .ok()
        .and_then(|cache| cache.get(agent).cloned());
    let mut temporarily_unverified = None;
    for candidate in candidates {
        match probe_agent_executable(agent, &candidate).await {
            Ok(_) => {
                if let Ok(mut cache) = healthy_agent_executables().lock() {
                    cache.insert(agent.to_string(), candidate.clone());
                }
                return (Some(candidate), failures);
            }
            Err(error) if error == "version check timed out" => {
                tokio::time::sleep(AGENT_UPDATE_SETTLE_DELAY).await;
                match probe_agent_executable_with_timeout(
                    agent,
                    &candidate,
                    AGENT_UPDATE_RETRY_TIMEOUT,
                )
                .await
                {
                    Ok(_) => {
                        if let Ok(mut cache) = healthy_agent_executables().lock() {
                            cache.insert(agent.to_string(), candidate.clone());
                        }
                        return (Some(candidate), failures);
                    }
                    Err(retry_error) => {
                        failures.push(format!(
                            "{}: {retry_error} after update retry",
                            candidate.display()
                        ));
                        if cached.as_ref() == Some(&candidate) {
                            temporarily_unverified = Some(candidate);
                        }
                    }
                }
            }
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }
    (temporarily_unverified, failures)
}

pub(crate) async fn resolve_agent_executable(
    agent: &str,
    explicit: Option<&str>,
) -> Result<PathBuf, String> {
    let def = AGENT_DEFS
        .iter()
        .find(|def| def.kind == agent)
        .ok_or("Unknown agent provider")?;
    let candidates = explicit
        .map(explicit_agent_candidates)
        .unwrap_or_else(|| automatic_agent_candidates(def));
    let (resolved, failures) = first_healthy_agent_candidate(agent, candidates).await;
    resolved.ok_or_else(|| {
        if failures.is_empty() {
            format!("{} executable was not found", def.label)
        } else {
            failures.join("; ")
        }
    })
}

/// Agent CLIs that are installed for the current user. The app's PATH is fixed
/// from the login shell during boot, so this matches what spawned PTYs can run.
#[tauri::command]
pub async fn available_agents(profiles: Vec<AgentProfileRequest>) -> Vec<AgentInfo> {
    let available = futures::future::join_all(AGENT_DEFS.iter().map(|def| async {
        let profile = profiles
            .iter()
            .find(|profile| profile.kind.as_str() == def.kind);
        let explicit = profile.and_then(|profile| profile.executable_path.as_deref());
        let candidates = explicit
            .map(explicit_agent_candidates)
            .unwrap_or_else(|| automatic_agent_candidates(def));
        if candidates.is_empty() && profile.is_none() {
            return None;
        }

        let (resolved, failures) = first_healthy_agent_candidate(def.kind, candidates).await;
        let config_path = profile.and_then(|profile| profile.config_path.clone());
        let warning = resolved.as_ref().and_then(|path| {
            (!failures.is_empty())
                .then(|| format!("Skipped {}; using {}", failures.join("; "), path.display()))
        });
        let error = resolved.is_none().then(|| {
            if failures.is_empty() {
                format!("{} executable was not found", def.label)
            } else {
                failures.join("; ")
            }
        });
        Some(AgentInfo {
            kind: def.kind,
            label: def.label,
            command: resolved
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| explicit.map(str::to_string))
                .unwrap_or_else(|| def.command.to_string()),
            available: resolved.is_some(),
            error,
            warning,
            profile_id: profile.and_then(|profile| profile.profile_id.clone()),
            config_path: config_path.clone(),
            default_model: configured_default_model(def.kind, config_path.as_deref()),
            default_effort: configured_default_effort(def.kind, config_path.as_deref()),
        })
    }))
    .await;
    available.into_iter().flatten().collect()
}

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

/// Account-level plan usage for providers that expose structured rolling
/// windows. This intentionally goes through each installed CLI so Sikemux uses
/// the same account, credential store, and provider semantics as the agent.
#[tauri::command]
pub async fn agent_usage(
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<AgentUsage, String> {
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
        AgentKind::Claude => claude_usage(&executable, config_path.as_deref()).await,
        AgentKind::Codex => codex_usage(&executable, config_path.as_deref()).await,
        _ => Err(format!(
            "{} does not expose structured plan usage",
            agent.as_str()
        )),
    }
}

const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(8);
const MODEL_CATALOG_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const MODEL_CATALOG_ERROR_DETAIL_LIMIT: usize = 240;
const CLAUDE_MODEL_CATALOG_ARGS: &[&str] = &[
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
    let mut command = Command::new(executable);
    apply_login_environment(&mut command);
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

fn model_catalog_error_detail(stderr: &[u8]) -> Option<String> {
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

const CLAUDE_USAGE_REQUEST_ID: &str = "sikemux-usage";
const CODEX_USAGE_INITIALIZE_ID: u64 = 1;
const CODEX_USAGE_REQUEST_ID: u64 = 2;
const USAGE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(12);
const USAGE_LOOKUP_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

async fn claude_usage(executable: &Path, config_path: Option<&str>) -> Result<AgentUsage, String> {
    let request = format!(
        "{{\"type\":\"control_request\",\"request_id\":\"{CLAUDE_USAGE_REQUEST_ID}\",\"request\":{{\"subtype\":\"get_usage\"}}}}\n"
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
        parse_claude_usage(&text, CLAUDE_USAGE_REQUEST_ID)
            .ok_or_else(|| "Claude returned an unreadable usage snapshot".to_string())
    })
}

async fn codex_usage(executable: &Path, config_path: Option<&str>) -> Result<AgentUsage, String> {
    run_codex_usage_executable(executable, config_path).await
}

async fn run_codex_usage_executable(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<AgentUsage, String> {
    let mut command = Command::new(executable);
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

const MAX_MODEL_LENGTH: usize = 256;

fn environment_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from).or_else(|| {
        crate::system::login_shell_environment()
            .get(key)
            .map(PathBuf::from)
    })
}

fn environment_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| crate::system::login_shell_environment().get(key).cloned())
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

fn omp_agent_root() -> Option<PathBuf> {
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

fn omp_session_dirs() -> Vec<PathBuf> {
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

fn grok_root() -> Option<PathBuf> {
    environment_path("GROK_HOME").or_else(|| Some(home_path()?.join(".grok")))
}

fn agent_config_root(kind: &str, configured: Option<&str>) -> Option<PathBuf> {
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

fn configured_default_model(kind: &str, config_path: Option<&str>) -> Option<String> {
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

fn configured_default_effort(kind: &str, config_path: Option<&str>) -> Option<String> {
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

/// Existing on-disk conversations for an agent.
#[tauri::command]
pub fn agent_sessions(
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Vec<AgentSession> {
    match agent {
        AgentKind::Claude => claude_sessions(&cwd, config_path.as_deref()),
        AgentKind::Codex => codex_sessions(&cwd, config_path.as_deref()),
        AgentKind::Hermes => hermes_sessions(),
        AgentKind::Pi => pi_sessions(&cwd),
        AgentKind::Opencode => opencode_sessions(&cwd),
        AgentKind::Omp => omp_sessions(&cwd),
        AgentKind::Grok => grok_sessions(&cwd),
    }
}

const AGENT_DEBOUNCE_MS: u64 = 200;

fn home_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn push_watch_target(out: &mut Vec<AgentWatchTarget>, dir: PathBuf, mode: RecursiveMode) {
    if !dir.is_dir() {
        return;
    }
    if let Some(existing) = out.iter_mut().find(|target| target.dir == dir) {
        if matches!(mode, RecursiveMode::Recursive) {
            existing.mode = RecursiveMode::Recursive;
        }
        return;
    }
    out.push(AgentWatchTarget { dir, mode });
}

fn push_existing_or_parent(
    out: &mut Vec<AgentWatchTarget>,
    path: PathBuf,
    existing_mode: RecursiveMode,
) {
    if path.is_dir() {
        push_watch_target(out, path, existing_mode);
    } else if let Some(parent) = path.parent() {
        push_watch_target(out, parent.to_path_buf(), RecursiveMode::NonRecursive);
    }
}

fn agent_watch_dirs(
    agent: AgentKind,
    cwd: &str,
    config_path: Option<&str>,
) -> Vec<AgentWatchTarget> {
    let mut out = Vec::new();
    match agent {
        AgentKind::Claude => {
            if let Some(root) = agent_config_root("claude", config_path) {
                let projects = root.join("projects");
                let project = projects.join(cwd.replace('/', "-"));
                if project.is_dir() {
                    push_watch_target(&mut out, project, RecursiveMode::Recursive);
                } else {
                    push_watch_target(&mut out, projects, RecursiveMode::NonRecursive);
                }
            }
        }
        AgentKind::Codex => {
            if let Some(codex) = agent_config_root("codex", config_path) {
                let sessions = codex.join("sessions");
                if sessions.is_dir() {
                    push_watch_target(&mut out, sessions, RecursiveMode::Recursive);
                } else {
                    push_watch_target(&mut out, codex, RecursiveMode::NonRecursive);
                }
            }
        }
        AgentKind::Hermes => {
            if let Some(home) = home_path() {
                push_watch_target(&mut out, home.join(".hermes"), RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Pi => {
            if let Some(root) = pi_session_dir() {
                push_existing_or_parent(&mut out, root, RecursiveMode::Recursive);
            }
        }
        AgentKind::Opencode => {
            for dir in opencode_data_dirs() {
                push_existing_or_parent(&mut out, dir, RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Omp => {
            for dir in omp_session_dirs() {
                push_existing_or_parent(&mut out, dir, RecursiveMode::Recursive);
            }
        }
        AgentKind::Grok => {
            if let Some(root) = grok_root() {
                push_existing_or_parent(&mut out, root.join("sessions"), RecursiveMode::Recursive);
            }
        }
    }
    out
}

fn agent_event_interesting(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn spawn_agent_debouncer(
    app: AppHandle,
    agent: &'static str,
    cwd: String,
    config_path: Option<String>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    tauri::async_runtime::spawn(async move {
        while rx.recv().await.is_some() {
            let sleep = tokio::time::sleep(Duration::from_millis(AGENT_DEBOUNCE_MS));
            tokio::pin!(sleep);
            let mut closed = false;
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    msg = rx.recv() => {
                        if msg.is_none() {
                            closed = true;
                            break;
                        }
                        sleep
                            .as_mut()
                            .reset(tokio::time::Instant::now() + Duration::from_millis(AGENT_DEBOUNCE_MS));
                    }
                }
            }
            if closed {
                return;
            }
            let _ = app.emit(
                "agent_sessions_changed",
                AgentSessionsChanged {
                    agent,
                    cwd: cwd.clone(),
                    config_path: config_path.clone(),
                },
            );
        }
    });
}

#[tauri::command]
pub fn agent_sessions_watch_start(
    app: AppHandle,
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Result<u32, String> {
    let dirs = agent_watch_dirs(agent, &cwd, config_path.as_deref());
    let id = NEXT_WATCH_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    spawn_agent_debouncer(app, agent.as_str(), cwd, config_path, rx);

    let mut watchers = Vec::new();
    for target in dirs {
        let tx_events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !agent_event_interesting(&event) {
                return;
            }
            let _ = tx_events.send(());
        })
        .map_err(|e| e.to_string())?;
        if watcher.watch(&target.dir, target.mode).is_ok() {
            watchers.push(watcher);
        }
    }

    watch_registry().lock().map_err(|e| e.to_string())?.insert(
        id,
        Arc::new(AgentWatchHandle {
            _watchers: watchers,
        }),
    );
    Ok(id)
}

#[tauri::command]
pub fn agent_sessions_watch_stop(id: u32) -> Result<(), String> {
    watch_registry()
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&id);
    Ok(())
}

fn allowed_agent_path(_agent: &str, _path: &Path) -> bool {
    true
}

fn mtime_of(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleCacheStamp {
    modified_ns: u128,
    len: u64,
}

fn title_cache_stamp(path: &Path) -> TitleCacheStamp {
    let metadata = fs::metadata(path).ok();
    TitleCacheStamp {
        modified_ns: metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or(0),
        len: metadata.map(|value| value.len()).unwrap_or(0),
    }
}

fn condense(text: &str) -> Option<String> {
    let c = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if c.is_empty() || c.starts_with('<') {
        return None;
    }
    Some(c.chars().take(72).collect())
}

fn text_from_content(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    content.as_array()?.iter().find_map(|b| {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            b.get("text").and_then(|t| t.as_str()).map(String::from)
        } else {
            None
        }
    })
}

/// Cached title per transcript: `path -> (high-resolution stamp, title)`.
type TitleCache = HashMap<PathBuf, (TitleCacheStamp, Option<String>, u64)>;
const MAX_TITLE_CACHE_ENTRIES: usize = 2_048;
const MAX_AGENT_TRANSCRIPT_PATHS: usize = 20_000;
const MAX_AGENT_TRANSCRIPTS_INSPECTED: usize = 5_000;

fn next_title_cache_access() -> u64 {
    static ACCESS: AtomicU64 = AtomicU64::new(1);
    ACCESS.fetch_add(1, Ordering::Relaxed)
}

/// Per-file title cache keyed by a high-resolution file stamp. Titles are derived from transcript
/// content that only ever grows, so an unchanged nanosecond timestamp and file
/// length means an unchanged title. Length prevents same-tick appends from
/// preserving a cached title-less result on coarse filesystems.
/// This turns the palette's cold scan of every session into a one-time cost:
/// reopening it re-reads only the sessions that have actually changed.
fn title_cache() -> &'static Mutex<TitleCache> {
    static CACHE: OnceLock<Mutex<TitleCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the cached title for `path` when its stamp matches, otherwise run
/// `compute`, store the result (including `None`, so title-less files are not
/// re-scanned), and return it.
fn cached_title<F>(path: &Path, stamp: TitleCacheStamp, compute: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if let Ok(mut cache) = title_cache().lock() {
        if let Some((cached_stamp, title, access)) = cache.get_mut(path) {
            if *cached_stamp == stamp {
                *access = next_title_cache_access();
                return title.clone();
            }
        }
    }
    let title = compute();
    if let Ok(mut cache) = title_cache().lock() {
        if cache.len() >= MAX_TITLE_CACHE_ENTRIES && !cache.contains_key(path) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, _, access))| *access)
                .map(|(path, _)| path.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            path.to_path_buf(),
            (stamp, title.clone(), next_title_cache_access()),
        );
    }
    title
}

/// Read up to `n` bytes from the start of `file` as lossy UTF-8.
fn read_prefix(file: &mut fs::File, n: u64) -> Option<String> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = Vec::new();
    file.by_ref().take(n).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Read from `start` to the end of `file` as lossy UTF-8.
fn read_suffix(file: &mut fs::File, start: u64) -> Option<String> {
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---- claude — ~/.claude/projects/<cwd-dashed>/<uuid>.jsonl --------------
fn claude_sessions(cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    let Some(root) = agent_config_root("claude", config_path) else {
        return Vec::new();
    };
    let dir = root.join("projects").join(cwd.replace('/', "-"));
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    paths.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    paths.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);
    // Titles come from reading each transcript, so fan the per-file work out
    // across rayon's pool instead of scanning sessions one at a time.
    let mut out: Vec<AgentSession> = paths
        .par_iter()
        .filter_map(|path| {
            let id = path.file_stem().and_then(|s| s.to_str())?;
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || claude_title(path))?;
            Some(AgentSession {
                id: id.to_string(),
                title,
                mtime,
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

/// Pull a title out of one Claude transcript line, updating the running
/// `ai_title` (last write wins) and `first_user` (first write wins).
fn scan_claude_line(line: &str, ai_title: &mut Option<String>, first_user: &mut Option<String>) {
    if line.contains("\"type\":\"ai-title\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()).and_then(condense) {
                *ai_title = Some(t);
            }
        }
    } else if first_user.is_none() && line.contains("\"type\":\"user\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user") {
                *first_user = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(text_from_content)
                    .and_then(|text| condense(&text));
            }
        }
    }
}

// Bound the per-file read: the first user prompt sits near the top and Claude
// emits `ai-title` entries continuously (so the freshest title sits at the end),
// which lets us skip the middle of multi-MB transcripts.
const CLAUDE_HEAD_BYTES: u64 = 128 * 1024;
const CLAUDE_TAIL_BYTES: u64 = 128 * 1024;

fn claude_title(path: &Path) -> Option<String> {
    // Claude writes the human-readable title (auto-generated, then overwritten by
    // `/rename`) as `{"type":"ai-title","aiTitle":...}` entries appended as the
    // session grows — last one wins. This is what Claude's own /resume picker
    // shows. Prefer it; fall back to the first user prompt for sessions that have
    // no title yet. Cheap substring guards keep us from JSON-parsing every line.
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut ai_title: Option<String> = None;
    let mut first_user: Option<String> = None;

    // Head: captures the first user prompt and any early ai-title. For small
    // transcripts this covers the whole file, keeping the result exact.
    let head = read_prefix(&mut file, CLAUDE_HEAD_BYTES.min(len))?;
    for line in head.lines() {
        scan_claude_line(line, &mut ai_title, &mut first_user);
    }

    // Tail: the most recent ai-title lives at the end of large transcripts. Skip
    // the first (likely partial) line, then take the last ai-title we can parse.
    if len > CLAUDE_HEAD_BYTES {
        if let Some(tail) = read_suffix(&mut file, len.saturating_sub(CLAUDE_TAIL_BYTES)) {
            for line in tail.lines().skip(1) {
                if line.contains("\"type\":\"ai-title\"") {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        if let Some(t) =
                            v.get("aiTitle").and_then(|t| t.as_str()).and_then(condense)
                        {
                            ai_title = Some(t);
                        }
                    }
                }
            }
        }
    }

    ai_title.or(first_user)
}

// ---- codex — ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl ----------------
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 || out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn codex_indexed_titles(root: &Path) -> HashMap<String, String> {
    let Ok(file) = fs::File::open(root.join("session_index.jsonl")) else {
        return HashMap::new();
    };
    let mut titles = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let Some(title) = value
            .get("thread_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        titles.insert(id.to_string(), title.to_string());
    }
    titles
}

fn codex_sessions(cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    let Some(root) = agent_config_root("codex", config_path) else {
        return Vec::new();
    };
    let indexed_titles = codex_indexed_titles(&root);
    let mut files = Vec::new();
    collect_jsonl(&root.join("sessions"), &mut files, 0);
    files.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            let file = fs::File::open(path).ok()?;
            let mut first = String::new();
            BufReader::new(file).read_line(&mut first).ok()?;
            let v = serde_json::from_str::<Value>(first.trim()).ok()?;
            if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
                return None;
            }
            let payload = v.get("payload")?;
            if payload.get("cwd").and_then(|c| c.as_str()) != Some(cwd) {
                return None;
            }
            let id = payload.get("id").and_then(|i| i.as_str())?;
            let mtime = mtime_of(path);
            let title = indexed_titles
                .get(id)
                .cloned()
                .or_else(|| cached_title(path, title_cache_stamp(path), || codex_title(path)))
                .unwrap_or_else(|| id.chars().take(8).collect());
            Some(AgentSession {
                id: id.to_string(),
                title,
                mtime,
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

fn codex_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(200).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = v.get("payload");
        if payload.and_then(|p| p.get("type")).and_then(|t| t.as_str()) != Some("user_message") {
            continue;
        }
        let msg = payload
            .and_then(|p| p.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        if let Some(t) = condense(msg) {
            return Some(t);
        }
    }
    None
}

// ---- pi — ~/.pi/agent/sessions/**/<session>.jsonl ----------------------
fn pi_session_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        return Some(PathBuf::from(dir).join("sessions"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".pi/agent/sessions"))
}

fn pi_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = pi_session_dir() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files, 0);
    files.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            let file = fs::File::open(path).ok()?;
            let mut first = String::new();
            BufReader::new(file).read_line(&mut first).ok()?;
            let v = serde_json::from_str::<Value>(first.trim()).ok()?;
            if v.get("type").and_then(|t| t.as_str()) != Some("session") {
                return None;
            }
            if v.get("cwd").and_then(|c| c.as_str()) != Some(cwd) {
                return None;
            }
            let id = path.to_string_lossy().to_string();
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || pi_title(path))
                .or_else(|| v.get("id").and_then(|i| i.as_str()).and_then(condense))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("session")
                        .chars()
                        .take(13)
                        .collect()
                });
            Some(AgentSession { id, title, mtime })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

fn pi_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut first_user: Option<String> = None;
    let mut named: Option<String> = None;
    for line in BufReader::new(file).lines().take(220).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_info") => {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()).and_then(condense) {
                    named = Some(name);
                }
            }
            Some("message") if first_user.is_none() => {
                let Some(message) = v.get("message") else {
                    continue;
                };
                if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                    continue;
                }
                if let Some(text) = message
                    .get("content")
                    .and_then(text_from_content)
                    .and_then(|t| condense(&t))
                {
                    first_user = Some(text);
                }
            }
            _ => {}
        }
    }
    named.or(first_user)
}

fn omp_sessions(cwd: &str) -> Vec<AgentSession> {
    omp_sessions_from_dirs(cwd, omp_session_dirs())
}

fn omp_sessions_from_dirs(cwd: &str, roots: Vec<PathBuf>) -> Vec<AgentSession> {
    let mut files = Vec::new();
    for root in roots {
        collect_jsonl(&root, &mut files, 0);
    }
    files.sort_unstable_by_key(|path| std::cmp::Reverse(mtime_of(path)));
    files.dedup();
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            let file = fs::File::open(path).ok()?;
            let header = BufReader::new(file)
                .lines()
                .take(40)
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
                .find(|value| value.get("type").and_then(Value::as_str) == Some("session"))?;
            if header.get("cwd").and_then(Value::as_str) != Some(cwd) {
                return None;
            }
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || omp_title(path))
                .or_else(|| {
                    header
                        .get("title")
                        .and_then(Value::as_str)
                        .and_then(condense)
                })
                .or_else(|| header.get("id").and_then(Value::as_str).and_then(condense))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("session")
                        .chars()
                        .take(13)
                        .collect()
                });
            Some(AgentSession {
                id: path.to_string_lossy().into_owned(),
                title,
                mtime,
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

fn scan_omp_line(line: &str, named: &mut Option<String>, first_user: &mut Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("title" | "session_info") => {
            if let Some(title) = value
                .get("title")
                .or_else(|| value.get("name"))
                .and_then(Value::as_str)
                .and_then(condense)
            {
                *named = Some(title);
            }
        }
        Some("session") if named.is_none() => {
            *named = value
                .get("title")
                .and_then(Value::as_str)
                .and_then(condense);
        }
        Some("message") if first_user.is_none() => {
            let Some(message) = value.get("message") else {
                return;
            };
            if message.get("role").and_then(Value::as_str) == Some("user") {
                *first_user = message
                    .get("content")
                    .and_then(text_from_content)
                    .and_then(|text| condense(&text));
            }
        }
        _ => {}
    }
}

fn omp_title(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut named = None;
    let mut first_user = None;
    let head = read_prefix(&mut file, CLAUDE_HEAD_BYTES.min(len))?;
    for line in head.lines() {
        scan_omp_line(line, &mut named, &mut first_user);
    }
    if len > CLAUDE_HEAD_BYTES {
        if let Some(tail) = read_suffix(&mut file, len.saturating_sub(CLAUDE_TAIL_BYTES)) {
            for line in tail.lines().skip(1) {
                scan_omp_line(line, &mut named, &mut first_user);
            }
        }
    }
    named.or(first_user)
}

fn collect_grok_session_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(groups) = fs::read_dir(root.join("sessions")) else {
        return out;
    };
    for group in groups
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
    {
        if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
            break;
        }
        let Ok(sessions) = fs::read_dir(group) else {
            continue;
        };
        for session in sessions
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
        {
            if out.len() >= MAX_AGENT_TRANSCRIPT_PATHS {
                break;
            }
            if session.join("updates.jsonl").is_file()
                || session.join("chat_history.jsonl").is_file()
            {
                out.push(session);
            }
        }
    }
    out
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let hex = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        out.push(hex(*bytes.get(index + 1)?)? * 16 + hex(*bytes.get(index + 2)?)?);
        index += 3;
    }
    String::from_utf8(out).ok()
}

fn grok_group_cwd(group: &Path) -> Option<PathBuf> {
    let explicit = fs::read_to_string(group.join(".cwd")).ok();
    if let Some(cwd) = explicit
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    {
        return Some(PathBuf::from(cwd));
    }
    let name = group.file_name()?.to_str()?;
    percent_decode(name)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
}

fn grok_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    content.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .or_else(|| part.get("text").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("")
    })
}

fn grok_user_query(text: &str) -> &str {
    text.split_once("<user_query>")
        .and_then(|(_, rest)| rest.split_once("</user_query>"))
        .map(|(body, _)| body)
        .unwrap_or(text)
}

fn grok_first_user(session_dir: &Path) -> Option<String> {
    if let Ok(file) = fs::File::open(session_dir.join("updates.jsonl")) {
        let mut chunks = String::new();
        for line in BufReader::new(file).lines().take(300).map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(update) = value.pointer("/params/update") else {
                continue;
            };
            match update.get("sessionUpdate").and_then(Value::as_str) {
                Some("user_message_chunk") => {
                    if let Some(text) = update.get("content").and_then(grok_content_text) {
                        chunks.push_str(&text);
                    }
                }
                Some(_) if !chunks.is_empty() => break,
                _ => {}
            }
        }
        if let Some(title) = condense(grok_user_query(&chunks)) {
            return Some(title);
        }
    }
    let file = fs::File::open(session_dir.join("chat_history.jsonl")).ok()?;
    for line in BufReader::new(file).lines().take(300).map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("user")
            || value.get("synthetic_reason").is_some()
        {
            continue;
        }
        let text = value.get("content").and_then(grok_content_text)?;
        if let Some(title) = condense(grok_user_query(&text)) {
            return Some(title);
        }
    }
    None
}

fn grok_session_mtime(session_dir: &Path) -> u64 {
    ["summary.json", "updates.jsonl", "chat_history.jsonl"]
        .iter()
        .map(|name| mtime_of(&session_dir.join(name)))
        .max()
        .unwrap_or(0)
}

fn grok_session(session_dir: &Path, cwd: &str) -> Option<AgentSession> {
    let summary_path = session_dir.join("summary.json");
    let summary = fs::read_to_string(&summary_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let workspace = summary
        .as_ref()
        .and_then(|value| value.pointer("/info/cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| session_dir.parent().and_then(grok_group_cwd))?;
    if workspace != Path::new(cwd) {
        return None;
    }
    let id = summary
        .as_ref()
        .and_then(|value| value.pointer("/info/id"))
        .and_then(Value::as_str)
        .or_else(|| session_dir.file_name().and_then(|name| name.to_str()))?;
    let title = summary
        .as_ref()
        .and_then(|value| value.get("generated_title"))
        .and_then(Value::as_str)
        .and_then(condense)
        .or_else(|| {
            summary
                .as_ref()
                .and_then(|value| value.get("session_summary"))
                .and_then(Value::as_str)
                .and_then(condense)
        })
        .or_else(|| grok_first_user(session_dir))
        .unwrap_or_else(|| id.chars().take(13).collect());
    Some(AgentSession {
        id: id.to_string(),
        title,
        mtime: grok_session_mtime(session_dir),
    })
}

fn grok_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = grok_root() else {
        return Vec::new();
    };
    let mut dirs = collect_grok_session_dirs(&root);
    dirs.sort_unstable_by_key(|path| std::cmp::Reverse(grok_session_mtime(path)));
    dirs.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);
    let mut out: Vec<_> = dirs
        .par_iter()
        .filter_map(|path| grok_session(path, cwd))
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

// ---- hermes — `sessions` table in ~/.hermes/state.db (SQLite) -----------
fn hermes_sessions() -> Vec<AgentSession> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let db = PathBuf::from(&home).join(".hermes/state.db");
    if !db.exists() {
        return Vec::new();
    }

    let conn = match Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, \
         COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
         CAST(COALESCE(started_at, 0) AS INTEGER) AS mtime \
         FROM sessions ORDER BY started_at DESC LIMIT 400",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        Ok(AgentSession {
            id: row.get::<_, String>(0)?,
            title: row.get::<_, String>(1)?,
            mtime: row.get::<_, i64>(2).unwrap_or(0) as u64,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// ---- opencode — SQLite in the user's opencode data dir ------------------
fn opencode_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("OPENCODE_DATA_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/opencode"));
        dirs.push(PathBuf::from(&home).join("Library/Application Support/opencode"));
        dirs.push(PathBuf::from(&home).join(".opencode/data"));
    }
    dirs
}

fn opencode_db_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in opencode_data_dirs() {
        let direct = dir.join("opencode.db");
        if direct.exists() {
            paths.push(direct);
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with("opencode")
                && name.ends_with(".db")
                && !paths.iter().any(|p| p == &path)
            {
                paths.push(path);
            }
        }
    }
    paths
}

fn normalize_unix_secs(raw: u64) -> u64 {
    if raw > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

fn opencode_sessions(cwd: &str) -> Vec<AgentSession> {
    let mut out = Vec::new();
    for db in opencode_db_paths() {
        let Ok(conn) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        out.extend(opencode_sessions_from_conn(&conn, cwd));
    }
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.id.clone()));
    out.truncate(400);
    out
}

fn opencode_sessions_from_conn(conn: &Connection, cwd: &str) -> Vec<AgentSession> {
    let with_project = "\
        SELECT s.id, \
               COALESCE(NULLIF(TRIM(s.title), ''), NULLIF(TRIM(s.slug), ''), substr(s.id, 1, 13)) AS title, \
               CAST(COALESCE(s.time_updated, s.time_created, 0) AS INTEGER) AS mtime \
        FROM session s \
        LEFT JOIN project p ON p.id = s.project_id \
        WHERE s.directory = ?1 OR s.path = ?1 OR p.worktree = ?1 \
        ORDER BY COALESCE(s.time_updated, s.time_created, 0) DESC \
        LIMIT 400";
    if let Some(rows) = opencode_query(conn, with_project, cwd) {
        return rows;
    }

    let session_only = "\
        SELECT id, \
               COALESCE(NULLIF(TRIM(title), ''), NULLIF(TRIM(slug), ''), substr(id, 1, 13)) AS title, \
               CAST(COALESCE(time_updated, time_created, 0) AS INTEGER) AS mtime \
        FROM session \
        WHERE directory = ?1 OR path = ?1 \
        ORDER BY COALESCE(time_updated, time_created, 0) DESC \
        LIMIT 400";
    if let Some(rows) = opencode_query(conn, session_only, cwd) {
        return rows;
    }

    let minimal = "\
        SELECT id, \
               COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
               CAST(COALESCE(time_updated, time_created, 0) AS INTEGER) AS mtime \
        FROM session \
        WHERE directory = ?1 \
        ORDER BY COALESCE(time_updated, time_created, 0) DESC \
        LIMIT 400";
    opencode_query(conn, minimal, cwd).unwrap_or_default()
}

fn opencode_query(conn: &Connection, sql: &str, cwd: &str) -> Option<Vec<AgentSession>> {
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([cwd], |row| {
            Ok(AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: normalize_unix_secs(row.get::<_, i64>(2).unwrap_or(0).max(0) as u64),
            })
        })
        .ok()?;
    Some(rows.filter_map(|r| r.ok()).collect())
}

#[cfg(test)]
mod executable_tests {
    use super::{
        agent_config_root, allowed_agent_path, cached_title, claude_sessions, codex_indexed_titles,
        codex_sessions, codex_title, grok_session, json_effort, omp_sessions_from_dirs, omp_title,
        parse_claude_models, parse_claude_usage, parse_codex_models, parse_codex_usage_result,
        parse_grok_models, parse_hermes_models, parse_line_models, parse_omp_models,
        parse_pi_models, percent_decode, qualify_model, title_cache_stamp, toml_effort, toml_model,
        toml_section_string, yaml_agent_reasoning_effort, yaml_model_section,
        yaml_top_level_scalar, AgentModelInfo, AgentUsageResetAt, CLAUDE_MODEL_CATALOG_ARGS,
    };
    #[cfg(unix)]
    use super::{
        first_healthy_agent_candidate, probe_agent_executable, probe_agent_executable_with_timeout,
        run_model_catalog_executable, MODEL_CATALOG_ERROR_DETAIL_LIMIT,
    };
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn opencode_installer_directory_is_a_valid_agent_location() {
        let home = Path::new("/home/tester");
        for name in ["opencode", "opencode.exe", "opencode.cmd"] {
            assert!(allowed_agent_path(
                "opencode",
                &home.join(".opencode").join("bin").join(name)
            ));
        }
    }

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

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_rejects_broken_wrappers() {
        use std::os::unix::fs::PermissionsExt;

        let mut healthy = tempfile::NamedTempFile::new().unwrap();
        writeln!(healthy, "#!/bin/sh\nprintf 'codex-cli test\\n'").unwrap();
        let mut permissions = healthy.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        healthy.as_file().set_permissions(permissions).unwrap();

        let mut broken = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            broken,
            "#!/bin/sh\nprintf 'saved launcher missing\\n' >&2\nexit 127"
        )
        .unwrap();
        let mut permissions = broken.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        broken.as_file().set_permissions(permissions).unwrap();

        assert!(probe_agent_executable("codex", healthy.path())
            .await
            .is_ok());
        assert!(probe_agent_executable("codex", broken.path())
            .await
            .unwrap_err()
            .contains("saved launcher missing"));

        let (selected, failures) = first_healthy_agent_candidate(
            "codex",
            vec![broken.path().to_path_buf(), healthy.path().to_path_buf()],
        )
        .await;
        assert_eq!(selected.as_deref(), Some(healthy.path()));
        assert_eq!(failures.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_reuses_success_and_invalidates_changed_files() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agent");
        let count = dir.path().join("count");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho x >> '{}'\necho version-one\n",
                count.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            probe_agent_executable("codex", &executable).await.unwrap(),
            "version-one"
        );
        assert_eq!(
            probe_agent_executable("codex", &executable).await.unwrap(),
            "version-one"
        );
        assert_eq!(fs::read_to_string(&count).unwrap(), "x\n");
        fs::write(&executable, "#!/bin/sh\necho version-two-changed\n").unwrap();
        assert_eq!(
            probe_agent_executable("codex", &executable).await.unwrap(),
            "version-two-changed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_bounds_temporarily_busy_updaters() {
        use std::os::unix::fs::PermissionsExt;

        let mut busy = tempfile::NamedTempFile::new().unwrap();
        writeln!(busy, "#!/bin/sh\nsleep 1\nprintf 'claude test\\n'").unwrap();
        let mut permissions = busy.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        busy.as_file().set_permissions(permissions).unwrap();

        assert_eq!(
            probe_agent_executable_with_timeout(
                "claude",
                busy.path(),
                std::time::Duration::from_millis(20)
            )
            .await
            .unwrap_err(),
            "version check timed out"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hermes_probe_skips_the_networked_version_check() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        // Mirrors the real CLI: `--version` blocks on an update check, while
        // `--help` answers immediately.
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("hermes");
        fs::write(
            &executable,
            "#!/bin/sh\ncase \"$1\" in\n--help) printf 'usage: hermes\\n';;\n*) sleep 30;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            probe_agent_executable("hermes", &executable).await.unwrap(),
            "usage: hermes"
        );
        assert!(
            probe_agent_executable_with_timeout(
                "claude",
                &executable,
                std::time::Duration::from_millis(20)
            )
            .await
            .is_err(),
            "other agents keep probing --version"
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

    #[test]
    fn claude_catalog_enables_print_mode_for_stream_json() {
        assert!(CLAUDE_MODEL_CATALOG_ARGS.contains(&"--print"));
        assert!(CLAUDE_MODEL_CATALOG_ARGS
            .windows(2)
            .any(|args| args == ["--input-format", "stream-json"]));
        assert!(CLAUDE_MODEL_CATALOG_ARGS
            .windows(2)
            .any(|args| args == ["--output-format", "stream-json"]));
    }

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

    #[test]
    fn codex_title_reads_the_current_user_message_shape() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            r#"{{"type":"session_meta","payload":{{"id":"session-1","cwd":"/repo"}}}}"#
        )
        .unwrap();
        writeln!(
            transcript,
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"Explain this codebase"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();

        assert_eq!(
            codex_title(transcript.path()).as_deref(),
            Some("Explain this codebase")
        );
    }

    #[test]
    fn codex_sessions_use_the_latest_indexed_thread_name() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("sessions");
        std::fs::create_dir(&sessions_dir).unwrap();
        std::fs::write(
            root.path().join("session_index.jsonl"),
            concat!(
                "{\"id\":\"session-1\",\"thread_name\":\"Initial title\"}\n",
                "{\"id\":\"session-1\",\"thread_name\":\"Add draggable project sorting\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("rollout-session-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\",\"cwd\":\"/repo\"}}\n",
        )
        .unwrap();

        assert_eq!(
            codex_indexed_titles(root.path())
                .get("session-1")
                .map(String::as_str),
            Some("Add draggable project sorting")
        );
        let sessions = codex_sessions("/repo", root.path().to_str());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Add draggable project sorting");
    }

    #[test]
    fn claude_sessions_omit_titleless_command_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("projects").join("-repo");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("conversation.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"Fix top bar overlaps\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("usage-command.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/usage</command-name>\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("cancelled-resume.jsonl"),
            "{\"type\":\"system\",\"subtype\":\"local_command\",\"content\":\"<command-name>/resume</command-name>\"}\n",
        )
        .unwrap();

        let sessions = claude_sessions("/repo", root.path().to_str());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "conversation");
        assert_eq!(sessions[0].title, "Fix top bar overlaps");
    }

    #[test]
    fn title_cache_rechecks_a_transcript_after_a_same_tick_append() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            r#"{{"type":"session_meta","payload":{{"id":"session-1","cwd":"/repo"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let first_stamp = title_cache_stamp(transcript.path());
        assert_eq!(
            cached_title(transcript.path(), first_stamp, || codex_title(
                transcript.path()
            )),
            None
        );

        writeln!(
            transcript,
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"Hello"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let second_stamp = title_cache_stamp(transcript.path());
        assert_ne!(first_stamp.len, second_stamp.len);
        assert_eq!(
            cached_title(transcript.path(), second_stamp, || codex_title(
                transcript.path()
            ))
            .as_deref(),
            Some("Hello")
        );
    }

    #[test]
    fn omp_title_prefers_the_latest_explicit_name() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"title","title":"Initial title"})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"session","id":"session-1","cwd":"/repo"})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"message","message":{"role":"user","content":"Fallback prompt"}})
        )
        .unwrap();
        writeln!(
            transcript,
            "{}",
            serde_json::json!({"type":"title","title":"Renamed session"})
        )
        .unwrap();
        transcript.flush().unwrap();

        assert_eq!(
            omp_title(transcript.path()).as_deref(),
            Some("Renamed session")
        );
    }

    #[test]
    fn omp_session_listing_accepts_title_before_header() {
        let root = tempfile::tempdir().unwrap();
        let transcript = root.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"title\",\"title\":\"Ship OMP support\"}\n",
                "{\"type\":\"session\",\"id\":\"session-1\",\"cwd\":\"/repo\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Fallback\"}}\n"
            ),
        )
        .unwrap();

        let sessions = omp_sessions_from_dirs("/repo", vec![root.path().to_path_buf()]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, transcript.to_string_lossy());
        assert_eq!(sessions[0].title, "Ship OMP support");
    }

    #[test]
    fn grok_summary_maps_workspace_title_and_resume_id() {
        let root = tempfile::tempdir().unwrap();
        let session = root.path().join("%2Frepo").join("session-1");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("summary.json"),
            r#"{"info":{"id":"session-1","cwd":"/repo"},"generated_title":"Fix flaky tests"}"#,
        )
        .unwrap();
        std::fs::write(session.join("updates.jsonl"), "{}\n").unwrap();

        let result = grok_session(&session, "/repo").unwrap();
        assert_eq!(result.id, "session-1");
        assert_eq!(result.title, "Fix flaky tests");
        assert_eq!(percent_decode("%2Frepo").as_deref(), Some("/repo"));
        assert!(grok_session(&session, "/another").is_none());
    }
}
