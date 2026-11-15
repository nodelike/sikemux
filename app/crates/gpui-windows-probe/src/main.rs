use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, FontWeight, Window, WindowBounds,
    WindowOptions,
};

struct Probe;

impl Render for Probe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let swatch = |colour: u32| div().size_10().rounded_lg().bg(rgb(colour)).shadow_md();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .size_full()
            .bg(rgb(0x15161c))
            .text_color(rgb(0xedecee))
            .font_family("Figtree")
            .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child("Sikemux on Windows"))
            .child(div().text_base().child("Figtree 400: the quick brown fox jumps over the lazy dog"))
            .child(div().text_sm().font_family("Segoe UI").child("Segoe UI: system font through DirectWrite"))
            .child(div().text_sm().font_family("JetBrainsMono NFM").child("fn main() { println!(\"mono\"); } // 0O1lI"))
            .child(div().text_sm().child("العربية · 日本語 · emoji 🙂 ✓"))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(swatch(0xa277ff))
                    .child(swatch(0x61ffca))
                    .child(swatch(0xffca85))
                    .child(swatch(0xff6767)),
            )
            .child(
                div()
                    .w(px(360.))
                    .h(px(80.))
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(0x3d375e))
                    .bg(rgb(0x1f1d2b)),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let fonts = vec![
            include_bytes!("../../../../helpers/notch/Fonts/Figtree_400Regular.ttf").as_slice().into(),
            include_bytes!("../../../../helpers/notch/Fonts/Figtree_600SemiBold.ttf").as_slice().into(),
            include_bytes!("../../../assets/fonts/JetBrainsMonoNerdFontMono-Regular-Base.ttf").as_slice().into(),
        ];
        if let Err(error) = cx.text_system().add_fonts(fonts) {
            eprintln!("fonts: {error:#}");
        }
        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Probe),
        )
        .expect("open the window");
        cx.activate(true);
        eprintln!("window open");
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(40)).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
