//! One goal: who leads it, how far its slices are, what comes next, what needs the person, a
//! line for each slice and the rules its agents follow.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::CardView;
use sikemux_wire::slice::{Gate, NextGate, Passer, SliceState, SliceView};

use super::{
    gate_name, state_count, state_mark, wait_words, waiting, waiting_words, At, GoalsPane,
};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_5, TEXT_2XS,
    TEXT_SM, TEXT_XL, TEXT_XS_H,
};
use crate::ui;

const STATES: [SliceState; 5] = [
    SliceState::Done,
    SliceState::Moving,
    SliceState::NeedsYou,
    SliceState::Failed,
    SliceState::Waiting,
];

/// Goal `id`: its title and team, a strip of its slices, what comes next, what needs the
/// person and every slice.
pub(super) fn goal(pane: &GoalsPane, id: u64, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(goal) = pane.card(id) else {
        return div().into_any_element();
    };
    let slices = pane.slices(id);
    let needs_you: Vec<_> = slices
        .iter()
        .filter(|(_, slice)| waiting(slice).is_some_and(|gate| gate.passer == Passer::Person))
        .collect();
    let counts = STATES
        .iter()
        .map(|state| (count(&slices, *state), *state))
        .filter(|(count, _)| *count > 0)
        .map(|(count, state)| state_count(count, state))
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .flex()
        .flex_col()
        .child(
            crumb("goals-back", "Home", cx)
                .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    ui::text(goal.title.clone(), TEXT_XL, FontWeight::SEMIBOLD)
                        .text_color(t.text_primary),
                )
                .child(div().flex_1())
                .when_some(goal.ticket.as_ref(), |head, ticket| {
                    head.child(ui::mono(ticket.key.clone(), TEXT_2XS, cx))
                }),
        )
        .child(team(goal, &slices, cx))
        .child(
            div()
                .pt(SPACE_3)
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(strip(&slices, gpui_kit::relative(1.), cx))
                .child(ui::meta(counts, cx)),
        )
        .children(next(pane, goal, cx))
        .when(!needs_you.is_empty(), |view| {
            view.child(section("Needs you", needs_you.len(), cx)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .children(needs_you.iter().map(|(card, slice)| needs(card, slice, cx))),
            )
        })
        .child(super::assertions::section_of(pane, goal, cx))
        .child(section("Slices", slices.len(), cx))
        .children(
            slices
                .iter()
                .map(|(card, slice)| slice_row(card, slice, cx)),
        )
        .children(super::rules::rules(pane, goal, cx))
        .into_any_element()
}

pub(super) fn count(slices: &[(&CardView, &SliceView)], state: SliceState) -> usize {
    slices
        .iter()
        .filter(|(_, slice)| slice.state == state)
        .count()
}

/// The goal's lead, then the agents that own its slices.
fn team(
    goal: &CardView,
    slices: &[(&CardView, &SliceView)],
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx);
    let mut owners: Vec<&CardView> = Vec::new();
    for (card, _) in slices {
        let other = card.owner.is_some() && card.owner != goal.owner;
        if other && !owners.iter().any(|known| known.owner == card.owner) {
            owners.push(card);
        }
    }
    let names = owners
        .iter()
        .filter_map(|card| card.owner_name.clone())
        .collect::<Vec<_>>();
    let marks = goal
        .owner_cli
        .iter()
        .chain(owners.iter().filter_map(|card| card.owner_cli.as_ref()))
        .map(|cli| ui::mark(cli, 12.));
    div()
        .pt(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(div().flex().gap(SPACE_0).children(marks))
        .when_some(goal.owner_name.clone(), |line, lead| {
            line.child(
                div()
                    .flex()
                    .gap(px(3.))
                    .child(
                        ui::text(lead, TEXT_2XS, FontWeight::MEDIUM).text_color(t.text_secondary),
                    )
                    .child(ui::meta("leads", cx)),
            )
        })
        .when(!names.is_empty(), |line| {
            line.child(ui::meta(format!("with {}", names.join(", ")), cx))
        })
        .into_any_element()
}

/// One segment per slice, in the colour of its state.
pub(super) fn strip(
    slices: &[(&CardView, &SliceView)],
    width: impl Into<gpui_kit::Length>,
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .w(width.into())
        .flex_none()
        .flex()
        .gap(px(3.))
        .children(slices.iter().map(|(_, slice)| {
            let segment = div().flex_1().h(px(4.)).rounded(px(2.));
            match slice.state {
                SliceState::Done => segment.bg(t.live),
                SliceState::Moving => segment.bg(t.live.opacity(0.45)),
                SliceState::NeedsYou => segment.bg(t.warn),
                SliceState::Failed => segment.bg(t.danger),
                SliceState::Waiting => segment
                    .border_1()
                    .border_dashed()
                    .border_color(t.text_disabled),
            }
        }))
        .into_any_element()
}

/// The gates the goal's slices wait at next, in the order the core expects them reached.
fn next(pane: &GoalsPane, goal: &CardView, cx: &Context<GoalsPane>) -> Option<AnyElement> {
    let t = Tokens::get(cx);
    let steps: Vec<String> = goal
        .next
        .iter()
        .take(3)
        .filter_map(|next| step(pane.cards(), next))
        .collect();
    if steps.is_empty() {
        return None;
    }
    Some(
        div()
            .mt(SPACE_3)
            .px(SPACE_3)
            .py(SPACE_2H)
            .flex()
            .items_start()
            .gap(SPACE_3)
            .rounded(theme::RADIUS_2)
            .bg(t.surface_sunken)
            .child(div().pt(px(1.)).child(ui::caps("Next", cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_secondary)
                    .child(steps.join(", then ")),
            )
            .into_any_element(),
    )
}

/// A gate a slice waits at, as a short sentence: who does what to which slice.
pub(super) fn step(cards: &[CardView], next: &NextGate) -> Option<String> {
    let card = cards.iter().find(|card| card.id == next.card)?;
    let you = next.passer == Passer::Person;
    let verb = match (next.gate, you) {
        (Gate::Built, true) => "build",
        (Gate::Built, false) => "builds",
        (Gate::LooksRight, true) => "look at",
        (Gate::LooksRight, false) => "looks at",
        (Gate::Landed, true) => "land",
        (Gate::Landed, false) => "lands",
    };
    let who = match next.passer {
        Passer::Person => "you",
        Passer::Core => "Sikemux",
        Passer::Owner => match card.owner_name.as_deref() {
            Some(owner) => owner,
            None => return Some(format!("{} waits for an owner", card.title)),
        },
    };
    let step = format!("{who} {verb} {}", card.title);
    Some(
        match next.wait.as_ref().and_then(|wait| wait_words(wait, card)) {
            Some(wait) => format!("{step} ({wait})"),
            None => step,
        },
    )
}

/// A slice waiting on the person: its title, what it promises, and how long it has waited.
fn needs(card: &CardView, slice: &SliceView, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let gate = waiting(slice);
    let since = slice
        .gates
        .iter()
        .filter_map(|gate| gate.at)
        .max()
        .map(ui::time::age);
    div()
        .id(SharedString::from(format!("needs-{id}")))
        .px(SPACE_3)
        .py(SPACE_2H)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(move |style| style.border_color(t.border_strong))
        .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Slice(id), cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    ui::text(card.title.clone(), TEXT_SM, FontWeight::MEDIUM)
                        .text_color(t.text_primary),
                )
                .when_some(gate, |line, gate| {
                    line.child(
                        ui::text(gate_name(gate.gate), TEXT_2XS, FontWeight::MEDIUM)
                            .text_color(t.warn),
                    )
                })
                .child(div().flex_1())
                .children(since.map(|since| ui::meta(since, cx))),
        )
        .when(!slice.promise.is_empty(), |item| {
            item.child(
                div()
                    .text_size(theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_secondary)
                    .child(slice.promise.clone()),
            )
        })
        .into_any_element()
}

/// One slice: its state, title and what it waits on, then its ticket and owner.
fn slice_row(card: &CardView, slice: &SliceView, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    row(SharedString::from(format!("slice-{id}")), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Slice(id), cx)))
        .child(state_mark(slice.state, cx))
        .child(
            ui::text(card.title.clone(), TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(
            waiting_words(card, slice)
                .map(|words| ui::meta(words, cx).flex_shrink_1().min_w_0().truncate()),
        )
        .child(div().flex_1())
        .when_some(card.ticket.as_ref(), |row, ticket| {
            row.child(ui::mono(ticket.key.clone(), TEXT_2XS, cx))
        })
        .when(card.owner.is_some(), |row| {
            row.child(crate::board::owner(card, cx))
        })
        .into_any_element()
}

/// A line of a list that opens what it names.
pub(super) fn row(id: SharedString, cx: &Context<GoalsPane>) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .h(px(34.))
        .px(SPACE_2)
        .mx(-SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(move |style| style.bg(t.surface_hover))
}

pub(super) fn section(title: &'static str, count: usize, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .pt(SPACE_5)
        .pb(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::text(title, TEXT_SM, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(ui::meta(count.to_string(), cx))
        .into_any_element()
}

/// A step back up, as `Name /` over a title.
pub(super) fn crumb(
    id: &'static str,
    name: impl Into<SharedString>,
    cx: &Context<GoalsPane>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .pb(SPACE_1)
        .flex()
        .gap(SPACE_1)
        .cursor_pointer()
        .text_color(t.text_tertiary)
        .hover(move |style| style.text_color(t.text_secondary))
        .child(ui::text(name, TEXT_2XS, FontWeight::NORMAL))
        .child(ui::text("/", TEXT_2XS, FontWeight::NORMAL))
}
