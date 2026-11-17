//! A code block too long for its box scrolls inside it, up and down and sideways, under a swipe
//! that starts over it.

use std::rc::Rc;

use gpui::{
    div, point, px, AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement,
    Pixels, Point, Render, ScrollDelta, ScrollEvent, Styled, TestAppContext, TouchPhase,
    VisualTestContext, Window,
};
use gpui_transcript::{Markdown, Pen, Row, Style, Transcript};
use serde_json::json;

struct Message(Markdown);

impl Row for Message {
    fn id(&self) -> &str {
        "code"
    }

    fn docs(&self) -> Vec<&Markdown> {
        vec![&self.0]
    }
}

struct Shell(Entity<Transcript<Message>>);

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn draw_row(
    rows: &[Message],
    ix: usize,
    pen: &Pen,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match rows.get(ix) {
        Some(Message(doc)) => div()
            .w_full()
            .child(pen.markdown("code", doc, window, cx))
            .into_any_element(),
        None => div().into_any_element(),
    }
}

/// A two-finger swipe over `at`, as a trackpad sends it.
fn swipe(cx: &mut VisualTestContext, at: Point<Pixels>, by: Point<Pixels>) {
    for (touch_phase, delta) in [
        (TouchPhase::Started, Point::default()),
        (TouchPhase::Moved, by),
        (TouchPhase::Ended, Point::default()),
    ] {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(delta),
            touch_phase,
            ..Default::default()
        });
    }
}

#[gpui::test]
fn a_long_code_block_scrolls_both_ways(cx: &mut TestAppContext) {
    let code = vec!["x".repeat(2_000); 200].join("\n");
    let doc = Markdown::new(&json!([{ "t": "pre", "lang": "rust", "v": code }]), 0);
    let transcript =
        cx.new(|cx| Transcript::new(vec![Message(doc)], Rc::new(Style::default()), draw_row, cx));
    let (_, cx) = cx.add_window_view(|_, _| Shell(transcript));

    let code_at = |cx: &mut VisualTestContext| {
        cx.debug_bounds("code text")
            .expect("the code block is drawn")
            .origin
    };
    let start = code_at(cx);
    let over = start + point(px(100.), px(100.));

    swipe(cx, over, point(px(0.), px(-150.)));
    assert_eq!(
        code_at(cx),
        start + point(px(0.), px(-150.)),
        "a swipe up over a long code block moves its lines up"
    );

    swipe(cx, over, point(px(-150.), px(0.)));
    assert_eq!(
        code_at(cx),
        start + point(px(-150.), px(-150.)),
        "a swipe left over a long code block moves its lines left"
    );
}
