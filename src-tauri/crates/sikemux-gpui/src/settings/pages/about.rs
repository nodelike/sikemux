//! Which releases updates come from and when they were last checked, and whether anonymous
//! usage is shared.

use gpui_kit::{
    div, px, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use serde_json::json;
use sikemux_wire::protocol::UpdateChannel;

use crate::icons::Icon;
use crate::settings::rows;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Updates", "Privacy", "Help"];

const RELEASES: &str = "https://github.com/nodelike/sikemux/releases";

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
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
            let channel = if *chosen == 0 { "stable" } else { "nightly" };
            this.update(cx, |bench, _| {
                crate::settings::set(bench, "updateChannel", channel)
            })
        })
    };
    let status = bench.read("update");
    let updates: AnyElement = match status.is_some_and(|status| status["updates"] == true) {
        true => {
            let status = status.cloned().unwrap_or_default();
            let busy = matches!(
                status["state"].as_str(),
                Some("downloading" | "installing" | "ready")
            );
            let checked = &status["checked"];
            let last: SharedString = match checked["at"]
                .as_i64()
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                None => "Not checked yet this session.".into(),
                Some(at) => {
                    let when = at.with_timezone(&chrono::Local).format("%-I:%M:%S %p");
                    let channel = checked["channel"].as_str().unwrap_or("stable");
                    match checked["error"].as_str() {
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
            div()
                .child(rows::row(
                    "Channel",
                    Some("Stable follows the latest signed release; nightly the newest build, prerelease or stable."),
                    channel,
                    cx,
                ))
                .child(rows::row_with(None, "Last checked", Some(last), check, cx))
                .into_any_element()
        }
        false => {
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
        Some("Once a day, sends a random install ID and a one-way hash of your account with the app version, update channel and system version. Never your files, commands or projects."),
        rows::switch("share-usage", "shareUsageData", preferences.share_usage_data, this),
        cx,
    );
    let help = {
        let (news, welcome, diagnostics) = (this.clone(), this.clone(), this.clone());
        div()
            .flex()
            .gap(px(8.))
            .p(px(14.))
            .child(
                ui::Button::new("help-whats-new", "What’s New", ui::Variant::Outline).on_click(
                    move |_, window, cx| {
                        news.update(cx, |bench, cx| {
                            crate::settings::close(bench, window, cx);
                            crate::experience::open_whats_new(bench, cx);
                        })
                    },
                ),
            )
            .child(
                ui::Button::new("help-welcome", "Show welcome", ui::Variant::Outline).on_click(
                    move |_, window, cx| {
                        welcome.update(cx, |bench, cx| {
                            crate::settings::close(bench, window, cx);
                            crate::experience::open_welcome(bench, cx);
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
                        crate::experience::open_diagnostics(bench, cx);
                    })
                }),
            )
    };
    vec![
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
    let checked = crate::core::request(&bench.asks, "updateCheck", json!({}));
    cx.spawn(async move |bench, cx| {
        let note = match checked.await {
            Ok(status) => {
                let channel = status["checked"]["channel"].as_str().unwrap_or("stable");
                match status["available"]["version"].as_str() {
                    None => toast::info(format!("Sikemux is up to date on the {channel} channel.")),
                    Some(version) if status["state"] == "idle" => {
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
