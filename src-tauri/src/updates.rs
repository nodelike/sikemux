use std::time::{Duration, Instant};

use futures::future::join_all;
use semver::Version;
use tauri::{ipc::Channel, AppHandle};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, Metadata, ScalarValue, SpanContext, SpanOutcome};
use crate::release_credits::{bundled_credits, ReleaseCredits};

const STABLE_ENDPOINT: &str =
    "https://github.com/nodelike/sikemux/releases/latest/download/latest.json";
const NIGHTLY_ENDPOINT: &str =
    "https://github.com/nodelike/sikemux/releases/download/nightly/latest.json";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
// GitHub serves release assets from four addresses, and a network can blackhole
// one of them: the SYN goes out and nothing comes back. Without a per-address
// deadline the client waits on that address until the whole request expires
// instead of failing over to the next one, so every check times out and reads
// as "no update available". curl hides this by rotating addresses quickly.
const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROGRESS_EVENT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
    credits: Option<ReleaseCredits>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum UpdateInstallPhase {
    Downloading,
    Installing,
    Installed,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallProgress {
    phase: UpdateInstallPhase,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

#[derive(Debug, Default)]
struct DownloadProgressReporter {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    last_emitted_at: Option<Instant>,
}

impl DownloadProgressReporter {
    fn snapshot(&self, phase: UpdateInstallPhase) -> UpdateInstallProgress {
        UpdateInstallProgress {
            phase,
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
        }
    }

    fn observe(
        &mut self,
        chunk_length: usize,
        content_length: Option<u64>,
        now: Instant,
    ) -> Option<UpdateInstallProgress> {
        let chunk_length = u64::try_from(chunk_length).unwrap_or(u64::MAX);
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(chunk_length);
        if content_length.is_some() {
            self.total_bytes = content_length;
        }

        let complete = self
            .total_bytes
            .is_some_and(|total| self.downloaded_bytes >= total);
        let due = self
            .last_emitted_at
            .is_none_or(|last| now.saturating_duration_since(last) >= PROGRESS_EVENT_INTERVAL);
        if !complete && !due {
            return None;
        }

        self.last_emitted_at = Some(now);
        Some(self.snapshot(UpdateInstallPhase::Downloading))
    }
}

fn channel_feeds(channel: &str) -> AppResult<&'static [&'static str]> {
    match channel {
        "stable" => Ok(&[STABLE_ENDPOINT]),
        // A stable release can overtake the newest nightly, so nightly follows both.
        "nightly" => Ok(&[NIGHTLY_ENDPOINT, STABLE_ENDPOINT]),
        _ => Err(AppError::BadArg("update channel must be stable or nightly")),
    }
}

async fn newest_update(
    app: &AppHandle,
    channel: &str,
    timeout: Duration,
) -> AppResult<Option<Update>> {
    if cfg!(debug_assertions) {
        return Err(AppError::Other(
            "development builds do not update themselves".into(),
        ));
    }
    let checks = channel_feeds(channel)?.iter().map(|endpoint| async move {
        feed_updater(app, endpoint, timeout)?
            .check()
            .await
            .map_err(|error| AppError::Other(format!("update check: {error}")))
    });
    pick_newest(join_all(checks).await, |update| &update.version)
}

// One unreachable feed must not hide a release the other feed offers.
fn pick_newest<T>(
    results: Vec<AppResult<Option<T>>>,
    version_of: impl Fn(&T) -> &str,
) -> AppResult<Option<T>> {
    let mut newest: Option<(Version, T)> = None;
    let mut failure = None;
    for result in results {
        match result {
            Ok(Some(update)) => {
                let version = Version::parse(version_of(&update))
                    .map_err(|error| AppError::Other(format!("update version: {error}")))?;
                if newest.as_ref().is_none_or(|(best, _)| version > *best) {
                    newest = Some((version, update));
                }
            }
            Ok(None) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    match (newest, failure) {
        (Some((_, update)), _) => Ok(Some(update)),
        (None, Some(error)) => Err(error),
        (None, None) => Ok(None),
    }
}

fn feed_updater(app: &AppHandle, endpoint: &str, timeout: Duration) -> AppResult<Updater> {
    let url = endpoint
        .parse()
        .map_err(|error| AppError::Other(format!("update endpoint: {error}")))?;
    app.updater_builder()
        .endpoints(vec![url])
        .map(|builder| {
            builder.configure_client(|client| client.connect_timeout(UPDATE_CONNECT_TIMEOUT))
        })
        .and_then(|builder| builder.timeout(timeout).build())
        .map_err(|error| AppError::Other(format!("updater: {error}")))
}

#[tauri::command]
pub async fn update_check(app: AppHandle, channel: String) -> AppResult<Option<UpdateInfo>> {
    Ok(newest_update(&app, &channel, UPDATE_CHECK_TIMEOUT)
        .await?
        .map(|update| UpdateInfo {
            version: update.version,
            current_version: update.current_version,
            date: update.date.map(|date| date.to_string()),
            credits: bundled_credits(&update.raw_json),
            notes: update.body,
        }))
}

#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    channel: String,
    on_progress: Channel<UpdateInstallProgress>,
) -> AppResult<UpdateInfo> {
    let observer = global_observability();
    let mut metadata = Metadata::new();
    metadata.insert("channel".to_owned(), ScalarValue::from(channel.as_str()));
    let span = observer.begin_span("update.install", None, metadata);
    let context = span.context();

    let result = update_install_inner(&app, &channel, on_progress, context).await;
    let outcome = if result.is_ok() {
        let _ = observer.increment_counter("update.install.success", 1);
        SpanOutcome::Success
    } else {
        let _ = observer.increment_counter("update.install.errors", 1);
        SpanOutcome::Error
    };
    span.finish(outcome);
    result
}

async fn update_install_inner(
    app: &AppHandle,
    channel: &str,
    on_progress: Channel<UpdateInstallProgress>,
    context: SpanContext,
) -> AppResult<UpdateInfo> {
    let update = newest_update(app, channel, UPDATE_INSTALL_TIMEOUT)
        .await?
        .ok_or_else(|| AppError::Other("no update is available".into()))?;
    let installed = UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
        credits: bundled_credits(&update.raw_json),
    };

    let observer = global_observability();
    let mut download_metadata = Metadata::new();
    download_metadata.insert(
        "version".to_owned(),
        ScalarValue::from(installed.version.as_str()),
    );
    observer.record_event("update.download.started", Some(context), download_metadata);

    let mut progress = DownloadProgressReporter::default();
    let bytes = update
        .download(
            |chunk_length, content_length| {
                if let Some(event) = progress.observe(chunk_length, content_length, Instant::now())
                {
                    let _ = on_progress.send(event);
                }
            },
            || {},
        )
        .await
        .map_err(|error| AppError::Other(format!("update install: {error}")))?;

    let mut downloaded_metadata = Metadata::new();
    downloaded_metadata.insert(
        "downloaded_bytes".to_owned(),
        ScalarValue::from(progress.downloaded_bytes),
    );
    if let Some(total_bytes) = progress.total_bytes {
        downloaded_metadata.insert("total_bytes".to_owned(), ScalarValue::from(total_bytes));
    }
    observer.record_event(
        "update.download.finished",
        Some(context),
        downloaded_metadata,
    );
    let _ = on_progress.send(progress.snapshot(UpdateInstallPhase::Downloading));
    let _ = on_progress.send(progress.snapshot(UpdateInstallPhase::Installing));
    observer.record_event("update.install.started", Some(context), Metadata::new());
    update
        .install(&bytes)
        .map_err(|error| AppError::Other(format!("update install: {error}")))?;
    observer.record_event("update.install.finished", Some(context), Metadata::new());
    let _ = on_progress.send(progress.snapshot(UpdateInstallPhase::Installed));
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn download_progress_reports_absolute_bounded_updates_and_completion() {
        let start = Instant::now();
        let mut reporter = DownloadProgressReporter::default();

        let first = reporter.observe(32, Some(100), start).unwrap();
        assert_eq!(first.downloaded_bytes, 32);
        assert_eq!(first.total_bytes, Some(100));
        assert!(reporter
            .observe(16, Some(100), start + Duration::from_millis(100))
            .is_none());

        let timed = reporter
            .observe(16, Some(100), start + PROGRESS_EVENT_INTERVAL)
            .unwrap();
        assert_eq!(timed.downloaded_bytes, 64);

        let complete = reporter
            .observe(36, Some(100), start + PROGRESS_EVENT_INTERVAL)
            .unwrap();
        assert_eq!(complete.downloaded_bytes, 100);
        assert_eq!(complete.phase, UpdateInstallPhase::Downloading);
    }

    #[test]
    fn progress_payload_uses_frontend_camel_case_contract() {
        let value = serde_json::to_value(UpdateInstallProgress {
            phase: UpdateInstallPhase::Installing,
            downloaded_bytes: 13_362_333,
            total_bytes: Some(13_362_333),
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "phase": "installing",
                "downloadedBytes": 13_362_333,
                "totalBytes": 13_362_333
            })
        );
    }

    // Network diagnostic, not part of the normal suite: run with
    // `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored updater_feed`.
    // Reproduces the updater plugin's own request (user agent, Accept header,
    // timeout) through this crate's reqwest/TLS features, then applies the same
    // target lookup and semver comparison the plugin uses, so a feed or
    // transport fault is visible without waiting on the in-app check.
    // Network diagnostic, excluded from the normal suite. Run with
    // `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored update_feed`.
    // Reproduces the plugin's own request — user agent, Accept header, and the
    // connect timeout that lets a blackholed release-asset address fail over —
    // so a broken feed or transport is diagnosable without waiting on the
    // in-app check, whose failures are deliberately quiet.
    #[test]
    #[ignore = "requires network access"]
    fn stable_update_feed_serves_this_platform() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let body: serde_json::Value = runtime.block_on(async {
            reqwest::Client::builder()
                .user_agent("tauri-plugin-updater/2.10.1")
                .timeout(UPDATE_CHECK_TIMEOUT)
                .connect_timeout(UPDATE_CONNECT_TIMEOUT)
                .build()
                .expect("build updater http client")
                .get(STABLE_ENDPOINT)
                .header("Accept", "application/json")
                .send()
                .await
                .expect("reach the stable update feed")
                .json()
                .await
                .expect("decode the stable update feed")
        });

        let target = format!("darwin-{}", std::env::consts::ARCH);
        let platform = body
            .get("platforms")
            .and_then(|platforms| platforms.get(&target))
            .unwrap_or_else(|| panic!("feed has no {target} entry: {body}"));
        assert!(
            platform.get("url").and_then(|url| url.as_str()).is_some(),
            "{target} entry has no url"
        );
        assert!(
            platform
                .get("signature")
                .and_then(|signature| signature.as_str())
                .is_some(),
            "{target} entry has no signature"
        );
        let version = body
            .get("version")
            .and_then(|version| version.as_str())
            .expect("feed version");
        assert!(
            version.split('.').count() == 3
                && version
                    .split('.')
                    .all(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit())),
            "feed version {version} is not a semver the updater can compare"
        );
    }

    fn pick(results: Vec<AppResult<Option<&'static str>>>) -> AppResult<Option<&'static str>> {
        pick_newest(results, |version| version)
    }

    #[test]
    fn nightly_follows_both_feeds() {
        assert_eq!(channel_feeds("stable").unwrap(), &[STABLE_ENDPOINT]);
        assert_eq!(
            channel_feeds("nightly").unwrap(),
            &[NIGHTLY_ENDPOINT, STABLE_ENDPOINT]
        );
        assert!(channel_feeds("beta").is_err());
    }

    #[test]
    fn a_stable_release_overtakes_its_own_nightlies() {
        let newest = pick(vec![Ok(Some("0.4.0-nightly.11")), Ok(Some("0.4.0"))]).unwrap();
        assert_eq!(newest, Some("0.4.0"));
    }

    #[test]
    fn a_nightly_ahead_of_stable_wins() {
        let newest = pick(vec![Ok(Some("0.5.0-nightly.1")), Ok(Some("0.4.1"))]).unwrap();
        assert_eq!(newest, Some("0.5.0-nightly.1"));
    }

    #[test]
    fn one_feed_offering_nothing_defers_to_the_other() {
        assert_eq!(
            pick(vec![Ok(None), Ok(Some("0.4.0"))]).unwrap(),
            Some("0.4.0")
        );
        assert_eq!(pick(vec![Ok(None), Ok(None)]).unwrap(), None);
    }

    #[test]
    fn a_failed_feed_only_surfaces_when_nothing_was_found() {
        let found = pick(vec![
            Err(AppError::Other("offline".into())),
            Ok(Some("0.4.0")),
        ]);
        assert_eq!(found.unwrap(), Some("0.4.0"));
        assert!(pick(vec![Ok(None), Err(AppError::Other("offline".into()))]).is_err());
    }

    #[test]
    fn progress_byte_counter_saturates() {
        let start = Instant::now();
        let mut reporter = DownloadProgressReporter {
            downloaded_bytes: u64::MAX - 1,
            ..DownloadProgressReporter::default()
        };

        let progress = reporter.observe(10, None, start).unwrap();
        assert_eq!(progress.downloaded_bytes, u64::MAX);
    }
}
