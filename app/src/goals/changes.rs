//! The changes the person asked for on a slice: numbered pins on the picture in front, and the
//! list under the evidence that its agent ticks off with proof, with the rules every agent keeps
//! folded at its end. Clicking the picture pins a new one where it was clicked.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ObjectFit, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, StyledImage,
};
use sikemux_core::server::board::BoardCard;
use sikemux_view::slice::{Evidence, Note, Proof, SliceView, Spot};

use super::{evidence, GoalsPane};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, IconButton};

const PICTURE_HEIGHT: f32 = 340.;
const PIN: f32 = 18.;

/// Where the picture in front was last laid out, which a click and the pins are placed by.
#[derive(Clone, Default)]
pub(super) struct Placed(Rc<Cell<Option<Bounds<Pixels>>>>);

/// The part of `frame` a picture of `size` fills when drawn whole and centred in it.
fn fitted(frame: Bounds<Pixels>, size: (u32, u32)) -> Bounds<Pixels> {
    let (width, height) = (f32::from(frame.size.width), f32::from(frame.size.height));
    let scale = (width / size.0 as f32).min(height / size.1 as f32);
    let (drawn_width, drawn_height) = (size.0 as f32 * scale, size.1 as f32 * scale);
    Bounds {
        origin: frame.origin
            + gpui_kit::point(
                px((width - drawn_width) / 2.),
                px((height - drawn_height) / 2.),
            ),
        size: gpui_kit::size(px(drawn_width), px(drawn_height)),
    }
}

/// A still picture in front, drawn large with its changes pinned on it. A click on it pins a
/// new change there; the eye opens it in the viewer.
pub(super) fn picture(
    pane: &GoalsPane,
    shown: &Evidence,
    slice: &SliceView,
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    let Proof::Picture {
        path,
        seconds: None,
        opened: page,
        ..
    } = &shown.proof
    else {
        return None;
    };
    let t = Tokens::get(cx).clone();
    let size = pane.picture_size(path);
    let placed = pane.placed.clone();
    let told = placed.clone();
    let drawn = placed
        .0
        .get()
        .zip(size)
        .map(|(frame, size)| fitted(frame, size));
    let at = |x: f32, y: f32| {
        drawn.map(|drawn| {
            (
                f32::from(drawn.origin.x - placed.0.get().unwrap_or_default().origin.x)
                    + x * f32::from(drawn.size.width),
                f32::from(drawn.origin.y - placed.0.get().unwrap_or_default().origin.y)
                    + y * f32::from(drawn.size.height),
            )
        })
    };
    let pins: Vec<AnyElement> = (slice.notes.iter())
        .filter(|note| note.evidence == Some(shown.id) && note.by.is_none())
        .filter_map(|note| match note.spot {
            Some(Spot::Point { x, y, .. }) => at(x, y).map(|spot| (note, spot)),
            _ => None,
        })
        .map(|(note, (x, y))| pin(note.id.to_string(), x, y, pin_colour(note, &t), cx))
        .chain(pane.spot.as_ref().and_then(|spot| match spot {
            Spot::Point { x, y, .. } => {
                at(*x, *y).map(|(x, y)| pin("+".to_owned(), x, y, t.accent, cx))
            }
            Spot::Line { .. } => None,
        }))
        .collect();
    let (viewed, id) = (path.clone(), shown.id);
    let frame = div()
        .id(SharedString::from(format!("evidence-{id}")))
        .relative()
        .w_full()
        .h(px(PICTURE_HEIGHT))
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .bg(t.surface_sunken)
        .cursor_crosshair()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |pane, event: &MouseDownEvent, window, cx| {
                let Some(drawn) = drawn else {
                    return;
                };
                if !drawn.contains(&event.position) {
                    return;
                }
                let along = |at: Pixels, from: Pixels, span: Pixels| {
                    (f32::from(at - from) / f32::from(span)).clamp(0., 1.)
                };
                pane.spot = Some(Spot::Point {
                    x: along(event.position.x, drawn.origin.x, drawn.size.width),
                    y: along(event.position.y, drawn.origin.y, drawn.size.height),
                    seconds: None,
                });
                pane.picked = Some(id);
                if pane.note.is_none() {
                    pane.write_note(window, cx);
                }
                cx.notify();
            }),
        )
        .child(
            img(std::path::PathBuf::from(path))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .children(pins)
        .child(
            div()
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    IconButton::new("evidence-view", Icon::Eye, "Open the picture").on_click(
                        cx.listener(move |pane, _, window, cx| {
                            pane.show_picture(&viewed, window, cx)
                        }),
                    ),
                ),
        );
    Some(
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                div()
                    .on_children_prepainted(move |bounds, window, _| {
                        let laid = bounds.first().copied();
                        if told.0.get() != laid {
                            told.0.set(laid);
                            window.refresh();
                        }
                    })
                    .child(frame),
            )
            .when_some(page.clone(), |column, page| {
                column.child(ui::meta(
                    format!("Sikemux opened {page} after the check"),
                    cx,
                ))
            })
            .into_any_element(),
    )
}

fn pin_colour(note: &Note, t: &Tokens) -> gpui_kit::Hsla {
    match note.ticked {
        Some(_) => t.live,
        None => t.warn,
    }
}

fn pin(
    label: String,
    x: f32,
    y: f32,
    colour: gpui_kit::Hsla,
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .absolute()
        .left(px(x - PIN / 2.))
        .top(px(y - PIN / 2.))
        .size(px(PIN))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(colour)
        .border_2()
        .border_color(t.surface_sunken)
        .child(ui::text(label, TEXT_2XS, FontWeight::SEMIBOLD).text_color(t.surface_sunken))
        .into_any_element()
}

/// Who an agent is, as the slice names it.
fn agent_name(agent: &str, card: &BoardCard) -> String {
    match (card.owner.as_deref() == Some(agent), &card.owner_name) {
        (true, Some(name)) => format!("@{name}"),
        _ => "its agent".to_owned(),
    }
}

/// Where a change is pinned, in a few words.
fn place(note: &Note, slice: &SliceView) -> Option<String> {
    let on = (slice.evidence.iter()).find(|evidence| Some(evidence.id) == note.evidence)?;
    let on = evidence::label(on).to_lowercase();
    Some(match &note.spot {
        Some(Spot::Point {
            seconds: Some(at), ..
        }) => format!("{at:.0}s into the {on}"),
        Some(Spot::Line { file, line }) => format!("{file}:{line}"),
        _ => format!("on the {on}"),
    })
}

/// The changes the person asked for, open ones first, then the rules folded, then the line
/// that asks for another.
pub(super) fn list(
    pane: &GoalsPane,
    card: &BoardCard,
    slice: &SliceView,
    rules: Vec<String>,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let mut changes: Vec<&Note> = (slice.notes.iter())
        .filter(|note| note.by.is_none())
        .collect();
    changes.sort_by_key(|note| (note.ticked.is_some(), note.id));
    let ticked = changes.iter().filter(|note| note.ticked.is_some()).count();
    let total = changes.len();
    div()
        .pt(SPACE_4)
        .flex()
        .flex_col()
        .when(total > 0, |list| {
            list.child(
                div()
                    .pb(SPACE_1)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(ui::caps("Changes", cx))
                    .child(ui::meta(format!("{ticked} of {total} ticked"), cx)),
            )
        })
        .children(changes.into_iter().map(|note| row(card, slice, note, cx)))
        .when(!rules.is_empty(), |list| {
            let count = rules.len();
            let open = pane.rules_open;
            list.child(
                div()
                    .id("changes-rules")
                    .px(SPACE_2)
                    .py(SPACE_1H)
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .hover(|row| row.bg(t.surface_hover))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.rules_open = !pane.rules_open;
                        cx.notify();
                    }))
                    .child(
                        ui::icon(Icon::Chevron, 10.)
                            .text_color(t.text_tertiary)
                            .when(!open, |chevron| {
                                chevron.with_transformation(gpui_kit::Transformation::rotate(
                                    gpui_kit::radians(-std::f32::consts::FRAC_PI_2),
                                ))
                            }),
                    )
                    .child(ui::meta(
                        match count {
                            1 => "1 standing rule, checked at review".to_owned(),
                            _ => format!("{count} standing rules, checked at review"),
                        },
                        cx,
                    )),
            )
            .when(open, |list| {
                list.children(rules.into_iter().map(|rule| {
                    div()
                        .pl(px(30.))
                        .pr(SPACE_2)
                        .py(SPACE_1)
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(rule)
                }))
            })
        })
        .when(pane.note.is_none(), |list| {
            list.child(
                div()
                    .id("changes-ask")
                    .mt(SPACE_2)
                    .px(SPACE_3)
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .rounded(theme::RADIUS_2)
                    .border_1()
                    .border_color(t.border)
                    .cursor_text()
                    .hover(|field| field.border_color(t.border_strong))
                    .on_click(cx.listener(|pane, _, window, cx| pane.write_note(window, cx)))
                    .child(ui::icon(Icon::Pencil, 12.).text_color(t.text_tertiary))
                    .child(ui::meta(
                        "Click the picture to pin a change, or write one for the whole slice",
                        cx,
                    )),
            )
        })
        .into_any_element()
}

/// One change: its tick, number and words; under them where it is pinned and who has it, or
/// how it was ticked off with the evidence that shows it, which Not done reopens.
fn row(
    card: &BoardCard,
    slice: &SliceView,
    note: &Note,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (id, card_id) = (note.id, card.id);
    let done = note.ticked.is_some();
    let mark = div().size(px(14.)).mt(px(2.)).flex_none().rounded_full();
    let mark = match done {
        true => mark
            .bg(t.live)
            .flex()
            .items_center()
            .justify_center()
            .child(ui::icon(Icon::Check, 9.).text_color(t.surface_sunken)),
        false => mark
            .border_1()
            .border_dashed()
            .border_color(t.text_tertiary),
    };
    let under = match &note.ticked {
        Some(tick) => [Some(format!("ticked by {}", agent_name(&tick.by, card)))]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        None => [
            place(note, slice),
            Some(ui::time::age(note.at)),
            note.to
                .as_deref()
                .map(|to| format!("to {}", agent_name(to, card))),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    let proof = (note.ticked.as_ref())
        .and_then(|tick| tick.evidence)
        .and_then(|shown| slice.evidence.iter().find(|evidence| evidence.id == shown));
    div()
        .id(SharedString::from(format!("change-{id}")))
        .group("change")
        .px(SPACE_2)
        .py(SPACE_1H)
        .flex()
        .items_start()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .hover(|row| row.bg(t.surface_hover))
        .child(mark)
        .child(div().w(px(16.)).flex_none().pt(px(1.)).child(
            ui::text(id.to_string(), TEXT_XS, FontWeight::MEDIUM).text_color(if done {
                t.live
            } else {
                t.warn
            }),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_SM))
                        .text_color(if done {
                            t.text_secondary
                        } else {
                            t.text_primary
                        })
                        .child(note.text.clone()),
                )
                .child(ui::meta(under.join(" · "), cx))
                .children(note.ticked.as_ref().map(|tick| {
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(format!("“{}”", tick.proof))
                })),
        )
        .children(proof.map(|proof| {
            let shown = proof.id;
            div()
                .id(SharedString::from(format!("change-proof-{id}")))
                .flex_none()
                .cursor_pointer()
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.picked = Some(shown);
                    pane.spot = None;
                    cx.notify();
                }))
                .child(evidence::thumb_face(proof, false, cx).w(px(46.)).h(px(30.)))
        }))
        .when(done, |row| {
            row.child(
                div()
                    .invisible()
                    .group_hover("change", |button| button.visible())
                    .child(
                        ui::Button::new(
                            SharedString::from(format!("change-reopen-{id}")),
                            "Not done",
                            ui::Variant::Outline,
                        )
                        .on_click(
                            cx.listener(move |pane, _, _, cx| pane.reopen_change(card_id, id, cx)),
                        ),
                    ),
            )
        })
        .into_any_element()
}
