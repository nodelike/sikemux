//! A page in Pages took no pinch, so it could not be zoomed or panned, once it was read in a view
//! of its own rather than a desk tab. Real trackpad events are sent to a real window, whose views
//! want the main thread, so this runs as its own program, answering nextest's `--list` itself.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

const TEST: &str = "a_pinched_page_zooms_and_pans";

fn listing() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|arg| arg == "--list") {
        return false;
    }
    if cfg!(target_os = "macos") && !args.iter().any(|arg| arg == "--ignored") {
        println!("{TEST}: test");
    }
    true
}

#[cfg(not(target_os = "macos"))]
fn main() {
    listing();
}

#[cfg(target_os = "macos")]
fn main() {
    if listing() {
        return;
    }
    mac::check();
    println!("test {TEST} ... ok");
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSEvent, NSWindow, NSWindowStyleMask};
    use objc2_core_graphics::{CGEvent, CGEventField, CGEventType, CGScrollEventUnit};
    use objc2_foundation::{NSDate, NSError, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
    use objc2_web_kit::WKWebView;
    use sikemux_browser::page_view::{PageView, Room};

    const SIZE: NSSize = NSSize::new(900.0, 700.0);
    const AT: NSPoint = NSPoint::new(300.0, 200.0);
    const PAGE: &str = "<body style='margin:0'>\
        <p style='position:absolute;left:200px;top:100px;width:250px;height:300px'>words</p>\
        <div style='width:1800px;height:3000px'></div>";

    pub fn check() {
        let mtm = MainThreadMarker::new().expect("runs on the main thread");
        NSApplication::sharedApplication(mtm).finishLaunching();
        // SAFETY: a fresh window on the main thread; it is kept alive to the end and never closed.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(100.0, 100.0), SIZE),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the window is owned here, so AppKit must not free it on close.
        unsafe { window.setReleasedWhenClosed(false) };
        window.orderBack(None);

        let page = PageView::new(mtm, PAGE, Room::Served("http://127.0.0.1:9/page"), |_| {});
        let view = page.view();
        view.setFrame(NSRect::new(NSPoint::ZERO, SIZE));
        window
            .contentView()
            .expect("a window has a content view")
            .addSubview(&view);
        let webview = view.downcast::<WKWebView>().expect("a page is a web view");
        // SAFETY: `isLoading` is a plain getter on a live view, on the main thread.
        spin("loading", || unsafe { !webview.isLoading() });
        spin("laying out", || ask(&webview, "innerWidth") == "900");

        let pans = |x, y| {
            pan(&window, x, 0);
            pan(&window, 0, y);
        };
        let smart_zooms = || window.sendEvent(&gesture(&window, SMART_ZOOM, 0, 0.0));
        let shows = |what: &str, act: &dyn Fn(), is: &dyn Fn(f64, f64, f64) -> bool| {
            shows(what, act, || {
                let (scale, left, top) = shown(&webview);
                is(scale, left, top)
            });
        };

        shows(
            "a wider page panned",
            &|| pans(-100, -100),
            &|scale, left, top| scale == 1.0 && left > 0.0 && top > 0.0,
        );
        shows(
            "the page panned back",
            &|| pans(600, 600),
            &|scale, left, top| scale == 1.0 && left == 0.0 && top == 0.0,
        );
        shows(
            "a zoom around the pointer",
            &|| zoom(&window, 1.0),
            &|scale, left, top| {
                let held = |at: f64| at - at / scale;
                scale > 1.2 && (left - held(AT.x)).abs() < 1.0 && (top - held(AT.y)).abs() < 1.0
            },
        );
        let zoomed = shown(&webview);
        shows(
            "a zoomed page panned",
            &|| pans(-100, -60),
            &|scale, left, top| scale == zoomed.0 && left > zoomed.1 && top > zoomed.2,
        );

        page.show(PAGE);
        shows(
            "the next page at its own size",
            &|| (),
            &|scale, left, top| scale == 1.0 && left == 0.0 && top == 0.0,
        );
        shows(
            "a smart zoom on a paragraph",
            &smart_zooms,
            &|scale, _, _| scale > 1.2,
        );
        rest(0.5);
        shows("a smart zoom back out", &smart_zooms, &|scale, _, _| {
            scale == 1.0
        });
    }

    const PINCH: i64 = 8;
    const SMART_ZOOM: i64 = 22;
    const BEGAN: i64 = 1;
    const CHANGED: i64 = 2;
    const ENDED: i64 = 4;

    /// A page that has just loaded can drop part of a gesture, so it is made again until it shows.
    fn shows(what: &str, act: &dyn Fn(), is: impl Fn() -> bool) {
        for _ in 0..5 {
            act();
            for _ in 0..40 {
                if is() {
                    return;
                }
                rest(0.05);
            }
        }
        panic!("WebKit never showed {what}");
    }

    /// How far the page is zoomed, and where on it the window's top left corner lies.
    fn shown(webview: &WKWebView) -> (f64, f64, f64) {
        let said = ask(
            webview,
            "[visualViewport.scale, visualViewport.pageLeft, visualViewport.pageTop]",
        );
        let numbers: Vec<f64> = said
            .split(',')
            .filter_map(|number| number.parse().ok())
            .collect();
        match numbers[..] {
            [scale, left, top] => (scale, left, top),
            _ => panic!("the page said {said}"),
        }
    }

    fn zoom(window: &NSWindow, by: f64) {
        for (phase, delta) in [
            (BEGAN, 0.0),
            (CHANGED, by / 2.0),
            (CHANGED, by / 2.0),
            (ENDED, 0.0),
        ] {
            window.sendEvent(&gesture(window, PINCH, phase, delta));
            rest(0.05);
        }
    }

    fn pan(window: &NSWindow, x: i32, y: i32) {
        for (phase, x, y) in [
            (BEGAN, x / 2, y / 2),
            (CHANGED, x / 2, y / 2),
            (ENDED, 0, 0),
        ] {
            window.sendEvent(&scroll(window, phase, x, y));
            rest(0.05);
        }
        rest(0.3);
    }

    fn gesture(window: &NSWindow, kind: i64, phase: i64, delta: f64) -> Retained<NSEvent> {
        let event = CGEvent::new(None).expect("an event");
        CGEvent::set_type(Some(&event), CGEventType(29));
        CGEvent::set_integer_value_field(Some(&event), CGEventField(110), kind);
        CGEvent::set_integer_value_field(Some(&event), CGEventField(132), phase);
        CGEvent::set_double_value_field(Some(&event), CGEventField(113), delta);
        placed(window, &event)
    }

    fn scroll(window: &NSWindow, phase: i64, x: i32, y: i32) -> Retained<NSEvent> {
        let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, y, x, 0)
            .expect("an event");
        CGEvent::set_integer_value_field(Some(&event), CGEventField(88), 1);
        CGEvent::set_integer_value_field(Some(&event), CGEventField(99), phase);
        placed(window, &event)
    }

    extern "C-unwind" {
        fn CGEventSetWindowLocation(event: &CGEvent, at: NSPoint);
    }

    fn placed(window: &NSWindow, event: &CGEvent) -> Retained<NSEvent> {
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField(51),
            window.windowNumber() as i64,
        );
        let title_bar = window.frame().size.height - SIZE.height;
        // SAFETY: Core Graphics' own setter, which takes the event and a point from the top.
        unsafe { CGEventSetWindowLocation(event, NSPoint::new(AT.x, title_bar + AT.y)) };
        NSEvent::eventWithCGEvent(event).expect("an AppKit event")
    }

    fn ask(webview: &WKWebView, script: &str) -> String {
        let answer: Rc<RefCell<Option<String>>> = Rc::default();
        let sink = answer.clone();
        let done = RcBlock::new(move |result: *mut AnyObject, _: *mut NSError| {
            // SAFETY: WebKit hands a JavaScript string back as an NSString, or nil.
            let text = unsafe { result.as_ref() }
                .and_then(|object| object.downcast_ref::<NSString>())
                .map(NSString::to_string);
            *sink.borrow_mut() = Some(text.unwrap_or_default());
        });
        // SAFETY: main thread, and the block lives until WebKit calls it.
        unsafe {
            webview.evaluateJavaScript_completionHandler(
                &NSString::from_str(&format!("String({script})")),
                Some(&done),
            );
        }
        spin("an answer", || answer.borrow().is_some());
        let said = answer.borrow().clone();
        said.unwrap_or_default()
    }

    fn rest(seconds: f64) {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(seconds));
    }

    fn spin(what: &str, until: impl Fn() -> bool) {
        for _ in 0..200 {
            if until() {
                return;
            }
            rest(0.05);
        }
        panic!("WebKit did not finish {what} within 10 seconds");
    }
}
