//! Updating the app this core runs inside, on the person's channel. The core
//! checks the release feeds, downloads the newest build while checking its
//! signature, and puts it in place of the app. Once no chat would lose work it
//! says the update is ready, the windows start again into it, and the new
//! build takes over this core with its agents still running. A Linux host's
//! core has no app around it: root puts the new core in place, once no chat
//! is in a turn, and the core hands itself over to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use semver::Version;
use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::changed::Topic;
use super::connection::blocking;
use super::{upgrade, Core, CoreError, CoreResult};
use crate::frozen::UpdateBlocker;
use crate::host_update::{self, HostUpdate, UpdateStep, INSTALLED_CORE};
use crate::protocol::{BuildChannel, UpdateChannel};
use crate::release_feed::{self, Newest};
use crate::releases::{bundled_credits, ReleaseCredits};

const FIRST_CHECK: Duration = Duration::from_secs(4);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
const BLOCKERS_EVERY: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How long a host's core waits after `sikemux update` hears what root put in
/// place, so the answer reaches it before the core hands itself over.
const ANSWER_GRACE: Duration = Duration::from_secs(1);
const UPDATES_OFF: &str =
    "updates are off for this host; turn them on in Sikemux, or with `sudo sikemux update --on`";

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
    /// What to run in a terminal to install `available`, for an app that its
    /// installer updates rather than itself.
    pub install_command: Option<String>,
    /// Whether this host puts updates in place by itself, which only the
    /// person turns off; none on anything but a host's core.
    pub automatic: Option<bool>,
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
    /// Rings when a chat may have come to rest, for a host waiting for a quiet
    /// moment to update.
    quiet: Notify,
}

/// The app an update replaces.
#[derive(Clone)]
enum OwnApp {
    /// The macOS app bundle this core was started from.
    Bundle(PathBuf),
    /// The AppImage file the Linux app runs from.
    AppImage(PathBuf),
    /// The core a Linux host's installer put in place, which root replaces.
    Host,
}

/// The app this core runs inside, when it is a published release that can
/// replace itself. A build made anywhere else, such as a local build, never
/// updates, so a release can never take its place.
fn own_app() -> Option<OwnApp> {
    if crate::usage::Install::Host.published() {
        return installed_host().then_some(OwnApp::Host);
    }
    if !crate::usage::Install::App.published() {
        return None;
    }
    if cfg!(target_os = "macos") {
        let core = std::env::current_exe().ok()?;
        return sikemux_process::paths::app_bundle_of_core(&core)
            .map(|bundle| OwnApp::Bundle(bundle.to_path_buf()));
    }
    // The AppImage runtime names the file it runs from, and the app's core inherits it.
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|file| cfg!(target_os = "linux") && file.is_file())
        .map(OwnApp::AppImage)
}

/// Whether this core runs from where the installer puts a host's core,
/// settled as it starts, before an update replaces that file.
fn installed_host() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        cfg!(target_os = "linux")
            && std::env::current_exe().is_ok_and(|exe| exe == Path::new(INSTALLED_CORE))
    })
}

fn is_host() -> bool {
    matches!(own_app(), Some(OwnApp::Host))
}

/// The channel to update on: the person's, or nightly on a nightly build,
/// which the person installed on purpose and a stable release would not pass.
fn channel(core: &Core) -> UpdateChannel {
    match (
        core.preferences.get().update_channel,
        BuildChannel::of_release(&core.build.version),
    ) {
        (UpdateChannel::Stable, BuildChannel::Nightly) if is_host() => UpdateChannel::Nightly,
        (channel, _) => channel,
    }
}

/// What this core's release is called in a feed's platforms.
fn own_platform() -> String {
    if is_host() {
        release_feed::host_platform()
    } else {
        release_feed::platform()
    }
}

/// A published Linux app that is a deb or an rpm was put in place by the
/// installer, which updates it.
fn installer_command(channel: UpdateChannel) -> Option<String> {
    if !cfg!(target_os = "linux") || !crate::usage::Install::App.published() || own_app().is_some()
    {
        return None;
    }
    let channel = match channel {
        UpdateChannel::Stable => "",
        UpdateChannel::Nightly => "SIKEMUX_CHANNEL=nightly ",
    };
    Some(format!(
        "curl -fsSL https://sikemux.com/install | {channel}sh -s -- --app"
    ))
}

impl Updates {
    pub(crate) fn status(&self) -> UpdateStatus {
        self.held.locked().status.clone()
    }

    fn edit(&self, core: &Core, change: impl FnOnce(&mut Held)) {
        change(&mut self.held.locked());
        core.changes.changed(Topic::Update, None, None);
        if is_host() {
            core.view_changed();
        }
    }

    /// Checks again now, as after the person changes channel.
    pub(crate) fn check_soon(&self) {
        self.wake.notify_one();
    }

    /// A chat may have come to rest.
    pub(crate) fn chats_changed(&self) {
        self.quiet.notify_waiters();
    }
}

/// Checks a little after the core starts and every half hour after, while the
/// core runs in an app that can update.
pub(crate) async fn serve(core: Arc<Core>) {
    let by_installer = installer_command(core.preferences.get().update_channel);
    let automatic = match is_host() {
        true => Some(blocking(|| Ok(read_switch())).await.unwrap_or(true)),
        false => None,
    };
    core.updates.edit(&core, |held| {
        held.status.updates = own_app().is_some();
        held.status.current = core.build.version.clone();
        held.status.install_command = by_installer.clone();
        held.status.automatic = automatic;
    });
    if !core.updates.status().updates && by_installer.is_none() {
        return;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        if check(&core).await.is_ok() {
            update_host_when_quiet(&core).await;
        }
        let _ = tokio::time::timeout(CHECK_EVERY, core.updates.wake.notified()).await;
    }
}

/// Whether this person's host updates by itself. A switch that cannot be read
/// counts as on, as it is until the person turns it off.
fn read_switch() -> bool {
    sikemux_keychain::read(host_update::SWITCH_SERVICE, host_update::SWITCH_ACCOUNT)
        .map_or(true, |switch| {
            switch.as_deref() != Some(host_update::SWITCH_OFF)
        })
}

/// A host puts the release `check` found in place by itself, once no chat is
/// in a turn, or at once when the account no longer works with this build,
/// since it then reaches nobody. It waits at most until the next check.
async fn update_host_when_quiet(core: &Arc<Core>) {
    let status = core.updates.status();
    if !is_host()
        || status.automatic != Some(true)
        || status.available.is_none()
        || status.state != UpdateState::Idle
    {
        return;
    }
    let until = tokio::time::Instant::now() + CHECK_EVERY;
    loop {
        let rested = core.updates.quiet.notified();
        if core.chats.in_turn() == 0 || core.remote.too_old() {
            break;
        }
        if tokio::time::timeout_at(until, rested).await.is_err() {
            return;
        }
    }
    let _ = install(core, InstallParams { force: false });
}

/// Looks for a release newer than this one on the person's channel. One feed
/// that does not answer must not hide a release the other one offers.
pub(crate) async fn check(core: &Core) -> CoreResult<UpdateStatus> {
    let status = core.updates.status();
    let channel = channel(core);
    let by_installer = installer_command(channel);
    if !status.updates && by_installer.is_none() {
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
    let current =
        Version::parse(&status.current).map_err(|error| CoreError::from(error.to_string()))?;
    let (newest, error) = match release_feed::newest(channel).await {
        Ok(newest) => (Some(newest), None),
        Err(error) => (None, Some(error)),
    };
    let release = newest.filter(|newest| newest.version > current).and_then(
        |Newest { feed, raw, .. }| {
            let found = feed.platforms.get(&own_platform()).map(|asset| Found {
                url: asset.url.clone(),
                signature: asset.signature.clone(),
            });
            if status.updates && found.is_none() {
                return None;
            }
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
        },
    );
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
        held.status.install_command = by_installer;
        held.found = release.and_then(|(_, found)| found);
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
    let app = own_app().ok_or_else(|| CoreError::from("this build does not update itself"))?;
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
        if let Err(error) = put_in_place(&core, &app, &found, installed).await {
            core.updates.edit(&core, |held| {
                held.status.state = UpdateState::Failed;
                held.status.error = Some(error);
            });
            return;
        }
        wait_for_chats(&core).await;
        if matches!(app, OwnApp::Host) && core.updates.status().state == UpdateState::Ready {
            restart_into_installed(&core).await;
        }
    });
    Ok(status)
}

/// Asks root to put the newest release in place of this host's core, and
/// answers with what it did and the version now in place, when that is not
/// the one running.
async fn ask_root(core: &Core) -> (Vec<UpdateStep>, Option<String>) {
    let mut steps = Vec::new();
    let last = host_update::ask_root(channel(core), |step| {
        if let UpdateStep::Downloaded { bytes, .. } = step {
            core.updates.edit(core, |held| {
                held.status.downloaded_bytes = *bytes;
                held.status.total_bytes = Some(*bytes);
                held.status.state = UpdateState::Installing;
            });
        }
        steps.push(step.clone());
    })
    .await;
    if last == UpdateStep::Off {
        core.updates
            .edit(core, |held| held.status.automatic = Some(false));
    }
    let in_place = match &last {
        UpdateStep::Installed { version, .. } | UpdateStep::Current { version } => {
            Some(version.clone()).filter(|version| *version != core.build.version)
        }
        _ => None,
    };
    steps.push(last);
    (steps, in_place)
}

fn refusal(steps: &[UpdateStep]) -> String {
    match steps.last() {
        Some(UpdateStep::Failed { message }) => message.clone(),
        Some(UpdateStep::Off) => UPDATES_OFF.into(),
        _ => "the update service found no newer release yet".into(),
    }
}

/// Hands this core over to the one root put in place, keeping its terminals
/// and chats. Returns only if that failed.
async fn restart_into_installed(core: &Arc<Core>) {
    let error = upgrade::replace_with(core, PathBuf::from(INSTALLED_CORE)).await;
    core.updates.edit(core, |held| {
        held.status.state = UpdateState::Failed;
        held.status.error = Some(format!("could not restart into it: {error}"));
    });
}

/// What a host's devices show of its updates; none on anything but a host.
pub(crate) fn host_view(core: &Core) -> Option<serde_json::Value> {
    is_host()
        .then(|| serde_json::to_value(core.updates.status()).ok())
        .flatten()
}

/// `sikemux update` on a host: puts the newest release in place now, without
/// waiting for a quiet moment, then hands over to it once no chat is in the
/// middle of a request.
pub(crate) async fn update_host(core: &Arc<Core>) -> CoreResult<HostUpdate> {
    if !is_host() {
        return Err("this core was not put in place by Sikemux's installer, so it does not update itself; rerun the installer".into());
    }
    let _ = check(core).await;
    if !matches!(
        core.updates.status().state,
        UpdateState::Idle | UpdateState::Failed
    ) {
        return Err("this host is already updating".into());
    }
    core.updates.edit(core, |held| {
        held.status.state = UpdateState::Installing;
        held.status.error = None;
    });
    let (steps, in_place) = ask_root(core).await;
    let failed = matches!(steps.last(), Some(UpdateStep::Failed { .. }));
    core.updates.edit(core, |held| {
        held.installed = in_place.is_some();
        held.status.state = match (&in_place, failed) {
            (Some(_), _) => UpdateState::Waiting,
            (None, true) => UpdateState::Failed,
            (None, false) => UpdateState::Idle,
        };
        held.status.error = failed.then(|| refusal(&steps));
    });
    if in_place.is_some() {
        let core = core.clone();
        tokio::spawn(async move {
            tokio::time::sleep(ANSWER_GRACE).await;
            wait_for_chats(&core).await;
            if core.updates.status().state == UpdateState::Ready {
                restart_into_installed(&core).await;
            }
        });
    }
    Ok(HostUpdate {
        steps,
        handing_over: in_place,
    })
}

#[derive(Deserialize)]
pub(crate) struct SwitchParams {
    on: bool,
}

/// Turns a host's own updates on or off, for the person this core runs as.
pub(crate) async fn set_automatic(
    core: &Arc<Core>,
    params: SwitchParams,
) -> CoreResult<UpdateStatus> {
    if !is_host() {
        return Err("only a host's core updates itself by itself".into());
    }
    let on = params.on;
    blocking(move || {
        let stored = if on {
            sikemux_keychain::delete(host_update::SWITCH_SERVICE, host_update::SWITCH_ACCOUNT)
        } else {
            sikemux_keychain::write(
                host_update::SWITCH_SERVICE,
                host_update::SWITCH_ACCOUNT,
                host_update::SWITCH_OFF,
            )
        };
        stored.map_err(|error| CoreError::from(error.to_string()))
    })
    .await?;
    core.updates
        .edit(core, |held| held.status.automatic = Some(on));
    core.updates.check_soon();
    Ok(core.updates.status())
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
    app: &OwnApp,
    found: &Found,
    installed: bool,
) -> Result<(), String> {
    if installed {
        return Ok(());
    }
    if let OwnApp::Host = app {
        let (steps, in_place) = ask_root(core).await;
        if in_place.is_none() {
            return Err(refusal(&steps));
        }
        core.updates.edit(core, |held| held.installed = true);
        return Ok(());
    }
    let key = release_feed::release_key()?;
    let archive = match app {
        OwnApp::Bundle(_) => {
            std::env::temp_dir().join(format!("sikemux-update-{}.tar.gz", std::process::id()))
        }
        // Beside the AppImage, so putting it in place is one rename on the same disk.
        OwnApp::AppImage(file) => beside(file, "update")?,
        OwnApp::Host => unreachable!("root puts a host's core in place"),
    };
    let mut last = Instant::now();
    let fetched = release_feed::fetch_verified(
        &found.url,
        &found.signature,
        &key,
        &archive,
        None,
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
    let (app, unpack) = (app.clone(), archive.clone());
    let swapped = blocking(move || {
        match &app {
            OwnApp::Bundle(bundle) => swap(bundle, &unpack),
            OwnApp::AppImage(file) => replace_appimage(file, &unpack),
            OwnApp::Host => unreachable!("root puts a host's core in place"),
        }
        .map_err(CoreError::from)
    })
    .await;
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

/// `.<name>.<suffix>` in the folder `file` is in.
fn beside(file: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = file.file_name().ok_or("the app has no name")?;
    Ok(file.with_file_name(format!(".{}.{suffix}", name.to_string_lossy())))
}

/// Puts the AppImage at `download` in place of `file`. The running app keeps
/// the copy it mounted until it starts again from the new one.
fn replace_appimage(file: &Path, download: &Path) -> Result<(), String> {
    let cannot =
        |error: std::io::Error| format!("Sikemux cannot write to {}: {error}", file.display());
    sikemux_process::os::make_executable(download).map_err(cannot)?;
    std::fs::rename(download, file).map_err(cannot)
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
