//! One slice: what it promises, its ladder of gates, the evidence for the gate waiting large
//! with the rest in a strip under it, its checks in a line, and what the person can say.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use sikemux_core::server::CardView;
use sikemux_view::slice::{Evidence, GateState, GateView, Passer, Proof, SliceView, Verdict};

use super::evidence;
use super::goal::crumb;
use super::{need_words, passer_name, step_name, waiting, At, GoalsPane};
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, SPACE_5, TEXT_2XS,
    TEXT_SM, TEXT_XL, TEXT_XS_H,
};
use crate::ui::{self, Button, Field, Variant};

impl GoalsPane {
    /// The evidence in front: the one the person picked, else the one the waiting gate leads with.
    pub(super) fn shown_evidence<'a>(&self, slice: &'a SliceView) -> Option<&'a Evidence> {
        self.picked
            .and_then(|id| slice.evidence.iter().find(|evidence| evidence.id == id))
            .or_else(|| (slice.evidence.iter()).find(|evidence| Some(evidence.id) == slice.leading))
    }
}

pub(super) fn slice(pane: &GoalsPane, id: u64, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some((card, slice)) = pane
        .card(id)
        .and_then(|card| Some((card, card.slice.as_ref()?)))
    else {
        return div().into_any_element();
    };
    let goal = super::goals_of(pane.cards())
        .into_iter()
        .find(|goal| goal.goal_slices.contains(&id));
    let shown = pane.shown_evidence(slice);
    let gate = waiting(slice).filter(|gate| {
        gate.passer == Passer::Person || (gate.environment.is_some() && gate.needs.is_empty())
    });
    div()
        .flex()
        .flex_col()
        .when_some(goal.filter(|goal| goal.id != id), |view, goal| {
            let goal_id = goal.id;
            view.child(
                crumb("slice-back", super::goal_name(goal), cx)
                    .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Goal(goal_id), cx))),
            )
        })
        .child(
            div()
                .flex()
                .items_start()
                .gap(SPACE_2)
                .child(
                    ui::text(card.title.clone(), TEXT_XL, FontWeight::SEMIBOLD)
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_color(t.text_primary),
                )
                .child(super::on_board(card.id, cx)),
        )
        .when(!slice.promise.is_empty(), |view| {
            view.child(
                div()
                    .pt(SPACE_1)
                    .text_size(theme::ui_text(TEXT_SM))
                    .text_color(t.text_secondary)
                    .child(slice.promise.clone()),
            )
        })
        .child(owner_line(card, cx))
        .children(stalled_line(pane, card.id, cx))
        .child(ladder(card, slice, cx))
        .children(goal.and_then(|goal| super::assertions::ticks(goal, slice, cx)))
        .when_some(shown, |view, shown| {
            view.child(div().pt(SPACE_4).child(evidence::large(shown, cx)))
        })
        .when(slice.evidence.len() > 1, |view| {
            view.child(div().pt(SPACE_2H).flex().flex_wrap().gap(SPACE_2).children(
                slice.evidence.iter().rev().map(|evidence| {
                    let chosen = shown.is_some_and(|shown| shown.id == evidence.id);
                    let serves = goal
                        .and_then(|goal| super::assertions::numbers(goal, &evidence.assertions));
                    evidence::thumb(evidence, chosen, serves, cx)
                }),
            ))
        })
        .children(checks(slice, cx))
        .when_some(gate, |view, gate| {
            view.child(decide(pane, card, gate, shown, cx))
        })
        .children(pane.note.as_ref().map(|(input, _)| {
            div()
                .pt(SPACE_2)
                .child(Field::new(input).icon(crate::icons::Icon::Pencil))
        }))
        .when(!slice.notes.is_empty(), |view| {
            view.child(notes(card, slice, cx))
        })
        .into_any_element()
}

/// Who the slice lost and why, with what the person can do about it, as the inbox says.
fn stalled_line(pane: &GoalsPane, id: u64, cx: &mut Context<GoalsPane>) -> Option<AnyElement> {
    let bench = pane.bench.upgrade()?;
    let item = (bench.read(cx).shown())?.inbox.items.iter().find(|item| {
        item.stalled.is_some()
            && item
                .card
                .as_ref()
                .is_some_and(|card| card.id == id && pane.here.as_deref() == Some(&card.project))
    })?;
    let item = item.clone();
    let t = Tokens::get(cx).clone();
    Some(
        div()
            .pt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::ui_text(TEXT_SM))
                    .text_color(t.warn)
                    .child(item.detail.clone().unwrap_or_else(|| item.what.clone())),
            )
            .children(super::home::stall_buttons(&item, cx))
            .into_any_element(),
    )
}

/// Who owns the slice, which opens their chat, and its ticket.
fn owner_line(card: &CardView, cx: &Context<GoalsPane>) -> AnyElement {
    div()
        .pt(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .when_some(card.owner.clone(), |line, agent| {
            line.child(
                div()
                    .id("slice-owner")
                    .cursor_pointer()
                    .hover(|owner| owner.opacity(0.8))
                    .on_click(cx.listener(move |pane, _, _, cx| pane.open_agent(&agent, cx)))
                    .child(crate::board::owner(card, cx)),
            )
            .child(ui::meta("owns it", cx))
        })
        .when_some(card.ticket.as_ref(), |line, ticket| {
            line.child(crate::board::ticket_line(ticket, cx))
        })
        .into_any_element()
}

/// The gates in order, joined by a line that is green as far as they passed; under each, who
/// passed it and when, or who may pass it and what it still needs.
fn ladder(card: &CardView, slice: &SliceView, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx);
    let last = slice.gates.len().saturating_sub(1);
    div()
        .pt(SPACE_5)
        .flex()
        .children(slice.gates.iter().enumerate().map(|(at, gate)| {
            let you = gate.state == GateState::Waiting && gate.passer == Passer::Person;
            let node = div().size(px(10.)).rounded_full().flex_none();
            let node = match gate.state {
                GateState::Passed => node.bg(t.live),
                GateState::Waiting => {
                    node.border(px(1.5))
                        .border_color(if you { t.warn } else { t.live })
                }
                GateState::Ahead => node.border(px(1.5)).border_color(t.text_tertiary),
            };
            let joined = gate.state == GateState::Passed;
            let under = match gate.state {
                GateState::Passed => [Some(passer_name(gate, card)), gate.at.map(ui::time::ago)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                _ if !gate.needs.is_empty() => format!(
                    "needs {}",
                    gate.needs
                        .iter()
                        .map(|need| need_words(*need))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                _ => gate.why.clone().unwrap_or_else(|| passer_name(gate, card)),
            };
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(node)
                        .when(at < last, |line| {
                            line.child(div().flex_1().h(px(1.5)).bg(if joined {
                                t.live
                            } else {
                                t.border
                            }))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            ui::text(step_name(gate).to_owned(), TEXT_SM, FontWeight::MEDIUM)
                                .text_color(if you { t.warn } else { t.text_primary }),
                        )
                        .child(ui::meta(under, cx).truncate()),
                )
        }))
        .into_any_element()
}

/// The checks the core ruled on, in one line: how many passed, and which failed.
fn checks(slice: &SliceView, cx: &Context<GoalsPane>) -> Option<AnyElement> {
    let t = Tokens::get(cx);
    let ruled: Vec<&Evidence> = slice
        .evidence
        .iter()
        .filter(|evidence| evidence.verdict.is_some())
        .collect();
    if ruled.is_empty() {
        return None;
    }
    let failed: Vec<&str> = ruled
        .iter()
        .filter(|evidence| evidence.verdict == Some(Verdict::Failed))
        .map(|evidence| match &evidence.proof {
            Proof::Run { command, .. } => command.as_str(),
            _ => evidence::label(evidence),
        })
        .collect();
    let (verdict, words) = match failed.as_slice() {
        [] if ruled.len() == 1 => (Verdict::Passed, "1 check passed".to_owned()),
        [] => (Verdict::Passed, format!("{} checks passed", ruled.len())),
        [only] if ruled.len() == 1 => (Verdict::Failed, format!("{only} failed")),
        failed => (
            Verdict::Failed,
            format!(
                "{} of {} checks failed: {}",
                failed.len(),
                ruled.len(),
                failed.join(", ")
            ),
        ),
    };
    Some(
        div()
            .mt(SPACE_3)
            .px(SPACE_3)
            .h(px(34.))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .rounded(theme::RADIUS_2)
            .border_1()
            .border_color(t.border)
            .child(evidence::verdict_mark(verdict, 14., cx))
            .child(ui::text(words, TEXT_XS_H, FontWeight::NORMAL).text_color(t.text_secondary))
            .into_any_element(),
    )
}

/// The person's two answers to the gate waiting on them: pass it, or say what needs work.
fn decide(
    pane: &GoalsPane,
    card: &CardView,
    gate: &GateView,
    shown: Option<&Evidence>,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let (id, passing, environment) = (card.id, gate.gate, gate.environment.clone());
    let pinned = shown.map(|shown| {
        format!(
            "A note pins to the {}",
            evidence::label(shown).to_lowercase()
        )
    });
    div()
        .pt(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            Button::new("slice-pass", pass_words(gate), Variant::Primary)
                .disabled(pane.busy)
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.pass(id, passing, environment.clone(), cx)
                })),
        )
        .child(
            Button::new("slice-needs-work", "Needs work", Variant::Outline)
                .pressed(pane.note.is_some())
                .on_click(cx.listener(|pane, _, window, cx| match pane.note {
                    Some(_) => {
                        pane.note = None;
                        cx.notify();
                    }
                    None => pane.write_note(window, cx),
                })),
        )
        .child(div().flex_1())
        .children(pinned.map(|pinned| ui::meta(pinned, cx)))
        .into_any_element()
}

/// What passing a step says: its gate's word, or that it passes an environment by name.
fn pass_words(gate: &GateView) -> String {
    match &gate.environment {
        Some(environment) => format!("Pass {environment}"),
        None => step_name(gate).to_owned(),
    }
}

/// What was written about the slice, newest first, each with the evidence it is pinned to.
fn notes(card: &CardView, slice: &SliceView, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .pt(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .children(slice.notes.iter().rev().map(|note| {
            let by = match &note.by {
                None => "you".to_owned(),
                Some(agent) if card.owner.as_deref() == Some(agent) => card
                    .owner_name
                    .clone()
                    .unwrap_or_else(|| "its owner".into()),
                Some(_) => "an agent".to_owned(),
            };
            let on = note
                .evidence
                .and_then(|id| slice.evidence.iter().find(|evidence| evidence.id == id))
                .map(|evidence| format!("on the {}", evidence::label(evidence).to_lowercase()));
            div()
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .gap(SPACE_1H)
                        .child(
                            ui::text(by, TEXT_2XS, FontWeight::MEDIUM).text_color(t.text_secondary),
                        )
                        .children(on.map(|on| ui::meta(on, cx)))
                        .child(div().flex_1())
                        .child(ui::meta(ui::time::age(note.at), cx)),
                )
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(note.text.clone()),
                )
        }))
        .into_any_element()
}
