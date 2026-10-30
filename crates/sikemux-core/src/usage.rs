//! One anonymous `app_active` event a day for each install, and once a UTC day
//! is over, how much its agents did that day: tokens by agent and model,
//! turns, sessions and commits, never which projects or what was said. Only
//! builds the release workflow publishes send anything; local builds stay silent.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::protocol::{BuildChannel, Preferences, UpdateChannel};
use crate::server::activity_log::{ReportedDay, ReportedTokens};
use crate::server::Core;

const CAPTURE_ENDPOINT: &str = "https://eu.i.posthog.com/i/v0/e/";
const BATCH_ENDPOINT: &str = "https://eu.i.posthog.com/batch/";
const PROJECT_TOKEN: &str = "phc_yGaffnPw38wHvQKgusATcKtYmkyDCbNNGpTo3sK9pQoD";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const SECONDS_PER_DAY: u64 = 86_400;
const HOST_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How far back an install's first report reaches into the activity it kept before.
const FIRST_REPORT_DAYS: u64 = 400;

/// What was installed: the desktop app, or the core alone on a host with no screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    App,
    Host,
}

impl Install {
    fn name(self) -> &'static str {
        match self {
            Install::App => "app",
            Install::Host => "host",
        }
    }

    /// Built by the release workflow, which sets `SIKEMUX_USAGE_REPORTING` to
    /// the kind's name. Only such a build reports usage or updates itself.
    pub fn published(self) -> bool {
        !cfg!(debug_assertions) && option_env!("SIKEMUX_USAGE_REPORTING") == Some(self.name())
    }
}

#[derive(Clone, Copy)]
pub struct Active<'a> {
    pub install: Install,
    pub version: &'a str,
    pub channel: BuildChannel,
    /// The account this host is signed in to, so one person on several hosts
    /// counts once. Only a hash of it leaves the machine.
    pub account: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageRecord {
    install_id: String,
    last_reported_day: Option<u64>,
    /// The last whole day whose activity was reported, or passed over while
    /// the person did not share usage.
    last_activity_day: Option<u64>,
}

impl UsageRecord {
    fn fresh() -> Self {
        Self {
            install_id: uuid::Uuid::new_v4().to_string(),
            last_reported_day: None,
            last_activity_day: None,
        }
    }
}

struct Platform {
    os: &'static str,
    os_version: Option<String>,
    distro: Option<String>,
    arch: &'static str,
}

/// Reports to the record one at a time, so two reports never both send.
static IN_FLIGHT: Mutex<()> = Mutex::const_new(());

/// Sends today's event unless this install already sent it.
pub async fn report_active(data_dir: &Path, active: Active<'_>) -> Result<(), String> {
    if !active.install.published() {
        return Ok(());
    }
    let _one_at_a_time = IN_FLIGHT.lock().await;

    let path = data_dir.join("usage.json");
    let mut record = load_record(&path)?;
    let today = utc_day(SystemTime::now());
    if record.last_reported_day == Some(today) {
        return Ok(());
    }

    let event = active_event(&record.install_id, &active, &platform());
    client()
        .map_err(|error| error.to_string())?
        .post(CAPTURE_ENDPOINT)
        .json(&event)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("usage report: {error}"))?;

    record.last_reported_day = Some(today);
    save_record(&path, &record)
}

/// Sends each whole day's activity since the last one reported. `days` reads
/// the days with activity from the first up to but not including the second.
async fn report_activity(
    data_dir: &Path,
    active: &Active<'_>,
    days: impl FnOnce(u64, u64) -> Vec<ReportedDay> + Send + 'static,
) -> Result<(), String> {
    let _one_at_a_time = IN_FLIGHT.lock().await;
    let path = data_dir.join("usage.json");
    let mut record = load_record(&path)?;
    let today = utc_day(SystemTime::now());
    let first = record
        .last_activity_day
        .map_or(today.saturating_sub(FIRST_REPORT_DAYS), |day| day + 1);
    if first >= today {
        return Ok(());
    }
    let days = tokio::task::spawn_blocking(move || days(first, today))
        .await
        .map_err(|error| error.to_string())?;
    let platform = platform();
    let batch: Vec<Value> = days
        .iter()
        .flat_map(|day| activity_events(&record.install_id, active, &platform, day))
        .collect();
    if !batch.is_empty() {
        client()
            .map_err(|error| error.to_string())?
            .post(BATCH_ENDPOINT)
            .json(&json!({ "api_key": PROJECT_TOKEN, "batch": batch }))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("activity report: {error}"))?;
    }
    record.last_activity_day = Some(today - 1);
    save_record(&path, &record)
}

/// Days the person did not share stay unreported, even once they share again.
async fn pass_over_activity(data_dir: &Path) -> Result<(), String> {
    let _one_at_a_time = IN_FLIGHT.lock().await;
    let path = data_dir.join("usage.json");
    let mut record = load_record(&path)?;
    let yesterday = utc_day(SystemTime::now()).checked_sub(1);
    if record.last_activity_day >= yesterday {
        return Ok(());
    }
    record.last_activity_day = yesterday;
    save_record(&path, &record)
}

/// Counts this install active once a UTC day while the person shares usage: an
/// app's core on a day one of its windows is open, a host's core on a day it
/// runs. `DO_NOT_TRACK` in the core's environment turns it off as well.
pub(crate) async fn serve(core: Arc<Core>, data_dir: PathBuf) {
    let Some(install) = [Install::App, Install::Host]
        .into_iter()
        .find(|install| install.published())
    else {
        return;
    };
    if std::env::var_os("DO_NOT_TRACK").is_some_and(|value| !value.is_empty() && value != "0") {
        return;
    }
    let host = crate::server::host_info();
    let mut every_hour = tokio::time::interval(HOST_CHECK_INTERVAL);
    loop {
        tokio::select! {
            _ = every_hour.tick() => {}
            _ = core.usage_wake.notified() => {}
        }
        let preferences = core.preferences.get();
        if !preferences.share_usage_data {
            if let Err(error) = pass_over_activity(&data_dir).await {
                eprintln!("sikemux core: {error}");
            }
        }
        let Some(channel) =
            reported_channel(install, &preferences, core.window.is_open(), host.channel)
        else {
            continue;
        };
        let account = core.remote.owner();
        let active = Active {
            install,
            version: &host.version,
            channel,
            account: account.as_deref(),
        };
        if let Err(error) = report_active(&data_dir, active).await {
            eprintln!("sikemux core: {error}");
        }
        let log = core.activity_log.clone();
        let days = move |first: u64, end: u64| log.reported_days(first as i64, end as i64);
        if let Err(error) = report_activity(&data_dir, &active, days).await {
            eprintln!("sikemux core: {error}");
        }
    }
}

/// The channel today's use counts under, or none when nothing may be sent: the person
/// does not share usage, or no window of the app is open.
fn reported_channel(
    install: Install,
    preferences: &Preferences,
    window_open: bool,
    host_channel: BuildChannel,
) -> Option<BuildChannel> {
    if !preferences.share_usage_data || (install == Install::App && !window_open) {
        return None;
    }
    Some(match (install, preferences.update_channel) {
        (Install::Host, _) => host_channel,
        (Install::App, UpdateChannel::Stable) => BuildChannel::Stable,
        (Install::App, UpdateChannel::Nightly) => BuildChannel::Nightly,
    })
}

fn load_record(path: &Path) -> Result<UsageRecord, String> {
    if let Some(record) = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        return Ok(record);
    }
    let record = UsageRecord::fresh();
    save_record(path, &record)?;
    Ok(record)
}

fn save_record(path: &Path, record: &UsageRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(path, &bytes, false).map_err(|error| error.to_string())
}

fn utc_day(now: SystemTime) -> u64 {
    sikemux_process::clock::epoch_ms_of(now) / 1000 / SECONDS_PER_DAY
}

fn account_hash(account: &str) -> String {
    hex::encode(Sha256::digest(format!("sikemux-usage|{account}")))
}

fn active_event(install_id: &str, active: &Active<'_>, platform: &Platform) -> Value {
    json!({
        "api_key": PROJECT_TOKEN,
        "event": "app_active",
        "distinct_id": install_id,
        "properties": common_properties(active, platform),
    })
}

/// The properties every event carries, beside its own.
fn common_properties(active: &Active<'_>, platform: &Platform) -> Value {
    json!({
        "$process_person_profile": false,
        "$lib": "sikemux",
        "source": "app",
        "install": active.install.name(),
        "user": active.account.map(account_hash),
        "version": active.version,
        "update_channel": active.channel.name(),
        "os": platform.os,
        "os_version": platform.os_version,
        "distro": platform.distro,
        "arch": platform.arch,
    })
}

/// One `activity_day` event for the day, and one `tokens_day` for each agent,
/// model and speed that used tokens, all dated at the day's middle.
fn activity_events(
    install_id: &str,
    active: &Active<'_>,
    platform: &Platform,
    day: &ReportedDay,
) -> Vec<Value> {
    let midday_ms = day.day * 86_400_000 + 43_200_000;
    let timestamp = time::OffsetDateTime::from_unix_timestamp(midday_ms / 1000)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        });
    let date = timestamp.as_deref().map(|at| at[..10].to_owned());
    let event = |name: &str, own: Value| {
        let mut properties = common_properties(active, platform);
        properties["day"] = json!(date);
        if let (Some(properties), Value::Object(own)) = (properties.as_object_mut(), own) {
            properties.extend(own);
        }
        json!({
            "event": name,
            "distinct_id": install_id,
            "timestamp": timestamp,
            "uuid": event_id(install_id, name, day.day, &properties),
            "properties": properties,
        })
    };
    let total = |tokens: &ReportedTokens| {
        tokens.input + tokens.output + tokens.cache_read + tokens.cache_write
    };
    let priced: Vec<f64> = day
        .tokens
        .iter()
        .filter_map(|tokens| tokens.cost_usd)
        .collect();
    let mut events = vec![event(
        "activity_day",
        json!({
            "sessions": day.sessions,
            "resumed_sessions": day.resumed,
            "chat_sessions": day.chat_sessions,
            "terminal_sessions": day.terminal_sessions,
            "turns": day.turns,
            "agent_ms": day.agent_ms,
            "peak_agents": day.peak_agents,
            "commits": day.commits,
            "agent_commits": day.agent_commits,
            "tokens": day.tokens.iter().map(total).sum::<i64>(),
            "features": features(&day.features),
            "cost_usd": priced.iter().sum::<f64>(),
        }),
    )];
    events.extend(day.tokens.iter().map(|tokens| {
        event(
            "tokens_day",
            json!({
                "agent": tokens.agent,
                "model": (!tokens.model.is_empty()).then_some(&tokens.model),
                "speed": tokens.speed,
                "input_tokens": tokens.input,
                "output_tokens": tokens.output,
                "cache_read_tokens": tokens.cache_read,
                "cache_write_tokens": tokens.cache_write,
                "tokens": total(tokens),
                "cost_usd": tokens.cost_usd,
            }),
        )
    }));
    events
}

/// Uses by feature, named with who asked when it was not the app: `startChat`,
/// `startChat@device`, `git.commit@local`.
fn features(used: &[(String, String, i64)]) -> serde_json::Map<String, Value> {
    used.iter()
        .map(|(feature, via, count)| match via.as_str() {
            "app" => (feature.clone(), json!(count)),
            via => (format!("{feature}@{via}"), json!(count)),
        })
        .collect()
}

/// The same day's event from the same install keeps its id, so PostHog drops
/// a report sent again after its answer was lost.
fn event_id(install_id: &str, name: &str, day: i64, properties: &Value) -> String {
    let key = [
        &properties["agent"],
        &properties["model"],
        &properties["speed"],
    ]
    .map(|value| value.as_str().unwrap_or_default());
    let digest = Sha256::digest(format!("{install_id}|{name}|{day}|{}", key.join("|")));
    uuid::Builder::from_random_bytes(digest[..16].try_into().expect("16 bytes"))
        .into_uuid()
        .to_string()
}

fn platform() -> Platform {
    let (distro, release) = os_release();
    Platform {
        os: std::env::consts::OS,
        os_version: release.or_else(os_version),
        distro,
        arch: std::env::consts::ARCH,
    }
}

#[cfg(target_os = "linux")]
fn os_release() -> (Option<String>, Option<String>) {
    let text = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    (
        os_release_field(&text, "ID"),
        os_release_field(&text, "VERSION_ID"),
    )
}

#[cfg(not(target_os = "linux"))]
fn os_release() -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(target_os = "linux")]
fn os_release_field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .map(|(_, value)| value.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(target_os = "macos")]
pub(crate) fn os_version() -> Option<String> {
    let mut buffer = [0u8; 32];
    let mut length = buffer.len();
    // SAFETY: the name is nul-terminated and sysctl writes at most `length`
    // bytes into `buffer`, which has that many.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    std::ffi::CStr::from_bytes_until_nul(&buffer[..length.min(buffer.len())])
        .ok()?
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn os_version() -> Option<String> {
    None
}

fn client() -> reqwest::Result<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    crate::http::shared(&CLIENT, || crate::http::builder().timeout(REQUEST_TIMEOUT))
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    #[test]
    fn nothing_is_sent_once_the_person_opts_out() {
        let opted_out = Preferences {
            share_usage_data: false,
            ..Preferences::default()
        };
        for install in [Install::App, Install::Host] {
            assert_eq!(
                reported_channel(install, &opted_out, true, BuildChannel::Stable),
                None
            );
        }
    }

    #[test]
    fn a_day_turns_over_at_utc_midnight() {
        let midnight = UNIX_EPOCH + Duration::from_secs(20_000 * SECONDS_PER_DAY);
        assert_eq!(utc_day(midnight - Duration::from_secs(1)), 19_999);
        assert_eq!(utc_day(midnight), 20_000);
    }

    #[test]
    fn a_missing_or_corrupt_record_starts_a_new_install() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("usage.json");

        let first = load_record(&path).unwrap();
        assert_eq!(load_record(&path).unwrap().install_id, first.install_id);

        fs::write(&path, b"not json").unwrap();
        let replaced = load_record(&path).unwrap();
        assert_ne!(replaced.install_id, first.install_id);
        assert_eq!(replaced.last_reported_day, None);
    }

    #[test]
    fn the_event_carries_only_the_install_id_a_hashed_account_and_build_details() {
        let platform = Platform {
            os: "linux",
            os_version: Some("24.04".into()),
            distro: Some("ubuntu".into()),
            arch: "x86_64",
        };
        let active = Active {
            install: Install::Host,
            version: "0.4.2",
            channel: BuildChannel::Nightly,
            account: Some("user-1"),
        };
        let event = active_event("install-1", &active, &platform);

        assert_eq!(event["distinct_id"], "install-1");
        assert_eq!(event["event"], "app_active");
        let properties = event["properties"].as_object().unwrap();
        let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "$lib",
                "$process_person_profile",
                "arch",
                "distro",
                "install",
                "os",
                "os_version",
                "source",
                "update_channel",
                "user",
                "version"
            ]
        );
        assert_eq!(properties["$process_person_profile"], false);
        assert_eq!(properties["user"], account_hash("user-1"));
        assert!(!properties["user"].as_str().unwrap().contains("user-1"));
    }
}
