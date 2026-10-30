//! Every agent Sikemux can run, with what it needs next: nothing, an install, or a
//! sign-in, which runs in the sheet `agent_setup` draws.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::agent_accounts::status::AgentStatus;

use crate::agents::setup::{installing, run, signing_in, status_label, Setup, Sheet};
use crate::settings::rows;
use crate::theme::{SPACE_1H, SPACE_2, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

pub fn render(
    bench: &Workbench,
    setup: &Setup,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = crate::theme::Tokens::get(cx).clone();
    let body: AnyElement = match &setup.found {
        None => rows::note("Looking for agents…", cx),
        Some(found) => div()
            .children(found.iter().map(|agent| {
                let detail: Option<SharedString> = match &agent.status {
                    AgentStatus::Broken { reason } => Some(reason.clone().into()),
                    AgentStatus::Missing => Some(agent.about.clone().into()),
                    _ => None,
                };
                let tone = match agent.status {
                    AgentStatus::Ready { .. } | AgentStatus::Unknown => ui::Tone::Ok,
                    AgentStatus::SignedOut => ui::Tone::Pending,
                    AgentStatus::Missing => ui::Tone::Idle,
                    AgentStatus::Broken { .. } => ui::Tone::Failed,
                };
                let status = div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .px(SPACE_2)
                    .h(px(22.))
                    .rounded_full()
                    .border_1()
                    .border_color(t.border)
                    .child(ui::dot(tone, cx))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.text_secondary)
                            .child(status_label(&agent.status)),
                    );
                let folder = bench
                    .accounts
                    .current(&agent.kind)
                    .and_then(|profile| profile.config_path.clone());
                let sign_in = matches!(agent.status, AgentStatus::SignedOut)
                    .then(|| signing_in(agent, folder.as_deref()))
                    .flatten();
                let broken = matches!(agent.status, AgentStatus::Broken { .. });
                let missing = matches!(agent.status, AgentStatus::Missing);
                let install = (missing || broken).then(|| installing(agent)).flatten();
                let sheet_button = |id: String, label: &'static str, sheet: Sheet| {
                    let this = this.clone();
                    ui::Button::new(SharedString::from(id), label, ui::Variant::Outline).on_click(
                        move |_, _, cx| {
                            let sheet = sheet.clone();
                            this.update(cx, |bench, cx| run(bench, sheet, cx))
                        },
                    )
                };
                let actions = div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(status)
                    .children(sign_in.map(|sheet| {
                        sheet_button(format!("agent-sign-in-{}", agent.kind), "Sign in", sheet)
                    }))
                    .children(install.map(|sheet| {
                        sheet_button(
                            format!("agent-install-{}", agent.kind),
                            if broken { "Reinstall" } else { "Install" },
                            sheet,
                        )
                    }));
                rows::row_with(
                    Some(
                        ui::mark_box(32., ui::mark(&agent.kind, 18.))
                            .rounded(crate::theme::RADIUS_2)
                            .bg(t.surface_raised)
                            .border_1()
                            .border_color(t.border)
                            .into_any_element(),
                    ),
                    agent.label.clone(),
                    detail,
                    actions,
                    cx,
                )
                .when(missing, |row| row.opacity(0.75))
                .into_any_element()
            }))
            .into_any_element(),
    };
    let ready = setup.found.as_ref().map(|found| {
        let ready = found
            .iter()
            .filter(|agent| matches!(agent.status, AgentStatus::Ready { .. }))
            .count();
        SharedString::from(format!("{ready} ready"))
    });
    rows::section(
        "Agents on this host",
        ready,
        Some("Install an agent here and it lands where its own updater expects, so it works in your terminal too.".into()),
        lit("Agents on this host"),
        body,
        cx,
    )
}
