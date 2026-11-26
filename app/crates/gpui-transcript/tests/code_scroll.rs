//! A code block too long for its box scrolls inside it, up and down and sideways, under a swipe
//! that starts over it.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, point, px, size, AnyElement, App, AppContext, Context, Entity, IntoElement, Modifiers,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollEvent, Styled,
    TestAppContext, TouchPhase, VisualTestContext, Window,
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

/// Draws `count` frames, as the screen asks for them.
fn frames(cx: &mut VisualTestContext, count: usize) {
    for _ in 0..count {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
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

#[gpui::test]
fn a_selection_dragged_past_a_code_blocks_side_scrolls_it_sideways(cx: &mut TestAppContext) {
    let code = vec!["x".repeat(2_000); 3].join("\n");
    let doc = Markdown::new(&json!([{ "t": "pre", "lang": "rust", "v": code }]), 0);
    let transcript =
        cx.new(|cx| Transcript::new(vec![Message(doc)], Rc::new(Style::default()), draw_row, cx));
    let (_, cx) = cx.add_window_view({
        let transcript = transcript.clone();
        |_, _| Shell(transcript)
    });
    cx.simulate_resize(size(px(600.), px(400.)));
    cx.run_until_parked();

    let code_at = |cx: &mut VisualTestContext| {
        cx.debug_bounds("code text")
            .expect("the code block is drawn")
            .origin
    };
    let start = code_at(cx);
    let press = start + point(px(20.), px(10.));
    cx.simulate_mouse_down(press, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(px(700.), press.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    frames(cx, 10);
    let held = code_at(cx);
    assert!(
        held.x < start.x - px(100.),
        "the code moved from {start:?} only to {held:?}"
    );
    let selected = cx
        .update(|window, cx| transcript.read(cx).focused_selection(window))
        .unwrap_or_default();
    let shown = f32::from(px(600.) - start.x) as usize / 6;
    assert!(
        selected.len() > shown,
        "only {} of the line is selected",
        selected.len()
    );

    cx.simulate_mouse_up(
        point(px(700.), press.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    let let_go = code_at(cx);
    frames(cx, 10);
    assert_eq!(code_at(cx), let_go, "letting go stops the scroll");
}
