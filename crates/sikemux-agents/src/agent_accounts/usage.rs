//! How much of each account's plan is used, for providers that report
//! rolling windows. Each lookup goes through the agent's own CLI, so Sikemux
//! sees the same account and the same limits the agent does.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;
use sikemux_pty::user_shell::login_shell_environment;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{cli, key};
use crate::agent_catalog::AgentKind;
use crate::launch::discovery::error_detail;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum UsageResetAt {
    Unix(u64),
    Iso(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub label: String,
    /// The model or limit the window is for, such as Opus; none for the whole plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<UsageResetAt>,
    pub window_minutes: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub provider: String,
    pub plan: Option<String>,
    /// The plan as its provider names it to the person, such as Claude's "Ultra" or Codex's "Pro".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub unavailable_reason: Option<String>,
    /// The limit resets the account holds; none when its provider offers it none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets: Option<Resets>,
    /// Read in one small request rather than by booting the CLI, so it may be read again soon.
    #[serde(skip)]
    pub light: bool,
}

/// Limit resets an account was given, by kind.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resets {
    pub left: u32,
    pub kinds: Vec<ResetKind>,
}

/// Resets of one kind, such as Claude's weekly or Fable resets.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResetKind {
    /// The grant spending one uses: the kind's first to run out. None when the agent chooses.
    pub id: Option<String>,
    pub label: String,
    pub left: u32,
    /// When the first of them expires, in Unix milliseconds; none when none do.
    pub use_by: Option<u64>,
    /// The label of the window one refills; none when one starts every used-up window over.
    pub window: Option<String>,
}

/// What spending a reset did.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ResetOutcome {
    /// The account's used-up windows started over.
    Reset,
    /// No window was far enough along to reset, so none was spent.
    NothingToReset,
    /// The account holds no resets.
    NoReset,
}

impl UsageResetAt {
    /// When the window starts again, in Unix milliseconds. A CLI may give the seconds as
    /// digits in a string.
    pub fn unix_ms(&self) -> Option<u64> {
        match self {
            Self::Unix(seconds) => seconds.checked_mul(1000),
            Self::Iso(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
                text.parse::<u64>().ok()?.checked_mul(1000)
            }
            Self::Iso(text) => crate::accounts::live::parse_rfc3339(text),
        }
    }
}

impl UsageWindow {
    /// A window of `duration`, such as 7d, over `scope`, labelled as the person reads it.
    fn new(
        scope: Option<&str>,
        duration: &str,
        used_percent: f64,
        resets_at: Option<UsageResetAt>,
        window_minutes: Option<u64>,
    ) -> Self {
        Self {
            label: match scope {
                Some(scope) => format!("{scope} · {duration}"),
                None => duration.to_owned(),
            },
            scope: scope.map(str::to_owned),
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at,
            window_minutes,
        }
    }

    /// Whether the window has started over since it was read, so its numbers are the last
    /// window's.
    pub fn is_past_reset(&self, now_ms: u64) -> bool {
        (self.resets_at.as_ref())
            .and_then(UsageResetAt::unix_ms)
            .is_some_and(|reset| reset <= now_ms)
    }

    /// How much of the window is used at `now_ms`: nothing once it has started over.
    pub fn used_at(&self, now_ms: u64) -> f64 {
        if self.is_past_reset(now_ms) {
            0.0
        } else {
            self.used_percent.clamp(0.0, 100.0)
        }
    }
}

impl Usage {
    /// The plan's own week: its first week-long window, before any one model's.
    pub fn week(&self) -> Option<&UsageWindow> {
        (self.windows.iter()).find(|window| window.window_minutes == Some(10_080))
    }

    /// The plan's five-hour session window.
    pub fn session(&self) -> Option<&UsageWindow> {
        (self.windows.iter())
            .find(|window| window.window_minutes == Some(300) && window.scope.is_none())
    }

    /// When the account can be used again, in Unix milliseconds: once each
    /// window it has used up resets. `None` when none reads used up, or one
    /// that does never says when it resets.
    pub fn resets_at(&self) -> Option<u64> {
        let full = self
            .windows
            .iter()
            .filter(|window| window.used_percent >= 100.0);
        full.map(|window| window.resets_at.as_ref()?.unix_ms())
            .collect::<Option<Vec<u64>>>()?
            .into_iter()
            .max()
    }
}

/// A lookup asks the provider's usage endpoint, which refuses callers that ask
/// often, and Codex's boots its CLI. So an answer is kept for a while, a failed
/// one too, and a refusal for longer. Claude's is one small request, the one
/// claude.ai makes each time its page is looked at, so it is kept briefly.
pub const USAGE_TTL: Duration = Duration::from_secs(120);
const CLAUDE_USAGE_TTL: Duration = Duration::from_secs(20);
const FAILED_TTL: Duration = Duration::from_secs(60);
const REFUSED_TTL: Duration = Duration::from_secs(600);
/// What a lookup fails with when the agent says its account is signed out.
pub const SIGNED_OUT: &str = "Not signed in";

type Lookup = Result<Usage, String>;
/// When a lookup was kept, for how long, and what it answered.
type Kept = (Instant, Duration, Lookup);

fn usage_cache() -> &'static Mutex<HashMap<String, Kept>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Kept>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One lookup at a time per account, so callers that ask together share it.
fn in_flight(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new())).locked();
    locks.entry(key.to_owned()).or_default().clone()
}

fn remembered(key: &str) -> Option<Lookup> {
    let mut cache = usage_cache().locked();
    cache.retain(|_, (stored, ttl, _)| stored.elapsed() < *ttl);
    cache.get(key).map(|(_, _, lookup)| lookup.clone())
}

fn refused(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("429") || error.contains("rate limit") || error.contains("rate_limit")
}

/// The plan usage of one account, as its CLI last reported it.
pub async fn usage(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
) -> Result<Usage, String> {
    let key = key(provider, config_path);
    if let Some(lookup) = remembered(&key) {
        return lookup;
    }
    let lock = in_flight(&key);
    let _turn = lock.lock().await;
    if let Some(lookup) = remembered(&key) {
        return lookup;
    }
    let read =
        AgentKind::from_name(provider).and_then(|kind| kind.ops().usage(program, config_path));
    let lookup = match read {
        Some(read) => read.await,
        None => Err(format!("{provider} does not expose structured plan usage")),
    };
    let ttl = match &lookup {
        Ok(usage) if usage.light => CLAUDE_USAGE_TTL,
        Ok(_) => USAGE_TTL,
        Err(error) if refused(error) => REFUSED_TTL,
        Err(_) => FAILED_TTL,
    };
    if let Ok(usage) = &lookup {
        note_reading(&key, usage);
    }
    usage_cache()
        .locked()
        .insert(key, (Instant::now(), ttl, lookup.clone()));
    lookup
}

/// An account's last good reading and when it was taken, in Unix milliseconds.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub read_at: u64,
    pub usage: Usage,
}

/// Each account's last good reading, kept in a file across restarts once the core names one,
/// so the limits show at once after a restart.
#[derive(Default)]
struct Readings {
    file: Option<PathBuf>,
    by_account: BTreeMap<String, Reading>,
}

fn readings() -> &'static Mutex<Readings> {
    static READINGS: OnceLock<Mutex<Readings>> = OnceLock::new();
    READINGS.get_or_init(Default::default)
}

/// Keeps each account's last reading in `file`, taking up what an earlier core kept there.
pub fn keep_readings_in(file: PathBuf) {
    let kept = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    *readings().locked() = Readings {
        file: Some(file),
        by_account: kept,
    };
}

/// How fast one account's shortest window fills, from two readings in the same window.
struct Burn {
    at: u64,
    used: f64,
    reset: Option<u64>,
    per_minute: f64,
    rated_at: u64,
}

fn burns() -> &'static Mutex<HashMap<String, Burn>> {
    static BURNS: OnceLock<Mutex<HashMap<String, Burn>>> = OnceLock::new();
    BURNS.get_or_init(Default::default)
}

/// A rate older than this says nothing of the chats running now.
const BURN_HOLDS_FOR_MS: u64 = 15 * 60 * 1000;
/// Readings closer together than this are too near to rate.
const BURN_SPAN_MS: u64 = 60 * 1000;

fn note_burn(key: &str, usage: &Usage, now: u64) {
    let Some(short) = (usage.windows.iter())
        .filter(|window| window.scope.is_none())
        .find(|window| {
            window
                .window_minutes
                .is_some_and(|minutes| minutes < 10_080)
        })
    else {
        return;
    };
    let (used, reset) = (
        short.used_at(now),
        short.resets_at.as_ref().and_then(UsageResetAt::unix_ms),
    );
    let mut burns = burns().locked();
    let burn = burns.entry(key.to_owned()).or_insert(Burn {
        at: now,
        used,
        reset,
        per_minute: 0.0,
        rated_at: 0,
    });
    if burn.reset != reset {
        *burn = Burn {
            at: now,
            used,
            reset,
            per_minute: 0.0,
            rated_at: 0,
        };
        return;
    }
    let span = now.saturating_sub(burn.at);
    if span < BURN_SPAN_MS {
        return;
    }
    burn.per_minute = ((used - burn.used) / (span as f64 / 60_000.0)).max(0.0);
    (burn.at, burn.used, burn.rated_at) = (now, used, now);
}

/// How fast the chats running on the account fill its shortest window, in percent a minute; none
/// while nothing measured it lately.
pub fn burn(provider: &str, config_path: Option<&str>) -> f64 {
    let now = sikemux_process::clock::epoch_ms();
    burns()
        .locked()
        .get(&key(provider, config_path))
        .filter(|burn| now.saturating_sub(burn.rated_at) < BURN_HOLDS_FOR_MS)
        .map_or(0.0, |burn| burn.per_minute)
}

fn note_reading(key: &str, usage: &Usage) {
    let usage = usage.clone();
    let read_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    note_burn(key, &usage, read_at);
    let mut readings = readings().locked();
    readings
        .by_account
        .insert(key.to_owned(), Reading { read_at, usage });
    save(&readings);
    drop(readings);
    landed().send_modify(|count| *count += 1);
}

fn landed() -> &'static tokio::sync::watch::Sender<u64> {
    static LANDED: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    LANDED.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// Changes each time a reading of any account lands, read from its CLI or reported by a chat's
/// agent as it works.
pub fn readings_landed() -> tokio::sync::watch::Receiver<u64> {
    landed().subscribe()
}

fn save(readings: &Readings) {
    let Some(file) = &readings.file else {
        return;
    };
    let written = serde_json::to_vec(&readings.by_account)
        .map_err(std::io::Error::other)
        .and_then(|bytes| sikemux_process::files::write_atomic(file, &bytes, true));
    match written {
        Err(error) => eprintln!("sikemux core: could not keep the plan limits read: {error}"),
        Ok(()) => {
            static FIRST: std::sync::Once = std::sync::Once::new();
            FIRST.call_once(|| {
                eprintln!(
                    "sikemux core: kept plan limits in {} (first save since start)",
                    file.display()
                )
            });
        }
    }
}

/// Folds the windows an agent reported while it worked into its account's reading, which then
/// counts as fresh, and answers whether that changed what a person would see.
pub fn note_live(provider: &str, config_path: Option<&str>, live: Vec<UsageWindow>) -> bool {
    if live.is_empty() {
        return false;
    }
    let key = key(provider, config_path);
    let mut readings = readings().locked();
    let reading = readings
        .by_account
        .entry(key.clone())
        .or_insert_with(|| Reading {
            read_at: 0,
            usage: Usage {
                provider: provider.to_owned(),
                plan: None,
                tier: None,
                windows: Vec::new(),
                unavailable_reason: None,
                resets: None,
                light: false,
            },
        });
    let mut changed = false;
    for window in live {
        match (reading.usage.windows.iter_mut()).find(|kept| kept.label == window.label) {
            Some(kept) => {
                changed |= (kept.used_percent - window.used_percent).abs() >= 1.0
                    || kept.resets_at.as_ref().and_then(UsageResetAt::unix_ms)
                        != window.resets_at.as_ref().and_then(UsageResetAt::unix_ms);
                *kept = window;
            }
            None => {
                reading.usage.windows.push(window);
                changed = true;
            }
        }
    }
    reading.read_at = sikemux_process::clock::epoch_ms();
    let usage = reading.usage.clone();
    note_burn(&key, &usage, reading.read_at);
    if changed {
        save(&readings);
    }
    drop(readings);
    usage_cache()
        .locked()
        .insert(key, (Instant::now(), USAGE_TTL, Ok(usage)));
    landed().send_modify(|count| *count += 1);
    changed
}

/// The windows in the limit Claude reports on its stream as a turn goes, whose use is a
/// fraction where its usage lookup gives a percentage.
pub fn claude_live(info: &Value) -> Vec<UsageWindow> {
    let Some(windows) = info.get("unifiedWindows").and_then(Value::as_object) else {
        return Vec::new();
    };
    let named = |key: &str| match key {
        "five_hour" => Some((None, "5h", 300)),
        "seven_day" => Some((None, "7d", 10_080)),
        "seven_day_opus" => Some((Some("Opus"), "7d", 10_080)),
        "seven_day_sonnet" => Some((Some("Sonnet"), "7d", 10_080)),
        _ => None,
    };
    (windows.iter())
        .filter_map(|(key, window)| {
            let (scope, duration, minutes) = named(key)?;
            Some(UsageWindow::new(
                scope,
                duration,
                window.get("utilization")?.as_f64()? * 100.0,
                window.get("resetsAt").and_then(usage_reset_at),
                Some(minutes),
            ))
        })
        .collect()
}

/// The windows in the limits Codex sends as they change.
pub fn codex_live(params: &Value) -> Vec<UsageWindow> {
    let Some(limits) = params.get("rateLimits").filter(|limits| limits.is_object()) else {
        return Vec::new();
    };
    parse_codex_usage_result(&serde_json::json!({ "rateLimits": limits })).windows
}

/// The account's last good reading, however old, without asking its CLI.
pub fn last_reading(provider: &str, config_path: Option<&str>) -> Option<Reading> {
    readings()
        .locked()
        .by_account
        .get(&key(provider, config_path))
        .cloned()
}

/// Why the account's latest lookup failed, while that failure is remembered.
pub fn last_problem(provider: &str, config_path: Option<&str>) -> Option<String> {
    match usage_cache().locked().get(&key(provider, config_path)) {
        Some((_, _, Err(problem))) => Some(problem.clone()),
        _ => None,
    }
}

/// Drops the answer kept for one account's lookup, so the next one asks its CLI again.
pub fn forget_lookup(provider: &str, config_path: Option<&str>) {
    usage_cache().locked().remove(&key(provider, config_path));
}

/// Drops the usage remembered for one account, whose sign-in just changed.
pub fn forget(provider: &str, config_path: Option<&str>) {
    forget_lookup(provider, config_path);
    let mut readings = readings().locked();
    if readings
        .by_account
        .remove(&key(provider, config_path))
        .is_some()
    {
        save(&readings);
    }
}

fn clean(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= 256).then(|| value.to_string())
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

pub const CLAUDE_STREAM_ARGS: &[&str] = &[
    "--print",
    "--no-session-persistence",
    "--output-format",
    "stream-json",
    "--verbose",
    "--system-prompt",
    "",
    "--tools",
    "",
    "--strict-mcp-config",
    "--mcp-config",
    "{\"mcpServers\":{}}",
    "--input-format",
    "stream-json",
];

async fn read_claude_usage(
    executable: &Path,
    config_path: Option<&str>,
    removed_env: &[&str],
) -> Result<Usage, String> {
    let request = format!(
        "{{\"type\":\"control_request\",\"request_id\":\"{CLAUDE_USAGE_REQUEST_ID}\",\"request\":{{\"subtype\":\"get_usage\",\"skip_behaviors\":true}}}}\n"
    );
    let text = run(
        "claude",
        executable,
        CLAUDE_STREAM_ARGS,
        &request,
        config_path,
        removed_env,
    )
    .await?;
    parse_claude_usage(&text, CLAUDE_USAGE_REQUEST_ID)
        .ok_or_else(|| "Claude returned an unreadable usage snapshot".to_string())
}

/// Runs `executable` once with `input`, clearing `removed_env` after the
/// login shell's variables, so one the profile set is dropped too.
async fn run(
    provider: &str,
    executable: &Path,
    args: &[&str],
    input: &str,
    config_path: Option<&str>,
    removed_env: &[&str],
) -> Result<String, String> {
    let mut command = cli(provider, executable, config_path, BTreeMap::new());
    for key in removed_env {
        command.env_remove(key);
    }
    ask(provider, command, args, input).await
}

/// Runs `command` once with `args`, writing `input` and answering what it printed.
pub async fn ask(
    provider: &str,
    mut command: Command,
    args: &[&str],
    input: &str,
) -> Result<String, String> {
    command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start the {provider} usage lookup"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("Could not open the {provider} usage lookup input"))?;
    stdin
        .write_all(input.as_bytes())
        .await
        .map_err(|_| format!("Could not write the {provider} usage lookup input"))?;
    // A CLI reading stream-json input answers and exits only once its input ends.
    drop(stdin);
    let output = tokio::time::timeout(USAGE_LOOKUP_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("The {provider} usage lookup timed out"))?
        .map_err(|_| format!("Could not read the {provider} usage lookup output"))?;
    if !output.status.success() {
        return Err(match error_detail(&output.stderr) {
            Some(detail) => format!("{provider} usage lookup exited unsuccessfully: {detail}"),
            None => format!("{provider} usage lookup exited unsuccessfully"),
        });
    }
    if output.stdout.len() > USAGE_LOOKUP_OUTPUT_LIMIT {
        return Err(format!("The {provider} usage lookup said too much"));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("The {provider} usage lookup was not valid UTF-8"))
}

/// Runs the account's CLI once, which renews its sign-in when that has run out.
pub(super) async fn renew_claude_sign_in(executable: &Path, config_path: Option<&str>) {
    let _ = read_claude_usage(executable, config_path, &[]).await;
}

pub(crate) async fn claude_usage(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<Usage, String> {
    let tier = claude_plan_name(config_path);
    if let Some(mut usage) = super::claude_resets::usage(executable, config_path).await {
        usage.plan = tier.as_deref().map(str::to_ascii_lowercase);
        usage.tier = tier;
        return Ok(usage);
    }
    let mut usage = claude_plan_usage(executable, config_path).await?;
    if usage.unavailable_reason.is_none() {
        usage.resets = super::claude_resets::read(executable, config_path, &usage.windows).await;
    }
    usage.tier = tier;
    Ok(usage)
}

/// What Claude calls the account's plan to the person, kept in its config by the CLI.
fn claude_plan_name(config_path: Option<&str>) -> Option<String> {
    let file = super::folders::claude_config_file(config_path, &BTreeMap::new());
    let config: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let name = config
        .pointer("/oauthAccount/planDisplayName")?
        .as_str()?
        .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn capitalized(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

pub(crate) async fn use_claude_reset(
    executable: &Path,
    config_path: Option<&str>,
    kind: Option<&str>,
) -> Result<ResetOutcome, String> {
    super::claude_resets::spend(executable, config_path, kind).await
}

async fn claude_plan_usage(executable: &Path, config_path: Option<&str>) -> Result<Usage, String> {
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

pub(crate) async fn codex_usage(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<Usage, String> {
    codex_app_server(executable, config_path, "account/rateLimits/read", "usage")
        .await
        .map(|result| parse_codex_usage_result(&result))
}

pub(super) async fn codex_app_server(
    executable: &Path,
    config_path: Option<&str>,
    method: &str,
    topic: &str,
) -> Result<Value, String> {
    let command = cli("codex", executable, config_path, BTreeMap::new());
    codex_ask(command, method, Value::Null, topic).await
}

/// Asks `command`, a `codex` program, one question as a short-lived app server.
pub async fn codex_ask(
    mut command: Command,
    method: &str,
    params: Value,
    topic: &str,
) -> Result<Value, String> {
    command
        .args(["app-server", "--stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start Codex {topic} lookup"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("Could not open Codex {topic} lookup input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Could not read Codex {topic} lookup output"))?;
    let mut lines = BufReader::new(stdout).lines();

    let lookup = tokio::time::timeout(USAGE_LOOKUP_TIMEOUT, async {
        let initialize = serde_json::json!({
            "id": CODEX_USAGE_INITIALIZE_ID,
            "method": "initialize",
            "params": crate::sessions::codex::initialize_params(),
        });
        stdin
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .map_err(|_| format!("Could not initialize Codex {topic} lookup"))?;
        stdin
            .flush()
            .await
            .map_err(|_| format!("Could not initialize Codex {topic} lookup"))?;

        let mut output_bytes = 0usize;
        let mut initialized = false;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| format!("Could not read Codex {topic} lookup output"))?
        {
            output_bytes = output_bytes.saturating_add(line.len());
            if output_bytes > USAGE_LOOKUP_OUTPUT_LIMIT {
                return Err(format!("Codex {topic} lookup output was too large"));
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let response_id = value.get("id").and_then(Value::as_u64);
            if response_id == Some(CODEX_USAGE_INITIALIZE_ID) && !initialized {
                if value.get("error").is_some() {
                    return Err(format!("Codex rejected {topic} lookup initialization"));
                }
                let requests = format!(
                    "{{\"method\":\"initialized\"}}\n{}\n",
                    serde_json::json!({ "id": CODEX_USAGE_REQUEST_ID, "method": method, "params": params })
                );
                stdin
                    .write_all(requests.as_bytes())
                    .await
                    .map_err(|_| format!("Could not request Codex {topic}"))?;
                stdin
                    .flush()
                    .await
                    .map_err(|_| format!("Could not request Codex {topic}"))?;
                initialized = true;
                continue;
            }
            if response_id != Some(CODEX_USAGE_REQUEST_ID) {
                continue;
            }
            if let Some(error) = value.get("error") {
                let message = error.get("message").and_then(Value::as_str);
                if message.is_some_and(|message| message.contains("authentication required")) {
                    return Err(SIGNED_OUT.to_owned());
                }
                return Err(format!("Codex rejected the {topic} lookup"));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| format!("Codex returned an empty {topic} answer"));
        }
        Err(format!("Codex {topic} lookup closed before responding"))
    })
    .await;

    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;

    lookup.map_err(|_| format!("Codex {topic} lookup timed out"))?
}

fn usage_reset_at(value: &Value) -> Option<UsageResetAt> {
    value.as_u64().map(UsageResetAt::Unix).or_else(|| {
        value
            .as_str()
            .map(|value| UsageResetAt::Iso(value.to_string()))
    })
}

fn push_claude_usage_window(
    windows: &mut Vec<UsageWindow>,
    limits: &Value,
    key: &str,
    (scope, duration): (Option<&str>, &str),
    window_minutes: u64,
) {
    let Some(window) = limits.get(key) else {
        return;
    };
    let Some(used_percent) = window.get("utilization").and_then(Value::as_f64) else {
        return;
    };
    windows.push(UsageWindow::new(
        scope,
        duration,
        used_percent,
        window.get("resets_at").and_then(usage_reset_at),
        Some(window_minutes),
    ));
}

/// What Claude answered the control request `request_id`, among the lines it printed.
pub fn control_answer(text: &str, request_id: &str) -> Option<Value> {
    text.lines().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let response = value.get("response")?;
        if value.get("type").and_then(Value::as_str) != Some("control_response")
            || response.get("request_id").and_then(Value::as_str) != Some(request_id)
            || response.get("subtype").and_then(Value::as_str) != Some("success")
        {
            return None;
        }
        response.get("response").cloned()
    })
}

fn parse_claude_usage(text: &str, request_id: &str) -> Option<Usage> {
    let payload = control_answer(text, request_id)?;

    if !payload.get("rate_limits").is_some_and(Value::is_object)
        && payload
            .get("rate_limits_available")
            .and_then(Value::as_bool)
            .is_none()
    {
        return None;
    }
    let available = payload
        .get("rate_limits_available")
        .and_then(Value::as_bool);
    let reported = payload.get("rate_limits").is_some_and(Value::is_object);
    let unavailable_reason = match available {
        Some(false) => Some("Plan limits are unavailable for this login. API keys and third-party providers do not report subscription limits; OAuth access also requires profile permission.".to_string()),
        Some(true) if !reported => Some("Claude did not report plan limits this time. Refresh to ask again.".to_string()),
        _ => None,
    };
    let plan = payload
        .get("subscription_type")
        .and_then(Value::as_str)
        .and_then(clean);
    let windows = (payload.get("rate_limits"))
        .filter(|value| value.is_object())
        .map(claude_windows)
        .unwrap_or_default();

    Some(Usage {
        provider: "claude".to_owned(),
        plan,
        tier: None,
        windows,
        unavailable_reason,
        resets: None,
        light: false,
    })
}

/// The windows in Claude's usage answer, as its usage endpoint sends them and its CLI repeats.
pub(super) fn claude_windows(limits: &Value) -> Vec<UsageWindow> {
    if let Some(rows) = limits.get("limits").and_then(Value::as_array) {
        return rows.iter().filter_map(claude_limit_row).collect();
    }
    let mut windows = Vec::new();
    {
        let windows_named = [
            ("five_hour", None, "5h", 300),
            ("seven_day", None, "7d", 10_080),
            ("seven_day_opus", Some("Opus"), "7d", 10_080),
            ("seven_day_sonnet", Some("Sonnet"), "7d", 10_080),
            ("seven_day_oauth_apps", Some("OAuth apps"), "7d", 10_080),
        ];
        for (key, scope, duration, minutes) in windows_named {
            push_claude_usage_window(&mut windows, limits, key, (scope, duration), minutes);
        }
        if let Some(model_scoped) = limits.get("model_scoped").and_then(Value::as_array) {
            for window in model_scoped {
                let Some(display_name) = window
                    .get("display_name")
                    .and_then(Value::as_str)
                    .and_then(clean)
                else {
                    continue;
                };
                let Some(used_percent) = window.get("utilization").and_then(Value::as_f64) else {
                    continue;
                };
                windows.push(UsageWindow::new(
                    Some(&display_name),
                    "7d",
                    used_percent,
                    window.get("resets_at").and_then(usage_reset_at),
                    Some(10_080),
                ));
            }
        }
    }
    windows
}

/// One of the rows Claude's usage endpoint lists its meters in, by the kind the server gives it:
/// the session, the whole week, or one model's or surface's share of the week.
fn claude_limit_row(row: &Value) -> Option<UsageWindow> {
    let percent = row.get("percent")?.as_f64()?;
    let resets_at = row.get("resets_at").and_then(usage_reset_at);
    let scope = |of: &str| {
        row.pointer(&format!("/scope/{of}/display_name"))
            .and_then(Value::as_str)
            .and_then(clean)
    };
    match row.get("kind")?.as_str()? {
        "session" => Some(UsageWindow::new(None, "5h", percent, resets_at, Some(300))),
        "weekly_all" => Some(UsageWindow::new(
            None,
            "7d",
            percent,
            resets_at,
            Some(10_080),
        )),
        "weekly_scoped" => {
            let name = scope("model").or_else(|| scope("surface"))?;
            Some(UsageWindow::new(
                Some(&name),
                "7d",
                percent,
                resets_at,
                Some(10_080),
            ))
        }
        _ => None,
    }
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
    windows: &mut Vec<UsageWindow>,
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
    windows.push(UsageWindow::new(
        scope,
        &duration,
        used_percent,
        window.get("resetsAt").and_then(usage_reset_at),
        window_minutes,
    ));
}

fn parse_codex_usage_result(result: &Value) -> Usage {
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
            .and_then(clean)
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
            .and_then(clean);
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

    Usage {
        provider: "codex".to_owned(),
        tier: plan.as_deref().map(capitalized),
        plan,
        unavailable_reason: windows.is_empty().then(|| "This account did not report subscription limits. API-key accounts do not provide plan usage.".to_string()),
        windows,
        resets: codex_resets(result),
        light: false,
    }
}

/// The resets in a Codex limits answer; none when it sent no count.
fn codex_resets(result: &Value) -> Option<Resets> {
    let summary = result.get("rateLimitResetCredits")?;
    let left = summary.get("availableCount")?.as_u64()?;
    let use_by = (summary.get("credits").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter(|credit| credit.get("status").and_then(Value::as_str) == Some("available"))
        .filter_map(|credit| credit.get("expiresAt")?.as_u64()?.checked_mul(1000))
        .min();
    let left = u32::try_from(left).unwrap_or(u32::MAX);
    Some(Resets {
        left,
        kinds: (left > 0)
            .then(|| ResetKind {
                id: None,
                label: "Reset".to_owned(),
                left,
                use_by,
                window: None,
            })
            .into_iter()
            .collect(),
    })
}

pub(crate) async fn use_codex_reset(
    executable: &Path,
    config_path: Option<&str>,
) -> Result<ResetOutcome, String> {
    let command = cli("codex", executable, config_path, BTreeMap::new());
    let params = serde_json::json!({ "idempotencyKey": uuid::Uuid::new_v4().to_string() });
    let answer = codex_ask(
        command,
        "account/rateLimitResetCredit/consume",
        params,
        "reset",
    )
    .await?;
    match answer.get("outcome").and_then(Value::as_str) {
        Some("reset" | "alreadyRedeemed") => Ok(ResetOutcome::Reset),
        Some("nothingToReset") => Ok(ResetOutcome::NothingToReset),
        Some("noCredit") => Ok(ResetOutcome::NoReset),
        _ => Err("Codex did not say whether the reset went through".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_claude_usage, parse_codex_usage_result, run, UsageResetAt};
    use std::path::Path;

    const ERROR_DETAIL_LIMIT: usize = 240;

    #[tokio::test]
    async fn a_lookup_drops_the_removed_environment() {
        const READ_HOME: &str = "printf '%s' \"${HOME-unset}\"";
        let sh = Path::new("/bin/sh");
        let kept = run("test-agent", sh, &["-c", READ_HOME], "", None, &[])
            .await
            .unwrap();
        let dropped = run("test-agent", sh, &["-c", READ_HOME], "", None, &["HOME"])
            .await
            .unwrap();
        assert_ne!(kept, "unset");
        assert_eq!(dropped, "unset");
    }

    #[tokio::test]
    async fn a_lookup_ends_its_input_so_a_cli_reading_to_the_end_answers() {
        let started = std::time::Instant::now();
        let answer = run(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", "cat >/dev/null; printf done"],
            "request\n",
            None,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(answer, "done");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_failed_lookup_says_why_in_bounded_words() {
        let long_detail = "x".repeat(ERROR_DETAIL_LIMIT * 2);
        let script = format!("printf 'first\\nsecond {long_detail}' >&2; exit 7");
        let error = run(
            "test-agent",
            Path::new("/bin/sh"),
            &["-c", &script],
            "",
            None,
            &[],
        )
        .await
        .unwrap_err();
        assert!(error.contains("first second "), "{error}");
        assert!(error.ends_with('…'), "{error}");
        assert!(error.chars().count() < long_detail.len(), "{error}");
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
            Some(UsageResetAt::Iso("2026-08-13T12:00:00Z".to_string()))
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
    fn claude_usage_says_so_when_limits_apply_but_none_came_back() {
        let usage = parse_claude_usage(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"usage","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":null}}}"#,
            "usage",
        ).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert!(usage.windows.is_empty());
        assert!(usage
            .unavailable_reason
            .unwrap()
            .contains("did not report plan limits this time"));
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
            Some(UsageResetAt::Unix(1787157619))
        );
    }
}
