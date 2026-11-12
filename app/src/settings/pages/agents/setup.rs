//! Every agent Sikemux can run, with what it needs next: nothing, an install, a sign-in,
//! which runs in the sheet `agent_setup` draws, or an update to its newest release, on this
//! host or any account host reached.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::agent_accounts::status::AgentStatus;
use sikemux_core::server::agent_updates::Updating;

use crate::agents::setup::{
    installing, run, signing_in, status_label, update, update_everywhere, Found, Setup, Sheet,
};
use crate::hosts::Place;
use crate::settings::rows;
use crate::theme::{SPACE_1H, SPACE_2, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] =
    &[crate::settings::index::section(
        crate::settings::index::Page::Installed,
        "Agents on this host",
        "install reinstall update upgrade version latest missing claude codex opencode hermes pi omp grok cli",
    )];

pub fn render(
    bench: &Workbench,
    setup: &Setup,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let body: AnyElement = match &setup.found {
        None => rows::note(t!("Looking for agents…"), cx),
        Some(found) => list(bench, found, &Place::This, this, cx),
    };
    let ready = setup.found.as_ref().map(|found| {
        let ready = found
            .iter()
            .filter(|agent| matches!(agent.status, AgentStatus::Ready { .. }))
            .count();
        SharedString::from(format!("{ready} ready"))
    });
    let behind = |found: &[Found]| found.iter().any(|agent| agent.update_to().is_some());
    let here_behind = setup.found.as_deref().is_some_and(behind);
    let elsewhere_behind = setup.elsewhere.iter().any(|host| behind(&host.found));
    let update_all = (here_behind || elsewhere_behind).then(|| {
        let this = this.clone();
        let label = if elsewhere_behind {
            "Update every host"
        } else {
            "Update all"
        };
        ui::Button::new("agent-update-all", label, ui::Variant::Outline)
            .on_click(move |_, _, cx| this.update(cx, update_everywhere))
            .into_any_element()
    });
    let here = rows::section_with(
        t!("Agents on this host"),
        ready,
        update_all,
        Some("Install an agent here and it lands where its own updater expects, so it works in your terminal too.".into()),
        lit("Agents on this host"),
        body,
        cx,
    );
    div()
        .flex()
        .flex_col()
        .gap(crate::theme::SPACE_4)
        .child(here)
        .children(
            setup
                .elsewhere
                .iter()
                .filter(|host| !host.found.is_empty())
                .map(|host| {
                    let place = Place::Account(host.host.clone());
                    let body = list(bench, &host.found, &place, this, cx);
                    let update_host = behind(&host.found).then(|| {
                        let (this, place) = (this.clone(), place.clone());
                        ui::Button::new(
                            SharedString::from(format!("agent-update-all-{}", host.host)),
                            "Update all",
                            ui::Variant::Outline,
                        )
                        .on_click(move |_, _, cx| {
                            let place = place.clone();
                            this.update(cx, |bench, cx| update(bench, place, Vec::new(), cx))
                        })
                        .into_any_element()
                    });
                    rows::section_with(
                        format!("Agents on {}", bench.host_name(&host.host)),
                        None,
                        update_host,
                        None,
                        false,
                        body,
                        cx,
                    )
                }),
        )
        .into_any_element()
}

/// The release line under an agent's name: what it runs, what is out, and how an update goes.
fn releases(agent: &Found) -> Option<SharedString> {
    match &agent.updating {
        Some(Updating::Queued) => return Some("Waiting for another update to finish".into()),
        Some(Updating::Running { said }) => {
            return Some(said.clone().unwrap_or_else(|| "Updating…".into()).into())
        }
        Some(Updating::Failed { reason }) => {
            return Some(format!("Update failed: {reason}").into())
        }
        None => {}
    }
    let version = agent.version.as_deref()?;
    let mut line = version.to_owned();
    if let Some(latest) = agent.update_to() {
        line.push_str(&format!(" · {latest} out"));
        if !agent.unlocks.is_empty() {
            line.push_str(&format!(", brings {}", agent.unlocks.join(", ")));
        }
    } else if agent.latest.is_some() {
        line.push_str(" · newest");
    }
    let older: Vec<&str> = (agent.running.iter())
        .filter(|running| running.as_str() != version)
        .map(String::as_str)
        .collect();
    if !older.is_empty() {
        line.push_str(&format!(
            " · open chats run {} until restarted",
            older.join(", ")
        ));
    }
    Some(line.into())
}

fn list(
    bench: &Workbench,
    found: &[Found],
    place: &Place,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = crate::theme::Tokens::get(cx).clone();
    let here = *place == Place::This;
    div()
        .children(found.iter().map(|agent| {
            let detail: Option<SharedString> = match &agent.status {
                AgentStatus::Broken { reason } => Some(reason.clone().into()),
                AgentStatus::Missing => Some(agent.about.clone().into()),
                _ => releases(agent),
            };
            let tone = match agent.status {
                AgentStatus::Ready { .. } | AgentStatus::Unknown => ui::Tone::Ok,
                AgentStatus::SignedOut => ui::Tone::Pending,
                AgentStatus::Missing => ui::Tone::Idle,
                AgentStatus::Broken { .. } => ui::Tone::Failed,
            };
            let status = div()
                .min_w_0()
                .overflow_hidden()
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
                        .min_w_0()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_secondary)
                        .child(status_label(&agent.status)),
                );
            let folder = bench
                .accounts
                .current(&agent.kind)
                .and_then(|profile| profile.config_path.clone());
            let sign_in = (here && matches!(agent.status, AgentStatus::SignedOut))
                .then(|| signing_in(agent, folder.as_deref()))
                .flatten();
            let broken = matches!(agent.status, AgentStatus::Broken { .. });
            let missing = matches!(agent.status, AgentStatus::Missing);
            let install = (here && (missing || broken))
                .then(|| installing(agent))
                .flatten();
            let sheet_button = |id: String, label: &'static str, sheet: Sheet| {
                let this = this.clone();
                ui::Button::new(SharedString::from(id), label, ui::Variant::Outline).on_click(
                    move |_, _, cx| {
                        let sheet = sheet.clone();
                        this.update(cx, |bench, cx| run(bench, sheet, cx))
                    },
                )
            };
            let failed = matches!(agent.updating, Some(Updating::Failed { .. }));
            let update_button = (!agent.updating_now() && (failed || agent.update_to().is_some()))
                .then(|| {
                    let (this, place, kind) = (this.clone(), place.clone(), agent.kind.clone());
                    let id = match place {
                        Place::This => format!("agent-update-{kind}"),
                        Place::Account(ref host) => format!("agent-update-{host}-{kind}"),
                    };
                    ui::Button::new(
                        SharedString::from(id),
                        if failed { "Retry" } else { "Update" },
                        ui::Variant::Outline,
                    )
                    .on_click(move |_, _, cx| {
                        let (place, kind) = (place.clone(), kind.clone());
                        this.update(cx, |bench, cx| update(bench, place, vec![kind], cx))
                    })
                });
            let running = matches!(agent.updating, Some(Updating::Running { .. }));
            let actions = div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .when(running, |actions| {
                    actions.child(ui::sign_in::spinner("agent-update", cx))
                })
                .child(status)
                .children(update_button)
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
        .into_any_element()
}
