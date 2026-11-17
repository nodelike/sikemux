//! A toast and a dialog show over a web page, as over anything else in the window. The
//! person saw "Downloaded …" toasts hidden behind the desk's browser more than once.

use gpui::{
    div, px, size, AppContext, Bounds, Context, IntoElement, ParentElement, Point, Render, Styled,
    TestAppContext, VisualTestContext, Window,
};
use gpui_kit::component::WindowExt;
use gpui_native_view::{native_view, NativeSurface, Surface};
use sikemux_gpui::{theme, toast};

struct Page;

impl Surface for Page {
    fn place(&self, _: Option<gpui_native_view::Frame>) {}
}

struct Desk {
    page: NativeSurface,
}

impl Render for Desk {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(native_view("page", &self.page).size_full())
    }
}

fn desk(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.set_reduce_motion(true);
    });
    let (_, window) = cx.add_window_view(|window, cx| {
        let desk = cx.new(|_| Desk {
            page: NativeSurface::new(Page),
        });
        gpui_kit::base::Root::new(desk, window, cx)
    });
    window.simulate_resize(size(px(1200.), px(800.)));
    window.run_until_parked();
    window
}

/// Where the window draws over the page and where it takes the pointer from it, in points.
fn over_page(window: &mut VisualTestContext) -> (Vec<Bounds<f32>>, Vec<Bounds<f32>>) {
    window.update(|window, _| {
        let scale = window.scale_factor();
        let (drawn, input) = window.painted_over_native_views();
        let drawn = drawn
            .iter()
            .map(|bounds| bounds.map(|value| value.0 / scale))
            .collect();
        let input = input.iter().map(|bounds| bounds.map(f32::from)).collect();
        (drawn, input)
    })
}

fn covers(places: &[Bounds<f32>], point: Point<f32>) -> bool {
    places.iter().any(|bounds| bounds.contains(&point))
}

#[gpui::test]
fn a_toast_shows_over_a_page_and_takes_its_clicks(cx: &mut TestAppContext) {
    let window = desk(cx);
    assert_eq!(over_page(window), (Vec::new(), Vec::new()));

    window
        .update(|window, cx| window.push_notification(toast::success("Downloaded report.pdf"), cx));
    window.run_until_parked();
    let (drawn, input) = over_page(window);
    let toast = Point::new(600., 800. - 40.);
    assert!(
        covers(&drawn, toast),
        "the toast is drawn under the page: {drawn:?}"
    );
    assert!(
        covers(&input, toast),
        "the page takes the toast's clicks: {input:?}"
    );
    assert!(
        !covers(&input, Point::new(600., 300.)),
        "the toast takes the page's clicks"
    );
}

#[gpui::test]
fn a_dialog_shows_over_a_page(cx: &mut TestAppContext) {
    let window = desk(cx);
    window.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Close this tab?"))
    });
    window.run_until_parked();
    let (drawn, input) = over_page(window);
    let middle = Point::new(600., 400.);
    assert!(
        covers(&drawn, middle),
        "the dialog is drawn under the page: {drawn:?}"
    );
    assert!(
        covers(&input, middle),
        "the page takes the dialog's clicks: {input:?}"
    );
}
