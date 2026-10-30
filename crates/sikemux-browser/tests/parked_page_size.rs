//! A parked tab sits far above the window. Under an opaque title bar WebKit
//! once took that distance off the page, which then laid out with a negative
//! height. WebKit and AppKit want the main thread, so this runs as its own program,
//! answering nextest's `--list` itself.

const TEST: &str = "a_parked_page_lays_out_at_its_own_size";

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

#[cfg(target_os = "macos")]
fn main() {
    if listing() {
        return;
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSDate, NSError, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
    use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);

    let window_size = NSSize::new(900.0, 700.0);
    // SAFETY: a fresh window on the main thread; it is kept alive to the end and never closed.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(100.0, 100.0), window_size),
            NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is owned here, so AppKit must not free it on close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitlebarAppearsTransparent(false);
    window.orderFront(None);

    let page = NSSize::new(1200.0, 800.0);
    // SAFETY: a fresh WKWebView with a fresh configuration, on the main thread.
    let webview: Retained<WKWebView> = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::new(NSPoint::new(-100_000.0, -100_000.0), page),
            &WKWebViewConfiguration::new(mtm),
        )
    };
    sikemux_browser::macos::leave_insets_to_the_app(&webview);
    let content = window.contentView().expect("a window has a content view");
    content.addSubview(&webview);
    let parked_above = if content.isFlipped() {
        sikemux_browser::PARKED_ORIGIN
    } else {
        window_size.height - sikemux_browser::PARKED_ORIGIN - page.height
    };
    webview.setFrameOrigin(NSPoint::new(sikemux_browser::PARKED_ORIGIN, parked_above));
    // SAFETY: main thread, and `webview` is alive.
    unsafe { webview.loadHTMLString_baseURL(&NSString::from_str("<p>parked</p>"), None) };

    let spin = |until: &dyn Fn() -> bool| {
        for _ in 0..200 {
            if until() {
                return;
            }
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
        }
        panic!("WebKit did not answer within 10 seconds");
    };
    // SAFETY: `isLoading` is a plain getter on a live view, on the main thread.
    spin(&|| unsafe { !webview.isLoading() });

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
            &NSString::from_str("innerWidth + 'x' + innerHeight"),
            Some(&done),
        );
    }
    spin(&|| answer.borrow().is_some());

    let laid_out = answer.borrow().clone().unwrap_or_default();
    assert_eq!(
        laid_out, "1200x800",
        "a parked page lays out at its own size"
    );
    println!("test {TEST} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    listing();
}
