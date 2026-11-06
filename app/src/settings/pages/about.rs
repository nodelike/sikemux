//! Which releases updates come from and when they were last checked, and whether anonymous
//! usage is shared.

use crate::core::Target;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::UpdateState;

use crate::icons::Icon;
use crate::screens::whats_new::RELEASES;
use crate::settings::index::Page::About;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::theme::{SPACE_2, SPACE_3H, SPACE_4, TEXT_2XS_H};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(About, "Sikemux", "version build"),
    section(About, "Updates", "version upgrade release"),
    row(
        About,
        "Updates",
        "Channel",
        "nightly stable prerelease beta",
    ),
    row(About, "Updates", "Last checked", "check for updates now"),
    section(About, "Privacy", "analytics telemetry tracking"),
    row(
        About,
        "Privacy",
        "Share anonymous usage",
        "analytics telemetry tracking opt out posthog",
    ),
    section(
        About,
        "Help",
        "what's new changelog diagnostics welcome onboarding",
    ),
];

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let preferences = bench.preferences();
    let channel = {
        let this = this.clone();
        let chosen = match preferences.update_channel {
            UpdateChannel::Stable => 0,
            _ => 1,
        };
        ui::Segmented::new(
            "update-channel",
            vec![
                (Icon::Shield, SharedString::from("Stable")),
                (Icon::Flask, SharedString::from("Nightly")),
            ],
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let channel = if *chosen == 0 {
                UpdateChannel::Stable
            } else {
                UpdateChannel::Nightly
            };
            this.update(cx, |bench, _| {
                crate::settings::set(bench, PreferenceChange::UpdateChannel(channel))
            })
        })
    };
    let status = bench.reads.update.as_ref();
    let updates: AnyElement = match status
        .filter(|status| status.updates || status.install_command.is_some())
    {
        Some(status) => {
            let busy = matches!(
                status.state,
                UpdateState::Downloading | UpdateState::Installing | UpdateState::Ready
            );
            let last: SharedString = match status.checked.as_ref().and_then(|checked| {
                let at = chrono::DateTime::from_timestamp_millis(checked.at as i64)?;
                Some((checked, at))
            }) {
                None => "Not checked yet this session.".into(),
                Some((checked, at)) => {
                    let when = at.with_timezone(&chrono::Local).format("%-I:%M:%S %p");
                    let channel = crate::updates::channel_name(checked.channel);
                    match &checked.error {
                        Some(error) => {
                            format!("Last {channel} check failed at {when} — {error}").into()
                        }
                        None => format!("Checked {channel} at {when}.").into(),
                    }
                }
            };
            let check = {
                let this = this.clone();
                ui::Button::new("update-check", "Check now", ui::Variant::Outline)
                    .disabled(busy)
                    .on_click(move |_, _, cx| this.update(cx, check_now))
            };
            let by_hand = crate::updates::by_installer(status).map(|(version, command)| {
                let this = this.clone();
                let about = format!("Run this in a terminal to install it: {command}");
                let copy =
                    ui::Button::new("update-copy-command", "Copy command", ui::Variant::Outline)
                        .on_click(move |_, _, cx| {
                            this.update(cx, |bench, cx| {
                                crate::updates::copy_install_command(bench, &command, cx)
                            })
                        });
                rows::row_with(
                    None,
                    format!("Update v{version}"),
                    Some(about.into()),
                    copy,
                    cx,
                )
            });
            div()
                .child(rows::row(
                    "Channel",
                    Some("Stable follows the latest signed release; nightly the newest build, prerelease or stable."),
                    channel,
                    cx,
                ))
                .child(rows::row_with(None, "Last checked", Some(last), check, cx))
                .children(by_hand)
                .into_any_element()
        }
        None => {
            let this = this.clone();
            rows::row(
                "New versions",
                Some("This build does not update itself. Download each new release from GitHub."),
                ui::Button::new("update-releases", "Open releases", ui::Variant::Outline)
                    .on_click(move |_, _, cx| this.update(cx, |_, cx| cx.open_url(RELEASES))),
                cx,
            )
            .into_any_element()
        }
    };
    let privacy = rows::row(
        "Share anonymous usage",
        Some("Once a day, sends a random install ID and a one-way hash of your account with the app version, update channel and system version, and the day before's totals: tokens by agent and model, turns, sessions, commits and how often each feature was used. Never your files, prompts, commands or projects."),
        rows::switch("share-usage", PreferenceChange::ShareUsageData, preferences.share_usage_data, this),
        cx,
    );
    let help = {
        let (news, welcome, diagnostics) = (this.clone(), this.clone(), this.clone());
        div()
            .flex()
            .gap(SPACE_2)
            .p(SPACE_4)
            .child(
                ui::Button::new("help-whats-new", "What’s New", ui::Variant::Outline).on_click(
                    move |_, window, cx| {
                        news.update(cx, |bench, cx| {
                            crate::settings::close(bench, window, cx);
                            crate::screens::experience::open_whats_new(bench, cx);
                        })
                    },
                ),
            )
            .child(
                ui::Button::new("help-welcome", "Show welcome", ui::Variant::Outline).on_click(
                    move |_, window, cx| {
                        welcome.update(cx, |bench, cx| {
                            crate::settings::close(bench, window, cx);
                            crate::screens::experience::open_welcome(bench, cx);
                        })
                    },
                ),
            )
            .child(
                ui::Button::new(
                    "help-diagnostics",
                    "Runtime diagnostics",
                    ui::Variant::Outline,
                )
                .on_click(move |_, window, cx| {
                    diagnostics.update(cx, |bench, cx| {
                        crate::settings::close(bench, window, cx);
                        crate::screens::experience::open_diagnostics(bench, cx);
                    })
                }),
            )
    };
    let t = crate::theme::Tokens::get(cx).clone();
    let version = match bench.connection.version.is_empty() {
        true => "Version unknown".to_owned(),
        false => format!("Version {}", bench.connection.version),
    };
    let app = div()
        .flex()
        .items_center()
        .gap(SPACE_3H)
        .p(SPACE_4)
        .child(
            div()
                .size(px(44.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(crate::theme::RADIUS_3)
                .bg(t.surface_raised)
                .border_1()
                .border_color(t.border)
                .child(ui::icon(Icon::Logo, 22.).text_color(t.accent)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    ui::text("Sikemux", 17., gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(t.text_primary),
                )
                .child(ui::mono(version, TEXT_2XS_H, cx)),
        );
    vec![
        rows::card(lit("Sikemux"), app, cx),
        rows::section("Updates", None, None, lit("Updates"), updates, cx),
        rows::section("Privacy", None, None, lit("Privacy"), privacy, cx),
        rows::section(
            "Help",
            None,
            Some("All three are also searchable from the command deck.".into()),
            lit("Help"),
            help,
            cx,
        ),
    ]
}

/// A check the person asked for, which says how it went, as on the Mac.
fn check_now(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let checked = Target::here(&bench.asks).request_typed::<call::UpdateCheck>(NoParams {});
    cx.spawn(async move |bench, cx| {
        let note = match checked.await {
            Ok(status) => {
                let channel = match status.checked.as_ref().map(|checked| checked.channel) {
                    Some(UpdateChannel::Nightly) => "nightly",
                    _ => "stable",
                };
                match status.available.as_ref().map(|release| &release.version) {
                    None => toast::info(format!("Sikemux is up to date on the {channel} channel.")),
                    Some(version) if status.install_command.is_some() => toast::success(
                        format!("Update v{version} is out. Settings → About has the command that installs it."),
                    ),
                    Some(version) if status.state == UpdateState::Idle => {
                        toast::success(format!("Update v{version} is ready to install."))
                    }
                    Some(_) => return,
                }
            }
            Err(error) => toast::error(error),
        };
        let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
    })
    .detach();
}
