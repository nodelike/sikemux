//! How updating this app stands: the rail's note, the screen an update that must be taken puts
//! over the window, and installing it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, Context, FontWeight, Global, Hsla, InteractiveElement,
    IntoElement, ParentElement, Styled,
};
use sikemux_calls::params::NoParams;
use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::{UpdateOutcomeState, UpdateState, UpdateStatus};

use tokio::sync::mpsc::UnboundedSender;

use crate::core::{Ask, Target};
use crate::icons::Icon;
use crate::screens::experience;
use crate::screens::whats_new::RELEASES;
use crate::theme::{Tokens, SPACE_2, SPACE_2H, SPACE_3, TEXT_SM};
use crate::workbench::Workbench;
use crate::{theme, ui};
use sikemux_calls::call;
use sikemux_calls::updates::UpdateInstallParams;

/// The update the core found for this app, while one is there to install.
/// How long the person's keys must rest before an update restarts the windows under them.
const KEYS_REST: Duration = Duration::from_secs(3);

/// How long the person must leave the keys alone before an update that began by itself
/// restarts the app.
const IDLE: Duration = Duration::from_secs(10 * 60);

/// When the person last pressed a key in any window, or when the app started if none yet.
pub(crate) struct LastKey(pub Instant);

impl Global for LastKey {}

/// Set once this app has begun restarting. On the Mac every restart opens a new copy of the app,
/// so a second one would leave two windows behind.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Set while a wait for the person to go idle is running, so the update turning ready again
/// does not start another one beside it.
static WAITING_FOR_IDLE: AtomicBool = AtomicBool::new(false);

/// Starts the app again, once however many times it is asked.
pub(crate) fn restart_once(cx: &mut App) {
    if !RESTARTING.swap(true, Ordering::SeqCst) {
        cx.restart();
    }
}

pub(crate) fn is_restarting() -> bool {
    RESTARTING.load(Ordering::SeqCst)
}

/// Starts the app again on the update in place. On Windows the core only fetched the release's
/// installer, which the Sikemux service runs: it replaces the app and the core and opens the app
/// again, so this hands over to it through the core.
fn restart_into_update(asks: &UnboundedSender<Ask>, cx: &mut App) {
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    #[cfg(windows)]
    if sikemux_process::paths::update_setup().is_some_and(|setup| setup.is_file()) {
        let applied = Target::here(asks).request_typed::<call::UpdateApply>(NoParams {});
        cx.spawn(async move |cx| match applied.await {
            Ok(true) => cx.update(|cx| cx.quit()),
            Ok(false) => {
                eprintln!("sikemux: no Sikemux service runs this core to install the update");
                RESTARTING.store(false, Ordering::SeqCst);
            }
            Err(error) => {
                eprintln!("sikemux: the update did not start: {error}");
                RESTARTING.store(false, Ordering::SeqCst);
            }
        })
        .detach();
        return;
    }
    #[cfg(not(windows))]
    let _ = asks;
    // Clicks call this while the workbench is being updated, and restarting updates it again.
    cx.defer(|cx| cx.restart());
}

/// Restarts into the installed update once the person's keys have rested, so the windows never
/// close under a word being typed.
pub(crate) fn restart_when_keys_rest(asks: UnboundedSender<Ask>, cx: &mut App) {
    cx.spawn(async move |cx| loop {
        let rested = cx.update(|cx| cx.try_global::<LastKey>().map(|key| key.0.elapsed()));
        match rested {
            Some(rested) if rested < KEYS_REST => {
                cx.background_executor().timer(KEYS_REST - rested).await;
            }
            _ => {
                cx.update(|cx| restart_into_update(&asks, cx));
                return;
            }
        }
    })
    .detach();
}

/// Restarts into an update that began by itself once the person has been idle a while.
pub(crate) fn restart_when_idle(cx: &mut Context<Workbench>) {
    if WAITING_FOR_IDLE.swap(true, Ordering::SeqCst) {
        return;
    }
    let waiting = WaitingForIdle;
    cx.spawn(async move |bench, cx| {
        let _waiting = waiting;
        loop {
            let Ok((rested, asks)) = bench.update(cx, |bench, cx| {
                let ready = pending(bench)
                    .is_some_and(|update| update.by_itself && update.state == UpdateState::Ready);
                let rested = ready.then(|| cx.try_global::<LastKey>().map(|key| key.0.elapsed()));
                (rested, bench.asks.clone())
            }) else {
                return;
            };
            match rested {
                None => return,
                Some(Some(rested)) if rested < IDLE => {
                    cx.background_executor().timer(IDLE - rested).await;
                }
                Some(_) => {
                    cx.update(|cx| restart_into_update(&asks, cx));
                    return;
                }
            }
        }
    })
    .detach();
}

/// Clears `WAITING_FOR_IDLE` however the wait ends, so the update turning ready again later
/// starts a new one.
struct WaitingForIdle;

impl Drop for WaitingForIdle {
    fn drop(&mut self) {
        WAITING_FOR_IDLE.store(false, Ordering::SeqCst);
    }
}

/// Installs the update, or restarts into one that is in place and waiting for the person
/// to be idle.
pub(crate) fn install_now(bench: &Workbench, cx: &mut App) {
    match pending(bench) {
        Some(update) if update.by_itself && update.state == UpdateState::Ready => {
            restart_into_update(&bench.asks, cx)
        }
        _ => install(bench, false),
    }
}

/// Whether `status` offers a release newer than an app running `own`.
pub(crate) fn is_pending_for(status: &UpdateStatus, own: &str) -> bool {
    let Some(available) = status.available.as_ref() else {
        return false;
    };
    (status.updates || status.install_command.is_some())
        && sikemux_corekit::build::older_version(own, &available.version)
}

pub(crate) fn pending(bench: &Workbench) -> Option<&UpdateStatus> {
    bench
        .reads
        .update
        .as_ref()
        .filter(|update| is_pending_for(update, env!("CARGO_PKG_VERSION")))
}

/// The update's version and the command that installs it, for an app its installer updates.
pub fn by_installer(update: &UpdateStatus) -> Option<(String, String)> {
    let version = &update.available.as_ref()?.version;
    let command = update.install_command.as_ref()?;
    Some((version.clone(), command.clone()))
}

pub fn copy_install_command(bench: &mut Workbench, command: &str, cx: &mut Context<Workbench>) {
    cx.write_to_clipboard(ClipboardItem::new_string(command.to_owned()));
    bench.notify(
        crate::toast::success(t!("Copied. Run it in a terminal to install the update.")),
        cx,
    );
}

/// The channel's name as the person reads it in a sentence.
pub fn channel_name(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Nightly => "nightly",
        UpdateChannel::Staff => "staff",
    }
}

/// The version of the update found, or nothing until one is.
pub(crate) fn available_version(update: &UpdateStatus) -> &str {
    update
        .available
        .as_ref()
        .map_or("", |release| release.version.as_str())
}

/// Which build the core runs and since when, so nobody reads its age off the process.
pub(crate) fn running(update: &UpdateStatus) -> String {
    let commit = update.commit.get(..7).unwrap_or(&update.commit);
    match update.running_since {
        0 => t!(
            "Running v{version} ({commit}).",
            version = update.current,
            commit = commit
        ),
        since => t!(
            "Running v{version} ({commit}), took over {ago}.",
            version = update.current,
            commit = commit,
            ago = crate::ui::time::ago(since)
        ),
    }
}

/// The core went back to the build before this app's, as this build's core stopped right after
/// it took over, so this app starts again as the one put back.
pub(crate) fn put_back_from_here(update: &UpdateStatus) -> bool {
    let own = env!("CARGO_PKG_VERSION");
    update.current != own
        && update.outcome.as_ref().is_some_and(|outcome| {
            outcome.state == UpdateOutcomeState::RolledBack && outcome.to.as_deref() == Some(own)
        })
}

/// Says once, on the build put back, which update was undone and why.
pub(crate) fn tell_put_back(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(outcome) = (bench.reads.update.as_ref())
        .and_then(|update| update.outcome.as_ref())
        .filter(|outcome| {
            outcome.state == UpdateOutcomeState::RolledBack
                && outcome.from == env!("CARGO_PKG_VERSION")
        })
    else {
        return;
    };
    let Some(told) = sikemux_process::paths::config_file("update-told", "") else {
        return;
    };
    if std::fs::read_to_string(&told).is_ok_and(|id| id == outcome.id) {
        return;
    }
    if let Err(error) = sikemux_process::files::write_atomic(&told, outcome.id.as_bytes(), true) {
        eprintln!("sikemux: the undone update is told again next time: {error}");
    }
    let said = t!(
        "Update to v{to} undone: {reason}. Back on v{from}; running terminals stopped, chats picked up again.",
        to = outcome.to.as_deref().unwrap_or("?"),
        from = outcome.from,
        reason = outcome.reason.as_deref().unwrap_or_default()
    );
    bench.notify(crate::toast::error(said), cx);
}

/// The rail's note that an update is out, or how its install is going, which opens this
/// screen; with no update, the quiet note that a nightly's notes wait unread.
pub fn rail_chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let Some(update) = pending(bench) else {
        return news_note(bench, cx);
    };
    let version = available_version(update);
    let (label, icon, variant, tip) = match update.state {
        UpdateState::Failed => (
            t!("Update failed — retry").to_owned(),
            Icon::Warning,
            ui::Variant::Outline,
            Some(t!(
                "v{version} did not install: {error}",
                version = version,
                error = update.error.as_deref().unwrap_or(t!("unknown error"))
            )),
        ),
        UpdateState::Ready if update.by_itself => (
            install_button(update).0,
            Icon::Refresh,
            ui::Variant::Primary,
            Some(t!("It restarts once you leave the keys for 10 minutes; terminals and chats carry over.").into()),
        ),
        state @ (UpdateState::Downloading | UpdateState::Installing | UpdateState::Ready) => (
            install_button(update).0,
            if state == UpdateState::Downloading {
                Icon::Download
            } else {
                Icon::Refresh
            },
            ui::Variant::Outline,
            None,
        ),
        UpdateState::Idle => (
            t!("Update · v{version}", version = version),
            Icon::Download,
            ui::Variant::Primary,
            Some(t!(
                "v{version} is out; you have v{current}.",
                version = version,
                current = update.current
            )),
        ),
    };
    let tip = match tip {
        Some(tip) => format!("{tip}\n{}", running(update)),
        None => running(update),
    };
    let chip = ui::Button::new("rail-update", label, variant)
        .icon(icon)
        .tooltip(tip)
        .on_click(cx.listener(|bench, _, _, cx| match pending(bench) {
            Some(update) if update.by_itself && update.state == UpdateState::Ready => {
                restart_into_update(&bench.asks, cx)
            }
            _ => experience::open_whats_new(bench, cx),
        }));
    Some(rail_slot(chip))
}

/// A nightly installed itself without opening its notes, so the rail offers every change
/// since the build the person last read; reading them clears it.
fn news_note(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let since = bench.reads.news_since.as_deref()?;
    let t = Tokens::get(cx);
    let build = match since.split_once("-nightly.") {
        Some((_, number)) => t!("since nightly {number}", number = number),
        None => t!("since v{version}", version = since),
    };
    let mut tip = t!("New since v{version}.", version = since);
    if let Some(update) = bench.reads.update.as_ref() {
        tip = format!("{tip}\n{}", running(update));
    }
    let note = ui::Button::new("rail-whats-new", t!("What's new"), ui::Variant::Ghost)
        .lead(ui::icon(Icon::Sparkle, 11.).text_color(t.accent))
        .trail(
            div()
                .flex_1()
                .flex()
                .justify_end()
                .text_color(t.text_tertiary)
                .child(build),
        )
        .tooltip(tip)
        .on_click(cx.listener(|bench, _, _, cx| experience::open_whats_new(bench, cx)));
    Some(rail_slot(note))
}

/// Where the rail draws its update chip or news note, above the account.
fn rail_slot(chip: impl IntoElement) -> AnyElement {
    div()
        .flex_none()
        .mx(SPACE_2H)
        .mb(SPACE_2)
        .flex()
        .flex_col()
        .child(chip)
        .into_any_element()
}

/// Covers the window while the accounts server no longer works with this build, which keeps
/// remote access and the account off; the ways on are updating and quitting.
pub fn required(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let required = bench.remote.as_ref()?.update_required.as_ref()?;
    let t = Tokens::get(cx).clone();
    let update = bench.reads.update.as_ref();
    let found = pending(bench);
    let in_app = update.is_some_and(|update| update.updates);
    let checked = update.and_then(|update| update.checked.as_ref());
    let problem = match found {
        Some(found) if found.state == UpdateState::Failed => found.error.as_deref(),
        Some(_) => None,
        None => checked.and_then(|checked| checked.error.as_deref()),
    };
    let nothing_newer = found.is_none() && checked.is_some_and(|checked| checked.error.is_none());
    let channel = checked.map_or("stable", |checked| channel_name(checked.channel));
    let note = |text: String| {
        div()
            .text_size(theme::ui_text(TEXT_SM))
            .line_height(theme::ui_text(13. * 1.5))
            .text_color(t.text_secondary)
            .child(text)
    };
    let releases = |id: &'static str, label: &'static str, variant: ui::Variant| {
        ui::Button::new(id, label, variant).on_click(|_, _, cx| cx.open_url(RELEASES))
    };
    let go_on = match found {
        _ if !in_app => releases(
            "required-download",
            t!("Download the new version"),
            ui::Variant::Primary,
        ),
        Some(found) => {
            let (label, pressable) = install_button(found);
            let label = match found.state {
                UpdateState::Idle => t!("Update to v{version}", version = available_version(found)),
                _ => label,
            };
            ui::Button::new("required-update", label, ui::Variant::Primary)
                .disabled(!pressable)
                .on_click(cx.listener(|bench, _, _, _| install(bench, false)))
        }
        None => ui::Button::new(
            "required-check",
            t!("Check for updates"),
            ui::Variant::Primary,
        )
        .on_click(cx.listener(|bench, _, _, _| {
            Target::here(&bench.asks).call_typed::<call::UpdateCheck>(NoParams {});
        })),
    };
    Some(
        div()
            .id("update-required")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla {
                a: 0.82,
                ..t.surface_sunken
            })
            .occlude()
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .text_size(theme::ui_text(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_primary)
                            .child(t!("Update Sikemux")),
                    )
                    .child(note(t!(
                        "This version, {current}, is older than Sikemux's servers work with now. Remote access and your account stay off until you update to {minimum} or later.",
                        current = required.current,
                        minimum = required.minimum
                    )))
                    .when_some(problem, |column, problem| {
                        column.child(ui::meta(problem.to_owned(), cx).whitespace_normal())
                    })
                    .when(nothing_newer, |column| {
                        column.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_2)
                                .child(note(t!("No newer version on the {channel} channel yet.", channel = channel)))
                                .child(releases("required-releases", t!("See every release"), ui::Variant::Outline)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(SPACE_2)
                            .child(
                                ui::Button::new("required-quit", t!("Quit Sikemux"), ui::Variant::Outline)
                                    .on_click(|_, _, cx| cx.quit()),
                            )
                            .child(go_on),
                    ),
            )
            .into_any_element(),
    )
}

pub(crate) fn install(bench: &Workbench, force: bool) {
    Target::here(&bench.asks).call_typed::<call::UpdateInstall>(UpdateInstallParams { force });
}

/// `2048` as `2.0 KB`, `15728640` as `15 MB`.
pub(crate) fn bytes(count: u64) -> String {
    let mut size = count as f64 / 1024.;
    let mut unit = "KB";
    for larger in ["MB", "GB", "TB"] {
        if size < 1024. {
            break;
        }
        size /= 1024.;
        unit = larger;
    }
    match size < 10. {
        true => format!("{size:.1} {unit}"),
        false => format!("{size:.0} {unit}"),
    }
}

/// What the install button says for where the update stands, and whether it can be pressed.
pub(crate) fn install_button(update: &UpdateStatus) -> (String, bool) {
    let version = available_version(update);
    match update.state {
        UpdateState::Downloading => {
            let done = update.downloaded_bytes;
            let label = match update.total_bytes.filter(|total| *total > 0) {
                Some(total) => t!(
                    "Downloading · {percent}%",
                    percent = (done * 100 / total).min(100)
                ),
                None if done > 0 => t!("Downloading · {size}", size = bytes(done)),
                None => t!("Downloading…").to_owned(),
            };
            (label, false)
        }
        UpdateState::Installing => (t!("Verifying & installing…").to_owned(), false),
        UpdateState::Ready if update.by_itself => (t!("Restart to update").to_owned(), true),
        UpdateState::Ready => (t!("Restarting…").to_owned(), false),
        UpdateState::Failed => (t!("Retry v{version}", version = version), true),
        UpdateState::Idle => (t!("Install v{version}", version = version), true),
    }
}

#[cfg(test)]
mod tests {
    use sikemux_view::updates::{UpdateRelease, UpdateState, UpdateStatus};

    use super::is_pending_for;

    fn status_with(version: &str) -> UpdateStatus {
        UpdateStatus {
            updates: true,
            current: "0.6.4-nightly.726".into(),
            available: Some(UpdateRelease {
                version: version.into(),
                notes: None,
                date: None,
                credits: None,
            }),
            state: UpdateState::Ready,
            by_itself: true,
            ..Default::default()
        }
    }

    #[test]
    fn hides_update_already_running_on_restarted_app() {
        let ready = status_with("0.6.4-nightly.733");
        assert!(is_pending_for(&ready, "0.6.4-nightly.726"));
        assert!(!is_pending_for(&ready, "0.6.4-nightly.733"));
        assert!(!is_pending_for(&ready, "0.6.4-nightly.734"));
        assert!(!is_pending_for(&ready, "0.6.4"));
    }

    #[test]
    fn offers_newer_stable_over_nightly() {
        let stable = status_with("0.6.4");
        assert!(is_pending_for(&stable, "0.6.4-nightly.733"));
        assert!(!is_pending_for(&stable, "0.6.4"));
    }
}
