//! Hovering a part's reply button in a narrow pane, where the button overlaps the end of the line,
//! stays steady without oscillating hover state, blocks clicks on links beneath it, and still
//! allows clicks on the button itself.

#![allow(
    clippy::expect_used,
    reason = "a failing test helper is meant to panic"
)]

use std::rc::Rc;

use gpui::{
    div, point, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    Modifiers, ParentElement, Render, Styled, TestAppContext, Window,
};
use gpui_clicks::Clicks;
use gpui_transcript::{Markdown, Pen, Reply, Row, Style, Transcript};
use serde_json::json;

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
    transcript: Entity<Transcript<Message>>,
    _hover: Entity<HoverState>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(360.)).h(px(300.)).child(self.transcript.clone())
    }
}

#[gpui::test]
fn narrow_reply_button_stays_hovered_without_flickering(cx: &mut TestAppContext) {
    let doc = Markdown::new(
        &json!([{
            "t": "ul",
            "c": [
                {
                    "t": "li",
                    "c": [{
                        "t": "p",
                        "c": [
                            "What changed: metadata and config moved to ",
                            { "t": "a", "href": "https://example.com", "c": ["https://example.com/very-long-link-at-the-right-edge"] }
                        ]
                    }]
                },
                {
                    "t": "li",
                    "c": [{ "t": "p", "c": ["Second bullet item below it."] }]
                }
            ]
        }]),
        0,
    );

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
                        .debug_selector(|| format!("reply-btn-{place}"))
                        .w(px(24.))
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
                gutter: px(6.),
                on_hover,
                on_point,
                button,
            }),
            ..(*pen.style()).clone()
        });
        div()
            .debug_selector(|| "row-0".into())
            .w_full()
            .px(px(22.))
            .child(pen.markdown_with(&look, "narrow", doc, window, cx))
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

    let (_, cx) = cx.add_window_view(|_, _| Shell {
        transcript,
        _hover: hover.clone(),
    });
    cx.run_until_parked();

    let row_bounds = cx.debug_bounds("row-0").expect("row is drawn");
    let text_point = row_bounds.origin + point(px(60.), px(10.));

    // Hover the first list item so its reply button mounts over the narrow right edge.
    cx.simulate_mouse_move(text_point, None, Modifiers::none());
    cx.run_until_parked();

    let button_bounds = cx
        .debug_bounds("reply-btn-0")
        .expect("reply button mounts when hovering the paragraph");
    let button_center = button_bounds.center();

    // Move onto the overlapping reply button and let frames settle.
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
    assert_eq!(
        after.0,
        Some(0),
        "button stays pointed over the narrow edge"
    );
    assert_eq!(
        after.2, settled,
        "hover state must not oscillate across frames while parked on the reply button"
    );
    assert!(
        cx.debug_bounds("reply-btn-0").is_some(),
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
    assert!(cx.debug_bounds("reply-btn-0").is_none());

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
