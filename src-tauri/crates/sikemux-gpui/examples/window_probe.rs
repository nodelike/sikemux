//! Opens one window that draws two bands of colour and a line of text, then quits after the
//! seconds given (20 by default), so CI can capture it and see whether GPUI draws there at all.

use std::time::Duration;

use gpui_kit::{
    div, px, rgb, size, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions,
};

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x15141b))
            .child(div().h(px(96.)).bg(rgb(0xa277ff)))
            .child(div().h(px(96.)).bg(rgb(0x61ffca)))
            .child(
                div()
                    .p(px(16.))
                    .text_size(px(24.))
                    .text_color(rgb(0xedecee))
                    .child("GPUI draws here"),
            )
    }
}

fn main() {
    let seconds = std::env::args()
        .nth(1)
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(20);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let bounds = Bounds::centered(None, size(px(480.), px(320.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Probe)).expect("window opens");
        cx.activate(true);
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(seconds))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
