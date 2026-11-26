//! Dragging a selection past the top or bottom of a markdown drawn on its own, as a file's
//! preview is, scrolls it while the pointer is held there.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    canvas, div, point, px, size, App, AppContext, Context, FocusHandle, InteractiveElement,
    IntoElement, Modifiers, MouseButton, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window,
};
use gpui_transcript::{DocSelect, Markdown, Style};
use serde_json::json;

struct Preview {
    doc: Rc<Markdown>,
    select: DocSelect,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pen = self.select.pen(Rc::new(Style::default()), &self.doc, None);
        let (pressed, followed, doc) = (self.select.clone(), self.select.clone(), self.doc.clone());
        let scroll = self.scroll.clone();
        let focus = self.focus.clone();
        let pages = div()
            .id("pages")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                focus.focus(window, cx);
                pressed.press(event, &doc);
                window.refresh();
            })
            .child(pen.markdown("doc", &self.doc, window, cx));
        div().relative().size_full().child(pages).child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| followed.follow(bounds, &scroll, window),
            )
            .absolute()
            .inset_0(),
        )
    }
}

fn frames(cx: &mut VisualTestContext, count: usize) {
    for _ in 0..count {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

fn selected(preview: &gpui::Entity<Preview>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx: &mut App| {
        let preview = preview.read(cx);
        preview.select.selected_text(&preview.doc)
    })
    .unwrap_or_default()
}

#[gpui::test]
fn holding_a_drag_below_a_preview_keeps_scrolling_and_selecting(cx: &mut TestAppContext) {
    let blocks: Vec<_> = (0..200)
        .map(|n| json!({ "t": "p", "c": [format!("line {n}")] }))
        .collect();
    let doc = Rc::new(Markdown::new(&json!(blocks), 0));
    let preview = cx.new(|cx| Preview {
        doc,
        select: DocSelect::default(),
        scroll: ScrollHandle::new(),
        focus: cx.focus_handle(),
    });
    let (_, cx) = cx.add_window_view({
        let preview = preview.clone();
        move |_, _| ShellOf(preview)
    });
    cx.simulate_resize(size(px(600.), px(400.)));
    cx.run_until_parked();

    let scroll = preview.read_with(cx, |preview, _| preview.scroll.clone());
    cx.simulate_mouse_down(
        point(px(10.), px(10.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(10.), px(460.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    frames(cx, 20);

    let held = scroll.offset().y;
    assert!(held < px(-400.), "the preview scrolled only to {held:?}");
    let text = selected(&preview, cx);
    assert!(
        text.contains("line 1\n") && text.contains("line 40"),
        "{text:?}"
    );

    cx.simulate_mouse_up(
        point(px(10.), px(460.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    frames(cx, 1);
    let let_go = scroll.offset().y;
    frames(cx, 10);
    assert_eq!(
        scroll.offset().y,
        let_go,
        "letting go outside stops the scroll"
    );
}

struct ShellOf(gpui::Entity<Preview>);

impl Render for ShellOf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}
