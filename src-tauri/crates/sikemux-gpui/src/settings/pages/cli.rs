//! The `sikemux` command: whether it is installed, and how it is used.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, px, AnyElement, Context, Entity, ParentElement, SharedString, Styled, Window};
use serde_json::{json, Value};

use crate::core;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Shell integration", "Usage"];

/// Asks the core how the command is installed, or installs it.
fn ask(bench: &mut Workbench, method: &'static str, cx: &mut Context<Workbench>) {
    let Some(settings) = bench.settings.as_mut() else {
        return;
    };
    settings.pages.cli.get_or_insert(Value::Null);
    settings.pages.cli_busy = method == "installCli";
    bench.parts.redraw(&bench.parts.stage, cx);
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let answer = core::request(&asks, method, json!({})).await;
        let answer = match (answer, method) {
            (Ok(answer), _) => answer,
            (Err(_), "installCli") => core::request(&asks, "cliInstall", json!({}))
                .await
                .unwrap_or_default(),
            (Err(_), _) => Value::Null,
        };
        let _ = bench.update(cx, |bench, cx| {
            if let Some(settings) = bench.settings.as_mut() {
                settings.pages.cli = Some(answer);
                settings.pages.cli_busy = false;
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

pub fn render(
    _: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let Some(status) = &pages.cli else {
        cx.defer_in(window, |bench, _, cx| ask(bench, "cliInstall", cx));
        return Vec::new();
    };
    let state = status["state"].as_str().unwrap_or("checking");
    let label: SharedString = state
        .chars()
        .flat_map(|letter| match letter.is_uppercase() {
            true => vec![' ', letter.to_ascii_lowercase()],
            false => vec![letter],
        })
        .collect::<String>()
        .into();
    let message: SharedString = status["message"]
        .as_str()
        .unwrap_or("Checking the packaged command-line integration…")
        .to_owned()
        .into();
    let install_dir = status["installDir"]
        .as_str()
        .filter(|dir| !dir.is_empty())
        .unwrap_or("—")
        .to_owned();
    let busy = pages.cli_busy;
    let button = match (busy, state) {
        (true, _) => "Installing…",
        (_, "outdated") => "Update CLI",
        (_, "installed") => "Installed",
        _ => "Install CLI",
    };
    let can_install =
        !busy && !matches!(state, "installed" | "unavailable" | "conflict" | "checking");
    let code = |text: String| {
        div()
            .px(px(6.))
            .rounded(theme::RADIUS_2)
            .bg(t.surface_sunken)
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(12.))
            .child(text)
    };
    let path_line = |name: &'static str, value: String| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(120.))
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(t.text_tertiary)
                    .child(name),
            )
            .child(code(value))
    };
    let (refresh, install) = (this.clone(), this.clone());
    let integration = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .child(path_line("Command", "sikemux".into()))
        .child(path_line("Install directory", install_dir.clone()))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    ui::Button::new("cli-refresh", "Refresh", ui::Variant::Outline)
                        .icon(Icon::Refresh)
                        .on_click(move |_, _, cx| {
                            refresh.update(cx, |bench, cx| ask(bench, "cliInstall", cx))
                        }),
                )
                .child(
                    ui::Button::new("cli-install", button, ui::Variant::Primary)
                        .when(state == "installed", |button| button.icon(Icon::Check))
                        .disabled(!can_install)
                        .on_click(move |_, _, cx| {
                            install.update(cx, |bench, cx| ask(bench, "installCli", cx))
                        }),
                ),
        )
        .when(state == "conflict", |section| {
            section.child(
                div()
                    .text_size(crate::theme::ui_text(12.))
                    .text_color(t.danger)
                    .child(format!(
                    "Sikemux will not overwrite {}. Move the existing file yourself, then refresh.",
                    status["cliPath"].as_str().unwrap_or_default()
                )),
            )
        })
        .when(
            state == "installed" && status["pathConfigured"].as_bool() == Some(false),
            |section| {
                section.child(
                    div()
                        .text_size(crate::theme::ui_text(12.))
                        .text_color(t.text_tertiary)
                        .child(format!(
                "Add {install_dir} to your shell’s PATH. Sikemux never edits shell startup files."
            )),
                )
            },
        );
    let usage = div()
        .p(px(14.))
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_secondary)
        .flex()
        .flex_col()
        .gap(px(4.))
        .child("sikemux .")
        .child("sikemux src/App.tsx:42:5")
        .child("sikemux open --wait README.md");
    vec![
        rows::section("Shell integration", Some(label), Some(message), lit("Shell integration"), integration, cx),
        rows::section(
            "Usage",
            None,
            Some("Existing files open in an editor tab. Directories focus or create their workspace.".into()),
            lit("Usage"),
            usage,
            cx,
        ),
    ]
}
