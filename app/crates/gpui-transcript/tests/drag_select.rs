//! Dragging a selection past the top or bottom of the transcript scrolls it, and keeps
//! scrolling while the pointer is held there, so a message taller than the view selects whole.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, point, px, size, AnyElement, App, AppContext, Context, Entity, IntoElement, Modifiers,
    MouseButton, ParentElement, Render, ScrollDelta, ScrollEvent, Styled, TestAppContext,
    VisualTestContext, Window,
};
use gpui_transcript::{Markdown, Pen, Row, Style, Transcript};
use serde_json::json;

struct Message(String, Markdown);

impl Row for Message {
    fn id(&self) -> &str {
        &self.0
    }

    fn docs(&self) -> Vec<&Markdown> {
        vec![&self.1]
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
        Some(Message(id, doc)) => div()
            .w_full()
            .child(pen.markdown(id.clone(), doc, window, cx))
            .into_any_element(),
        None => div().into_any_element(),
    }
}

fn transcript(cx: &mut TestAppContext) -> (Entity<Transcript<Message>>, &mut VisualTestContext) {
    let rows = (0..300)
        .map(|n| {
            let text = format!("line {n}");
            let doc = Markdown::new(&json!([{ "t": "p", "c": [text] }]), 0);
            Message(format!("row-{n}"), doc)
        })
        .collect();
    let transcript = cx.new(|cx| Transcript::new(rows, Rc::new(Style::default()), draw_row, cx));
    let (_, cx) = cx.add_window_view({
        let transcript = transcript.clone();
        |_, _| Shell(transcript)
    });
    cx.simulate_resize(size(px(600.), px(400.)));
    cx.run_until_parked();
    (transcript, cx)
}

/// The reader scrolls up from the latest rows with the wheel.
fn scroll_up(cx: &mut VisualTestContext) {
    for _ in 0..8 {
        cx.simulate_event(ScrollEvent {
            position: point(px(40.), px(200.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(1_000.))),
            ..Default::default()
        });
        cx.run_until_parked();
    }
}

fn top_row(transcript: &Entity<Transcript<Message>>, cx: &mut VisualTestContext) -> usize {
    transcript.read_with(cx, |transcript, _| {
        transcript.list().logical_scroll_top().item_ix
    })
}

fn hold_below(cx: &mut VisualTestContext) {
    cx.simulate_mouse_down(
        point(px(40.), px(20.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(40.), px(380.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(40.), px(460.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
    }
}

#[gpui::test]
fn holding_a_drag_below_the_transcript_keeps_scrolling_and_selecting(cx: &mut TestAppContext) {
    let (transcript, cx) = transcript(cx);
    scroll_up(cx);
    let first = top_row(&transcript, cx);

    hold_below(cx);

    let top = top_row(&transcript, cx);
    assert!(
        top > first + 20,
        "the view only moved from row {first} to row {top}"
    );
    let selected = cx
        .update(|window, cx| transcript.read(cx).focused_selection(window))
        .unwrap_or_default();
    assert!(
        selected.contains(&format!("line {}", first + 1)),
        "{selected:?}"
    );
    assert!(selected.contains(&format!("line {top}")), "{selected:?}");
}

#[gpui::test]
fn letting_go_outside_the_transcript_ends_the_drag(cx: &mut TestAppContext) {
    let (transcript, cx) = transcript(cx);
    scroll_up(cx);
    cx.simulate_mouse_down(
        point(px(40.), px(20.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(40.), px(460.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(px(40.), px(460.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let after_up = top_row(&transcript, cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(top_row(&transcript, cx), after_up);
}
