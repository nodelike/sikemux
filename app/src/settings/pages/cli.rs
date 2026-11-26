//! The `sikemux` command: whether it is installed, and how it is used.

use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, FontWeight, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_calls::call;
use sikemux_calls::cli_install::{CliInstallState, CliInstallStatus};
use sikemux_calls::params::NoParams;

use crate::core::Target;
use crate::icons::Icon;
use crate::settings::index::Page::Developer;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS, TEXT_SM,
    TEXT_XS,
};
use crate::toast;
use crate::ui::{self, Tone};
use crate::workbench::Workbench;

pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Developer,
        "Command line",
        "developer cli command line install terminal path shell integration",
    ),
    section(
        Developer,
        "Opening from the shell",
        "developer cli editor git commit open usage shell",
    ),
];

pub(super) fn read(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let asked = super::change_pages(bench, cx, |pages, _| {
        pages.cli.get_or_insert(None);
        pages.cli_busy = false;
    });
    if asked.is_none() {
        return;
    }
    let asked = Target::here(&bench.asks).request_typed::<call::CliInstall>(NoParams {});
    cx.spawn(async move |bench, cx| {
        let answer = asked.await.ok();
        told(bench, answer, None, cx);
    })
    .detach();
}

fn install(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let asked = super::change_pages(bench, cx, |pages, _| {
        pages.cli.get_or_insert(None);
        pages.cli_busy = true;
    });
    if asked.is_none() {
        return;
    }
    let here = Target::here(&bench.asks);
    cx.spawn(async move |bench, cx| {
        let (answer, note) = match here.request_typed::<call::InstallCli>(NoParams {}).await {
            Ok(answer) => {
                let ok = answer.state == CliInstallState::Installed;
                let note = if ok {
                    Some(toast::success(t!("Command line tool installed")))
                } else {
                    Some(toast::error(answer.message.clone()))
                };
                (Some(answer), note)
            }
            Err(error) => (
                here.request_typed::<call::CliInstall>(NoParams {})
                    .await
                    .ok(),
                Some(toast::error(error)),
            ),
        };
        told(bench, answer, note, cx);
    })
    .detach();
}

fn told(
    bench: gpui_kit::WeakEntity<Workbench>,
    answer: Option<CliInstallStatus>,
    note: Option<Notification>,
    cx: &mut gpui_kit::AsyncApp,
) {
    let _ = bench.update(cx, |bench, cx| {
        super::change_pages(bench, cx, |pages, _| {
            pages.cli = Some(answer);
            pages.cli_busy = false;
        });
        if let Some(note) = note {
            bench.notify(note, cx);
        }
    });
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let Some(status) = &pages.cli else {
        return Vec::new();
    };
    let status = status.as_ref();
    let state = status.map(|status| status.state);
    let (label, tone): (SharedString, Tone) = match state {
        None => (t!("Checking…").into(), Tone::Idle),
        Some(CliInstallState::Unavailable) => (t!("Unavailable").into(), Tone::Idle),
        Some(CliInstallState::NotInstalled) => (t!("Not installed").into(), Tone::Idle),
        Some(CliInstallState::Installed) => (t!("Installed").into(), Tone::Ok),
        Some(CliInstallState::Outdated) => (t!("Outdated").into(), Tone::Pending),
        Some(CliInstallState::Conflict) => (t!("Conflict").into(), Tone::Failed),
    };
    let install_path = status
        .map(|status| {
            if !status.cli_path.is_empty() {
                status.cli_path.as_str()
            } else if !status.install_dir.is_empty() {
                status.install_dir.as_str()
            } else {
                "—"
            }
        })
        .unwrap_or("—")
        .to_owned();
    let install_dir = status
        .map(|status| status.install_dir.as_str())
        .filter(|dir| !dir.is_empty())
        .unwrap_or("—")
        .to_owned();
    let release = bench
        .connection
        .version
        .split_once('-')
        .map_or(bench.connection.version.as_str(), |(release, _)| release)
        .to_owned();
    let busy = pages.cli_busy;
    let button = match (busy, state) {
        (true, _) => t!("Installing…"),
        (_, Some(CliInstallState::Outdated)) => t!("Update CLI"),
        (_, Some(CliInstallState::Installed)) => t!("Installed"),
        _ => t!("Install CLI"),
    };
    let can_install = !busy
        && matches!(
            state,
            Some(CliInstallState::NotInstalled | CliInstallState::Outdated)
        );
    let tile = div()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .bg(t.surface_raised)
        .border(crate::theme::border())
        .border_color(t.border)
        .child(ui::icon(Icon::Terminal, 15.).text_color(t.text_primary));
    let name = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child("sikemux"),
        )
        .when(!release.is_empty(), |row| {
            row.child(
                div()
                    .px(SPACE_1H)
                    .py(px(1.))
                    .rounded(theme::radius_1())
                    .bg(t.surface_raised)
                    .border(crate::theme::border())
                    .border_color(t.border)
                    .font_family(theme::mono_font())
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(release),
            )
        });
    let about = div()
        .font_family(theme::mono_font())
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .truncate()
        .child(install_path)
        .into_any_element();
    let (refresh, this_install) = (this.clone(), this.clone());
    let controls = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .mr(SPACE_1)
                .child(ui::dot(tone, cx))
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(label),
                ),
        )
        .child(
            ui::IconButton::new("cli-refresh", Icon::Refresh, t!("Refresh"))
                .on_click(move |_, _, cx| refresh.update(cx, read)),
        )
        .child(
            ui::Button::new("cli-install", button, ui::Variant::Outline)
                .when(state == Some(CliInstallState::Installed), |button| {
                    button.icon(Icon::Check)
                })
                .disabled(!can_install)
                .on_click(move |_, _, cx| this_install.update(cx, install)),
        );
    let example = |command: &'static str, note: &'static str| {
        div()
            .flex()
            .items_baseline()
            .gap(SPACE_3)
            .child(
                div()
                    .w(px(230.))
                    .flex_none()
                    .font_family(theme::mono_font())
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.text_primary)
                    .child(command),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(note),
            )
    };
    let examples = div()
        .px(SPACE_4)
        .py(SPACE_2H)
        .border_b(crate::theme::border())
        .border_color(t.line_soft)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(example(
            "sikemux .",
            t!("Focus or create a workspace for this folder"),
        ))
        .child(example(
            "sikemux src/App.tsx:42:5",
            t!("Open a file at a line and column"),
        ))
        .child(example(
            "sikemux open --wait README.md",
            t!("Open and wait until the tab closes"),
        ));
    let body = div()
        .flex()
        .flex_col()
        .child(rows::row_parts(
            Some(tile.into_any_element()),
            name,
            Some(about),
            controls,
            cx,
        ))
        .when(state == Some(CliInstallState::Conflict), |section| {
            section.child(
                div()
                    .px(SPACE_4)
                    .py(SPACE_2)
                    .border_b(crate::theme::border())
                    .border_color(t.line_soft)
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.danger)
                    .child(t!(
                        "Sikemux will not overwrite {path}. Move the existing file yourself, then refresh.",
                        path = status.map_or("", |status| status.cli_path.as_str())
                    )),
            )
        })
        .when(
            state == Some(CliInstallState::Installed)
                && status.is_some_and(|status| !status.path_configured),
            |section| {
                section.child(
                    div()
                        .px(SPACE_4)
                        .py(SPACE_2)
                        .border_b(crate::theme::border())
                        .border_color(t.line_soft)
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(t!(
                            "Add {install_dir} to your shell’s PATH. Sikemux never edits shell startup files.",
                            install_dir = install_dir
                        )),
                )
            },
        )
        .when(
            state == Some(CliInstallState::Unavailable),
            |section| {
                section.child(
                    div()
                        .px(SPACE_4)
                        .py(SPACE_2)
                        .border_b(crate::theme::border())
                        .border_color(t.line_soft)
                        .text_size(theme::ui_text(TEXT_SM))
                        .text_color(t.text_tertiary)
                        .child(
                            status
                                .map(|status| status.message.clone())
                                .unwrap_or_default(),
                        ),
                )
            },
        )
        .child(examples);
    vec![rows::section(
        n!("Command line"),
        None,
        None,
        lit("Command line") || lit("Opening from the shell"),
        body,
        cx,
    )]
}
