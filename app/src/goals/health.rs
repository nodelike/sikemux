//! The project's codebase health on the Overnight page: when drift audits run, and what the
//! latest found, as the core keeps it. Each finding is a card on the board and opens there.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::audit::{AuditEveryParams, AuditNowParams};
use sikemux_core::server::call;
use sikemux_view::audit::{AuditDigest, AuditView};
use sikemux_view::chat::TriggerWhen;

use super::goal::heading;
use super::GoalsPane;
use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_5, TEXT_SM};
use crate::ui::{self, Button, Pick, Variant};

const MORNING: u32 = 9 * 60;

impl GoalsPane {
    fn audit_every(&mut self, every: Option<TriggerWhen>, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        self.ask_moved::<call::BoardAuditEvery>(AuditEveryParams { project, every }, |_| {}, cx);
    }

    fn audit_now(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        self.ask_moved::<call::BoardAuditNow>(AuditNowParams { project }, |_| {}, cx);
    }
}

fn every_picks() -> Vec<Pick> {
    vec![
        Pick::new("off", "Off"),
        Pick::new("daily", "Every day at 09:00"),
        Pick::new("12", "Every 12 hours"),
        Pick::new("6", "Every 6 hours"),
    ]
}

fn every_choice(every: Option<&TriggerWhen>) -> (String, String) {
    match every {
        Some(TriggerWhen::Daily { minute, .. }) => (
            "daily".into(),
            format!("Every day at {:02}:{:02}", minute / 60, minute % 60),
        ),
        Some(TriggerWhen::Every { seconds }) => {
            let hours = seconds / 3600;
            (hours.to_string(), format!("Every {hours} hours"))
        }
        _ => ("off".into(), "Off".into()),
    }
}

fn every_of(key: &str) -> Option<TriggerWhen> {
    match key {
        "off" => None,
        "daily" => Some(TriggerWhen::Daily {
            minute: MORNING,
            weekdays: Vec::new(),
        }),
        hours => hours.parse::<u64>().ok().map(|hours| TriggerWhen::Every {
            seconds: hours * 3600,
        }),
    }
}

/// The health part of the Overnight page for the project in front.
pub(super) fn health(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let view = pane
        .board()
        .and_then(|board| board.audit.clone())
        .unwrap_or_default();
    let AuditView {
        every,
        next_at,
        latest,
    } = view;
    let (key, words) = every_choice(every.as_ref());
    let entity = cx.entity();
    let schedule = ui::select(
        "health-every",
        words,
        every_picks(),
        Some(key.into()),
        pane.busy,
        move |picked, _, cx| {
            let every = every_of(picked.as_ref());
            entity.update(cx, |pane, cx| pane.audit_every(every, cx));
        },
        cx,
    );
    let running = latest.as_ref().is_some_and(|latest| latest.running);
    let about = match (&latest, next_at) {
        (Some(latest), _) => {
            let by = latest
                .agent_name
                .as_ref()
                .map(|name| format!(" by @{name}"));
            Some(format!(
                "audit {}{}",
                ui::time::clock(latest.at),
                by.unwrap_or_default()
            ))
        }
        (None, Some(next)) => Some(format!("first audit {}", ui::time::clock(next))),
        (None, None) => None,
    };
    div()
        .flex()
        .flex_col()
        .pt(SPACE_5)
        .child(heading("Codebase health", None, about, cx))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .pb(SPACE_2)
                .child(ui::meta("Drift audits", cx))
                .child(schedule)
                .children(runs_on(pane, cx).map(|runs_on| ui::meta(runs_on, cx)))
                .child(div().flex_1())
                .when(!running, |line| {
                    line.child(
                        Button::new("health-now", "Audit now", Variant::Outline)
                            .on_click(cx.listener(|pane, _, _, cx| pane.audit_now(cx))),
                    )
                }),
        )
        .children(latest.map(|latest| digest(&latest, cx)))
        .into_any_element()
}

/// Who an audit runs on, as the board sets Auto's agent, since each audit spends on that model.
fn runs_on(pane: &GoalsPane, cx: &Context<GoalsPane>) -> Option<String> {
    let settings = &pane.board()?.settings;
    let bench = pane.bench.upgrade()?;
    let launchers = bench.read(cx).launchers();
    let shown = (settings.launcher.as_deref())
        .and_then(|id| launchers.iter().find(|known| known.id == id))
        .or(launchers.first())?;
    let model = (shown.model.as_ref()).and_then(|models| {
        let current = settings.model.as_deref().unwrap_or(&models.current);
        (models.choices.iter())
            .find(|choice| choice.value == current)
            .map(|choice| choice.label.clone())
    });
    let mut on = match model {
        Some(model) => format!("on {} · {model}", shown.label),
        None => format!("on {}", shown.label),
    };
    if let Some(provider) = &settings.provider {
        on.push_str(&format!(", paid by {provider}"));
    }
    on.push_str(", as Auto's agent is set on the board");
    Some(on)
}

fn digest(latest: &AuditDigest, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let open: Vec<_> = latest.findings.iter().filter(|found| !found.done).collect();
    let mut counts = Vec::new();
    if latest.since.is_some() {
        counts.push(match latest.commits {
            1 => "1 commit since the audit before".to_owned(),
            commits => format!("{commits} commits since the audit before"),
        });
    }
    counts.push(match latest.findings.len() {
        1 => "1 finding".to_owned(),
        found => format!("{found} findings"),
    });
    if latest.fixed > 0 {
        counts.push(format!("{} fixed from before", latest.fixed));
    }
    let row = |name: &str| {
        div()
            .flex()
            .gap(SPACE_2)
            .py(SPACE_1H)
            .border_t_1()
            .border_color(t.border)
            .child(
                ui::caps(name, cx)
                    .w(px(super::report::LABEL_W))
                    .flex_none()
                    .pt(px(2.)),
            )
    };
    div()
        .flex()
        .flex_col()
        .px(SPACE_3)
        .py(SPACE_1)
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .child(row("Since").border_t_0().child(
            ui::text(counts.join(" · "), TEXT_SM, FontWeight::NORMAL).text_color(t.text_secondary),
        ))
        .child(
            row("Findings").child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(SPACE_0)
                    .when(latest.running && open.is_empty(), |list| {
                        list.child(ui::meta("Still reading…", cx))
                    })
                    .when(!latest.running && latest.findings.is_empty(), |list| {
                        list.child(ui::meta("Nothing worth fixing", cx))
                    })
                    .children(latest.findings.iter().map(|found| {
                        let card = found.card;
                        div()
                            .id(SharedString::from(format!("finding-{card}")))
                            .flex()
                            .items_center()
                            .gap(SPACE_1)
                            .min_w_0()
                            .cursor_pointer()
                            .hover(|line| line.underline())
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.open_on_board(card, cx)),
                            )
                            .child(
                                ui::text(
                                    format!("#{card} {}", found.title),
                                    TEXT_SM,
                                    FontWeight::NORMAL,
                                )
                                .truncate()
                                .text_color(match found.done {
                                    true => t.text_tertiary,
                                    false => t.text_secondary,
                                }),
                            )
                            .when(found.done, |line| line.child(ui::meta("· fixed", cx)))
                    })),
            ),
        )
        .children(latest.summary.clone().map(|summary| {
            row("Overall").child(
                ui::text(summary, TEXT_SM, FontWeight::NORMAL)
                    .flex_1()
                    .min_w_0()
                    .text_color(t.text_secondary),
            )
        }))
        .into_any_element()
}
