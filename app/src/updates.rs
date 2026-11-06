//! How updating this app stands: the rail's note, the screen an update that must be taken puts
//! over the window, and installing it.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, ClipboardItem, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Styled,
};
use serde_json::json;
use sikemux_core::server::updates::{UpdateState, UpdateStatus};
use sikemux_view::prefs::UpdateChannel;

use crate::core::Ask;
use crate::icons::Icon;
use crate::screens::experience;
use crate::screens::whats_new::RELEASES;
use crate::theme::{Tokens, SPACE_2, SPACE_2H, SPACE_3, TEXT_SM};
use crate::workbench::Workbench;
use crate::{theme, ui};
use sikemux_core::server::call;
use sikemux_core::server::updates::UpdateInstallParams;

/// The update the core found for this app, while one is there to install.
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
        crate::toast::success("Copied. Run it in a terminal to install the update."),
        cx,
    );
}

/// The channel's name as the person reads it in a sentence.
pub fn channel_name(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Nightly => "nightly",
    }
}

/// The version of the update found, or nothing until one is.
pub(crate) fn available_version(update: &UpdateStatus) -> &str {
    update
        .available
        .as_ref()
        .map_or("", |release| release.version.as_str())
}

/// The rail's note that an update is out, or how its install is going, which opens this
/// screen; none while there is no update.
pub fn rail_chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let update = pending(bench)?;
    let version = available_version(update);
    let (label, icon, variant, tip) = match update.state {
        UpdateState::Failed => (
            "Update failed — retry".to_owned(),
            Icon::Warning,
            ui::Variant::Outline,
            Some(format!(
                "v{version} did not install: {}",
                update.error.as_deref().unwrap_or("unknown error")
            )),
        ),
        UpdateState::Waiting => (
            install_button(update).0,
            Icon::Download,
            ui::Variant::Outline,
            Some("Some agents can't be carried over yet. Open to see them or update now.".into()),
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
            format!("Update · v{version}"),
            Icon::Download,
            ui::Variant::Primary,
            Some(format!("v{version} is out; you have v{}.", update.current)),
        ),
    };
    let chip = ui::Button::new("rail-update", label, variant)
        .icon(icon)
        .when_some(tip, |chip, tip| chip.tooltip(tip))
        .on_click(cx.listener(|bench, _, _, cx| experience::open_whats_new(bench, cx)));
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
            "Download the new version",
            ui::Variant::Primary,
        ),
        Some(found) => {
            let (label, pressable) = install_button(found);
            let label = match found.state {
                UpdateState::Idle => format!("Update to v{}", available_version(found)),
                _ => label,
            };
            ui::Button::new("required-update", label, ui::Variant::Primary)
                .disabled(!pressable)
                .on_click(cx.listener(|bench, _, _, _| install(bench, false)))
        }
        None => ui::Button::new("required-check", "Check for updates", ui::Variant::Primary)
            .on_click(cx.listener(|bench, _, _, _| {
                let _ = bench.asks.send(Ask::Call {
                    method: "updateCheck",
                    params: json!({}),
                });
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
                            .child("Update Sikemux"),
                    )
                    .child(note(format!(
                        "This version, {}, is older than Sikemux's servers work with now. Remote access and your account stay off until you update to {} or later.",
                        required.current, required.minimum
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
                                .child(note(format!("No newer version on the {channel} channel yet.")))
                                .child(releases("required-releases", "See every release", ui::Variant::Outline)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(SPACE_2)
                            .child(
                                ui::Button::new("required-quit", "Quit Sikemux", ui::Variant::Outline)
                                    .on_click(|_, _, cx| cx.quit()),
                            )
                            .child(go_on),
                    ),
            )
            .into_any_element(),
    )
}

pub(crate) fn install(bench: &Workbench, force: bool) {
    bench.call_typed::<call::UpdateInstall>(UpdateInstallParams { force });
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
                Some(total) => format!("Downloading · {}%", (done * 100 / total).min(100)),
                None if done > 0 => format!("Downloading · {}", bytes(done)),
                None => "Downloading…".to_owned(),
            };
            (label, false)
        }
        UpdateState::Installing => ("Verifying & installing…".to_owned(), false),
        UpdateState::Ready => ("Restarting…".to_owned(), false),
        UpdateState::Waiting => (waiting_label(update.blockers.len()), false),
        UpdateState::Failed => (format!("Retry v{version}"), true),
        UpdateState::Idle => (format!("Install v{version}"), true),
    }
}

pub(crate) fn waiting_label(count: usize) -> String {
    format!(
        "Update waiting on {count} agent{}",
        if count == 1 { "" } else { "s" }
    )
}
