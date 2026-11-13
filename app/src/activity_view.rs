//! The activity overview and calendar, under a chat's welcome and in
//! Settings.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Datelike, NaiveDate};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, point, px, quad, relative, size, App, BorderStyle, Bounds, Div, Entity,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Pixels,
    Point, StatefulInteractiveElement, Styled,
};
use sikemux_calls::activity_log::ActivityDay;
use sikemux_calls::activity_log::ActivityLevels;
use sikemux_calls::activity_log::ActivityShare;
use sikemux_calls::activity_log::ActivityStreaks;
use sikemux_calls::activity_log::ActivitySummary;
use sikemux_calls::activity_log::ActivityTotals;

use crate::home::METRICS;
use crate::theme::{
    Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_3XS, TEXT_XL, TEXT_XS,
};
use crate::ui::numbers::{compact, money, whole};
use crate::ui::Toggle;
use crate::workbench::Workbench;
use crate::{theme, ui};

/// What the activity calendar shows: the measure picked above it, and the day under the pointer,
/// counted from the Unix epoch.
#[derive(Default)]
pub struct Calendar {
    pub metric: Metric,
    pub day: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Metric {
    #[default]
    Tokens,
    Cost,
    Commits,
    AgentTime,
    Sessions,
}

impl Metric {
    fn of_day(self, day: &ActivityDay) -> f64 {
        match self {
            Metric::Tokens => day.tokens as f64,
            Metric::Cost => day.cost_usd,
            Metric::Commits => day.commits as f64,
            Metric::AgentTime => day.agent_ms as f64,
            Metric::Sessions => day.sessions as f64,
        }
    }

    /// How full the calendar draws a day, from 0 to 4.
    fn level(self, levels: &ActivityLevels) -> u8 {
        match self {
            Metric::Tokens => levels.tokens,
            Metric::Cost => levels.cost_usd,
            Metric::Commits => levels.commits,
            Metric::AgentTime => levels.agent_ms,
            Metric::Sessions => levels.sessions,
        }
    }

    pub fn of_share(self, share: &ActivityShare) -> f64 {
        match self {
            Metric::Tokens => share.tokens as f64,
            Metric::Cost => share.cost_usd,
            Metric::Commits => share.commits as f64,
            Metric::AgentTime => share.agent_ms as f64,
            Metric::Sessions => share.sessions as f64,
        }
    }
}

pub(crate) fn is_empty(totals: &ActivityTotals) -> bool {
    totals.sessions == 0 && totals.turns == 0 && totals.commits == 0
}

/// Tokens the agents read and wrote, leaving out what they read again from cache.
pub(crate) fn burnt(totals: &ActivityTotals) -> f64 {
    (totals.input + totals.cache_write + totals.output) as f64
}

pub(crate) fn overview(totals: &ActivityTotals, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let tokens = burnt(totals);
    let cost = totals.cost_usd;
    let quiet = t.text_tertiary;
    let stats = [
        (
            t!("Agent time"),
            None,
            crate::ui::time::total(totals.agent_ms as f64),
            vec![(turns(totals.turns as f64), quiet)],
        ),
        (
            t!("Sessions"),
            None,
            whole(totals.sessions as f64),
            vec![(
                t!("{count} resumed", count = whole(totals.resumed as f64)),
                quiet,
            )],
        ),
        (
            t!("Tokens"),
            None,
            compact(tokens),
            vec![
                (
                    t!("{tokens} read", tokens = compact(totals.cache_read as f64)),
                    quiet,
                ),
                (
                    t!(
                        "{tokens} write",
                        tokens = compact(totals.cache_write as f64)
                    ),
                    t.warn,
                ),
            ],
        ),
        (
            t!("Cost"),
            Some(t!("API est.")),
            if cost > 0. { money(cost) } else { "—".into() },
            vec![match cost > 0. {
                true => (
                    t!(
                        "{cost} on writes",
                        cost = format!("${}", whole(totals.costs.cache_write))
                    ),
                    quiet,
                ),
                false => (t!("not priced yet").into(), quiet),
            }],
        ),
        (
            t!("Commits"),
            None,
            whole(totals.commits as f64),
            vec![(
                t!(
                    "{count} by agents",
                    count = whole(totals.agent_commits as f64)
                ),
                quiet,
            )],
        ),
    ];
    div().flex().children(stats.into_iter().enumerate().map(
        |(index, (label, badge, value, detail))| {
            div()
                .flex_auto()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .py(SPACE_1)
                .px(SPACE_3)
                .when(index == 0, |stat| stat.pl_0())
                .when(index > 0, |stat| stat.border_l_1().border_color(t.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .text_color(t.text_tertiary)
                        .child(ui::text(label, TEXT_XS, FontWeight::MEDIUM))
                        .children(badge.map(|badge| ui::text(badge, TEXT_3XS, FontWeight::MEDIUM))),
                )
                .child(
                    ui::text(value, TEXT_XL, FontWeight::SEMIBOLD)
                        .line_height(crate::theme::ui_text(22.))
                        .font_features(theme::tabular())
                        .text_color(t.text_primary),
                )
                .child(div().children(detail.into_iter().map(|(line, ink)| {
                    ui::mono(line, TEXT_2XS, cx)
                        .whitespace_nowrap()
                        .text_color(ink)
                })))
        },
    ))
}

/// The calendar of a year's days, coloured by the measure picked above it.
pub(crate) fn calendar(
    summary: &ActivitySummary,
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let measure = bench.calendar.metric;
    let today = (chrono::Local::now().date_naive() - epoch()).num_days();
    let start = today - (today + 3).rem_euclid(7) - (WEEKS - 1) * 7;
    let days: HashMap<i64, &ActivityDay> =
        (summary.days.iter()).map(|day| (day.day, day)).collect();
    let level = |day: i64| days.get(&day).map_or(0, |day| measure.level(&day.levels));
    let hovered = bench.calendar.day.filter(|day| *day <= today);
    let colour = |level: u8| match level {
        0 => Hsla {
            a: 0.07,
            ..t.text_primary
        },
        1 => Hsla {
            a: 0.30,
            ..t.accent
        },
        2 => Hsla {
            a: 0.52,
            ..t.accent
        },
        3 => Hsla {
            a: 0.76,
            ..t.accent
        },
        _ => t.accent,
    };
    let fills: Vec<Option<Hsla>> = (0..WEEKS * 7)
        .map(|at| (start + at <= today).then(|| colour(level(start + at))))
        .collect();
    let lit = hovered.map(|day| day - start);
    let ring = t.text_primary;
    let placed = Rc::new(Cell::new(Bounds::default()));
    let grid = {
        let placed = placed.clone();
        canvas(
            move |bounds, _, _| placed.set(bounds),
            move |bounds, _, window, _| {
                let side = day_side(bounds);
                for (at, fill) in fills.iter().enumerate() {
                    let Some(fill) = fill else {
                        continue;
                    };
                    let border = if lit == Some(at as i64) { 1. } else { 0. };
                    window.paint_quad(quad(
                        day_bounds(bounds, side, at as i64),
                        px(2.),
                        *fill,
                        px(border),
                        ring,
                        BorderStyle::Solid,
                    ));
                }
            },
        )
        .size_full()
    };
    let grid = div()
        .id("activity-days")
        .w_full()
        .aspect_ratio(GRID_RATIO)
        .on_mouse_move({
            let this = this.clone();
            move |event: &MouseMoveEvent, _, cx| {
                let day = day_at(placed.get(), event.position)
                    .map(|at| start + at)
                    .filter(|day| *day <= today);
                this.update(cx, |bench, cx| {
                    if bench.calendar.day != day {
                        bench.calendar.day = day;
                        bench.parts.redraw_stage(cx);
                    }
                })
            }
        })
        .on_hover({
            let this = this.clone();
            move |over: &bool, _, cx| {
                this.update(cx, |bench, cx| {
                    if !*over && bench.calendar.day.take().is_some() {
                        bench.parts.redraw_stage(cx);
                    }
                })
            }
        })
        .child(grid);
    let mut last_month = None;
    let mut last_label = -MONTH_LABEL_GAP;
    let months: Vec<Div> = (0..WEEKS)
        .filter_map(|week| {
            let first = epoch() + chrono::Duration::days(start + week * 7);
            let label = (last_month != Some(first.month())
                && week <= WEEKS - 3
                && week - last_label >= MONTH_LABEL_GAP)
                .then(|| first.format("%b").to_string());
            last_month = Some(first.month());
            if label.is_some() {
                last_label = week;
            }
            label.map(|label| {
                small(label, &t)
                    .absolute()
                    .bottom_0()
                    .left(relative(week as f32 / WEEKS as f32))
            })
        })
        .collect();
    let streaks = &summary.streaks;
    let toggles: Vec<_> = METRICS
        .iter()
        .enumerate()
        .map(|(index, (shown, label))| {
            let (this, shown) = (this.clone(), *shown);
            Toggle::new(("metric", index), crate::i18n::tr(label), shown == measure).on_click(
                move |_, _, cx| {
                    this.update(cx, |bench, cx| {
                        bench.calendar.metric = shown;
                        bench.parts.redraw_stage(cx);
                    })
                },
            )
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(
            div()
                .min_h(px(24.))
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(SPACE_3)
                .child(
                    ui::text(
                        match hovered {
                            Some(day) => format!(
                                "{} · {}",
                                long_date(day),
                                metric_text(
                                    measure,
                                    days.get(&day).map_or(0., |day| measure.of_day(day))
                                )
                            ),
                            None => readout(streaks),
                        },
                        TEXT_XS,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_secondary),
                )
                .child(div().flex().gap(SPACE_1).children(toggles)),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_2)
                .child(
                    div().relative().w(px(22.)).flex_none().child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .pt(px(14. + CELL_GAP))
                            .gap(px(CELL_GAP))
                            .children([t!("Mon"), "", t!("Wed"), "", t!("Fri"), "", ""].map(
                                |label| {
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .flex()
                                        .items_center()
                                        .child(small(label.into(), &t))
                                },
                            )),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(CELL_GAP))
                        .child(div().h(px(14.)).relative().children(months))
                        .child(grid),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(CELL_GAP))
                .child(small(t!("Less").into(), &t).mr(SPACE_1))
                .children((0..5).map(|level| {
                    div()
                        .size(px(10.))
                        .rounded(crate::theme::corner(px(2.)))
                        .bg(colour(level))
                }))
                .child(small(t!("More").into(), &t).ml(SPACE_1)),
        )
}

pub(crate) fn day_side(bounds: Bounds<Pixels>) -> Pixels {
    let gap = px(CELL_GAP);
    let across = (bounds.size.width - gap * (WEEKS - 1) as f32) / WEEKS as f32;
    let down = (bounds.size.height - gap * 6.) / 7.;
    across.min(down)
}

/// Where the day `at`, counted down each week's column from the first, is drawn.
pub(crate) fn day_bounds(bounds: Bounds<Pixels>, side: Pixels, at: i64) -> Bounds<Pixels> {
    let step = side + px(CELL_GAP);
    let origin = point(
        bounds.left() + step * (at / 7) as f32,
        bounds.top() + step * (at % 7) as f32,
    );
    Bounds::new(origin, size(side, side))
}

/// The day under `position`, counted as [`day_bounds`] counts them.
pub(crate) fn day_at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Option<i64> {
    if !bounds.contains(&position) {
        return None;
    }
    let step = day_side(bounds) + px(CELL_GAP);
    let week = ((position.x - bounds.left()) / step).floor() as i64;
    let weekday = ((position.y - bounds.top()) / step).floor() as i64;
    (week < WEEKS && weekday < 7).then_some(week * 7 + weekday)
}

pub(crate) fn small(label: String, t: &Tokens) -> Div {
    ui::text(label, TEXT_3XS, FontWeight::NORMAL)
        .line_height(crate::theme::ui_text(TEXT_3XS))
        .text_color(t.text_tertiary)
}

pub(crate) fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default()
}

/// A day counted from 1970 as the calendar reads it out, such as "Thu, Oct 22, 2026".
pub(crate) fn long_date(day: i64) -> String {
    (epoch() + chrono::Duration::days(day))
        .format("%a, %b %-d, %Y")
        .to_string()
}

pub(crate) fn readout(streaks: &ActivityStreaks) -> String {
    let active = streaks.active_days as f64;
    if active == 0. {
        return t!("No activity yet").into();
    }
    let current = streaks.current as f64;
    let active = match active == 1. {
        true => t!("1 active day").to_owned(),
        false => t!("{count} active days", count = whole(active)),
    };
    let longest = match streaks.longest == 1 {
        true => t!("1 day").to_owned(),
        false => t!("{count} days", count = whole(streaks.longest as f64)),
    };
    let mut readout = format!("{active} · {}", t!("longest streak {days}", days = longest));
    if current > 1. {
        readout.push_str(" · ");
        readout.push_str(&t!("{count} days running", count = current));
    }
    readout
}

fn turns(count: f64) -> String {
    match count == 1. {
        true => t!("1 turn").to_owned(),
        false => t!("{count} turns", count = whole(count)),
    }
}

/// A day's measure as the calendar reads it out.
pub(crate) fn metric_text(measure: Metric, value: f64) -> String {
    match (measure, value > 0.) {
        (Metric::AgentTime, true) => {
            t!("{time} of agent time", time = crate::ui::time::total(value))
        }
        (Metric::AgentTime, false) => t!("No agent time").into(),
        (Metric::Tokens, true) => t!("{count} tokens", count = compact(value)),
        (Metric::Tokens, false) => t!("No tokens").into(),
        (Metric::Sessions, true) if value == 1. => t!("1 session").into(),
        (Metric::Sessions, true) => t!("{count} sessions", count = whole(value)),
        (Metric::Sessions, false) => t!("No sessions").into(),
        (Metric::Cost, true) => t!("{cost} at API prices", cost = money(value)),
        (Metric::Cost, false) => t!("No cost").into(),
        (Metric::Commits, true) if value == 1. => t!("1 commit").into(),
        (Metric::Commits, true) => t!("{count} commits", count = whole(value)),
        (Metric::Commits, false) => t!("No commits").into(),
    }
}

pub(crate) const WEEKS: i64 = 53;

pub(crate) const CELL_GAP: f32 = 3.;

/// The fewest weeks between two month names over the calendar, so a month that starts a
/// week after the last one's name drops its own rather than running into it.
pub(crate) const MONTH_LABEL_GAP: i64 = 3;

/// How wide the calendar is for its height, with square days of about 11px.
pub(crate) const GRID_RATIO: f32 =
    (WEEKS as f32 * 11. + (WEEKS - 1) as f32 * CELL_GAP) / (7. * 11. + 6. * CELL_GAP);
