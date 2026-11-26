//! Reproduces the bug where an async `browser_evaluate` / `tab.eval` that triggers a
//! top-level navigation hung for the full 29 s timeout because WebKit tore down the JS
//! context before `postMessage` fired. Verifies that `macos::call_async` settles
//! immediately on navigation.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

const TEST: &str = "eval_settles_immediately_when_script_triggers_navigation";

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
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};

    use sikemux_browser::macos::{call_async, forget, open, Parent, Start, World};
    use sikemux_browser::{DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall};

    struct TestEvents;
    impl TabEvents for TestEvents {
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
            let read = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(buf.get(..read).unwrap_or_default());
            let body = if req.starts_with("GET /next") {
                "<!doctype html><html><body><h1>Next</h1></body></html>"
            } else {
                "<!doctype html><html><body><a id='go' href='/next'>Go</a></body></html>"
            };
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
    // SAFETY: a fresh window on the main thread; it is kept alive to the end and never closed.
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
    let content = window.contentView().expect("content view");

    let start_url = format!("http://127.0.0.1:{port}/");
    let tab_id = "test-eval-nav".to_owned();
    let webview = open(
        Parent {
            view: &content,
            beneath: false,
        },
        "agent-test".into(),
        tab_id.clone(),
        Start::Load(&start_url),
        sikemux_browser::user_agent(),
        (800.0, 600.0),
        Arc::new(TestEvents),
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
        "initial page did not finish loading"
    );

    let result: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
    let sink = Arc::clone(&result);
    let started = Instant::now();
    call_async(
        &webview,
        "document.getElementById('go').click(); await new Promise(() => {}); return 'never';",
        World::Page,
        Box::new(move |answer| {
            *sink.lock().expect("lock") = Some(answer);
        }),
    );

    assert!(
        spin(
            &|| result.lock().expect("lock").is_some(),
            Duration::from_secs(5)
        ),
        "pending eval hung instead of settling on navigation"
    );
    let elapsed = started.elapsed();
    let outcome = result.lock().expect("lock").take().expect("result");
    forget(&tab_id);

    match outcome {
        Err(message) if message.contains("navigated") => {}
        other => panic!("expected navigation settlement error, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "eval took {elapsed:?} to settle on navigation"
    );
    println!("test {TEST} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    if listing() {
        return;
    }
    println!("test {TEST} ... ok");
}
