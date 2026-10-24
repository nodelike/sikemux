//! Updating the app this core runs inside, on the person's channel. The core
//! checks the release feeds, downloads the newest build while checking its
//! signature, and puts it in place of the app. Once no chat would lose work it
//! says the update is ready, the windows start again into it, and the new
//! build takes over this core with its agents still running.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::StreamExt;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::changed::Topic;
use super::connection::blocking;
use super::{Core, CoreError, CoreResult};
use crate::frozen::UpdateBlocker;
use crate::protocol::UpdateChannel;
use crate::releases::{bundled_credits, ReleaseCredits};

const STABLE_FEED: &str =
    "https://github.com/nodelike/sikemux/releases/latest/download/latest.json";
const NIGHTLY_FEED: &str =
    "https://github.com/nodelike/sikemux/releases/download/nightly/latest.json";
const FIRST_CHECK: Duration = Duration::from_secs(4);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const BLOCKERS_EVERY: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// The release scripts check updates against the key in the app's config, so
/// the core reads the same one.
const APP_CONFIG: &str = include_str!("../../../../tauri.conf.json");

/// Where an update stands, as every client shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    #[default]
    Idle,
    /// Downloaded and in place, waiting for the chats in `blockers` to finish
    /// what they are doing.
    Waiting,
    Downloading,
    Installing,
    /// In place; the windows start again into it.
    Ready,
    Failed,
}

/// The newest release on the person's channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub credits: Option<ReleaseCredits>,
}

/// The last check, kept because a failed one has nowhere else to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// Unix milliseconds.
    pub at: u64,
    pub channel: UpdateChannel,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// This build updates itself: a release running inside its app.
    pub updates: bool,
    pub current: String,
    pub checked: Option<UpdateCheck>,
    pub available: Option<UpdateRelease>,
    pub state: UpdateState,
    pub error: Option<String>,
    pub blockers: Vec<UpdateBlocker>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    /// The person chose to stop the chats rather than wait, which the window
    /// that starts again tells the next core.
    pub forced: bool,
}

/// Where to fetch the release `available` names.
#[derive(Clone)]
struct Found {
    url: String,
    signature: String,
}

#[derive(Default)]
struct Held {
    status: UpdateStatus,
    found: Option<Found>,
    /// The download is in place of the app already.
    installed: bool,
}

#[derive(Default)]
pub(crate) struct Updates {
    held: std::sync::Mutex<Held>,
    /// Rings for a check out of turn, such as when the person changes channel.
    wake: Notify,
}

#[derive(Deserialize)]
struct Feed {
    version: String,
    notes: Option<String>,
    pub_date: Option<String>,
    platforms: std::collections::HashMap<String, Asset>,
}

#[derive(Deserialize)]
struct Asset {
    signature: String,
    url: String,
}

/// The app bundle this core was started from, when it is a published release
/// that can replace itself. A build made anywhere else, such as a local try-out,
/// never updates, so a release can never take its place.
fn own_bundle() -> Option<PathBuf> {
    if !crate::usage::Install::App.published() || !cfg!(target_os = "macos") {
        return None;
    }
    let core = std::env::current_exe().ok()?;
    sikemux_process::paths::app_bundle_of_core(&core).map(Path::to_path_buf)
}

fn platform() -> String {
    format!("darwin-{}", std::env::consts::ARCH)
}

fn feeds(channel: UpdateChannel) -> &'static [&'static str] {
    match channel {
        UpdateChannel::Stable => &[STABLE_FEED],
        // A stable release can overtake the newest nightly, so nightly follows both.
        UpdateChannel::Nightly => &[NIGHTLY_FEED, STABLE_FEED],
    }
}

/// The key every release is signed with.
fn release_key() -> Result<PublicKey, String> {
    let config: Value = serde_json::from_str(APP_CONFIG).map_err(|error| error.to_string())?;
    let encoded = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .ok_or("the app's config has no update key")?;
    let text = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;
    PublicKey::decode(&String::from_utf8_lossy(&text)).map_err(|error| error.to_string())
}

impl Updates {
    pub(crate) fn status(&self) -> UpdateStatus {
        self.held.locked().status.clone()
    }

    fn edit(&self, core: &Core, change: impl FnOnce(&mut Held)) {
        change(&mut self.held.locked());
        core.changes.changed(Topic::Update, None, None);
    }

    /// Checks again now, as after the person changes channel.
    pub(crate) fn check_soon(&self) {
        self.wake.notify_one();
    }
}

/// Checks a little after the core starts and every half hour after, while the
/// core runs in an app that can update.
pub(crate) async fn serve(core: Arc<Core>) {
    core.updates.edit(&core, |held| {
        held.status.updates = own_bundle().is_some();
        held.status.current = core.build.version.clone();
    });
    if !core.updates.status().updates {
        return;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        let _ = check(&core).await;
        let _ = tokio::time::timeout(CHECK_EVERY, core.updates.wake.notified()).await;
    }
}

/// A feed's release, and the feed as it was, which carries the release's credits.
async fn read_feed(url: &'static str) -> Result<(Version, Feed, Value), String> {
    let client = crate::http::client().map_err(|error| error.to_string())?;
    let raw: Value = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(CHECK_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let feed: Feed = serde_json::from_value(raw.clone()).map_err(|error| error.to_string())?;
    let version = Version::parse(feed.version.trim_start_matches('v'))
        .map_err(|error| format!("the feed's version: {error}"))?;
    Ok((version, feed, raw))
}

/// Looks for a release newer than this one on the person's channel. One feed
/// that does not answer must not hide a release the other one offers.
pub(crate) async fn check(core: &Core) -> CoreResult<UpdateStatus> {
    let status = core.updates.status();
    if !status.updates {
        return Err("this build does not update itself".into());
    }
    if matches!(
        status.state,
        UpdateState::Downloading
            | UpdateState::Installing
            | UpdateState::Waiting
            | UpdateState::Ready
    ) {
        return Ok(status);
    }
    let channel = core.preferences.get().update_channel;
    let current =
        Version::parse(&status.current).map_err(|error| CoreError::from(error.to_string()))?;
    let reads: Vec<_> = feeds(channel)
        .iter()
        .map(|url| tokio::spawn(read_feed(url)))
        .collect();
    let mut newest: Option<(Version, Feed, Value)> = None;
    let mut failure = None;
    for read in reads {
        match read
            .await
            .map_err(|error| error.to_string())
            .and_then(|read| read)
        {
            Ok(read) if newest.as_ref().is_none_or(|(best, _, _)| read.0 > *best) => {
                newest = Some(read);
            }
            Ok(_) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    let error = failure.filter(|_| newest.is_none());
    let release = newest
        .filter(|(version, _, _)| *version > current)
        .and_then(|(_, feed, raw)| {
            let asset = feed.platforms.get(&platform())?;
            let found = Found {
                url: asset.url.clone(),
                signature: asset.signature.clone(),
            };
            let credits = bundled_credits(&raw);
            Some((
                UpdateRelease {
                    version: feed.version,
                    notes: feed.notes,
                    date: feed.pub_date,
                    credits,
                },
                found,
            ))
        });
    core.updates.edit(core, |held| {
        held.status.checked = Some(UpdateCheck {
            at: epoch_ms(),
            channel,
            error: error.clone(),
        });
        if error.is_some() {
            return;
        }
        held.status.available = release.as_ref().map(|(release, _)| release.clone());
        held.found = release.map(|(_, found)| found);
        held.status.error = None;
        held.status.state = UpdateState::Idle;
    });
    match error {
        Some(error) => Err(CoreError::from(format!("update check: {error}"))),
        None => Ok(core.updates.status()),
    }
}

#[derive(Deserialize)]
pub(crate) struct InstallParams {
    /// Stop the chats an update would wait on rather than wait.
    #[serde(default)]
    force: bool,
}

/// Downloads and puts in place the release `check` found, in the background;
/// clients follow it in the status.
pub(crate) fn install(core: &Arc<Core>, params: InstallParams) -> CoreResult<UpdateStatus> {
    let status = core.updates.status();
    if matches!(
        status.state,
        UpdateState::Downloading | UpdateState::Installing | UpdateState::Ready
    ) {
        return Ok(status);
    }
    let bundle =
        own_bundle().ok_or_else(|| CoreError::from("this build does not update itself"))?;
    let (found, installed) = {
        let held = core.updates.held.locked();
        (held.found.clone(), held.installed)
    };
    let found = found.ok_or_else(|| CoreError::from("there is no update to install"))?;
    core.updates.edit(core, |held| {
        held.status.forced = params.force;
        held.status.error = None;
        held.status.downloaded_bytes = 0;
        held.status.total_bytes = None;
        held.status.state = if installed {
            UpdateState::Waiting
        } else {
            UpdateState::Downloading
        };
    });
    let status = core.updates.status();
    let core = core.clone();
    tokio::spawn(async move {
        if let Err(error) = put_in_place(&core, &bundle, &found, installed).await {
            core.updates.edit(&core, |held| {
                held.status.state = UpdateState::Failed;
                held.status.error = Some(error);
            });
            return;
        }
        wait_for_chats(&core).await;
    });
    Ok(status)
}

/// Stops waiting on chats; the update stays in place for later.
pub(crate) fn cancel(core: &Core) -> UpdateStatus {
    core.updates.edit(core, |held| {
        if held.status.state == UpdateState::Waiting {
            held.status.state = UpdateState::Idle;
            held.status.blockers.clear();
        }
    });
    core.updates.status()
}

async fn put_in_place(
    core: &Core,
    bundle: &Path,
    found: &Found,
    installed: bool,
) -> Result<(), String> {
    if installed {
        return Ok(());
    }
    let key = release_key()?;
    let archive =
        std::env::temp_dir().join(format!("sikemux-update-{}.tar.gz", std::process::id()));
    let mut last = Instant::now();
    let fetched = fetch_verified(
        &found.url,
        &found.signature,
        &key,
        &archive,
        |done, total| {
            if last.elapsed() >= PROGRESS_EVERY || total == Some(done) {
                last = Instant::now();
                core.updates.edit(core, |held| {
                    held.status.downloaded_bytes = done;
                    held.status.total_bytes = total;
                });
            }
        },
    )
    .await;
    if let Err(error) = fetched {
        let _ = std::fs::remove_file(&archive);
        return Err(error);
    }
    core.updates
        .edit(core, |held| held.status.state = UpdateState::Installing);
    let (bundle, unpack) = (bundle.to_path_buf(), archive.clone());
    let swapped = blocking(move || swap(&bundle, &unpack).map_err(CoreError::from)).await;
    let _ = std::fs::remove_file(&archive);
    swapped.map_err(|error| error.to_string())?;
    core.updates.edit(core, |held| held.installed = true);
    Ok(())
}

/// Says the update is ready once no chat would lose work to the restart, or at
/// once when the person chose to stop them.
async fn wait_for_chats(core: &Core) {
    loop {
        let status = core.updates.status();
        if !matches!(status.state, UpdateState::Installing | UpdateState::Waiting) {
            return;
        }
        let blockers: Vec<UpdateBlocker> = if status.forced {
            Vec::new()
        } else {
            core.chats
                .blockers()
                .into_iter()
                .map(|blocker| UpdateBlocker {
                    agent_id: blocker.agent_id,
                    provider: blocker.provider,
                    title: blocker.title,
                    reason: blocker.reason,
                })
                .collect()
        };
        let ready = blockers.is_empty();
        core.updates.edit(core, |held| {
            held.status.blockers = blockers;
            held.status.state = if ready {
                UpdateState::Ready
            } else {
                UpdateState::Waiting
            };
        });
        if ready {
            return;
        }
        tokio::time::sleep(BLOCKERS_EVERY).await;
    }
}

/// Downloads `url` to `to`, checking it against the release's `signature` as
/// it comes, and reports each step as bytes done of the whole.
pub(crate) async fn fetch_verified(
    url: &str,
    signature: &str,
    key: &PublicKey,
    to: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let text = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|error| format!("the release's signature: {error}"))?;
    let signature = Signature::decode(&String::from_utf8_lossy(&text))
        .map_err(|error| format!("the release's signature: {error}"))?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|_| "the release is signed with another key".to_owned())?;
    let client = crate::http::client().map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("download: {error}"))?;
    let total = response.content_length();
    let mut file = std::fs::File::create(to).map_err(|error| error.to_string())?;
    let mut done = 0u64;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| format!("download: {error}"))?;
        verifier.update(&chunk);
        file.write_all(&chunk).map_err(|error| error.to_string())?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().map_err(|error| error.to_string())?;
    verifier
        .finalize()
        .map_err(|_| "the download does not match the release's signature".to_owned())
}

/// Puts the app in `archive` in place of `bundle`, keeping the old one until
/// the new one is there, so a failure leaves the app as it was.
pub(crate) fn swap(bundle: &Path, archive: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("the app is not in a folder")?;
    let name = bundle
        .file_name()
        .ok_or("the app has no name")?
        .to_string_lossy()
        .into_owned();
    let cannot =
        |error: std::io::Error| format!("Sikemux cannot write to {}: {error}", parent.display());
    let staging = parent.join(format!(".{name}.update"));
    let old = parent.join(format!(".{name}.old"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir(&staging).map_err(cannot)?;
    let unpacked = sikemux_process::user_environment::command("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|error| error.to_string())?;
    let new = std::fs::read_dir(&staging)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "app"));
    let Some(new) = new.filter(|_| unpacked.success()) else {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("the download holds no app".into());
    };
    std::fs::rename(bundle, &old).map_err(cannot)?;
    if let Err(error) = std::fs::rename(&new, bundle) {
        let _ = std::fs::rename(&old, bundle);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(cannot(error));
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}
