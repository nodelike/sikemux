//! Every agent Sikemux can run, with what it needs next: nothing, an install, or a
//! sign-in, which runs in the sheet `agent_setup` draws.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::agent_accounts::status::AgentStatus;

use crate::agent_setup::{installing, run, signing_in, status_label, Setup, Sheet};
use crate::settings::rows;
use crate::ui;
use crate::workbench::Workbench;

pub fn render(
    bench: &Workbench,
    setup: &Setup,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let body: AnyElement = match &setup.found {
        None => rows::note("Looking for agents…", cx),
        Some(found) => div()
            .children(found.iter().map(|agent| {
                let detail: Option<SharedString> = match &agent.status {
                    AgentStatus::Broken { reason } => Some(reason.clone().into()),
                    AgentStatus::Missing => Some(agent.about.clone().into()),
                    _ => None,
                };
                let about: SharedString = match &detail {
                    Some(detail) => format!("{} · {detail}", status_label(&agent.status)).into(),
                    None => status_label(&agent.status).into(),
                };
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
                    .gap(px(8.))
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
                    Some(ui::mark(&agent.kind, 18.)),
                    agent.label.clone(),
                    Some(about),
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
        "Agents on this machine",
        ready,
        Some("Install an agent here and it lands where its own updater expects, so it works in your terminal too.".into()),
        lit("Agents on this machine"),
        body,
        cx,
    )
}
