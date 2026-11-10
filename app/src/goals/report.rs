//! A goal's report at the head of its page: the same rows every time, what waits on the person
//! first, then what got done, what moves, what waits on something else, what comes next and when
//! it should be through. A dot marks what the report before did not say. The core writes the
//! reports on the schedule picked here; this only draws them and steps back through earlier ones.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::report::{GoalReportsParams, ReportEveryParams};
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::call;
use sikemux_view::slice::{Gate, GoalReport, ReportEvery, ReportLine};

use super::goal::step_words;
use super::{wait_words, GoalsPane};
use crate::board::ChangesBoards;
use crate::core;
use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_SM};
use crate::ui::{self, Pick};

/// How many lines a row of the report shows before it counts the rest.
const SHOWN: usize = 4;
/// Wide enough for "Waiting on you" in spaced capitals.
pub(super) const LABEL_W: f32 = 120.;

/// The earlier reports of the goal open, and the one shown, while the person steps back.
pub(super) struct Stepping {
    goal: u64,
    reports: Vec<GoalReport>,
    shown: usize,
}

impl GoalsPane {
    /// Shows the report before the one in front, reading the goal's reports first.
    fn step_back(&mut self, goal: u64, cx: &mut Context<Self>) {
        if let Some(stepping) = self
            .stepping
            .as_mut()
            .filter(|stepping| stepping.goal == goal)
        {
            stepping.shown = stepping.shown.saturating_sub(1);
            cx.notify();
            return;
        }
        let Some(project) = self.here.clone() else {
            return;
        };
        let asked = core::Target::here(&self.asks)
            .request_typed::<call::GoalReports>(GoalReportsParams { project, id: goal });
        self._reading_reports = cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                match answer {
                    Ok(reports) if reports.len() > 1 => {
                        let shown = reports.len() - 2;
                        pane.stepping = Some(Stepping {
                            goal,
                            reports,
                            shown,
                        });
                    }
                    Ok(_) => {}
                    Err(error) => pane.said(&format!("Couldn't read the reports: {error}"), cx),
                }
                cx.notify();
            });
        });
    }

    /// Shows the report after the one in front, back to the latest.
    fn step_on(&mut self, cx: &mut Context<Self>) {
        if let Some(stepping) = self.stepping.as_mut() {
            stepping.shown += 1;
            if stepping.shown + 1 >= stepping.reports.len() {
                self.stepping = None;
            }
        }
        cx.notify();
    }

    fn report_every(&mut self, goal: u64, every: Option<ReportEvery>, cx: &mut Context<Self>) {
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = ReportEveryParams {
            project,
            id: goal,
            every,
        };
        self.stepping = None;
        self.ask_moved::<call::BoardReportEvery>(params, |_| {}, cx);
    }
}

fn every_picks() -> Vec<Pick> {
    vec![
        Pick::new("off", "Off"),
        Pick::new("change", "On change").hint("Whenever a slice moves, lands or waits on you"),
        Pick::new("15", "Every 15 min"),
        Pick::new("30", "Every 30 min"),
        Pick::new("60", "Every hour"),
    ]
}

fn every_choice(every: Option<ReportEvery>) -> (String, String) {
    match every {
        None => ("off".into(), "Off".into()),
        Some(ReportEvery::Change) => ("change".into(), "On change".into()),
        Some(ReportEvery::Minutes { minutes: 60 }) => ("60".into(), "Every hour".into()),
        Some(ReportEvery::Minutes { minutes }) => {
            (minutes.to_string(), format!("Every {minutes} min"))
        }
    }
}

fn every_of(key: &str) -> Option<ReportEvery> {
    match key {
        "off" => None,
        "change" => Some(ReportEvery::Change),
        minutes => minutes
            .parse()
            .ok()
            .map(|minutes| ReportEvery::Minutes { minutes }),
    }
}

/// The report part of goal `goal`'s page: its heading with the schedule, and the report itself
/// once there is one.
pub(super) fn report(
    pane: &GoalsPane,
    goal: &BoardCard,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = goal.id;
    let stepping = (pane.stepping.as_ref()).filter(|stepping| stepping.goal == id);
    let shown = match stepping {
        Some(stepping) => stepping.reports.get(stepping.shown),
        None => goal.report.as_ref(),
    };
    let (key, words) = every_choice(goal.report_every);
    let entity = cx.entity();
    let schedule = ui::select(
        "goal-report-every",
        words,
        every_picks(),
        Some(key.into()),
        pane.busy,
        move |picked, _, cx| {
            let every = every_of(picked.as_ref());
            entity.update(cx, |pane, cx| pane.report_every(id, every, cx));
        },
        cx,
    );
    let earlier = stepping.map_or(
        goal.report
            .as_ref()
            .is_some_and(|report| report.since.is_some()),
        |stepping| stepping.shown > 0,
    );
    let later = stepping.is_some();
    let when = shown.map(|report| {
        let mut when = ui::time::clock(report.at);
        if let (None, Some(ReportEvery::Minutes { minutes })) = (stepping, goal.report_every) {
            when.push_str(&format!(
                " · next {}",
                ui::time::clock(report.at + u64::from(minutes) * 60_000)
            ));
        }
        when
    });
    let head = div()
        .mt(SPACE_3)
        .mb(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(ui::caps("Report", cx))
        .child(div().flex_1().h(px(1.)).bg(t.line))
        .children(when.map(|when| ui::meta(when, cx)))
        .child(schedule)
        .when(earlier || later, |head| {
            head.child(
                div()
                    .flex()
                    .gap(SPACE_0)
                    .child(
                        arrow("goal-report-back", "‹", earlier, cx)
                            .on_click(cx.listener(move |pane, _, _, cx| pane.step_back(id, cx))),
                    )
                    .child(
                        arrow("goal-report-on", "›", later, cx)
                            .on_click(cx.listener(|pane, _, _, cx| pane.step_on(cx))),
                    ),
            )
        });
    div()
        .flex()
        .flex_col()
        .child(head)
        .children(shown.map(|report| panel(pane, report, cx)))
        .into_any_element()
}

fn arrow(
    id: &'static str,
    glyph: &'static str,
    on: bool,
    cx: &Context<GoalsPane>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .px(SPACE_1)
        .rounded(theme::RADIUS_1)
        .text_color(if on {
            t.text_secondary
        } else {
            t.text_disabled
        })
        .when(on, |arrow| {
            arrow
                .cursor_pointer()
                .hover(move |style| style.bg(t.surface_hover))
        })
        .child(ui::text(glyph, TEXT_SM, FontWeight::MEDIUM))
}

fn panel(pane: &GoalsPane, report: &GoalReport, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let needs: Vec<String> = (report.needs_you.iter())
        .map(|line| {
            let gate = gate_name(line.gate, line.environment.as_deref());
            match line.at {
                Some(at) => format!("{gate} · since {}", ui::time::clock(at)),
                None => gate,
            }
        })
        .collect();
    let done: Vec<String> = (report.done.iter())
        .map(|line| {
            let at = line.at.map(ui::time::clock);
            let by = line.owner_name.as_ref().map(|owner| format!("@{owner}"));
            [at, by]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .collect();
    let under_way = |line: &ReportLine| {
        let owner = line.owner_name.as_deref();
        let wait = line.wait.as_ref().and_then(|wait| wait_words(wait, owner));
        match (owner, wait) {
            (_, Some(wait)) => wait,
            (Some(owner), None) => format!("@{owner}"),
            (None, None) => "no owner yet".to_owned(),
        }
    };
    let moving: Vec<String> = report.moving.iter().map(under_way).collect();
    let waiting: Vec<String> = report.waiting.iter().map(under_way).collect();
    let next: Vec<String> = (report.next.iter().take(2))
        .map(|next| {
            let card = pane.card(next.card);
            let title =
                card.map_or_else(|| format!("#{}", next.card), |card| format!("#{}", card.id));
            let owner = card.and_then(|card| card.owner_name.as_deref());
            step_words(next, &title, owner)
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .px(SPACE_3)
        .py(SPACE_1)
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_raised)
        .child(lines_row(
            "Waiting on you",
            &report.needs_you,
            &needs,
            true,
            cx,
        ))
        .child(lines_row("Done", &report.done, &done, false, cx))
        .child(lines_row("Moving", &report.moving, &moving, false, cx))
        .child(lines_row(
            "Waiting on",
            &report.waiting,
            &waiting,
            false,
            cx,
        ))
        .child(text_row("Next", next.join(", then "), None, cx))
        .child(eta_row(report, cx))
        .into_any_element()
}

/// One row of the report: its name in spaced capitals, then what it holds.
fn row(name: &str, you: bool, first: bool, cx: &Context<GoalsPane>) -> gpui_kit::Div {
    let t = Tokens::get(cx).clone();
    let name = ui::caps(name, cx).w(px(LABEL_W)).flex_none().pt(px(2.));
    div()
        .flex()
        .gap(SPACE_2)
        .py(SPACE_1H)
        .when(!first, |row| row.border_t_1().border_color(t.border))
        .child(match you {
            true => name.text_color(t.warn),
            false => name,
        })
}

fn lines_row(
    name: &str,
    lines: &[ReportLine],
    about: &[String],
    you: bool,
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let shown = lines.iter().zip(about).take(SHOWN).map(|(line, about)| {
        div()
            .id(SharedString::from(format!("report-{name}-{}", line.card)))
            .flex()
            .items_center()
            .min_w_0()
            .gap(SPACE_1H)
            .when(line.new, |item| {
                item.child(div().size(px(6.)).flex_none().rounded_full().bg(t.accent))
            })
            .child(
                ui::text(
                    format!("#{} {}", line.card, line.promise),
                    TEXT_SM,
                    FontWeight::NORMAL,
                )
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_secondary),
            )
            .when(!about.is_empty(), |item| {
                item.child(ui::meta(format!("· {about}"), cx).flex_none())
            })
    });
    let more = lines.len().saturating_sub(SHOWN);
    row(name, you && !lines.is_empty(), you, cx)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(SPACE_0)
                .children(shown)
                .when(lines.is_empty(), |list| list.child(ui::meta("Nothing", cx)))
                .when(more > 0, |list| {
                    list.child(ui::meta(format!("and {more} more"), cx))
                }),
        )
        .into_any_element()
}

fn text_row(name: &str, text: String, warn: Option<String>, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let text = match text.is_empty() {
        true => ui::meta("Nothing yet", cx),
        false => ui::text(text, TEXT_SM, FontWeight::NORMAL).text_color(t.text_secondary),
    };
    row(name, false, false, cx)
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(SPACE_1)
                .min_w_0()
                .child(text)
                .children(warn.map(|warn| ui::meta(warn, cx).text_color(t.warn))),
        )
        .into_any_element()
}

/// When the goal should be through, and how much later than the report before said, with why.
fn eta_row(report: &GoalReport, cx: &Context<GoalsPane>) -> AnyElement {
    let Some(eta) = &report.eta else {
        return text_row("ETA", String::new(), None, cx);
    };
    let through = match (eta.at, eta.then_you) {
        (Some(at), true) => format!("{}, then you", ui::time::clock(at)),
        (Some(at), false) => ui::time::clock(at),
        (None, _) => "No estimate".to_owned(),
    };
    let later =
        (eta.at.zip(report.eta_before).zip(report.since)).and_then(|((at, before), since)| {
            let minutes = at.checked_sub(before)? / 60_000;
            (minutes > 0)
                .then(|| format!("{minutes} min later than {} said", ui::time::clock(since)))
        });
    let slip = eta.slip.map(|slip| slip.words());
    let warn = match (later, slip) {
        (Some(later), Some(slip)) => Some(format!("· {later}: {slip}")),
        (Some(later), None) => Some(format!("· {later}")),
        (None, Some(slip)) => Some(format!("· slipped: {slip}")),
        (None, None) => None,
    };
    text_row("ETA", through, warn, cx)
}

fn gate_name(gate: Option<Gate>, environment: Option<&str>) -> String {
    match (environment, gate) {
        (Some(environment), _) => environment.to_owned(),
        (None, Some(Gate::Built)) => "Built".to_owned(),
        (None, Some(Gate::LooksRight)) => "Looks right".to_owned(),
        (None, Some(Gate::Landed)) => "Landed".to_owned(),
        (None, None) => "your call".to_owned(),
    }
}
