use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Datelike, NaiveDate};
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, point, px, quad, relative, size, Anchor, AnyElement, App, BorderStyle, Bounds,
    Context, Div, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window,
};
use serde_json::Value;
use sikemux_wire::protocol::AgentStatus;

use crate::agent_menu::AgentMenu;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui::{self, Button, IconButton, PaneHeader, Segmented, Toggle, Variant};
use crate::workbench::{AgentRow, Workbench};
use crate::{chat, terminal};

const WEEKS: i64 = 53;
const CELL_GAP: f32 = 3.;

/// The calendar's metrics, as the activity summary names them, in the order its buttons show.
pub const METRICS: [(&str, &str); 5] = [
    ("tokens", "Tokens"),
    ("costUsd", "Cost"),
    ("commits", "Commits"),
    ("agentMs", "Agent time"),
    ("sessions", "Sessions"),
];

/// The stage: the tab in front, its views side by side, or what a project with nothing
/// open says.
pub fn stage(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    if bench.settings.is_some() {
        return crate::settings::view(bench, window, cx);
    }
    if bench
        .strip()
        .is_some_and(|strip| bench.tabs(&strip).is_empty())
    {
        return div()
            .flex_1()
            .min_w_0()
            .child(ui::pane(cx).child(empty_stage(cx)))
            .into_any_element();
    }
    let Some(tab) = bench.tab() else {
        return div().flex_1().min_w_0().into_any_element();
    };
    let shown = crate::track::stage(bench, tab, window, cx);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .child(shown)
        .into_any_element()
}

/// One agent's view of a tab: its terminal or its chat under its header, or what a new
/// chat opens on.
pub fn agent_view(
    bench: &Workbench,
    tab: &str,
    view: &str,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let project = bench.project();
    let agent = project.and_then(|project| {
        bench
            .agents_of(&project.id)
            .into_iter()
            .find(|row| row.id == agent)
    });
    let header = agent
        .as_ref()
        .filter(|_| project.is_some())
        .map(|agent| pane_header(bench, agent, tab, view, cx));
    if let Some(session) = agent
        .as_ref()
        .and_then(|agent| bench.agent_terminal(&agent.id))
    {
        return ui::pane(cx)
            .p(px(ui::AGENT_INSET))
            .child(div().size_full().pt(px(40.)).child(terminal::pane(
                bench,
                &cx.entity(),
                &crate::hosts::Place::This,
                &session.to_string(),
                cx,
            )))
            .children(header)
            .into_any_element();
    }
    if let Some((agent, chat)) = agent
        .as_ref()
        .and_then(|agent| Some((agent, bench.chats.get(&agent.id)?)))
    {
        let resumes = bench.read_only(&agent.id).is_none()
            && bench.shown().is_some_and(|view| {
                view.workspace
                    .agents
                    .iter()
                    .any(|saved| saved.id == agent.id && saved.resume_id.is_some())
            });
        let welcome = chat.is_empty(cx).then(|| {
            chat::connection_note(agent, chat, resumes, cx)
                .unwrap_or_else(|| welcome(bench, Some(agent), cx).into_any_element())
        });
        return ui::pane(cx)
            .p(px(ui::AGENT_INSET))
            .child(chat::pane(bench, agent, chat, welcome, cx))
            .children(header)
            .into_any_element();
    }
    ui::pane(cx)
        .p(px(ui::AGENT_INSET))
        .child(centred(welcome(bench, agent.as_ref(), cx)).size_full())
        .children(header)
        .into_any_element()
}

/// A project with every tab closed, which would otherwise show nothing to click.
fn empty_stage(cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let hint = |id: &str| crate::keymap::hint(id, cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .text_color(t.text_tertiary)
        .child(
            ui::text("Nothing open in this project", 12.5, FontWeight::NORMAL)
                .text_color(t.text_secondary),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::new("empty-new-agent", "New agent", Variant::Outline)
                        .keys(hint("agent.new"))
                        .on_click(cx.listener(|bench, _, window, cx| bench.new_chat(window, cx))),
                )
                .child(
                    Button::new("empty-new-terminal", "New terminal", Variant::Outline)
                        .keys(hint("terminal.new"))
                        .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    Button::new("empty-open-file", "Open file", Variant::Outline)
                        .keys(hint("palette.files"))
                        .on_click(cx.listener(|bench, _, window, cx| {
                            bench.toggle_finder(window, cx);
                            cx.notify();
                        })),
                ),
        )
}

/// What a chat opens on before its first message: the agent's face beside the tokens burnt in
/// the project, over the calendar of them.
pub fn welcome(
    bench: &Workbench,
    agent: Option<&AgentRow>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    div()
        .w_full()
        .max_w(px(756.))
        .mx(px(22.))
        .flex()
        .flex_col()
        .gap(px(24.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(div().flex().child(match agent {
                    Some(agent) => ui::agent_mark(agent, 40., cx),
                    None => ui::mark("claude", 40.),
                }))
                .child(div().flex_1().min_w_0().child(bench.parts.burn.clone())),
        )
        .child(div().w_full().child(bench.parts.activity.clone()))
}

/// The welcome in the middle of the space it is given, across and down.
pub fn centred(welcome: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .justify_center()
        .child(div().w_full().flex().justify_center().child(welcome))
}

/// A piece of the welcome drawn from the project's activity: a view of its own, drawn again
/// when the workbench changes but not when a key is typed in the composer beside it.
pub struct Activity {
    bench: WeakEntity<Workbench>,
    draw: fn(&Workbench, &mut Context<Workbench>) -> AnyElement,
    _bench: Subscription,
}

impl Activity {
    /// The calendar of the tokens burnt each day.
    pub fn calendar(bench: &Entity<Workbench>, cx: &mut Context<Self>) -> Self {
        Self::new(bench, welcome_calendar, cx)
    }

    /// The tokens burnt in the project, or on the day the pointer is over.
    pub fn burn(bench: &Entity<Workbench>, cx: &mut Context<Self>) -> Self {
        Self::new(bench, burn, cx)
    }

    fn new(
        bench: &Entity<Workbench>,
        draw: fn(&Workbench, &mut Context<Workbench>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            bench: bench.downgrade(),
            draw,
            _bench: cx.observe(bench, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for Activity {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        self.bench
            .update(cx, |bench, cx| draw(bench, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

fn welcome_calendar(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(summary) = bench.read("activity") else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let more = div()
        .id("welcome-more")
        .group("welcome-more")
        .py(px(2.))
        .flex()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .text_size(theme::ui_text(11.))
        .text_color(t.ink_faint)
        .hover(|more| more.text_color(t.text_primary))
        .child("View activity")
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(t.ink_faint)
                .group_hover("welcome-more", move |icon| icon.text_color(t.text_primary)),
        )
        .on_click(cx.listener(|bench, _, window, cx| {
            bench.open_settings(crate::settings::Page::Activity, window, cx)
        }));
    calendar(summary, bench, Some(more.into_any_element()), cx).into_any_element()
}

fn burn(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let summary = bench.read("activity");
    let hovered = bench.metric_day;
    let tokens = match hovered {
        Some(day) => Some(
            summary
                .and_then(|summary| summary["days"].as_array())
                .and_then(|days| days.iter().find(|entry| entry["day"].as_i64() == Some(day)))
                .map_or(0., |entry| number(&entry["tokens"])),
        ),
        None => summary.map(|summary| burnt(&summary["totals"])),
    };
    let label = match hovered {
        Some(day) => div().child(format!("tokens on {}", long_date(day))),
        None => div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child("tokens burnt in")
            .child(ui::icon(Icon::Folder, 12.).text_color(t.live))
            .child(
                div().text_color(t.text_primary).child(
                    bench
                        .project()
                        .map(|project| project.name.clone())
                        .unwrap_or_default(),
                ),
            ),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .min_w_0()
        .child(
            ui::text(
                tokens.map_or_else(|| "—".to_owned(), compact),
                28.,
                FontWeight::SEMIBOLD,
            )
            .line_height(theme::ui_text(28. * 1.1))
            .letter_spacing(-0.028)
            .font_features(theme::tabular())
            .text_color(t.text_primary),
        )
        .child(
            label
                .text_size(theme::ui_text(12.))
                .font_features(theme::tabular())
                .text_color(t.ink_faint),
        )
        .into_any_element()
}

fn pane_header(
    bench: &Workbench,
    agent: &AgentRow,
    tab: &str,
    view: &str,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let menu = AgentMenu::new(bench, &agent.id, cx);
    let more = KitButton::new("agent-more")
        .ghost()
        .compact()
        .rounded_full()
        .w(px(24.))
        .h(px(24.))
        .tooltip("More")
        .child(ui::icon(Icon::More, 14.).text_color(t.text_secondary))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, _, _| match &menu {
            Some(menu) => menu.build(popup),
            None => popup,
        });
    let desk_open = bench.desk_of(tab, &agent.id).is_some();
    let (tab, view, agent_id) = (tab.to_owned(), view.to_owned(), agent.id.clone());
    let views = vec![
        (Icon::Agent, SharedString::from("GUI")),
        (Icon::Terminal, SharedString::from("TUI")),
    ];
    let in_terminal = bench.agent_terminal(&agent.id).is_some();
    let working = matches!(agent.status, AgentStatus::Working | AgentStatus::Blocked);
    let has_chat = bench.has_chat(&agent.provider);
    let switched = agent.id.clone();
    let shared = agent.id.clone();
    let open_share = cx.listener(move |bench, _, window, cx| bench.open_share(&shared, window, cx));
    let share = match bench.share_of(&agent.id) {
        Some(share) => {
            let viewers = share.viewers();
            div()
                .id("agent-share")
                .h(px(24.))
                .min_w(px(24.))
                .px(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .rounded_full()
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.live)
                .tooltip(ui::tooltip(share.watching()))
                .on_click(open_share)
                .child(ui::icon(Icon::Eye, 13.).text_color(t.live))
                .when(viewers > 0, |button| button.child(viewers.to_string()))
                .into_any_element()
        }
        None => IconButton::new("agent-share", Icon::Share, "Share this chat read-only")
            .round()
            .on_click(open_share)
            .into_any_element(),
    };
    let id = agent.id.clone();
    let editing = bench
        .retitling
        .as_ref()
        .filter(|retitling| retitling.agent == agent.id)
        .map(|retitling| {
            div()
                .key_context("Retitle")
                .min_w(px(160.))
                .on_action(
                    cx.listener(|bench, _: &crate::keymap::CancelRetitle, _, cx| {
                        bench.finish_retitle(false, cx)
                    }),
                )
                .child(
                    Input::new(&retitling.input)
                        .appearance(false)
                        .p_0()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary),
                )
        });
    PaneHeader::new(agent.title.clone())
        .title_view(editing)
        .on_title_double_click(
            cx.listener(move |bench, _, window, cx| bench.retitle(&id, window, cx)),
        )
        .mark(match agent.face.is_some() {
            true => {
                let picked = agent.id.clone();
                div()
                    .id("agent-buddy")
                    .cursor_pointer()
                    .tooltip(ui::tooltip("Change buddy"))
                    .child(ui::agent_mark(agent, 16., cx))
                    .on_click(
                        cx.listener(move |bench, _, _, cx| bench.open_buddy_picker(&picked, cx)),
                    )
                    .into_any_element()
            }
            false => ui::mark(&agent.provider, 16.).into_any_element(),
        })
        .name(
            agent
                .name
                .as_ref()
                .map(|name| SharedString::from(format!("@{name}"))),
        )
        .when_some(
            in_terminal
                .then(|| crate::worktree_head::badges(bench, &agent.id, cx))
                .flatten(),
            |header, badges| header.action(badges),
        )
        .when(has_chat, |header| header.action(share))
        .action(more)
        .controls(
            div().flex().items_center().gap(px(4.)).child(
                Segmented::new("agent-view", views, usize::from(in_terminal))
                    .pill()
                    .disabled_because(match (has_chat, working) {
                        (false, _) => Some("This agent has no built-in chat".to_owned()),
                        (true, true) => {
                            let other = if in_terminal { "GUI" } else { "TUI" };
                            Some(format!(
                                "Wait for the agent to finish before opening {other}"
                            ))
                        }
                        (true, false) => None,
                    })
                    .on_select(cx.listener(move |bench, picked: &usize, _, _| {
                        let terminal = *picked == 1;
                        if terminal != in_terminal {
                            bench.switch_view(&switched, terminal);
                        }
                    })),
            ),
        )
        .side(
            IconButton::new(
                "agent-desk",
                Icon::PanelRight,
                if desk_open {
                    "Close the desk"
                } else {
                    "Open the desk"
                },
            )
            .glyph(13.)
            .on_click(cx.listener(move |bench, _, _, _| bench.toggle_desk(&tab, &view, &agent_id))),
        )
}

pub(crate) fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.)
}

pub(crate) fn is_empty(totals: &Value) -> bool {
    number(&totals["sessions"]) == 0.
        && number(&totals["turns"]) == 0.
        && number(&totals["commits"]) == 0.
}

/// Tokens the agents read and wrote, leaving out what they read again from cache.
fn burnt(totals: &Value) -> f64 {
    number(&totals["input"]) + number(&totals["cacheWrite"]) + number(&totals["output"])
}

pub(crate) fn overview(totals: &Value, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let tokens = burnt(totals);
    let cost = number(&totals["costUsd"]);
    let saved = number(&totals["cacheSavingsUsd"]);
    let stats = [
        (
            "Agent time",
            None,
            duration(number(&totals["agentMs"])),
            plural(number(&totals["turns"]), "turn"),
        ),
        (
            "Sessions",
            None,
            whole(number(&totals["sessions"])),
            format!("{} resumed", whole(number(&totals["resumed"]))),
        ),
        (
            "Tokens",
            None,
            compact(tokens),
            format!("{} cached", compact(number(&totals["cacheRead"]))),
        ),
        (
            "Cost",
            Some("API est."),
            if cost > 0. { money(cost) } else { "—".into() },
            if saved > 0. {
                format!("{} saved", money(saved))
            } else {
                "not priced yet".into()
            },
        ),
        (
            "Commits",
            None,
            whole(number(&totals["commits"])),
            format!("{} by agents", whole(number(&totals["agentCommits"]))),
        ),
    ];
    div().flex().children(stats.into_iter().enumerate().map(
        |(index, (label, badge, value, detail))| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .py(px(4.))
                .px(px(16.))
                .when(index == 0, |stat| stat.pl_0())
                .when(index > 0, |stat| stat.border_l_1().border_color(t.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(t.text_tertiary)
                        .child(ui::text(label, 12., FontWeight::MEDIUM))
                        .children(badge.map(|badge| ui::text(badge, 10., FontWeight::MEDIUM))),
                )
                .child(
                    ui::text(value, 20., FontWeight::SEMIBOLD)
                        .line_height(crate::theme::ui_text(22.))
                        .font_features(theme::tabular())
                        .text_color(t.text_primary),
                )
                .child(ui::mono(detail, 11., cx).whitespace_normal())
        },
    ))
}

/// The calendar of a year's days, coloured by the measure picked above it. With a `footer`, as
/// under a chat's welcome, it shows only tokens, has no readout or measure choice, and the
/// footer leads the legend's row.
pub(crate) fn calendar(
    summary: &Value,
    bench: &Workbench,
    footer: Option<AnyElement>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let metric = bench.metric;
    let welcome = footer.is_some();
    let key = if welcome { "tokens" } else { METRICS[metric].0 };
    let today = (chrono::Local::now().date_naive() - epoch()).num_days();
    let start = today - (today + 3).rem_euclid(7) - (WEEKS - 1) * 7;
    let days: HashMap<i64, &Value> = summary["days"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|day| (day["day"].as_i64().unwrap_or(0), day))
        .collect();
    let level = |day: i64| {
        days.get(&day)
            .and_then(|day| day["levels"][key].as_u64())
            .unwrap_or(0)
    };
    let hovered = bench.metric_day.filter(|day| *day <= today);
    let colour = |level: u64| match level {
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
        .on_mouse_move(cx.listener(move |bench, event: &MouseMoveEvent, _, cx| {
            let day = day_at(placed.get(), event.position)
                .map(|at| start + at)
                .filter(|day| *day <= today);
            if bench.metric_day != day {
                bench.metric_day = day;
                bench.parts.redraw_activity(cx);
            }
        }))
        .on_hover(cx.listener(|bench, over: &bool, _, cx| {
            if !*over && bench.metric_day.take().is_some() {
                bench.parts.redraw_activity(cx);
            }
        }))
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
    let streaks = &summary["streaks"];
    let toggles: Vec<_> = METRICS
        .iter()
        .enumerate()
        .map(|(index, (_, label))| {
            Toggle::new(("metric", index), *label, index == metric).on_click(cx.listener(
                move |bench, _, _, cx| {
                    bench.metric = index;
                    bench.parts.redraw_activity(cx);
                },
            ))
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .when(!welcome, |calendar| {
            calendar.child(
                div()
                    .min_h(px(24.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        ui::text(
                            match hovered {
                                Some(day) => format!(
                                    "{} · {}",
                                    long_date(day),
                                    metric_text(
                                        key,
                                        days.get(&day).map_or(0., |day| number(&day[key]))
                                    )
                                ),
                                None => readout(streaks),
                            },
                            12.,
                            FontWeight::NORMAL,
                        )
                        .text_color(t.text_secondary),
                    )
                    .child(div().flex().gap(px(4.)).children(toggles)),
            )
        })
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    div().relative().w(px(22.)).flex_none().child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .pt(px(14. + CELL_GAP))
                            .gap(px(CELL_GAP))
                            .children(["Mon", "", "Wed", "", "Fri", "", ""].map(|label| {
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .items_center()
                                    .child(small(label.into(), &t))
                            })),
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
                .children(footer.map(|footer| div().flex_1().flex().child(footer)))
                .child(small("Less".into(), &t).mr(px(4.)))
                .children((0..5).map(|level| div().size(px(10.)).rounded(px(2.)).bg(colour(level))))
                .child(small("More".into(), &t).ml(px(4.))),
        )
}

/// The fewest weeks between two month names over the calendar, so a month that starts a
/// week after the last one's name drops its own rather than running into it.
const MONTH_LABEL_GAP: i64 = 3;

/// How wide the calendar is for its height, with square days of about 11px.
const GRID_RATIO: f32 =
    (WEEKS as f32 * 11. + (WEEKS - 1) as f32 * CELL_GAP) / (7. * 11. + 6. * CELL_GAP);

/// The side of one day's square in a calendar drawn in `bounds`.
fn day_side(bounds: Bounds<Pixels>) -> Pixels {
    let gap = px(CELL_GAP);
    let across = (bounds.size.width - gap * (WEEKS - 1) as f32) / WEEKS as f32;
    let down = (bounds.size.height - gap * 6.) / 7.;
    across.min(down)
}

/// Where the day `at`, counted down each week's column from the first, is drawn.
fn day_bounds(bounds: Bounds<Pixels>, side: Pixels, at: i64) -> Bounds<Pixels> {
    let step = side + px(CELL_GAP);
    let origin = point(
        bounds.left() + step * (at / 7) as f32,
        bounds.top() + step * (at % 7) as f32,
    );
    Bounds::new(origin, size(side, side))
}

/// The day under `position`, counted as [`day_bounds`] counts them.
fn day_at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Option<i64> {
    if !bounds.contains(&position) {
        return None;
    }
    let step = day_side(bounds) + px(CELL_GAP);
    let week = ((position.x - bounds.left()) / step).floor() as i64;
    let weekday = ((position.y - bounds.top()) / step).floor() as i64;
    (week < WEEKS && weekday < 7).then_some(week * 7 + weekday)
}

fn small(label: String, t: &Tokens) -> Div {
    ui::text(label, 10., FontWeight::NORMAL)
        .line_height(crate::theme::ui_text(10.))
        .text_color(t.text_tertiary)
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default()
}

/// A day counted from 1970 as the calendar reads it out, such as "Thu, Oct 22, 2026".
fn long_date(day: i64) -> String {
    (epoch() + chrono::Duration::days(day))
        .format("%a, %b %-d, %Y")
        .to_string()
}

fn readout(streaks: &Value) -> String {
    let active = number(&streaks["activeDays"]);
    if active == 0. {
        return "No activity yet".into();
    }
    let current = number(&streaks["current"]);
    let mut readout = format!(
        "{} · longest streak {}",
        plural(active, "active day"),
        plural(number(&streaks["longest"]), "day")
    );
    if current > 1. {
        readout.push_str(&format!(" · {current} days running"));
    }
    readout
}

/// A day's measure as the calendar reads it out.
fn metric_text(key: &str, value: f64) -> String {
    match (key, value > 0.) {
        ("agentMs", true) => format!("{} of agent time", duration(value)),
        ("agentMs", false) => "No agent time".into(),
        ("tokens", true) => format!("{} tokens", compact(value)),
        ("tokens", false) => "No tokens".into(),
        ("sessions", true) => plural(value, "session"),
        ("sessions", false) => "No sessions".into(),
        ("costUsd", true) => format!("{} at API prices", money(value)),
        ("costUsd", false) => "No cost".into(),
        (_, true) => plural(value, "commit"),
        (_, false) => "No commits".into(),
    }
}

pub(crate) fn whole(value: f64) -> String {
    let digits = format!("{}", value.round() as i64);
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 && digit != '-' {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

pub(crate) fn plural(count: f64, word: &str) -> String {
    format!(
        "{} {word}{}",
        whole(count),
        if count == 1. { "" } else { "s" }
    )
}

pub fn compact(value: f64) -> String {
    let (scaled, unit) = match value {
        v if v >= 1e9 => (v / 1e9, "B"),
        v if v >= 1e6 => (v / 1e6, "M"),
        v if v >= 1e3 => (v / 1e3, "K"),
        v => (v, ""),
    };
    let rounded = (scaled * 10.).round() / 10.;
    if rounded.fract() == 0. {
        format!("{rounded:.0}{unit}")
    } else {
        format!("{rounded:.1}{unit}")
    }
}

pub fn money(usd: f64) -> String {
    if usd >= 1000. {
        format!("${}", whole(usd))
    } else {
        format!("${usd:.2}")
    }
}

pub(crate) fn duration(ms: f64) -> String {
    let minutes = (ms / 60_000.).round() as i64;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours >= 100 {
        return format!("{}h", whole(hours as f64));
    }
    match minutes % 60 {
        0 => format!("{hours}h"),
        rest => format!("{hours}h {rest}m"),
    }
}
