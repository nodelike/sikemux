//! A native text view in a GPUI window, under a popover, beside a GPUI field.
//! Click or type into either; the line at the bottom names who has the keyboard.

#[cfg(target_os = "macos")]
fn main() {
    use gpui::{
        anchored, deferred, div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle,
        KeyDownEvent, MouseButton, Window, WindowBounds, WindowOptions,
    };
    use gpui_native_view::{native_view, NativeSurface, OwnedView, Retained};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSTextView, NSView};
    use objc2_foundation::NSRect;

    struct Demo {
        page: NativeSurface,
        page_focus: FocusHandle,
        field_focus: FocusHandle,
        typed: String,
        shown: bool,
        menu: bool,
    }

    impl Render for Demo {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let keyboard = if self.page_focus.is_focused(window) {
                "the page"
            } else if self.field_focus.is_focused(window) {
                "the field"
            } else {
                "nothing"
            };
            let button = |id: &'static str, label: String| {
                div()
                    .id(id)
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x2a2a33))
                    .child(label)
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .pt(px(40.))
                .bg(rgb(0x16161c))
                .text_color(rgb(0xe8e8ee))
                .font_family("Helvetica")
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(
                            div()
                                .id("field")
                                .track_focus(&self.field_focus)
                                .w(px(260.))
                                .px_2()
                                .py_1()
                                .border_1()
                                .border_color(if self.field_focus.is_focused(window) {
                                    rgb(0xa277ff)
                                } else {
                                    rgb(0x44444f)
                                })
                                .on_click(cx.listener(|demo, _, window, cx| {
                                    demo.field_focus.focus(window, cx)
                                }))
                                .on_key_down(cx.listener(|demo, event: &KeyDownEvent, _, cx| {
                                    if let Some(typed) = &event.keystroke.key_char {
                                        demo.typed.push_str(typed);
                                        cx.notify();
                                    }
                                }))
                                .child(if self.typed.is_empty() {
                                    "A GPUI field".to_string()
                                } else {
                                    self.typed.clone()
                                }),
                        )
                        .child(
                            button(
                                "shown",
                                if self.shown { "Hide page" } else { "Show page" }.into(),
                            )
                            .on_click(cx.listener(|demo, _, _, cx| {
                                demo.shown = !demo.shown;
                                cx.notify();
                            })),
                        )
                        .child(
                            button("menu", "Menu".into())
                                .on_click(cx.listener(|demo, _, _, cx| {
                                    demo.menu = !demo.menu;
                                    cx.notify();
                                }))
                                .when(self.menu, |menu| {
                                    menu.child(deferred(
                                        anchored().child(
                                            div()
                                                .id("popover")
                                                .occlude()
                                                .mt(px(30.))
                                                .w(px(240.))
                                                .h(px(160.))
                                                .p_3()
                                                .rounded_md()
                                                .bg(rgb(0x2f2f3a))
                                                .border_1()
                                                .border_color(rgb(0xa277ff))
                                                .child("A popover over the page")
                                                .on_mouse_down_out(cx.listener(
                                                    |demo, _, _, cx| {
                                                        demo.menu = false;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                    ))
                                }),
                        ),
                )
                .child(
                    div()
                        .id("pane")
                        .flex_1()
                        .border_1()
                        .border_color(rgb(0x44444f))
                        .on_mouse_down(MouseButton::Left, |_, _, _| {
                            println!("GPUI heard a click meant for the page")
                        })
                        .when(self.shown, |pane| {
                            pane.child(
                                native_view("page", &self.page)
                                    .track_focus(&self.page_focus)
                                    .size_full(),
                            )
                        }),
                )
                .child(format!("The keyboard is with {keyboard}."))
        }
    }

    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(820.), px(560.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            window
                .enable_scene_overlay()
                .expect("the overlay plane is available");
            let mtm = MainThreadMarker::new().expect("GPUI runs on the main thread");
            let text = NSTextView::initWithFrame(mtm.alloc(), NSRect::ZERO);
            text.setVerticallyResizable(false);
            let view: Retained<NSView> = Retained::into_super(Retained::into_super(text));
            let page = OwnedView::new(view, window).expect("the window has a view");
            cx.new(|cx| Demo {
                page: NativeSurface::new(page),
                page_focus: cx.focus_handle(),
                field_focus: cx.focus_handle(),
                typed: String::new(),
                shown: true,
                menu: false,
            })
        })
        .expect("the window opens");
        cx.activate(true);
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {}
