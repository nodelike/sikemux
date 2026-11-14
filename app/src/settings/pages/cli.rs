//! The `sikemux` command: whether it is installed, and how it is used.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, px, AnyElement, App, Context, Entity, ParentElement, SharedString, Styled};
use sikemux_calls::call;
use sikemux_calls::cli_install::CliInstallState;
use sikemux_calls::cli_install::CliInstallStatus;
use sikemux_calls::params::NoParams;

use crate::core::Target;
use crate::icons::Icon;
use crate::settings::index::Page::Cli;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(Cli, "Shell integration", "install terminal path"),
    section(Cli, "Usage", "editor git commit open"),
];

/// Asks the core how the command is installed.
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
        told(bench, answer, cx);
    })
    .detach();
}

/// Installs the command, then says how it stands; when that failed, as the core now reads it.
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
        let answer = match here.request_typed::<call::InstallCli>(NoParams {}).await {
            Ok(answer) => Some(answer),
            Err(_) => here
                .request_typed::<call::CliInstall>(NoParams {})
                .await
                .ok(),
        };
        told(bench, answer, cx);
    })
    .detach();
}

fn told(
    bench: gpui_kit::WeakEntity<Workbench>,
    answer: Option<CliInstallStatus>,
    cx: &mut gpui_kit::AsyncApp,
) {
    let _ = bench.update(cx, |bench, cx| {
        super::change_pages(bench, cx, |pages, _| {
            pages.cli = Some(answer);
            pages.cli_busy = false;
        });
    });
}

pub fn render(
    _: &Workbench,
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
    let label: SharedString = match state {
        None => t!("checking"),
        Some(CliInstallState::Unavailable) => t!("unavailable"),
        Some(CliInstallState::NotInstalled) => t!("not installed"),
        Some(CliInstallState::Installed) => t!("installed"),
        Some(CliInstallState::Outdated) => t!("outdated"),
        Some(CliInstallState::Conflict) => t!("conflict"),
    }
    .into();
    let message: SharedString = status
        .map_or(
            t!("Checking the packaged command-line integration…"),
            |status| status.message.as_str(),
        )
        .to_owned()
        .into();
    let install_dir = status
        .map(|status| status.install_dir.as_str())
        .filter(|dir| !dir.is_empty())
        .unwrap_or("—")
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
    let code = |text: String| {
        div()
            .px(SPACE_1H)
            .rounded(theme::radius_2())
            .bg(t.surface_sunken)
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .child(text)
    };
    let path_line = |name: &'static str, value: String| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .child(
                div()
                    .w(px(120.))
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(name),
            )
            .child(code(value))
    };
    let (refresh, this_install) = (this.clone(), this.clone());
    let integration = div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .p(SPACE_4)
        .child(path_line(t!("Command"), "sikemux".into()))
        .child(path_line(t!("Install directory"), install_dir.clone()))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(SPACE_2)
                .child(
                    ui::Button::new("cli-refresh", t!("Refresh"), ui::Variant::Outline)
                        .icon(Icon::Refresh)
                        .on_click(move |_, _, cx| refresh.update(cx, read)),
                )
                .child(
                    ui::Button::new("cli-install", button, ui::Variant::Primary)
                        .when(state == Some(CliInstallState::Installed), |button| {
                            button.icon(Icon::Check)
                        })
                        .disabled(!can_install)
                        .on_click(move |_, _, cx| this_install.update(cx, install)),
                ),
        )
        .when(state == Some(CliInstallState::Conflict), |section| {
            section.child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS))
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
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(t!("Add {install_dir} to your shell’s PATH. Sikemux never edits shell startup files.", install_dir = install_dir)),
                )
            },
        );
    let usage = div()
        .p(SPACE_4)
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_secondary)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child("sikemux .")
        .child("sikemux src/App.tsx:42:5")
        .child("sikemux open --wait README.md");
    vec![
        rows::section(n!("Shell integration"), Some(label), Some(message), lit("Shell integration"), integration, cx),
        rows::section(
            n!("Usage"),
            None,
            Some(t!("Existing files open in an editor tab. Directories focus or create their workspace.").into()),
            lit("Usage"),
            usage,
            cx,
        ),
    ]
}
