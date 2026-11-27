//! Reproduces the bug where clicking a page's Print button (`window.print()`) in a macOS
//! WKWebView silently did nothing, and verifies that hidden/parked background tabs never pop
//! a native print sheet while visible tabs open a non-blocking `NSPrintOperation` sheet.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

const TEST: &str = "window_print_opens_sheet_for_visible_tab_and_ignores_background_tab";

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

    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSModalResponseCancel, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};

    use sikemux_browser::macos::{call_async, forget, open, place, Parent, Start, World};
    use sikemux_browser::{DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall};

    struct PrintEvents;

    impl TabEvents for PrintEvents {
        fn may_load(&self, _url: &str) -> bool {
            true
        }
        fn loaded(&self, _load: PageLoad) {}
        fn titled(&self, _title: String) {}
        fn moved(&self, _url: String, _can_go_back: bool, _can_go_forward: bool) {}
        fn new_window(&self, _url: String, _opener: Option<Opener>) -> Option<Tab> {
            None
        }
        fn closed(&self) {}
        fn download_started(&self, _url: String, _suggested: &Path) -> Option<PathBuf> {
            None
        }
        fn download_finished(&self, _url: String, _saved: bool) {}
        fn dialog(&self, _dialog: Option<PageDialog>) {}
        fn upload(&self) -> Option<Vec<PathBuf>> {
            None
        }
        fn document(&self, _event: DocumentEvent) {}
        fn health(&self, _stall: Option<TabStall>) {}
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = "<!doctype html><html><head><title>Udyam Certificate</title></head>\
                        <body><h1>Certificate</h1></body></html>";
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });

    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);
    // SAFETY: a fresh window on the main thread; kept behind other apps and never freed on close.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(800.0, 600.0)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is owned here, so AppKit must not free it on close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.orderBack(None);
    let content = window.contentView().expect("content view");

    let start_url = format!("http://127.0.0.1:{port}/");
    let tab_id = "test-window-print".to_owned();
    let webview = open(
        Parent {
            view: &content,
            beneath: false,
        },
        "agent-print".into(),
        tab_id.clone(),
        Start::Load(&start_url),
        sikemux_browser::user_agent(),
        (800.0, 600.0),
        Arc::new(PrintEvents),
    );

    let spin = |until: &dyn Fn() -> bool, limit: Duration| {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if until() {
                return true;
            }
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.02));
        }
        false
    };

    assert!(
        spin(
            &|| {
                // SAFETY: `isLoading` is a plain getter on a live WKWebView on the main thread.
                unsafe { !webview.isLoading() }
            },
            Duration::from_secs(10),
        ),
        "page did not finish loading"
    );

    let eval_json = |script: &str| -> String {
        let out: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
        let sink = Arc::clone(&out);
        call_async(
            &webview,
            script,
            World::Page,
            Box::new(move |val| {
                *sink.lock().expect("lock") = Some(val);
            }),
        );
        assert!(
            spin(
                &|| out.lock().expect("lock").is_some(),
                Duration::from_secs(5)
            ),
            "eval timed out: {script}"
        );
        let value = out
            .lock()
            .expect("lock")
            .take()
            .expect("eval value")
            .expect("eval succeeded");
        value
    };

    // 1. Hidden & parked background tab: window.print() must return without opening a sheet.
    let done = eval_json("window.print(); return 'printed';");
    assert_eq!(done, "printed");
    assert!(
        !spin(
            &|| window.attachedSheet().is_some(),
            Duration::from_millis(300)
        ),
        "hidden/parked background tab opened a print sheet"
    );
    assert!(window.sheets().is_empty());

    // 2. Unhidden but still parked off-screen: still must not open a print sheet.
    webview.setHidden(false);
    let done = eval_json("window.print(); return 'printed-parked';");
    assert_eq!(done, "printed-parked");
    assert!(
        !spin(
            &|| window.attachedSheet().is_some(),
            Duration::from_millis(300)
        ),
        "parked off-screen tab opened a print sheet"
    );

    // 3. Visible on-screen tab: window.print() must attach a native print sheet.
    place(
        &webview,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0)),
        None,
    );
    let done = eval_json("window.print(); return 'printed-visible';");
    assert_eq!(done, "printed-visible");
    assert!(
        spin(&|| window.attachedSheet().is_some(), Duration::from_secs(5)),
        "visible tab calling window.print() did not attach a native print sheet"
    );

    // 4. Re-entrant window.print() while a sheet is already open must be a safe no-op.
    let done = eval_json("window.print(); return 'reentrant';");
    assert_eq!(done, "reentrant");
    assert_eq!(window.sheets().count(), 1);

    // Dismiss the sheet cleanly and verify a subsequent print works (confirming the
    // re-entrancy guard was cleared by `printOperationDidRun:success:contextInfo:`).
    let sheet = window.attachedSheet().expect("attached print sheet");
    window.endSheet_returnCode(&sheet, NSModalResponseCancel);
    assert!(
        spin(&|| window.attachedSheet().is_none(), Duration::from_secs(5)),
        "print sheet did not dismiss cleanly"
    );

    let done = eval_json("window.print(); return 'printed-again';");
    assert_eq!(done, "printed-again");
    assert!(
        spin(&|| window.attachedSheet().is_some(), Duration::from_secs(5)),
        "second window.print() after dismissing sheet did not attach a print sheet"
    );
    let sheet = window.attachedSheet().expect("second attached print sheet");
    window.endSheet_returnCode(&sheet, NSModalResponseCancel);
    assert!(
        spin(&|| window.attachedSheet().is_none(), Duration::from_secs(5)),
        "second print sheet did not dismiss cleanly"
    );

    forget(&tab_id);
    window.orderOut(None);
    println!("test {TEST} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    if listing() {
        return;
    }
    println!("test {TEST} ... ok");
}
