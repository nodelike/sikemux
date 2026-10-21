//! One anonymous `app_active` event a day for each install. Only builds the
//! release workflow publishes send anything; local builds stay silent.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::protocol::BuildChannel;

const CAPTURE_ENDPOINT: &str = "https://eu.i.posthog.com/i/v0/e/";
const PROJECT_TOKEN: &str = "phc_yGaffnPw38wHvQKgusATcKtYmkyDCbNNGpTo3sK9pQoD";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const SECONDS_PER_DAY: u64 = 86_400;
const HOST_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

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

    /// The release workflow builds each kind with `SIKEMUX_USAGE_REPORTING`
    /// set to its name.
    pub fn reports(self) -> bool {
        !cfg!(debug_assertions) && option_env!("SIKEMUX_USAGE_REPORTING") == Some(self.name())
    }
}

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
}

impl UsageRecord {
    fn fresh() -> Self {
        Self {
            install_id: uuid::Uuid::new_v4().to_string(),
            last_reported_day: None,
        }
    }
}

struct Platform {
    os: &'static str,
    os_version: Option<String>,
    distro: Option<String>,
    arch: &'static str,
}

/// Sends today's event unless this install already sent it.
pub async fn report_active(data_dir: &Path, active: Active<'_>) -> Result<(), String> {
    if !active.install.reports() {
        return Ok(());
    }
    static IN_FLIGHT: Mutex<()> = Mutex::const_new(());
    let _one_at_a_time = IN_FLIGHT.lock().await;

    let path = data_dir.join("usage.json");
    let mut record = load_record(&path)?;
    let today = utc_day(SystemTime::now());
    if record.last_reported_day == Some(today) {
        return Ok(());
    }

    let event = active_event(&record.install_id, &active, &platform());
    client()
        .post(CAPTURE_ENDPOINT)
        .json(&event)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("usage report: {error}"))?;

    record.last_reported_day = Some(today);
    save_record(&path, &record)
}

/// A host's core counts as active each day it runs. `DO_NOT_TRACK` in its
/// environment turns this off, since a host has no settings window.
pub async fn report_host(
    data_dir: PathBuf,
    version: String,
    channel: BuildChannel,
    account: impl Fn() -> Option<String>,
) {
    if std::env::var_os("DO_NOT_TRACK").is_some_and(|value| !value.is_empty() && value != "0") {
        return;
    }
    let mut every_hour = tokio::time::interval(HOST_CHECK_INTERVAL);
    loop {
        every_hour.tick().await;
        let account = account();
        let active = Active {
            install: Install::Host,
            version: &version,
            channel,
            account: account.as_deref(),
        };
        if let Err(error) = report_active(&data_dir, active).await {
            eprintln!("sikemux core: {error}");
        }
    }
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
        "properties": {
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
        },
    })
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
fn os_version() -> Option<String> {
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
fn os_version() -> Option<String> {
    None
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

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
