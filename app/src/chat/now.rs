//! The Now card floating at the top right of a chief's chat: how many of its project's agents
//! wait on the person, are ready to look at, are moving and stand unsure, set large; then the
//! rows the person acts on, the moving ones as marks, and the agents closed lately with a way to
//! open them again. Folded, it is the four counts alone. The core works out every group and fact;
//! this only words and draws them.

use super::*;
use crate::ui::Clicks;
use gpui_kit::ClickEvent;

use sikemux_view::now::{ClosedWhy, FixAt, NowClosed, NowFact, NowGroup, NowRow, ProjectNow};
use sikemux_view::turn::TurnWaitKind;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_XS, TEXT_XS_H};

const WIDTH: f32 = 356.;
const ROW: f32 = 27.;
const MARK: f32 = 24.;
/// Rows of a group shown before the rest fold into one line.
const SHOWN: usize = 4;
const CLOSED_SHOWN: usize = 3;

/// The card over `chief`'s transcript, or nothing for a chat that is not a chief's or a project
/// with nothing standing.
pub(super) fn card(
    bench: &Workbench,
    chief: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if !chief.chief {
        return None;
    }
    let now = (bench.view_at(chat.target.place())?.now.iter())
        .find(|now| now.project == chief.project)?;
    if now.rows.is_empty() && now.closed.is_empty() {
        return None;
    }
    let counts = Counts::of(now);
    let fold = {
        let agent = chief.id.clone();
        cx.listener(move |bench, _: &ClickEvent, _, cx| {
            if let Some(chat) = bench.chats.open.get_mut(&agent) {
                chat.now_folded = !chat.now_folded;
                cx.notify();
            }
        })
    };
    let t = Tokens::get(cx).clone();
    let face = div()
        .id("now-card")
        .block_mouse_except_scroll()
        .bg(t.surface_raised)
        .border_1()
        .border_color(t.border)
        .shadow_lg();
    let card = match chat.now_folded {
        true => face
            .rounded_full()
            .h(px(34.))
            .pl(px(5.))
            .pr(SPACE_1)
            .flex()
            .items_center()
            .gap(px(2.))
            .clicks(fold)
            .children(counts.pills(cx))
            .child(chevron(false, cx))
            .into_any_element(),
        false => {
            let bench_handle = cx.weak_entity();
            let rows = body(bench, now, &bench_handle, chat, cx);
            face.w(px(WIDTH))
                .rounded(theme::radius_4())
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(head(now, fold, cx))
                .child(counts.stats(cx))
                .child(
                    div()
                        .px(SPACE_1H)
                        .pb(SPACE_2)
                        .flex()
                        .flex_col()
                        .children(rows),
                )
                .into_any_element()
        }
    };
    Some(
        div()
            .absolute()
            .top(SPACE_3)
            .right(px(16.))
            .child(card)
            .into_any_element(),
    )
}

/// The rows of `group` the card counts and lists: a lead's workers stand under their lead,
/// except those waiting on the person, who each ask for themselves.
fn shown(now: &ProjectNow, group: NowGroup) -> impl Iterator<Item = &NowRow> {
    (now.rows.iter())
        .filter(move |row| row.group == group)
        .filter(move |row| row.lead.is_none() || group == NowGroup::WaitingOnYou)
}

/// How many of `lead`'s workers stand on the card under it.
fn workers(now: &ProjectNow, lead: &str) -> usize {
    (now.rows.iter())
        .filter(|row| row.lead.as_deref() == Some(lead) && row.group != NowGroup::WaitingOnYou)
        .count()
}

/// How many stand in each group.
struct Counts {
    you: usize,
    ready: usize,
    moving: usize,
    unsure: usize,
}

impl Counts {
    fn of(now: &ProjectNow) -> Self {
        let count = |group| shown(now, group).count();
        Counts {
            you: count(NowGroup::WaitingOnYou),
            ready: count(NowGroup::ReadyToLook),
            moving: count(NowGroup::Moving),
            unsure: count(NowGroup::NotSure),
        }
    }

    fn each(&self, t: &Tokens) -> [(usize, SharedString, Hsla); 4] {
        [
            (self.you, t!("For you").into(), t.accent),
            (self.ready, t!("Ready").into(), t.live),
            (self.moving, t!("Moving").into(), t.text_tertiary),
            (self.unsure, t!("Not sure").into(), t.warn),
        ]
    }

    /// The four counts set large, each over its tinted label.
    fn stats(&self, cx: &App) -> Div {
        let t = Tokens::get(cx);
        let each = self.each(t).into_iter().enumerate();
        div()
            .flex()
            .px(SPACE_2)
            .pb(SPACE_2)
            .border_b_1()
            .border_color(t.line_soft)
            .children(each.map(|(at, (n, label, tint))| {
                let ink = match (at, n) {
                    (_, 0) => t.text_disabled,
                    (0, _) => t.accent,
                    _ => t.text_primary,
                };
                div()
                    .flex_1()
                    .px(SPACE_2)
                    .py(SPACE_1)
                    .child(
                        ui::text(n.to_string(), 22., FontWeight::SEMIBOLD)
                            .font_features(theme::tabular())
                            .line_height(px(25.))
                            .text_color(ink),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .child(div().size(px(6.)).rounded_full().bg(tint))
                            .child(ui::caps(&label, cx).line_height(px(14.))),
                    )
            }))
    }

    /// The folded card: each count beside its tint, the person's own lit while anything waits.
    fn pills(&self, cx: &App) -> Vec<Div> {
        let t = Tokens::get(cx);
        let each = self.each(t).into_iter().enumerate();
        each.map(|(at, (n, _, tint))| {
            let pill = div()
                .h(px(24.))
                .px(px(9.))
                .rounded_full()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(div().size(px(6.)).rounded_full().bg(tint))
                .child(
                    ui::text(n.to_string(), TEXT_XS, FontWeight::SEMIBOLD)
                        .font_features(theme::tabular()),
                );
            match (at, n) {
                (_, 0) => pill.text_color(t.text_disabled),
                (0, _) => pill.bg(t.accent.opacity(0.16)).text_color(t.accent),
                _ => pill.text_color(t.text_secondary),
            }
        })
        .collect()
    }
}

fn chevron(open: bool, cx: &App) -> Div {
    let angle = match open {
        true => -std::f32::consts::FRAC_PI_2,
        false => std::f32::consts::FRAC_PI_2,
    };
    div()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(Tokens::get(cx).text_tertiary)
                .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::Radians(angle))),
        )
}

fn head(
    now: &ProjectNow,
    fold: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
    let agents = now.rows.len();
    div()
        .id("now-card-head")
        .h(px(36.))
        .pl(px(14.))
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .clicks(fold)
        .child(ui::chief_mark(13.))
        .child(ui::text(t!("Now"), TEXT_XS_H, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            ui::text(
                t!("{count} agents", count = agents),
                TEXT_XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        )
        .child(div().flex_1())
        .child(chevron(true, cx))
}

fn body(
    bench: &Workbench,
    now: &ProjectNow,
    handle: &WeakEntity<Workbench>,
    chat: &Chat,
    cx: &App,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let agent = |id: &str| bench.agent_by_id(id);
    for group in [NowGroup::WaitingOnYou, NowGroup::ReadyToLook] {
        let rows: Vec<&NowRow> = shown(now, group).collect();
        if rows.is_empty() {
            continue;
        }
        out.push(label(group_words(group), rows.len(), cx).into_any_element());
        for row in rows.iter().take(SHOWN) {
            if let Some(agent) = agent(&row.agent) {
                out.push(fact_row(&agent, row, handle, cx).into_any_element());
            }
        }
        if let Some(more) = rest(&rows[SHOWN.min(rows.len())..], &agent, cx) {
            out.push(more.into_any_element());
        }
    }
    let moving: Vec<&NowRow> = shown(now, NowGroup::Moving).collect();
    if !moving.is_empty() {
        out.push(label(group_words(NowGroup::Moving), moving.len(), cx).into_any_element());
        out.push(cluster(now, &moving, &agent, handle, cx).into_any_element());
    }
    let unsure: Vec<&NowRow> = shown(now, NowGroup::NotSure).collect();
    if !unsure.is_empty() {
        out.push(label(group_words(NowGroup::NotSure), unsure.len(), cx).into_any_element());
        for row in unsure.iter().take(SHOWN) {
            if let Some(agent) = agent(&row.agent) {
                out.push(fact_row(&agent, row, handle, cx).into_any_element());
            }
        }
        if let Some(more) = rest(&unsure[SHOWN.min(unsure.len())..], &agent, cx) {
            out.push(more.into_any_element());
        }
    }
    if !now.closed.is_empty() {
        out.push(label(t!("Closed lately").into(), now.closed.len(), cx).into_any_element());
        for closed in now.closed.iter().take(CLOSED_SHOWN) {
            out.push(closed_row(closed, chat.target.clone(), cx).into_any_element());
        }
    }
    out
}

fn group_words(group: NowGroup) -> SharedString {
    match group {
        NowGroup::WaitingOnYou => t!("Waiting on you"),
        NowGroup::ReadyToLook => t!("Ready to look at"),
        NowGroup::Moving => t!("Moving"),
        NowGroup::NotSure | NowGroup::Unknown => t!("Not sure"),
    }
    .into()
}

fn label(words: SharedString, count: usize, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(24.))
        .mt(SPACE_1)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(&words, cx))
        .child(
            ui::text(count.to_string(), TEXT_2XS, FontWeight::MEDIUM)
                .font_features(theme::tabular())
                .text_color(t.text_secondary),
        )
}

fn line(id: SharedString, cx: &App) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx);
    div()
        .id(id)
        .h(px(ROW))
        .px(SPACE_2)
        .rounded(theme::radius_2())
        .flex()
        .items_center()
        .gap(px(9.))
        .hover(|row| row.bg(t.surface_hover))
}

fn fact_row(
    agent: &AgentRow,
    row: &NowRow,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
    let late = row.group == NowGroup::WaitingOnYou && age_ms(row.since) > 10 * 60_000;
    let open = {
        let (agent, handle) = (agent.id.clone(), handle.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            handle
                .update(cx, |bench, cx| bench.open_agent(&agent, cx))
                .ok();
        }
    };
    line(SharedString::from(format!("now:{}", agent.id)), cx)
        .clicks(open)
        .child(attach::agent_pill(agent, TEXT_XS, handle, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .child(fact_words(&row.fact)),
        )
        .children(matches!(row.group, NowGroup::NotSure).then(|| ui::dot(ui::Tone::Idle, cx)))
        .child(clock(row.since, late, cx))
}

/// What a group's rows past the first few fold into: their marks and names.
fn rest(
    rows: &[&NowRow],
    agent: &impl Fn(&str) -> Option<AgentRow>,
    cx: &App,
) -> Option<impl IntoElement> {
    if rows.is_empty() {
        return None;
    }
    let t = Tokens::get(cx);
    let agents: Vec<AgentRow> = rows.iter().filter_map(|row| agent(&row.agent)).collect();
    let names: Vec<String> = agents.iter().map(name_of).collect();
    Some(
        line(
            SharedString::from(format!("now-rest:{}", rows[0].agent)),
            cx,
        )
        .child(marks(&agents, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .child(names.join(", ")),
        ),
    )
}

fn marks(agents: &[AgentRow], cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .children(agents.iter().take(4).enumerate().map(|(at, agent)| {
            div()
                .size(px(18.))
                .when(at > 0, |mark| mark.ml(px(-4.)))
                .rounded(theme::radius_2())
                .bg(t.surface_active)
                .border_2()
                .border_color(t.surface_raised)
                .flex()
                .items_center()
                .justify_center()
                .child(agent.mark(11., cx))
        }))
}

/// The moving agents as marks, each with its live dot, a lead as a chip naming how many work
/// under it; under them, what the longest standing is doing.
fn cluster(
    now: &ProjectNow,
    moving: &[&NowRow],
    agent: &impl Fn(&str) -> Option<AgentRow>,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
    let stars = moving.iter().filter_map(|row| {
        let agent = agent(&row.agent)?;
        let under = workers(now, &agent.id);
        let open = {
            let (id, handle) = (agent.id.clone(), handle.clone());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                handle
                    .update(cx, |bench, cx| bench.open_agent(&id, cx))
                    .ok();
            }
        };
        let face = div()
            .id(SharedString::from(format!("now-mark:{}", agent.id)))
            .relative()
            .h(px(MARK))
            .rounded(px(8.))
            .bg(t.surface_active)
            .flex()
            .items_center()
            .justify_center()
            .hover(|face| face.bg(t.surface_selected))
            .tooltip(ui::tooltip(format!(
                "{} · {}",
                name_of(&agent),
                fact_words(&row.fact)
            )))
            .clicks(open)
            .child(agent.mark(13., cx))
            .child(
                div()
                    .absolute()
                    .right(px(-2.))
                    .bottom(px(-2.))
                    .size(px(7.))
                    .rounded_full()
                    .bg(t.live)
                    .border_2()
                    .border_color(t.surface_raised),
            );
        Some(match under {
            0 => face.w(px(MARK)).into_any_element(),
            n => face
                .px(SPACE_1H)
                .gap(px(5.))
                .child(
                    ui::text(name_of(&agent), TEXT_2XS, FontWeight::MEDIUM)
                        .text_color(t.text_secondary),
                )
                .child(
                    ui::text(format!("+{n}"), TEXT_2XS, FontWeight::NORMAL)
                        .text_color(t.text_tertiary),
                )
                .into_any_element(),
        })
    });
    let first = moving
        .first()
        .and_then(|row| Some((agent(&row.agent)?, *row)));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .px(SPACE_2)
        .pt(theme::SPACE_0)
        .pb(SPACE_1)
        .child(div().flex().flex_wrap().gap(px(5.)).children(stars))
        .children(first.map(|(agent, row)| {
            ui::text(
                format!("{} {}", name_of(&agent), fact_words(&row.fact)),
                TEXT_2XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary)
            .overflow_hidden()
            .text_ellipsis()
        }))
}

fn closed_row(closed: &NowClosed, on: crate::core::Target, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx);
    let why = match (closed.why, closed.work) {
        (ClosedWhy::Landed, _) => t!("landed"),
        (ClosedWhy::NothingChanged, _) => t!("changed nothing"),
        (ClosedWhy::ByPerson, _) => t!("closed by you"),
        (_, Some(FixAt::Uncommitted)) => t!("left changes uncommitted"),
        (_, Some(FixAt::Committed)) => t!("committed, not landed"),
        _ => t!("closed"),
    };
    let reopen = {
        let agent = closed.agent.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = cx;
            on.call_typed::<sikemux_calls::call::ReopenAgent>(sikemux_calls::params::AgentParams {
                agent_id: agent.clone(),
            });
        }
    };
    line(
        SharedString::from(format!("now-closed:{}", closed.agent)),
        cx,
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .text_size(theme::ui_text(TEXT_XS_H))
            .text_color(t.text_secondary)
            .child(t!("Closed @{name}, {why}", name = closed.name, why = why)),
    )
    .child(
        Button::new(
            SharedString::from(format!("now-reopen:{}", closed.agent)),
            t!("Reopen"),
            ui::Variant::Ghost,
        )
        .sized(20., 6., 11.)
        .on_click(reopen),
    )
    .child(clock(closed.at, false, cx))
}

fn name_of(agent: &AgentRow) -> String {
    match (agent.chief, agent.name.as_ref()) {
        (true, _) => "@Chief".into(),
        (false, Some(name)) => format!("@{name}"),
        (false, None) => agent.title.clone(),
    }
}

fn age_ms(since: u64) -> u64 {
    crate::ui::time::now_ms().saturating_sub(since)
}

fn clock(since: u64, late: bool, cx: &App) -> Div {
    let t = Tokens::get(cx);
    ui::text(crate::ui::time::age(since), TEXT_2XS, FontWeight::NORMAL)
        .font_features(theme::tabular())
        .text_color(if late { t.warn } else { t.text_tertiary })
}

/// The one thing a row says of its agent, in a few words.
fn fact_words(fact: &NowFact) -> String {
    match fact {
        NowFact::Asks { count: 1 } => t!("asks you something").into(),
        NowFact::Asks { count } => t!("asked you {count} things", count = count),
        NowFact::Failed { .. } => t!("its turn failed").into(),
        NowFact::Stopped => t!("stopped, nothing set to wake it").into(),
        NowFact::Working { on: Some(on) } => on.clone(),
        NowFact::Working { on: None } => t!("working").into(),
        NowFact::Waiting {
            on, what, place, ..
        } => match (on, place) {
            (_, Some(1)) => t!("next in line").into(),
            (_, Some(place)) => t!("{place} in line", place = ordinal(*place)),
            (TurnWaitKind::Ci, _) => t!("waiting on CI").into(),
            (TurnWaitKind::Agent, _) if !what.is_empty() => {
                t!("waiting on @{who}", who = what)
            }
            (_, _) if !what.is_empty() => t!("waiting on {what}", what = what),
            _ => t!("waiting").into(),
        },
        NowFact::Work { at, release, .. } => match (at, release) {
            (FixAt::Running, _) => t!("in your app").into(),
            (FixAt::Released, Some(tag)) => t!("in {tag}", tag = tag),
            (FixAt::Released, None) => t!("released").into(),
            (FixAt::Landed, _) => t!("landed").into(),
            (FixAt::Committed, _) => t!("committed, not landed").into(),
            (FixAt::Uncommitted, _) => t!("changes not committed").into(),
            (FixAt::Staged, _) => t!("on staging").into(),
            (FixAt::Shipped, _) => t!("shipped").into(),
            (FixAt::Unknown, _) => t!("done").into(),
        },
        NowFact::Answered => t!("answered").into(),
        NowFact::Unknown => String::new(),
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}
