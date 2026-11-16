//! The Now card floating at the top right of a chief's chat: its project's agents in four groups,
//! waiting on the person, ready to look at, moving and not sure, one row each, then the agents
//! closed lately with a way to open them again. Folded, it is the four counts alone. The core
//! works out every group and fact; this only words and draws them.

use super::*;
use crate::ui::Clicks;
use gpui_kit::{linear_color_stop, linear_gradient, ClickEvent};

use sikemux_view::now::{
    AskStand, ClosedWhy, FixAt, NowClosed, NowFact, NowGroup, NowRow, ProjectNow,
};
use sikemux_view::turn::TurnWaitKind;

use crate::theme::{
    SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_SM, TEXT_XS, TEXT_XS_H,
};

const WIDTH: f32 = 356.;
const ROW: f32 = 30.;
const NAME: f32 = 84.;
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
    if !chief.chief || chat.narrowed.is_some() {
        return None;
    }
    let now = (bench.view_at(chat.target.place())?.now.iter())
        .find(|now| now.project == chief.project)?;
    if now.rows.is_empty() && now.closed.is_empty() && now.asks.is_empty() {
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
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme::mix(t.text_primary, t.surface_raised, 0.035), 0.),
            linear_color_stop(t.surface_raised, 0.3),
        ))
        .border(crate::theme::border())
        .border_color(t.border)
        .shadow(t.shadow_float());
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
            .children(asks_pill(now, cx))
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
                .child(
                    div()
                        .px(SPACE_2)
                        .pt(SPACE_1H)
                        .pb(SPACE_2H)
                        .flex()
                        .flex_col()
                        .gap(SPACE_2H)
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

/// The person's open asks as one pill: amber with how many have nobody on them, else quiet with
/// how many there are.
fn asks_pill(now: &ProjectNow, cx: &App) -> Option<AnyElement> {
    if now.asks.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let nobody = (now.asks.iter())
        .filter(|ask| ask.stand == AskStand::Nobody)
        .count();
    let (words, ink, fill) = match nobody {
        0 => (
            match now.asks.len() {
                1 => t!("1 ask").to_owned(),
                count => t!("{count} asks", count = count),
            },
            t.text_secondary,
            t.text_primary.opacity(0.06),
        ),
        _ => (
            t!("{count} nobody", count = nobody),
            t.warn,
            t.warn.opacity(0.14),
        ),
    };
    Some(
        div()
            .h(px(24.))
            .px(px(9.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .bg(fill)
            .text_color(ink)
            .child(ui::text(words, TEXT_XS, FontWeight::SEMIBOLD).font_features(theme::tabular()))
            .into_any_element(),
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
        .h(px(44.))
        .pl(px(16.))
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b(crate::theme::border())
        .border_color(t.line_soft)
        .clicks(fold)
        .child(ui::chief_mark(14.))
        .child(ui::text(t!("Now"), TEXT_SM, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            ui::text(
                format!(
                    "· {}",
                    match agents {
                        1 => t!("1 agent").to_owned(),
                        agents => t!("{count} agents", count = agents),
                    }
                ),
                TEXT_XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        )
        .child(div().flex_1())
        .children(asks_pill(now, cx))
        .child(chevron(true, cx))
}

fn body(
    bench: &Workbench,
    now: &ProjectNow,
    handle: &WeakEntity<Workbench>,
    chat: &Chat,
    cx: &App,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx);
    let agent = |id: &str| bench.agent_by_id(id);
    let mut out = Vec::new();
    for (group, tint) in [
        (NowGroup::WaitingOnYou, t.accent),
        (NowGroup::ReadyToLook, t.live),
        (NowGroup::Moving, t.text_tertiary),
        (NowGroup::NotSure, t.warn),
    ] {
        let rows: Vec<&NowRow> = shown(now, group).collect();
        if rows.is_empty() {
            continue;
        }
        let lines = rows.iter().take(SHOWN).filter_map(|row| {
            let agent = agent(&row.agent)?;
            let under = workers(now, &agent.id);
            Some(fact_row(&agent, row, under, handle, cx).into_any_element())
        });
        let more = rest(&rows[SHOWN.min(rows.len())..], &agent, cx);
        out.push(
            div()
                .flex()
                .flex_col()
                .child(label(group_words(group), rows.len(), tint, cx))
                .children(lines)
                .children(more.map(IntoElement::into_any_element))
                .into_any_element(),
        );
    }
    if !now.closed.is_empty() {
        let lines = (now.closed.iter().take(CLOSED_SHOWN))
            .map(|closed| closed_row(closed, chat.target.clone(), cx).into_any_element());
        out.push(
            div()
                .flex()
                .flex_col()
                .child(label(
                    t!("Closed lately").into(),
                    now.closed.len(),
                    t.text_tertiary,
                    cx,
                ))
                .children(lines)
                .into_any_element(),
        );
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

/// A group's spaced capitals in its colour, how many stand in it, and a hairline to the edge.
fn label(words: SharedString, count: usize, tint: Hsla, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(24.))
        .px(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(&words, cx).flex_none().text_color(tint))
        .child(
            ui::text(count.to_string(), TEXT_2XS, FontWeight::NORMAL)
                .flex_none()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary),
        )
        .child(div().flex_1().h(px(1.)).bg(t.border))
}

fn line(id: SharedString, cx: &App) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx);
    div()
        .id(id)
        .h(px(ROW))
        .px(SPACE_2H)
        .rounded(theme::radius_2())
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .hover(|row| row.bg(t.surface_hover))
}

fn fact_row(
    agent: &AgentRow,
    row: &NowRow,
    under: usize,
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
        .child(div().flex_none().child(agent.mark(TEXT_XS_H, cx)))
        .child(
            div()
                .flex_none()
                .w(px(NAME))
                .flex()
                .items_baseline()
                .gap(SPACE_1)
                .overflow_hidden()
                .child(
                    ui::text(name_of(agent), TEXT_XS_H, FontWeight::MEDIUM)
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(t.text_primary),
                )
                .children((under > 0).then(|| {
                    ui::text(format!("+{under}"), TEXT_2XS, FontWeight::NORMAL)
                        .flex_none()
                        .text_color(t.text_tertiary)
                })),
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
                .child(fact_words(&row.fact)),
        )
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
        .flex_none()
        .min_w(px(28.))
        .flex()
        .justify_end()
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
