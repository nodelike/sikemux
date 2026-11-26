//! Hovering a part's reply button in narrow and wide panes stays steady without oscillating hover
//! state, keeps the button clear of both the paragraph text and the scrollbar, and lets clicks
//! reach the button reliably.

#![allow(
    clippy::expect_used,
    reason = "a failing test helper is meant to panic"
)]

use std::rc::Rc;

use gpui::{
    div, point, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    Modifiers, ParentElement, Pixels, Render, Styled, TestAppContext, Window,
};
use gpui_clicks::Clicks;
use gpui_transcript::{Markdown, Pen, Reply, Row, Style, Transcript};
use serde_json::json;

const ROW_MAX: f32 = 800.;
const ROW_SIDE: f32 = 22.;
const SCROLLBAR: f32 = 16.;
const REPLY_GUTTER: f32 = 32.;
const REPLY_BUTTON: f32 = 24.;

fn reply_outset(width: Option<Pixels>) -> Pixels {
    let Some(width) = width else {
        return px(REPLY_GUTTER);
    };
    let beside = (width - px(ROW_MAX)).max(px(0.)) / 2. + px(ROW_SIDE);
    (beside - px(SCROLLBAR)).clamp(px(0.), px(REPLY_GUTTER))
}

struct Message(Markdown);

impl Row for Message {
    fn id(&self) -> &str {
        "reply"
    }

    fn docs(&self) -> Vec<&Markdown> {
        vec![&self.0]
    }
}

#[derive(Default)]
struct HoverState {
    part: Option<usize>,
    button: Option<usize>,
    transitions: usize,
    button_clicks: usize,
    link_clicks: usize,
}

struct Shell {
    width: Pixels,
    transcript: Entity<Transcript<Message>>,
    _hover: Entity<HoverState>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.width)
            .h(px(300.))
            .child(self.transcript.clone())
    }
}

fn build_harness(
    cx: &mut TestAppContext,
    width: Pixels,
    doc: Markdown,
) -> (Entity<HoverState>, Entity<Transcript<Message>>) {
    let hover = cx.new(|_| HoverState::default());
    let draw_hover = hover.clone();
    let link_hover = hover.clone();
    let style = Rc::new(Style {
        on_link: Some(Rc::new(move |_, _, cx| {
            link_hover.update(cx, |state, _| state.link_clicks += 1);
        })),
        ..Style::default()
    });
    let draw_row = move |rows: &[Message],
                         ix: usize,
                         pen: &Pen,
                         window: &mut Window,
                         cx: &mut App|
          -> AnyElement {
        let Some(Message(doc)) = rows.get(ix) else {
            return div().into_any_element();
        };
        let state = draw_hover.read(cx);
        let hovered = state.button.or(state.part);
        let pointed = state.button.is_some();
        let on_hover = {
            let hover = draw_hover.clone();
            Rc::new(
                move |place: usize, over: bool, _: &mut Window, cx: &mut App| {
                    hover.update(cx, |state, cx| {
                        let here = Some(place);
                        if over {
                            if state.part == here {
                                return;
                            }
                            state.part = here;
                            if state.button != state.part {
                                state.button = None;
                            }
                        } else if state.part == here {
                            state.part = None;
                        } else {
                            return;
                        }
                        state.transitions += 1;
                        cx.notify();
                    });
                },
            )
        };
        let on_point = {
            let hover = draw_hover.clone();
            Rc::new(
                move |place: usize, over: bool, _: &mut Window, cx: &mut App| {
                    hover.update(cx, |state, cx| {
                        let here = Some(place);
                        if over {
                            if state.button == here {
                                return;
                            }
                            state.button = here;
                        } else if state.button == here {
                            state.button = None;
                        } else {
                            return;
                        }
                        state.transitions += 1;
                        cx.notify();
                    });
                },
            )
        };
        let button = {
            let hover = draw_hover.clone();
            Rc::new(
                move |place: usize, _: String, _: bool, _: &mut Window, _: &mut App| {
                    let hover = hover.clone();
                    div()
                        .id(("reply-btn", place))
                        .debug_selector(|| "reply-btn".into())
                        .w(px(REPLY_BUTTON))
                        .h(px(26.))
                        .clicks(move |_, _, cx| {
                            hover.update(cx, |state, _| state.button_clicks += 1);
                        })
                        .into_any_element()
                },
            )
        };
        let look = Rc::new(Style {
            reply: Some(Reply {
                hovered,
                pointed,
                chosen: Rc::from([]),
                gutter: px(REPLY_GUTTER),
                outset: reply_outset(pen.width()),
                on_hover,
                on_point,
                button,
            }),
            ..(*pen.style()).clone()
        });
        div()
            .debug_selector(|| "row-0".into())
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .debug_selector(|| "col-0".into())
                    .w_full()
                    .max_w(px(ROW_MAX))
                    .px(px(ROW_SIDE))
                    .child(pen.markdown_with(&look, "msg", doc, window, cx)),
            )
            .into_any_element()
    };

    let transcript = cx.new(|cx| Transcript::new(vec![Message(doc)], style, draw_row, cx));
    let observed = transcript.clone();
    cx.update(|cx| {
        cx.observe(&hover, move |_, cx| {
            observed.update(cx, |_, cx| cx.notify());
        })
        .detach();
    });

    let _ = width;
    (hover, transcript)
}

#[gpui::test]
fn narrow_reply_button_stays_hovered_without_flickering_or_overlapping_text(
    cx: &mut TestAppContext,
) {
    let doc = Markdown::new(
        &json!([{
            "t": "p",
            "c": [{
                "t": "a",
                "href": "https://example.com",
                "c": ["question-one question-two question-three question-four question-five question-six question-seven"]
            }]
        }]),
        0,
    );

    let width = px(360.);
    let (hover, transcript) = build_harness(cx, width, doc);
    let (_, cx) = cx.add_window_view(|_, _| Shell {
        width,
        transcript,
        _hover: hover.clone(),
    });
    cx.run_until_parked();

    let row_bounds = cx.debug_bounds("row-0").expect("row is drawn");
    let text_point = row_bounds.origin + point(px(60.), px(10.));

    // Hover the paragraph so its reply button mounts beside the text.
    cx.simulate_mouse_move(text_point, None, Modifiers::none());
    cx.run_until_parked();

    let button_bounds = cx
        .debug_bounds("reply-btn")
        .expect("reply button mounts when hovering the paragraph");
    let button_center = button_bounds.center();

    // Button stays clear of the 16px scrollbar on the right edge of the narrow pane.
    assert!(
        button_bounds.right() <= row_bounds.right() - px(SCROLLBAR) + px(0.5),
        "reply button right ({:?}) must stay clear of scrollbar ({:?})",
        button_bounds.right(),
        row_bounds.right() - px(SCROLLBAR)
    );

    // Move onto the reply button and let frames settle without flickering.
    cx.simulate_mouse_move(button_center, None, Modifiers::none());
    cx.run_until_parked();
    let settled = cx.read(|cx| hover.read(cx).transitions);

    for _ in 0..5 {
        let _ = cx.refresh();
        cx.run_until_parked();
    }

    let after = cx.read(|cx| {
        let state = hover.read(cx);
        (state.button, state.part, state.transitions)
    });
    assert_eq!(after.0, Some(0), "button stays pointed in narrow pane");
    assert_eq!(
        after.2, settled,
        "hover state must not oscillate across frames while parked on the reply button"
    );
    assert!(
        cx.debug_bounds("reply-btn").is_some(),
        "reply button stays mounted across frames"
    );

    // Clicking the button invokes the reply action without clicking any link underneath.
    cx.simulate_click(button_center, Modifiers::none());
    cx.run_until_parked();
    let (button_clicks, link_clicks) = cx.read(|cx| {
        let state = hover.read(cx);
        (state.button_clicks, state.link_clicks)
    });
    assert_eq!(button_clicks, 1);
    assert_eq!(link_clicks, 0);

    // The 8px gap immediately left of the button has no link text under it (text wrapped earlier).
    let gap_point = point(button_bounds.left() - px(4.), button_center.y);
    cx.simulate_click(gap_point, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| hover.read(cx).link_clicks),
        0,
        "paragraph text must wrap left of the gutter so nothing sits under or flush against the button"
    );

    // Clicking inside the wrapped text still activates the link.
    cx.simulate_click(text_point, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| hover.read(cx).link_clicks), 1);

    // Moving straight back onto the text keeps the button mounted and clears `pointed`.
    cx.simulate_mouse_move(text_point, None, Modifiers::none());
    cx.run_until_parked();
    let (button_hover, part_hover) = cx.read(|cx| {
        let state = hover.read(cx);
        (state.button, state.part)
    });
    assert_eq!(button_hover, None);
    assert_eq!(part_hover, Some(0));

    // Moving off the part hides the button.
    let away = point(px(10.), px(10.));
    cx.simulate_mouse_move(away, None, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("reply-btn").is_none());

    // Entering directly at the button's position from outside settles onto the button cleanly.
    cx.simulate_mouse_move(button_center, None, Modifiers::none());
    cx.run_until_parked();
    let settled_direct = cx.read(|cx| hover.read(cx).transitions);
    for _ in 0..5 {
        let _ = cx.refresh();
        cx.run_until_parked();
    }
    let (direct_button, direct_transitions) = cx.read(|cx| {
        let state = hover.read(cx);
        (state.button, state.transitions)
    });
    assert_eq!(direct_button, Some(0));
    assert_eq!(direct_transitions, settled_direct);
}

#[gpui::test]
fn wide_reply_button_sits_outside_the_text_column(cx: &mut TestAppContext) {
    let doc = Markdown::new(
        &json!([{
            "t": "p",
            "c": ["A wide paragraph whose reply button sits cleanly in the outer right margin."]
        }]),
        0,
    );

    let width = px(960.);
    let (hover, transcript) = build_harness(cx, width, doc);
    let (_, cx) = cx.add_window_view(|_, _| Shell {
        width,
        transcript,
        _hover: hover.clone(),
    });
    cx.run_until_parked();

    let col_bounds = cx.debug_bounds("col-0").expect("column is drawn");
    let text_right = col_bounds.right() - px(ROW_SIDE);
    let text_point = col_bounds.origin + point(px(60.), px(10.));

    cx.simulate_mouse_move(text_point, None, Modifiers::none());
    cx.run_until_parked();

    let button_bounds = cx
        .debug_bounds("reply-btn")
        .expect("reply button mounts in wide pane");
    assert!(
        button_bounds.left() >= text_right + px(REPLY_GUTTER - REPLY_BUTTON) - px(0.5),
        "wide reply button ({:?}) must sit outside the text column right edge ({:?})",
        button_bounds.left(),
        text_right
    );
}

#[gpui::test]
fn tall_narrow_message_keeps_reply_button_clear_of_text(cx: &mut TestAppContext) {
    let blocks: Vec<_> = (0..14)
        .map(|ix| {
            json!({
                "t": "p",
                "c": [{
                    "t": "a",
                    "href": "https://example.com",
                    "c": [format!("block-{ix} question-one question-two question-three question-four question-five question-six")]
                }]
            })
        })
        .collect();
    let doc = Markdown::new(&json!(blocks), 0);

    let width = px(360.);
    let (hover, transcript) = build_harness(cx, width, doc);
    let (_, cx) = cx.add_window_view(|_, _| Shell {
        width,
        transcript,
        _hover: hover.clone(),
    });
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.run_until_parked();

    let row_bounds = cx.debug_bounds("row-0").expect("row is drawn");
    // Hover the bottom visible paragraph (Tall renders visible blocks in the 300px viewport).
    let probe = point(row_bounds.left() + px(60.), px(280.));
    cx.simulate_mouse_move(probe, None, Modifiers::none());
    cx.run_until_parked();

    let _mounted = cx
        .read(|cx| hover.read(cx).part)
        .expect("a visible paragraph in Tall is hovered");
    let button_bounds = cx
        .debug_bounds("reply-btn")
        .expect("reply button mounts in Tall narrow pane");
    assert!(
        button_bounds.right() <= row_bounds.right() - px(SCROLLBAR) + px(0.5),
        "Tall narrow button right ({:?}) must stay clear of scrollbar ({:?})",
        button_bounds.right(),
        row_bounds.right() - px(SCROLLBAR)
    );
    let gap_point = point(button_bounds.left() - px(4.), button_bounds.center().y);
    cx.simulate_click(gap_point, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| hover.read(cx).link_clicks),
        0,
        "Tall narrow paragraph text must stay left of the reply button gutter"
    );
}
