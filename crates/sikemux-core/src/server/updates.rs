//! Updating the app this core runs inside, on the person's channel. The core
//! checks the release feeds, downloads the newest build while checking its
//! signature, and puts it in place of the app. Once no chat would lose work it
//! says the update is ready, the windows start again into it, and the new
//! build takes over this core with its agents still running. A Linux host's
//! core has no app around it: root puts the new core in place, once no chat
//! is in a turn, and the core hands itself over to it.

use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::{WorkspaceCore, WorkspaceParts};
pub use sikemux_calls::updates::{HostUpdatesParams, UpdateInstallParams};
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
use super::{update_reports, upgrade, Core, CoreError, CoreResult};
use crate::host_update::{self, HostAction, HostRequest, HostUpdate, UpdateStep, INSTALLED_CORE};
use crate::release_feed::{self, Newest};
use crate::releases::bundled_credits;
use sikemux_view::device::Flag;
use sikemux_view::prefs::{PreferenceChange, UpdateChannel};
use sikemux_view::updates::{
    UpdateBlocker, UpdateCheck, UpdateOutcome, UpdateOutcomeState, UpdateRelease, UpdateState,
    UpdateStatus,
};
use sikemux_wire::protocol::BuildChannel;

const FIRST_CHECK: Duration = Duration::from_secs(4);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
/// Nightlies are tagged several times a day, and the feed is a small file, so their
/// channel looks far more often and a fix reaches the person within minutes of its tag.
const NIGHTLY_CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
/// Staff take each build the moment it is published, so a push reaches them in about a minute.
const STAFF_CHECK_EVERY: Duration = Duration::from_secs(60);
const BLOCKERS_EVERY: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How long a host's core waits after `sikemux update` hears what root put in
/// place, so the answer reaches it before the core hands itself over.
const ANSWER_GRACE: Duration = Duration::from_secs(1);
const UPDATES_OFF: &str =
    "updates are off for this host; turn them on in Sikemux, or with `sudo sikemux update --on`";
/// The Arch packages the Linux app is published as: Sikemux's own, stable and
/// nightly, and the one Omarchy's repository builds.
const PACMAN_PACKAGES: [&str; 3] = ["sikemux-bin", "sikemux-nightly-bin", "sikemux"];
/// Where pacman records each installed package and the files it owns.
const PACMAN_INSTALLED: &str = "/var/lib/pacman/local";
/// The apt source the .deb writes, naming the suite it follows as
/// `packages-stable` or `packages-nightly`, and the command that updates from it.
const APT_SOURCE: &str = "/etc/apt/sources.list.d/sikemux.sources";
const APT_UPGRADE: &str = "sudo apt update && sudo apt install --only-upgrade sikemux";
/// How long a new host core runs before it says it started well, and how often
/// it may stop before that until the one it replaced is put back.
const READY_AFTER: Duration = Duration::from_secs(30);
const STARTS_BEFORE_ROLL_BACK: u32 = 3;
/// How long an update may download or install, and a host's core wait for its chats to hand
/// their agents over, before it counts as failed.
const WORKING_LIMIT: Duration = Duration::from_secs(30 * 60);
const HANDING_OVER_LIMIT: Duration = Duration::from_secs(5 * 60);
const WATCH_EVERY: Duration = Duration::from_secs(30);

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

    /// When `status.state` last changed.
    since: Option<Instant>,
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
    if pacman_package().is_some() {
        return None;
    }
    if super::usage::Install::Host.published() {
        return installed_host().then_some(OwnApp::Host);
    }
    if !super::usage::Install::App.published() {
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

/// The pacman package whose files this app runs from, settled as the core
/// starts. Pacman alone replaces those files.
fn pacman_package() -> Option<&'static str> {
    static PACKAGE: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *PACKAGE.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let running: Vec<PathBuf> = std::env::current_exe()
            .ok()
            .into_iter()
            .chain(std::env::var_os("APPIMAGE").map(PathBuf::from))
            .collect();
        pacman_owner(Path::new(PACMAN_INSTALLED), &running)
    })
}

/// Which of Sikemux's packages in pacman's records owns any of `files`.
fn pacman_owner(installed: &Path, files: &[PathBuf]) -> Option<&'static str> {
    std::fs::read_dir(installed)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let folder = entry.file_name();
            // Each package's record is named `<name>-<version>-<release>`.
            let name = folder.to_str()?.rsplitn(3, '-').nth(2)?;
            let package = PACMAN_PACKAGES
                .into_iter()
                .find(|package| *package == name)?;
            let owned = std::fs::read_to_string(entry.path().join("files")).ok()?;
            owned
                .lines()
                .any(|line| files.iter().any(|file| Path::new("/").join(line) == *file))
                .then_some(package)
        })
}

/// Whether a repository pacman syncs from carries `package`, so that a
/// system upgrade updates it.
fn in_sync_repository(package: &str) -> bool {
    sikemux_process::user_environment::command("/usr/bin/pacman")
        .args(["-Si", package])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn is_host() -> bool {
    matches!(own_app(), Some(OwnApp::Host))
}

/// The channel to update on: the person's. Nightly is staff for a staff account,
/// while the accounts server leaves the staff feed on.
fn channel(core: &Core) -> UpdateChannel {
    let chosen = core.preferences().get().update_channel;
    let staff_feed = core
        .account()
        .shown()
        .is_some_and(|account| account.flags.contains(&Flag::StaffFeed));
    match chosen {
        UpdateChannel::Stable => UpdateChannel::Stable,
        UpdateChannel::Nightly | UpdateChannel::Staff if staff_feed => UpdateChannel::Staff,
        UpdateChannel::Nightly | UpdateChannel::Staff => UpdateChannel::Nightly,
    }
}

/// A nightly a host or a pacman package put in place was installed on purpose, so it stays
/// on nightly once it takes a stable build, such as a candidate on the staff feed.
fn keep_installed_channel(core: &Core) {
    if core.preferences().get().update_channel == UpdateChannel::Stable
        && BuildChannel::of_release(&core.config().build.version) == BuildChannel::Nightly
        && (is_host() || pacman_package().is_some())
    {
        core.preferences()
            .change(PreferenceChange::UpdateChannel(UpdateChannel::Nightly));
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

/// What updates a published Linux app that is a deb or a pacman package: a
/// system upgrade when a repository the system syncs from carries the package
/// on this channel, otherwise the installer, which also moves a deb to the
/// channel's repository. Asks pacman, so it blocks.
fn installer_command(channel: UpdateChannel) -> Option<String> {
    if !cfg!(target_os = "linux") || !super::usage::Install::App.published() || own_app().is_some()
    {
        return None;
    }
    if pacman_package().is_some_and(in_sync_repository) {
        return Some("sudo pacman -Syu".into());
    }
    let suite = match channel {
        UpdateChannel::Stable => "packages-stable",
        UpdateChannel::Nightly | UpdateChannel::Staff => "packages-nightly",
    };
    if std::fs::read_to_string(APT_SOURCE).is_ok_and(|source| source.contains(suite)) {
        return Some(APT_UPGRADE.into());
    }
    let channel = match channel {
        UpdateChannel::Stable => "",
        UpdateChannel::Nightly | UpdateChannel::Staff => "SIKEMUX_CHANNEL=nightly ",
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
        let failed = {
            let mut held = self.held.locked();
            let before = held.status.state;
            change(&mut held);
            if held.status.state != before {
                held.since = Some(Instant::now());
            }
            (held.status.state == UpdateState::Failed && before != UpdateState::Failed)
                .then(|| (before, held.status.clone()))
        };
        if let Some((before, status)) = failed {
            report_failure(core, before, &status);
        }
        core.changed(What::Read(Topic::Update));
        if is_host() {
            core.changed(What::View);
        }
    }

    /// Checks again now, as after the person changes channel.
    pub(crate) fn check_soon(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn chats_changed(&self) {
        self.quiet.notify_waiters();
    }
}

fn check_every(core: &Core) -> Duration {
    match channel(core) {
        UpdateChannel::Staff => STAFF_CHECK_EVERY,
        UpdateChannel::Nightly => NIGHTLY_CHECK_EVERY,
        UpdateChannel::Stable => CHECK_EVERY,
    }
}

/// Checks a little after the core starts and then every minute on staff, five
/// minutes on nightly or half hour on stable, while the core runs in an app that
/// can update.
pub(crate) async fn serve(core: Arc<Core>) {
    let running_since = epoch_ms();
    let asking = core.clone();
    let by_installer = blocking(move || {
        keep_installed_channel(&asking);
        Ok(installer_command(channel(&asking)))
    })
    .await
    .ok()
    .flatten();
    let automatic = match own_app() {
        Some(OwnApp::Host) => Some(blocking(|| Ok(read_switch())).await.unwrap_or(true)),
        Some(_) => Some(when_idle(&core)),
        None => None,
    };
    let outcome = blocking(|| Ok(read_outcome())).await.ok().flatten();
    core.updates().edit(&core, |held| {
        held.status.updates = own_app().is_some();
        held.status.current = core.config().build.version.clone();
        held.status.commit = core.config().build.commit.clone();
        held.status.running_since = running_since;
        held.status.install_command = by_installer.clone();
        held.status.automatic = automatic;
        held.status.outcome = outcome;
    });
    match own_app() {
        Some(OwnApp::Host) => drop(tokio::spawn(settle_own_update(core.clone()))),
        #[cfg(target_os = "macos")]
        Some(OwnApp::Bundle(_)) => {
            let core = core.clone();
            drop(tokio::spawn(async move {
                super::rollback::settle(core.clone(), READY_AFTER).await;
                refresh_outcome(&core).await;
            }))
        }
        _ => {}
    }
    report_outcome(&core).await;
    if !core.updates().status().updates && by_installer.is_none() {
        return;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        if check(&core).await.is_ok() {
            update_when_quiet(&core).await;
        }
        let _ = tokio::time::timeout(check_every(&core), core.updates().wake.notified()).await;
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

/// No agent here is in a turn, or might be: a chat, or one in a terminal, including a
/// terminal whose agent the core cannot read. An update that goes in by itself waits for this.
fn no_agent_mid_turn(core: &Core) -> bool {
    core.chats().in_turn() == 0
        && !(core.running_agents().iter()).any(|agent| core.agent_now(agent).maybe_mid_turn())
}

/// Whether the desktop app installs updates by itself: the person's choice, else on for the
/// channels that build every push to main and off for stable.
fn when_idle(core: &Core) -> bool {
    (core.preferences().get().update_when_idle).unwrap_or(channel(core) != UpdateChannel::Stable)
}

/// Whether this app or host puts updates in place by itself, read afresh rather than from
/// the status, which the first check fills in after the window has asked.
pub(crate) fn updates_by_itself(core: &Core) -> bool {
    match own_app() {
        Some(OwnApp::Host) => core.updates().status().automatic == Some(true),
        Some(_) => when_idle(core),
        None => false,
    }
}

/// Follows the person's choice, or the channel's default, of whether the desktop app updates
/// by itself. Turning it off leaves an update that began by itself for the person to start.
pub(crate) fn follow_when_idle(core: &Core) {
    let on = when_idle(core);
    core.updates().edit(core, |held| {
        if is_host() || held.status.automatic.is_none() {
            return;
        }
        held.status.automatic = Some(on);
        if on || !held.status.by_itself {
            return;
        }
        held.status.by_itself = false;
        if matches!(held.status.state, UpdateState::Waiting | UpdateState::Ready) {
            held.status.state = UpdateState::Idle;
            held.status.blockers.clear();
        }
    });
    core.updates().check_soon();
}

/// A host, or a desktop app the person lets update by itself, puts the release `check` found
/// in place once no chat is in a turn, or at once when the account no longer works with this
/// build, since it then reaches nobody. It waits at most until the next check.
async fn update_when_quiet(core: &Arc<Core>) {
    let status = core.updates().status();
    if status.automatic != Some(true)
        || status.available.is_none()
        || status.state != UpdateState::Idle
    {
        return;
    }
    let until = tokio::time::Instant::now() + check_every(core);
    loop {
        let rested = core.updates().quiet.notified();
        if no_agent_mid_turn(core) || core.remote().too_old() {
            break;
        }
        if tokio::time::timeout_at(until, rested).await.is_err() {
            return;
        }
    }
    let force = core.remote().too_old();
    if force {
        let interrupted = core.chats().blockers().len();
        eprintln!(
            "sikemux core: this version is too old for the account, so it updates now, interrupting {interrupted} chats"
        );
    }
    let _ = begin_install(core, force, !force && !is_host());
}

/// Looks for a release newer than this one on the person's channel. One feed
/// that does not answer must not hide a release the other one offers.
pub(crate) async fn check(core: &Core) -> CoreResult<UpdateStatus> {
    let status = core.updates().status();
    let channel = channel(core);
    let by_installer = blocking(move || Ok(installer_command(channel)))
        .await
        .ok()
        .flatten();
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
    let put_back = status
        .outcome
        .as_ref()
        .filter(|outcome| outcome.state == UpdateOutcomeState::RolledBack)
        .and_then(|outcome| outcome.to.clone());
    let release = newest
        .filter(|newest| newest.version > current)
        .filter(|newest| put_back.as_deref() != Some(newest.version.to_string().as_str()))
        .and_then(|Newest { feed, raw, .. }| {
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
        });
    core.updates().edit(core, |held| {
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
        None => Ok(core.updates().status()),
    }
}

/// Downloads and puts in place the release `check` found, in the background;
/// clients follow it in the status.
pub(crate) fn install(core: &Arc<Core>, params: UpdateInstallParams) -> CoreResult<UpdateStatus> {
    begin_install(core, params.force, false)
}

/// `by_itself` is an update a desktop app began on its own, which waits for every chat to rest
/// however long that takes and stops none.
fn begin_install(core: &Arc<Core>, force: bool, by_itself: bool) -> CoreResult<UpdateStatus> {
    let status = core.updates().status();
    if matches!(
        status.state,
        UpdateState::Downloading | UpdateState::Installing | UpdateState::Ready
    ) {
        return Ok(status);
    }
    let app = own_app().ok_or_else(|| CoreError::from("this build does not update itself"))?;
    let (found, installed) = {
        let held = core.updates().held.locked();
        (held.found.clone(), held.installed)
    };
    let found = found.ok_or_else(|| CoreError::from("there is no update to install"))?;
    core.updates().edit(core, |held| {
        if !installed {
            begin_update(held);
        }
        held.status.forced = force;
        held.status.by_itself = by_itself;
        held.status.error = None;
        held.status.downloaded_bytes = 0;
        held.status.total_bytes = None;
        held.status.state = if installed {
            UpdateState::Waiting
        } else {
            UpdateState::Downloading
        };
    });
    let status = core.updates().status();
    let core = core.clone();
    tokio::spawn(async move {
        if let Err(error) = put_in_place(&core, &app, &found, installed).await {
            core.updates().edit(&core, |held| {
                held.status.state = UpdateState::Failed;
                held.status.error = Some(error);
            });
            return;
        }
        wait_for_chats(&core).await;
        if matches!(app, OwnApp::Host) && core.updates().status().state == UpdateState::Ready {
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
    let request = HostRequest {
        channel: channel(core),
        id: core.updates().status().id,
        action: HostAction::Install,
    };
    let last = host_update::ask_root(&request, |step| {
        if let UpdateStep::Downloaded { bytes, .. } = step {
            core.updates().edit(core, |held| {
                held.status.downloaded_bytes = *bytes;
                held.status.total_bytes = Some(*bytes);
                held.status.state = UpdateState::Installing;
            });
        }
        steps.push(step.clone());
    })
    .await;
    if last == UpdateStep::Off {
        core.updates()
            .edit(core, |held| held.status.automatic = Some(false));
    }
    refresh_outcome(core).await;
    let in_place = match &last {
        UpdateStep::Installed { version, .. } | UpdateStep::Current { version } => {
            Some(version.clone()).filter(|version| *version != core.config().build.version)
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
    let force = core.updates().status().forced;
    let error = upgrade::replace_with(
        core,
        PathBuf::from(INSTALLED_CORE),
        force,
        HANDING_OVER_LIMIT,
    )
    .await;
    core.updates().edit(core, |held| {
        held.status.state = UpdateState::Failed;
        held.status.error = Some(format!("could not restart into it: {error}"));
    });
}

/// How the last update ended: as root recorded it on a host, or this core in the Mac app.
fn read_outcome() -> Option<UpdateOutcome> {
    match own_app()? {
        OwnApp::Host => host_update::read_outcome(),
        #[cfg(target_os = "macos")]
        OwnApp::Bundle(_) => super::rollback::read_outcome(),
        _ => None,
    }
}

/// Reads again how the last update ended.
async fn refresh_outcome(core: &Core) {
    let outcome = blocking(|| Ok(read_outcome())).await.ok().flatten();
    core.updates()
        .edit(core, |held| held.status.outcome = outcome);
    report_outcome(core).await;
}

/// Reports the last update once its written outcome is final.
async fn report_outcome(core: &Core) {
    let Some(outcome) = core.updates().status().outcome else {
        return;
    };
    let channel = channel(core);
    let _ = blocking(move || {
        let platform = own_platform();
        if let Some(ended) = update_reports::of_outcome(&outcome, channel, &platform, is_host()) {
            update_reports::send(ended);
        }
        Ok(())
    })
    .await;
}

/// Reports an update that just failed at the step it was at `before`.
fn report_failure(core: &Core, before: UpdateState, status: &UpdateStatus) {
    let Some(id) = status.id.as_deref() else {
        return;
    };
    update_reports::send(update_reports::Ended {
        id,
        from: &status.current,
        to: status
            .available
            .as_ref()
            .map(|release| release.version.as_str()),
        channel: channel(core),
        platform: &own_platform(),
        result: update_reports::EndedAs::Failed,
        step: update_reports::step_of(before),
        reason: status.error.as_deref(),
    });
}

/// A new update gets an id, which root records its outcome under.
fn begin_update(held: &mut Held) {
    held.status.id = Some(uuid::Uuid::new_v4().to_string());
}

/// On a host core an update just put in place: says it started well once it has run a while,
/// so root drops the core it replaced.
async fn settle_own_update(core: Arc<Core>) {
    let Some(outcome) = core.updates().status().outcome.filter(|outcome| {
        outcome.state == UpdateOutcomeState::Pending
            && outcome.to.as_deref() == Some(core.config().build.version.as_str())
    }) else {
        return;
    };
    tokio::time::sleep(READY_AFTER).await;
    let channel = channel(&core);
    let id = outcome.id.clone();
    let answer = blocking(move || Ok(ask_root_to_settle(channel, &id, HostAction::Ready)))
        .await
        .unwrap_or_else(|error| UpdateStep::Failed {
            message: error.to_string(),
        });
    if let UpdateStep::Failed { message } = &answer {
        eprintln!("sikemux core: could not settle the update: {message}");
    }
    let _ = blocking(|| {
        forget_starts();
        Ok(())
    })
    .await;
    refresh_outcome(&core).await;
}

/// Counts this start of a host core an update just put in place, before anything else can
/// fail, and once it has stopped too often before it was ready, asks root to put the core it
/// replaced back and stops, to start again as that one.
pub(crate) fn roll_back_a_failing_update(version: &str) {
    if !is_host() {
        return;
    }
    let Some(outcome) = host_update::read_outcome().filter(|outcome| {
        outcome.state == UpdateOutcomeState::Pending && outcome.to.as_deref() == Some(version)
    }) else {
        return;
    };
    if count_start(&outcome.id) <= STARTS_BEFORE_ROLL_BACK {
        return;
    }
    let channel = match BuildChannel::of_release(version) {
        BuildChannel::Nightly => UpdateChannel::Nightly,
        BuildChannel::Stable | BuildChannel::Dev => UpdateChannel::Stable,
    };
    let answer = ask_root_to_settle(channel, &outcome.id, HostAction::RollBack);
    forget_starts();
    if let UpdateStep::Failed { message } = answer {
        eprintln!("sikemux core: could not put the previous core back: {message}");
        return;
    }
    eprintln!("sikemux core: {version} stopped before it was ready, so the previous core is back and this one stops to start as it");
    super::log_times::flush();
    std::process::exit(1);
}

/// Asks root to settle update `id`, leaving the note in the guarded store that proves the
/// person's core asks. Blocks.
fn ask_root_to_settle(channel: UpdateChannel, id: &str, action: HostAction) -> UpdateStep {
    let failed = |message: String| UpdateStep::Failed { message };
    let note = host_update::settle_note(id, action);
    if let Err(error) = sikemux_keychain::write(
        host_update::SWITCH_SERVICE,
        host_update::SETTLE_ACCOUNT,
        &note,
    ) {
        return failed(error.to_string());
    }
    let request = HostRequest {
        channel,
        id: Some(id.to_owned()),
        action,
    };
    let answer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_or_else(
            |error| failed(error.to_string()),
            |runtime| runtime.block_on(host_update::ask_root(&request, |_| {})),
        );
    let _ = sikemux_keychain::delete(host_update::SWITCH_SERVICE, host_update::SETTLE_ACCOUNT);
    answer
}

/// How many times a core has started for the update `id`, this one included.
fn count_start(id: &str) -> u32 {
    #[derive(Serialize, Deserialize)]
    struct Starts {
        id: String,
        count: u32,
    }
    let Some(path) = sikemux_process::paths::config_file("update-starts", "json") else {
        return 1;
    };
    let before = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Starts>(&bytes).ok())
        .filter(|starts| starts.id == id)
        .map_or(0, |starts| starts.count);
    let starts = Starts {
        id: id.to_owned(),
        count: before + 1,
    };
    if let Ok(bytes) = serde_json::to_vec(&starts) {
        let _ = sikemux_process::files::write_atomic(&path, &bytes, true);
    }
    starts.count
}

fn forget_starts() {
    if let Some(path) = sikemux_process::paths::config_file("update-starts", "json") {
        let _ = std::fs::remove_file(path);
    }
}

/// Marks an update failed once it has downloaded or installed for longer than any should.
async fn watch_stalls(core: Arc<Core>) {
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        let (state, since) = {
            let held = core.updates().held.locked();
            (held.status.state, held.since)
        };
        let working = matches!(state, UpdateState::Downloading | UpdateState::Installing);
        if working && since.is_some_and(|since| since.elapsed() > WORKING_LIMIT) {
            core.updates().edit(&core, |held| {
                held.status.state = UpdateState::Failed;
                held.status.error = Some(format!(
                    "it did not finish in {} minutes",
                    WORKING_LIMIT.as_secs() / 60
                ));
            });
        }
    }
}

/// What a host's devices show of its updates; none on anything but a host.
pub(crate) fn host_view(core: &Core) -> Option<UpdateStatus> {
    is_host().then(|| core.updates().status())
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
        core.updates().status().state,
        UpdateState::Idle | UpdateState::Failed
    ) {
        return Err("this host is already updating".into());
    }
    core.updates().edit(core, |held| {
        begin_update(held);
        held.status.forced = false;
        held.status.state = UpdateState::Installing;
        held.status.error = None;
    });
    let (steps, in_place) = ask_root(core).await;
    let failed = matches!(steps.last(), Some(UpdateStep::Failed { .. }));
    core.updates().edit(core, |held| {
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
            if core.updates().status().state == UpdateState::Ready {
                restart_into_installed(&core).await;
            }
        });
    }
    Ok(HostUpdate {
        steps,
        handing_over: in_place,
    })
}

/// Turns a host's own updates on or off, for the person this core runs as.
pub(crate) async fn set_automatic(
    core: &Arc<Core>,
    params: HostUpdatesParams,
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
    core.updates()
        .edit(core, |held| held.status.automatic = Some(on));
    core.updates().check_soon();
    Ok(core.updates().status())
}

/// Stops waiting on chats; the update stays in place for later.
pub(crate) fn cancel(core: &Core) -> UpdateStatus {
    core.updates().edit(core, |held| {
        if held.status.state == UpdateState::Waiting {
            held.status.state = UpdateState::Idle;
            held.status.blockers.clear();
        }
    });
    core.updates().status()
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
        core.updates().edit(core, |held| held.installed = true);
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
                core.updates().edit(core, |held| {
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
    core.updates()
        .edit(core, |held| held.status.state = UpdateState::Installing);
    #[cfg(target_os = "macos")]
    let bundle = matches!(app, OwnApp::Bundle(_));
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
    #[cfg(target_os = "macos")]
    if bundle {
        expect_the_new_core(core).await;
    }
    core.updates().edit(core, |held| held.installed = true);
    Ok(())
}

/// Writes down the update just put in place of the Mac app, so the core it brought can say it
/// started well, and this core watches it take over.
#[cfg(target_os = "macos")]
async fn expect_the_new_core(core: &Core) {
    let status = core.updates().status();
    let outcome = UpdateOutcome {
        id: status
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        state: UpdateOutcomeState::Pending,
        from: status.current,
        to: status.available.map(|release| release.version),
        at: epoch_ms(),
        reason: None,
        evidence: None,
    };
    let written = outcome.clone();
    let _ = blocking(move || {
        super::rollback::write_outcome(&written);
        Ok(())
    })
    .await;
    core.updates()
        .edit(core, |held| held.status.outcome = Some(outcome));
}

/// Says the update is ready once no chat would lose work to the restart, at once when the
/// person chose to stop them, or once the chats have had the core's patience, which then
/// stops the ones still busy and starts them again on their sessions. One that began by
/// itself has no end to its patience, needs every chat out of its turn, and keeps watching
/// until the app restarts, waiting again whenever a chat starts one.
async fn wait_for_chats(core: &Core) {
    let patience = tokio::time::Instant::now() + core.config().update_patience;
    loop {
        let status = core.updates().status();
        let watching = if status.by_itself {
            UpdateState::Ready
        } else {
            UpdateState::Waiting
        };
        if !matches!(status.state, UpdateState::Installing | UpdateState::Waiting)
            && status.state != watching
        {
            return;
        }
        let blockers: Vec<UpdateBlocker> = core
            .chats()
            .blockers()
            .into_iter()
            .map(|blocker| UpdateBlocker {
                agent_id: blocker.agent_id,
                provider: blocker.provider,
                title: blocker.title,
                reason: blocker.reason,
            })
            .collect();
        let out_of_patience = !status.by_itself && tokio::time::Instant::now() >= patience;
        let rested = blockers.is_empty() && (!status.by_itself || no_agent_mid_turn(core));
        let ready = status.forced || rested || out_of_patience;
        core.updates().edit(core, |held| {
            held.status.forced |= out_of_patience && !blockers.is_empty();
            held.status.blockers = blockers;
            held.status.state = if ready {
                UpdateState::Ready
            } else {
                UpdateState::Waiting
            };
        });
        if ready && !status.by_itself {
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
    #[cfg(unix)]
    std::fs::set_permissions(
        download,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .map_err(cannot)?;
    std::fs::rename(download, file).map_err(cannot)
}

/// The app an update replaced, kept beside it until the new one starts well.
pub(crate) fn previous_bundle(bundle: &Path) -> Option<PathBuf> {
    beside(bundle, "previous").ok()
}

/// Puts the app in `archive` in place of `bundle`, keeping the old one beside it to go back
/// to, so a failure leaves the app as it was.
fn swap(bundle: &Path, archive: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("the app is not in a folder")?;
    let name = bundle
        .file_name()
        .ok_or("the app has no name")?
        .to_string_lossy()
        .into_owned();
    let cannot =
        |error: std::io::Error| format!("Sikemux cannot write to {}: {error}", parent.display());
    let staging = parent.join(format!(".{name}.update"));
    let old = previous_bundle(bundle).ok_or("the app has no name")?;
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
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

impl super::part::Part for Updates {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            tokio::spawn(serve(core.clone())),
            tokio::spawn(watch_stalls(core.clone())),
        ]
    }
}
