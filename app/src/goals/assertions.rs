//! A goal's assertions: what must hold once its slices are done, each with the kind of evidence
//! that shows it. The goal lists them and the person adds or retires one; a slice ticks the ones
//! it fulfils as their evidence comes in, and each piece of evidence says which it serves. Each is
//! known by its number in the goal's list. A command an agent wrote waits here for the person to
//! accept it or turn it down.

use crate::board::ChangesBoards;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window,
};
use sikemux_core::server::board::goal::AnswerCommandParams;
use sikemux_core::server::board::AssertParams as BoardAssertParams;
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::call;
use sikemux_view::slice::{Assertion, Collect, NamedFile, SliceView};

use super::goal::quiet_section;
use super::GoalsPane;
use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_XS_H};
use crate::ui::{self, Button, Field, IconButton, Segmented, Variant};

/// The kinds of evidence the person picks from; a workflow also needs its name, a command itself.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Check,
    Recording,
    Screenshot,
    Workflow,
    Command,
}

const KINDS: [(Kind, &str); 5] = [
    (Kind::Check, "Check"),
    (Kind::Recording, "Recording"),
    (Kind::Screenshot, "Screenshot"),
    (Kind::Workflow, "CI workflow"),
    (Kind::Command, "Command"),
];

pub(super) struct Asserting {
    input: Entity<InputState>,
    /// The CI workflow's name, while the kind picked is a workflow.
    workflow: Entity<InputState>,
    /// The command to run, while the kind picked is a command.
    command: Entity<InputState>,
    kind: Kind,
    _typing: [Subscription; 3],
}

fn kind_words(kind: &Collect) -> String {
    match kind {
        Collect::Check => "the check passes".into(),
        Collect::Recording => "a recording".into(),
        Collect::Screenshot => "a screenshot".into(),
        Collect::Workflow { name } => format!("CI {name} passes"),
        Collect::Command { command } => format!("{command} passes"),
    }
}

fn kind_icon(kind: &Collect) -> Icon {
    match kind {
        Collect::Check | Collect::Workflow { .. } => Icon::Check,
        Collect::Recording => Icon::Record,
        Collect::Screenshot => Icon::Image,
        Collect::Command { .. } => Icon::Terminal,
    }
}

/// The number an assertion goes by: its place in the goal's list.
pub(super) fn number(goal: &BoardCard, assertion: u64) -> Option<usize> {
    goal.assertions
        .iter()
        .position(|kept| kept.id == assertion)
        .map(|at| at + 1)
}

/// The numbers of the assertions, as "1, 3".
pub(super) fn numbers(goal: &BoardCard, assertions: &[u64]) -> Option<String> {
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
        let workflow = cx.new(|cx| InputState::new(window, cx).placeholder("Workflow name"));
        let command = cx
            .new(|cx| InputState::new(window, cx).placeholder("Command whose exit code shows it"));
        let entered = |pane: &mut Self,
                       _: &Entity<InputState>,
                       event: &InputEvent,
                       window: &mut Window,
                       cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                pane.assert_new(window, cx);
            }
        };
        let typing = [
            cx.subscribe_in(&input, window, entered),
            cx.subscribe_in(&workflow, window, entered),
            cx.subscribe_in(&command, window, entered),
        ];
        self.asserting = Some(Asserting {
            input,
            workflow,
            command,
            kind: Kind::Check,
            _typing: typing,
        });
    }

    fn assert_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
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
        let evidence = match asserting.kind {
            Kind::Check => Collect::Check,
            Kind::Recording => Collect::Recording,
            Kind::Screenshot => Collect::Screenshot,
            Kind::Workflow => {
                let name = asserting.workflow.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    return self.said("Name the CI workflow whose pass shows it", cx);
                }
                Collect::Workflow { name }
            }
            Kind::Command => {
                let command = asserting.command.read(cx).value().trim().to_owned();
                if command.is_empty() {
                    return self.said("Say the command whose exit code shows it", cx);
                }
                Collect::Command { command }
            }
        };
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = BoardAssertParams {
            project,
            card,
            assertion: None,
            text: Some(text),
            evidence: Some(evidence),
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
                        pane.take_one(board);
                        if let Some(asserting) = &pane.asserting {
                            for input in [&asserting.input, &asserting.workflow, &asserting.command]
                            {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
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
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = BoardAssertParams {
            project,
            card,
            assertion: Some(assertion),
            text: None,
            evidence: None,
            retire: true,
        };
        self.ask_moved::<call::BoardAssert>(params, |_| {}, cx);
    }

    /// Answers the command the person sees, which the core holds them to.
    fn answer_command(
        &mut self,
        card: u64,
        assertion: u64,
        command: String,
        files: Vec<NamedFile>,
        accept: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let Some(project) = self.here.clone() else {
            return;
        };
        let params = AnswerCommandParams {
            project,
            card,
            assertion,
            command,
            files,
            accept,
        };
        self.ask_moved::<call::BoardAnswerCommand>(params, |_| {}, cx);
    }
}

/// What of an assertion waits for the person, with what answers it, as what an agent proposes for
/// a project's environments shows: the command an agent wrote, whole, or the files a command names
/// that a slice changed.
fn proposal(
    pane: &GoalsPane,
    goal: &BoardCard,
    assertion: &Assertion,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    if !assertion.needs_person() {
        return None;
    }
    let command = assertion.command()?.to_owned();
    let proposing = assertion.awaits_approval().is_some();
    let mut files: Vec<NamedFile> = Vec::new();
    for changed in &assertion.files.changed {
        if !files.contains(&changed.file) {
            files.push(changed.file.clone());
        }
    }
    let t = Tokens::get(cx).clone();
    let (card, id, busy) = (goal.id, assertion.id, pane.busy);
    let by = assertion.learned.name.as_deref().unwrap_or("An agent");
    let number = number(goal, id).unwrap_or_default();
    let on_accept = (command.clone(), files.clone());
    let on_decline = (command.clone(), files);
    let says = match proposing {
        true => format!(
            "@{by} proposes this command for assertion {number}. It runs and counts once you accept it."
        ),
        false => format!(
            "A file assertion {number}'s command names changed. It runs on that commit once you accept it."
        ),
    };
    let details: Vec<String> = match proposing {
        true => (assertion.files.proposed.iter())
            .map(|file| format!("Accepts {} as it stood when this was proposed", file.path))
            .chain(
                assertion
                    .approved
                    .iter()
                    .map(|earlier| format!("Turning it down keeps {earlier}")),
            )
            .collect(),
        false => (assertion.files.changed.iter())
            .map(|changed| match changed.file.blob {
                Some(_) => format!(
                    "{} changed on #{} at {}",
                    changed.file.path,
                    changed.card,
                    changed.head.get(..7).unwrap_or(&changed.head)
                ),
                None => format!(
                    "{} is gone on #{} at {}",
                    changed.file.path,
                    changed.card,
                    changed.head.get(..7).unwrap_or(&changed.head)
                ),
            })
            .chain(std::iter::once(
                "Turning it down keeps the command from running on what you see here".to_owned(),
            ))
            .collect(),
    };
    Some(
        div()
            .py(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(
                div()
                    .text_size(theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_primary)
                    .child(says),
            )
            .child(ui::mono(command, TEXT_2XS, cx).whitespace_normal())
            .children(
                details
                    .into_iter()
                    .map(|detail| ui::meta(detail, cx).whitespace_normal()),
            )
            .child(
                div()
                    .pt(SPACE_1H)
                    .flex()
                    .gap(SPACE_2)
                    .child(
                        Button::new(
                            SharedString::from(format!("assertion-accept-{id}")),
                            "Accept",
                            Variant::Primary,
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            let (command, files) = on_accept.clone();
                            pane.answer_command(card, id, command, files, true, cx)
                        })),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("assertion-decline-{id}")),
                            "Turn down",
                            Variant::Outline,
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            let (command, files) = on_decline.clone();
                            pane.answer_command(card, id, command, files, false, cx)
                        })),
                    ),
            )
            .into_any_element(),
    )
}

/// The goal's assertions in force, a line each, and the line that adds one.
pub(super) fn section_of(
    pane: &GoalsPane,
    goal: &BoardCard,
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
    let proposals: Vec<Option<AnyElement>> = (kept.iter())
        .map(|assertion| proposal(pane, goal, assertion, cx))
        .collect();
    div()
        .flex()
        .flex_col()
        .child(quiet_section("Assertions", kept.len(), cx))
        .children(
            kept.into_iter()
                .zip(proposals)
                .map(|(assertion, proposal)| {
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
                    let line = div()
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
                                    ui::icon(kind_icon(&assertion.evidence), 11.)
                                        .text_color(t.text_tertiary),
                                )
                                .child(
                                    ui::meta(kind_words(&assertion.evidence), cx)
                                        .max_w(px(220.))
                                        .truncate(),
                                ),
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
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.assert_retire(card, id, cx)),
                            ),
                        );
                    div().flex().flex_col().child(line).children(proposal)
                }),
        )
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
                .flex_col()
                .gap(SPACE_1H)
                .child(Field::new(&asserting.input))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(Segmented::words("assertion-kind", words, chosen).on_select(
                            cx.listener(|pane, chosen: &usize, _, cx| {
                                if let Some((kind, _)) = KINDS.get(*chosen) {
                                    pane.assert_kind(*kind, cx);
                                }
                            }),
                        ))
                        .when(asserting.kind == Kind::Workflow, |line| {
                            line.child(div().w(px(180.)).child(Field::new(&asserting.workflow)))
                        })
                        .when(asserting.kind == Kind::Command, |line| {
                            line.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Field::new(&asserting.command)),
                            )
                        })
                        .when(asserting.kind != Kind::Command, |line| {
                            line.child(div().flex_1())
                        })
                        .child(
                            Button::new("assertion-add", "Add", Variant::Outline)
                                .disabled(pane.busy)
                                .on_click(
                                    cx.listener(|pane, _, window, cx| pane.assert_new(window, cx)),
                                ),
                        ),
                )
        }))
        .into_any_element()
}

/// The goal's assertions a slice fulfils, each ticked once its evidence held up, then quietly
/// those it named that no longer stand on its goal, so the person sees what was freed.
pub(super) fn ticks(
    pane: &GoalsPane,
    goal: &BoardCard,
    slice: &SliceView,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    if slice.asserts.is_empty() && slice.detached.is_empty() {
        return None;
    }
    let proposals: Vec<AnyElement> = (slice.asserts.iter())
        .filter(|asserted| asserted.awaits_approval)
        .filter_map(|asserted| {
            goal.assertions
                .iter()
                .find(|kept| kept.id == asserted.assertion)
        })
        .filter_map(|assertion| proposal(pane, goal, assertion, cx))
        .collect();
    let cx = &*cx;
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
                        .child(
                            ui::meta(
                                match asserted.awaits_approval {
                                    true => "awaits your approval".to_owned(),
                                    false => kind_words(&assertion.evidence),
                                },
                                cx,
                            )
                            .max_w(px(220.))
                            .truncate(),
                        ),
                )
            }))
            .children(proposals)
            .children(slice.detached.iter().map(|id| {
                let kept = goal.assertions.iter().find(|kept| kept.id == *id);
                let words = kept.map_or("an assertion no longer on this goal".into(), |kept| {
                    kept.text.clone()
                });
                div()
                    .py(px(3.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(ui::mark_box(13., div()))
                    .child(div().w(px(12.)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::ui_text(TEXT_XS_H))
                            .text_color(t.text_disabled)
                            .line_through()
                            .child(words),
                    )
                    .child(ui::meta("freed", cx))
            }))
            .into_any_element(),
    )
}
