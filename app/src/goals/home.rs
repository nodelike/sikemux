//! The pane's first screen: what needs the person, by what for, and the goals with work under
//! way; and what happened while the person was away, by goal. The core ranks, sorts and counts
//! both; the pane only groups them under the goal each item is on.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::CardView;
use sikemux_wire::inbox::{InboxItem, InboxKind, InboxNeed};
use sikemux_wire::slice::SliceState;

use super::evidence::minute;
use super::goal::{count, crumb, row, section, strip};
use super::{empty, state_count, At, GoalsPane};
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_5, TEXT_2XS, TEXT_SM, TEXT_XL};
use crate::ui;

/// Something that finished or failed while the person was away.
#[derive(Clone, Copy)]
struct Happened<'a> {
    done: bool,
    item: &'a InboxItem,
}

const NEEDS: [(InboxNeed, &str); 3] = [
    (InboxNeed::Decide, "Decide"),
    (InboxNeed::Unblock, "Unblock"),
    (InboxNeed::Fix, "Fix"),
];

impl GoalsPane {
    /// The inbox items about this project, in the core's order.
    fn inbox_here(&self, cx: &gpui_kit::App) -> (Vec<InboxItem>, usize) {
        let (Some(project), Some(bench)) = (self.project(), self.bench.upgrade()) else {
            return (Vec::new(), 0);
        };
        let bench = bench.read(cx);
        let Some(inbox) = bench.shown().map(|view| &view.inbox) else {
            return (Vec::new(), 0);
        };
        let items = inbox
            .items
            .iter()
            .filter(|item| item.need.is_some() && item.project.as_deref() == Some(project))
            .cloned()
            .collect();
        (items, inbox.needs.get(project).copied().unwrap_or(0))
    }

    /// What finished or failed in this project while the person was away, in time order.
    fn happened_here(&self) -> Vec<Happened<'_>> {
        let Some(digest) = self.digest.as_ref() else {
            return Vec::new();
        };
        let finished = digest
            .finished
            .iter()
            .map(|item| Happened { done: true, item });
        let failed = digest
            .failed
            .iter()
            .map(|item| Happened { done: false, item });
        let mut happened: Vec<Happened> = finished.chain(failed).collect();
        happened.sort_by_key(|happened| happened.item.at);
        happened
    }

    /// The title of the goal an item is on, when it is on one here.
    fn goal_title(&self, item: &InboxItem) -> Option<String> {
        Some(self.goal_of(item.card.as_ref()?)?.title.clone())
    }
}

pub(super) fn home(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (needs, needed) = pane.inbox_here(cx);
    let goals = pane.goals();
    let (moving, finished): (Vec<&CardView>, Vec<&CardView>) = goals.iter().partition(|goal| {
        let slices = pane.slices(goal.id);
        count(&slices, SliceState::Done) < slices.len()
    });
    let happened = pane.happened_here().len();
    if goals.is_empty() && needs.is_empty() {
        return empty(
            Some("No goals yet"),
            "A goal is a card with slices under it, each with gates to pass.",
            cx,
        );
    }
    let summary = [
        (needed > 0).then(|| state_count(needed, SliceState::NeedsYou)),
        Some(format!("{} moving", moving.len())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    div()
        .flex()
        .flex_col()
        .child(ui::text("Goals", TEXT_XL, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            div()
                .pt(SPACE_1H)
                .flex()
                .gap(SPACE_1)
                .child(ui::meta(summary, cx))
                .when(happened > 0, |line| {
                    line.child(ui::meta("·", cx)).child(
                        div()
                            .id("goals-overnight")
                            .cursor_pointer()
                            .text_color(t.text_secondary)
                            .hover(move |style| style.text_color(t.text_primary))
                            .child(ui::text(
                                format!("{happened} happened while you were away"),
                                TEXT_2XS,
                                FontWeight::NORMAL,
                            ))
                            .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Overnight, cx))),
                    )
                }),
        )
        .when(!needs.is_empty(), |view| {
            view.child(section("Needs you", needed, cx))
                .children(NEEDS.iter().filter_map(|(need, name)| {
                    let listed: Vec<&InboxItem> = needs
                        .iter()
                        .filter(|item| item.need == Some(*need))
                        .collect();
                    (!listed.is_empty()).then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .pt(SPACE_2)
                            .child(ui::caps(name, cx))
                            .children(listed.into_iter().map(|item| needs_row(pane, item, cx)))
                    })
                }))
        })
        .when(!moving.is_empty(), |view| {
            view.child(section("Moving", moving.len(), cx))
                .children(moving.iter().map(|goal| goal_row(pane, goal, cx)))
        })
        .when(!finished.is_empty(), |view| {
            view.child(section("Done", finished.len(), cx))
                .children(finished.iter().map(|goal| goal_row(pane, goal, cx)))
        })
        .into_any_element()
}

/// What something waiting on the person is, by its mark.
fn need_icon(kind: InboxKind) -> Icon {
    match kind {
        InboxKind::Gate => Icon::Eye,
        InboxKind::Question | InboxKind::Assumption => Icon::Message,
        InboxKind::Permission => Icon::Shield,
        InboxKind::Input => Icon::Terminal,
        InboxKind::PullRequest => Icon::PullRequest,
        _ => Icon::Warning,
    }
}

/// One thing waiting on the person: what it needs, the goal or agent it is about, and how long
/// it has waited.
fn needs_row(pane: &GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let about = pane.goal_title(item).or_else(|| item.name.clone());
    let tint = match item.need {
        Some(InboxNeed::Fix) => t.danger,
        _ => t.warn,
    };
    let opened = item.clone();
    row(SharedString::from(format!("need-{}", item.id)), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(ui::icon(need_icon(item.kind), 14.).text_color(tint))
        .child(
            ui::text(item.what.clone(), TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(about.map(|about| ui::meta(about, cx).flex_shrink_1().min_w_0().truncate()))
        .child(div().flex_1())
        .child(ui::meta(ui::time::age(item.at), cx))
        .into_any_element()
}

/// A goal on one line: its title, how many slices are done, what comes next and what that
/// waits on, and its strip.
fn goal_row(pane: &GoalsPane, goal: &CardView, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = goal.id;
    let slices = pane.slices(id);
    let done = count(&slices, SliceState::Done);
    row(SharedString::from(format!("goal-{id}")), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Goal(id), cx)))
        .child(
            ui::text(goal.title.clone(), TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .child(ui::meta(format!("{done} of {} done", slices.len()), cx))
        .children(
            (goal.next.first())
                .and_then(|next| super::goal::step(pane, next))
                .map(|next| ui::meta(next, cx).flex_shrink_1().min_w_0().truncate()),
        )
        .child(div().flex_1())
        .child(strip(&slices, px(120.), cx))
        .into_any_element()
}

pub(super) fn overnight(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let back = crumb("overnight-back", "Goals", cx)
        .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx)));
    let Some(since) = pane.digest.as_ref().and_then(|digest| digest.since) else {
        return div()
            .flex()
            .flex_col()
            .child(back)
            .child(empty(
                Some("Nothing yet"),
                "What finishes and fails while you are away shows here once you have been.",
                cx,
            ))
            .into_any_element();
    };
    let happened = pane.happened_here();
    let finished = happened.iter().filter(|happened| happened.done).count();
    let failed = happened.len() - finished;
    let waiting = pane.inbox_here(cx).1;
    let mut groups: Vec<(Option<String>, Vec<Happened>)> = Vec::new();
    for happened in &happened {
        let goal = pane.goal_title(happened.item);
        match groups.iter_mut().find(|(title, _)| *title == goal) {
            Some((_, listed)) => listed.push(*happened),
            None => groups.push((goal, vec![*happened])),
        }
    }
    groups.sort_by_key(|(goal, _)| goal.is_none());
    let number = |value: usize, label: &'static str| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                ui::text(value.to_string(), TEXT_XL, FontWeight::SEMIBOLD)
                    .font_features(theme::tabular())
                    .text_color(t.text_primary),
            )
            .child(ui::meta(label, cx))
    };
    div()
        .flex()
        .flex_col()
        .child(back)
        .child(ui::text("Overnight", TEXT_XL, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            div()
                .pt(SPACE_1H)
                .child(ui::meta(format!("since {}", minute(since)), cx)),
        )
        .child(
            div()
                .pt(SPACE_5)
                .flex()
                .gap(px(40.))
                .child(number(finished, "finished"))
                .child(number(failed, "failed"))
                .child(number(waiting, "wait on you")),
        )
        .when(happened.is_empty(), |view| {
            view.child(
                div()
                    .pt(SPACE_5)
                    .child(ui::meta("Nothing finished or failed here since then.", cx)),
            )
        })
        .children(groups.into_iter().map(|(goal, listed)| {
            div()
                .flex()
                .flex_col()
                .pt(SPACE_5)
                .child(
                    div().pb(SPACE_1H).child(
                        ui::text(
                            goal.unwrap_or_else(|| "Other work".into()),
                            TEXT_SM,
                            FontWeight::SEMIBOLD,
                        )
                        .text_color(t.text_primary),
                    ),
                )
                .children(
                    listed
                        .into_iter()
                        .map(|happened| event_row(pane, happened, cx)),
                )
        }))
        .into_any_element()
}

/// One thing that happened: when, what, and what opening it shows.
fn event_row(pane: &GoalsPane, happened: Happened, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Happened { done, item } = happened;
    let on_slice = item
        .card
        .as_ref()
        .and_then(|card| pane.card(card.id))
        .is_some_and(|card| card.slice.is_some());
    let opens = match (on_slice, item.agent_id.is_some()) {
        (true, _) => Some("slice"),
        (false, true) => Some("chat"),
        (false, false) => None,
    };
    let subject = (item.card.as_ref().map(|card| card.title.clone()))
        .or_else(|| item.title.clone())
        .or_else(|| item.name.clone());
    let opened = item.clone();
    row(SharedString::from(format!("event-{}", item.id)), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(
            ui::mono(minute(item.at), TEXT_2XS, cx)
                .w(px(52.))
                .flex_none(),
        )
        .child(div().size(px(7.)).flex_none().rounded_full().bg(if done {
            t.live
        } else {
            t.danger
        }))
        .children(subject.map(|subject| {
            ui::text(subject, TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary)
        }))
        .child(
            ui::meta(item.what.clone(), cx)
                .flex_shrink_1()
                .min_w_0()
                .truncate(),
        )
        .child(div().flex_1())
        .children(opens.map(|opens| ui::meta(opens, cx)))
        .into_any_element()
}
