//! How updating this app stands: the rail's note, the screen an update that must be taken puts
//! over the window, and installing it.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, Context, FontWeight, Global, Hsla, InteractiveElement,
    IntoElement, ParentElement, Styled,
};
use sikemux_calls::params::NoParams;
use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::{UpdateOutcomeState, UpdateState, UpdateStatus};

use crate::core::Target;
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

/// When the person last pressed a key in any window.
pub(crate) struct LastKey(pub Instant);

impl Global for LastKey {}

/// Restarts into the installed update once the person's keys have rested, so the windows never
/// close under a word being typed.
pub(crate) fn restart_when_keys_rest(cx: &mut App) {
    cx.spawn(async move |cx| loop {
        let rested = cx.update(|cx| cx.try_global::<LastKey>().map(|key| key.0.elapsed()));
        match rested {
            Some(rested) if rested < KEYS_REST => {
                cx.background_executor().timer(KEYS_REST - rested).await;
            }
            _ => {
                cx.update(|cx| cx.restart());
                return;
            }
        }
    })
    .detach();
}

/// Restarts into an update that began by itself once the person has been idle a while,
/// unless a chat started a turn meanwhile, which the core reports by waiting again.
pub(crate) fn restart_when_idle(cx: &mut Context<Workbench>) {
    cx.spawn(async move |bench, cx| loop {
        let Ok(rested) = bench.update(cx, |bench, cx| {
            let ready = (bench.reads.update.as_ref())
                .is_some_and(|update| update.by_itself && update.state == UpdateState::Ready);
            ready.then(|| cx.try_global::<LastKey>().map(|key| key.0.elapsed()))
        }) else {
            return;
        };
        match rested {
            None => return,
            Some(Some(rested)) if rested < IDLE => {
                cx.background_executor().timer(IDLE - rested).await;
            }
            Some(_) => {
                cx.update(|cx| cx.restart());
                return;
            }
        }
    })
    .detach();
}

/// Installs the update, or restarts into one that is in place and waiting for the person
/// to be idle.
pub(crate) fn install_now(bench: &Workbench, cx: &mut App) {
    match bench.reads.update.as_ref() {
        Some(update) if update.by_itself && update.state == UpdateState::Ready => cx.restart(),
        _ => install(bench, false),
    }
}

pub(crate) fn pending(bench: &Workbench) -> Option<&UpdateStatus> {
    bench.reads.update.as_ref().filter(|update| {
        (update.updates || update.install_command.is_some()) && update.available.is_some()
    })
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
/// screen; none while there is no update.
pub fn rail_chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let update = pending(bench)?;
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
        UpdateState::Waiting => (
            install_button(update).0,
            Icon::Download,
            ui::Variant::Outline,
            Some(match update.by_itself {
                true => t!("It installs once no agent is in the middle of a turn. Open to update now.").into(),
                false => t!("Some agents can't be carried over yet. Within two minutes the update goes on and starts them again on their sessions. Open to see them or update now.").into(),
            }),
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
        .on_click(cx.listener(|bench, _, _, cx| {
            match bench.reads.update.as_ref() {
                Some(update) if update.by_itself && update.state == UpdateState::Ready => cx.restart(),
                _ => experience::open_whats_new(bench, cx),
            }
        }));
    Some(
        div()
            .flex_none()
            .mx(SPACE_2H)
            .mb(SPACE_2)
            .flex()
            .flex_col()
            .child(chip)
            .into_any_element(),
    )
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
    let mut size = count as f64;
    for unit in ["KB", "MB", "GB", "TB"] {
        size /= 1024.;
        if size < 1024. || unit == "TB" {
            return match size < 10. {
                true => format!("{size:.1} {unit}"),
                false => format!("{size:.0} {unit}"),
            };
        }
    }
    unreachable!()
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
        UpdateState::Waiting => (waiting_label(update.blockers.len()), false),
        UpdateState::Failed => (t!("Retry v{version}", version = version), true),
        UpdateState::Idle => (t!("Install v{version}", version = version), true),
    }
}

pub(crate) fn waiting_label(count: usize) -> String {
    match count {
        1 => t!("Update waiting on 1 agent").to_owned(),
        count => t!("Update waiting on {count} agents", count = count),
    }
}
