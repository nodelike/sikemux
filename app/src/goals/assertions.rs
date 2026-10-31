//! A goal's assertions: what must hold once its slices are done, each with the kind of evidence
//! that shows it. The goal lists them and the person adds or retires one; a slice ticks the ones
//! it fulfils as their evidence comes in, and each piece of evidence says which it serves. Each is
//! known by its number in the goal's list.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window,
};
use sikemux_core::server::{call, BoardAssertParams, CardView};
use sikemux_wire::slice::{Assertion, Collect, SliceView};

use super::goal::section;
use super::GoalsPane;
use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_XS_H};
use crate::ui::{self, Button, Field, IconButton, Segmented, Variant};

const KINDS: [(Collect, &str); 3] = [
    (Collect::Check, "Check"),
    (Collect::Recording, "Recording"),
    (Collect::Screenshot, "Screenshot"),
];

/// The line that writes a new assertion.
pub(super) struct Asserting {
    input: Entity<InputState>,
    kind: Collect,
    _typing: Subscription,
}

fn kind_words(kind: Collect) -> &'static str {
    match kind {
        Collect::Check => "the check passes",
        Collect::Recording => "a recording",
        Collect::Screenshot => "a screenshot",
    }
}

fn kind_icon(kind: Collect) -> Icon {
    match kind {
        Collect::Check => Icon::Check,
        Collect::Recording => Icon::Record,
        Collect::Screenshot => Icon::Image,
    }
}

/// The number an assertion goes by: its place in the goal's list.
pub(super) fn number(goal: &CardView, assertion: u64) -> Option<usize> {
    goal.assertions
        .iter()
        .position(|kept| kept.id == assertion)
        .map(|at| at + 1)
}

/// The numbers of the assertions, as "1, 3".
pub(super) fn numbers(goal: &CardView, assertions: &[u64]) -> Option<String> {
    let listed: Vec<String> = assertions
        .iter()
        .filter_map(|id| number(goal, *id))
        .map(|number| number.to_string())
        .collect();
    (!listed.is_empty()).then(|| listed.join(", "))
}

impl GoalsPane {
    /// Makes the line that writes a new assertion, which needs the window.
    pub(super) fn assert_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asserting.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("What must hold once this goal is done")
        });
        let typing = cx.subscribe_in(&input, window, |pane, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                pane.assert_new(window, cx);
            }
        });
        self.asserting = Some(Asserting {
            input,
            kind: Collect::Check,
            _typing: typing,
        });
    }

    fn assert_kind(&mut self, kind: Collect, cx: &mut Context<Self>) {
        if let Some(asserting) = &mut self.asserting {
            asserting.kind = kind;
            cx.notify();
        }
    }

    /// Writes the typed assertion on the open goal.
    fn assert_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (super::At::Goal(card), Some(asserting)) = (self.at, &self.asserting) else {
            return;
        };
        let text = asserting.input.read(cx).value().trim().to_owned();
        if text.is_empty() || self.busy {
            return;
        }
        let params = BoardAssertParams {
            project: self.folder.clone(),
            card,
            assertion: None,
            text: Some(text),
            evidence: Some(asserting.kind),
            retire: false,
        };
        let asked = Target::here(&self.asks).request_typed::<call::BoardAssert>(params);
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                pane.busy = false;
                match answer {
                    Ok(board) => {
                        pane.take(board);
                        if let Some(asserting) = &pane.asserting {
                            asserting
                                .input
                                .update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn assert_retire(&mut self, card: u64, assertion: u64, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let params = BoardAssertParams {
            project: self.folder.clone(),
            card,
            assertion: Some(assertion),
            text: None,
            evidence: None,
            retire: true,
        };
        self.ask::<call::BoardAssert>(params, |_| {}, cx);
    }
}

/// The goal's assertions in force, a line each, and the line that adds one.
pub(super) fn section_of(
    pane: &GoalsPane,
    goal: &CardView,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let kept: Vec<&Assertion> = goal
        .assertions
        .iter()
        .filter(|assertion| assertion.retired.is_none())
        .collect();
    let slices = pane.slices(goal.id);
    let card = goal.id;
    div()
        .flex()
        .flex_col()
        .child(section("Assertions", kept.len(), cx))
        .children(kept.into_iter().map(|assertion| {
            let id = assertion.id;
            let fulfilling: Vec<bool> = slices
                .iter()
                .filter_map(|(_, slice)| {
                    let asserted = slice
                        .asserts
                        .iter()
                        .find(|asserted| asserted.assertion == id);
                    asserted.map(|asserted| asserted.met)
                })
                .collect();
            let met = fulfilling.iter().filter(|met| **met).count();
            div()
                .py(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    ui::mono(
                        number(goal, id).unwrap_or_default().to_string(),
                        TEXT_2XS,
                        cx,
                    )
                    .w(px(16.))
                    .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::ui_text(TEXT_XS_H))
                        .text_color(t.text_primary)
                        .child(assertion.text.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            ui::icon(kind_icon(assertion.evidence), 11.)
                                .text_color(t.text_tertiary),
                        )
                        .child(ui::meta(kind_words(assertion.evidence), cx)),
                )
                .when(!fulfilling.is_empty(), |row| {
                    row.child(ui::meta(
                        format!("{met} of {} slices", fulfilling.len()),
                        cx,
                    ))
                })
                .child(
                    IconButton::new(
                        SharedString::from(format!("retire-assertion-{id}")),
                        Icon::Close,
                        "Retire",
                    )
                    .small()
                    .on_click(cx.listener(move |pane, _, _, cx| pane.assert_retire(card, id, cx))),
                )
        }))
        .children(pane.asserting.as_ref().map(|asserting| {
            let chosen = KINDS
                .iter()
                .position(|(kind, _)| *kind == asserting.kind)
                .unwrap_or(0);
            let words = KINDS
                .iter()
                .map(|(_, name)| (SharedString::from(*name), None))
                .collect();
            div()
                .pt(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(div().flex_1().min_w_0().child(Field::new(&asserting.input)))
                .child(
                    Segmented::words("assertion-kind", words, chosen).on_select(cx.listener(
                        |pane, chosen: &usize, _, cx| {
                            if let Some((kind, _)) = KINDS.get(*chosen) {
                                pane.assert_kind(*kind, cx);
                            }
                        },
                    )),
                )
                .child(
                    Button::new("assertion-add", "Add", Variant::Outline)
                        .disabled(pane.busy)
                        .on_click(cx.listener(|pane, _, window, cx| pane.assert_new(window, cx))),
                )
        }))
        .into_any_element()
}

/// The goal's assertions a slice fulfils, each ticked once its evidence held up.
pub(super) fn ticks(
    goal: &CardView,
    slice: &SliceView,
    cx: &Context<GoalsPane>,
) -> Option<AnyElement> {
    if slice.asserts.is_empty() {
        return None;
    }
    let t = Tokens::get(cx);
    Some(
        div()
            .pt(SPACE_3)
            .flex()
            .flex_col()
            .children(slice.asserts.iter().filter_map(|asserted| {
                let assertion = goal
                    .assertions
                    .iter()
                    .find(|kept| kept.id == asserted.assertion)?;
                let mark = match asserted.met {
                    true => ui::status_of_run("success", 13., cx),
                    false => ui::mark_box(
                        13.,
                        div()
                            .size(px(10.))
                            .rounded_full()
                            .border_1()
                            .border_dashed()
                            .border_color(t.text_tertiary),
                    )
                    .into_any_element(),
                };
                Some(
                    div()
                        .py(px(3.))
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(mark)
                        .child(
                            ui::mono(
                                number(goal, assertion.id).unwrap_or_default().to_string(),
                                TEXT_2XS,
                                cx,
                            )
                            .w(px(12.))
                            .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme::ui_text(TEXT_XS_H))
                                .text_color(if asserted.met {
                                    t.text_secondary
                                } else {
                                    t.text_primary
                                })
                                .child(assertion.text.clone()),
                        )
                        .child(ui::meta(kind_words(assertion.evidence), cx)),
                )
            }))
            .into_any_element(),
    )
}
