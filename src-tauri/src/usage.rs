use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};

const CAPTURE_ENDPOINT: &str = "https://eu.i.posthog.com/i/v0/e/";
const PROJECT_TOKEN: &str = "phc_yGaffnPw38wHvQKgusATcKtYmkyDCbNNGpTo3sK9pQoD";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const SECONDS_PER_DAY: u64 = 86_400;
const REPORTING_BUILD: bool =
    !cfg!(debug_assertions) && option_env!("SIKEMUX_USAGE_REPORTING").is_some();

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
    arch: &'static str,
}

/// Sends one anonymous `app_active` event per install per UTC day. Only builds
/// published by the release workflow send anything; local builds stay silent.
#[tauri::command]
pub async fn usage_report_active(app: AppHandle, channel: String) -> AppResult<()> {
    if !REPORTING_BUILD {
        return Ok(());
    }
    if channel != "stable" && channel != "nightly" {
        return Err(AppError::BadArg("update channel must be stable or nightly"));
    }
    static IN_FLIGHT: Mutex<()> = Mutex::const_new(());
    let _one_at_a_time = IN_FLIGHT.lock().await;

    let path = record_path(&app)?;
    let mut record = load_record(&path)?;
    let today = utc_day(SystemTime::now());
    if record.last_reported_day == Some(today) {
        return Ok(());
    }

    let event = active_event(
        &record.install_id,
        &app.package_info().version.to_string(),
        &channel,
        &platform(),
    );
    client()
        .post(CAPTURE_ENDPOINT)
        .json(&event)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| AppError::Http(format!("usage report: {error}")))?;

    record.last_reported_day = Some(today);
    save_record(&path, &record)
}

fn record_path(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("usage data directory unavailable: {error}")))?
        .join("usage.json"))
}

fn load_record(path: &Path) -> AppResult<UsageRecord> {
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

fn save_record(path: &Path, record: &UsageRecord) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec(record)?)?;
    Ok(())
}

fn utc_day(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() / SECONDS_PER_DAY)
        .unwrap_or(0)
}

fn active_event(install_id: &str, version: &str, channel: &str, platform: &Platform) -> Value {
    json!({
        "api_key": PROJECT_TOKEN,
        "event": "app_active",
        "distinct_id": install_id,
        "properties": {
            "$process_person_profile": false,
            "$lib": "sikemux",
            "source": "app",
            "version": version,
            "update_channel": channel,
            "os": platform.os,
            "os_version": platform.os_version,
            "arch": platform.arch,
        },
    })
}

fn platform() -> Platform {
    Platform {
        os: std::env::consts::OS,
        os_version: os_version(),
        arch: std::env::consts::ARCH,
    }
}

#[cfg(target_os = "macos")]
fn os_version() -> Option<String> {
    let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    Some(format!(
        "{}.{}.{}",
        version.majorVersion, version.minorVersion, version.patchVersion
    ))
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
    fn the_event_carries_only_the_install_id_and_build_details() {
        let platform = Platform {
            os: "macos",
            os_version: Some("15.4.1".into()),
            arch: "aarch64",
        };
        let event = active_event("install-1", "0.4.2", "nightly", &platform);

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
                "os",
                "os_version",
                "source",
                "update_channel",
                "version"
            ]
        );
        assert_eq!(properties["$process_person_profile"], false);
    }
}
