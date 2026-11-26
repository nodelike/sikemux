//! Reproduces the bug where a background/hidden browser tab calling `window.confirm()`,
//! `window.prompt()`, or `window.alert()` popped a native `NSAlert` sheet on top of the
//! app window, blocking the person's active chat and stealing Return/focus. Verifies that
//! JS dialogs stay held in Rust without attaching any sheet to the window and resolve
//! cleanly through `macos::answer_dialog`.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

const TEST: &str = "background_tab_js_dialogs_hold_without_native_sheet_and_resolve";

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

    use sikemux_browser::macos::{answer_dialog, call_async, forget, open, Parent, Start, World};
    use sikemux_browser::{DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall};

    struct DialogEvents {
        current: Arc<Mutex<Option<PageDialog>>>,
    }

    impl TabEvents for DialogEvents {
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
        fn dialog(&self, dialog: Option<PageDialog>) {
            *self.current.lock().expect("lock") = dialog;
        }
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
            let body = "<!doctype html><html><body><h1>Alerts</h1></body></html>";
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

    let current_dialog: Arc<Mutex<Option<PageDialog>>> = Arc::default();
    let start_url = format!("http://127.0.0.1:{port}/");
    let tab_id = "test-bg-dialog".to_owned();
    let webview = open(
        Parent {
            view: &content,
            beneath: false,
        },
        "agent-bg".into(),
        tab_id.clone(),
        Start::Load(&start_url),
        sikemux_browser::user_agent(),
        (800.0, 600.0),
        Arc::new(DialogEvents {
            current: Arc::clone(&current_dialog),
        }),
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

    let wait_eval = |script: &str, expected: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = String::new();
        while Instant::now() < deadline {
            last = eval_json(script);
            if last == expected {
                return;
            }
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.02));
        }
        panic!("expected {expected} for `{script}`, got {last}");
    };

    // 1. window.confirm() on hidden/parked background tab
    let _ = eval_json("setTimeout(() => { window.__confirm = window.confirm('I am a JS Confirm'); }, 10); return 'scheduled';");
    assert!(
        spin(
            &|| current_dialog.lock().expect("lock").is_some(),
            Duration::from_secs(5)
        ),
        "confirm dialog never arrived"
    );
    assert!(
        window.attachedSheet().is_none(),
        "background tab confirm popped a native NSAlert sheet on the app window"
    );
    assert!(
        window.sheets().is_empty(),
        "background tab confirm queued a native sheet on the app window"
    );
    let dialog = current_dialog
        .lock()
        .expect("lock")
        .clone()
        .expect("confirm dialog");
    assert_eq!(dialog.kind, "confirm");
    assert_eq!(dialog.message, "I am a JS Confirm");

    answer_dialog(&tab_id, true, None).expect("answer confirm");
    assert!(
        current_dialog.lock().expect("lock").is_none(),
        "confirm dialog did not clear after answer_dialog"
    );
    wait_eval("return JSON.stringify(window.__confirm ?? null);", "true");

    // 2. window.prompt() with custom text
    let _ = eval_json("setTimeout(() => { window.__prompt = window.prompt('Your name?', 'nobody'); }, 10); return 'scheduled';");
    assert!(
        spin(
            &|| current_dialog.lock().expect("lock").is_some(),
            Duration::from_secs(5)
        ),
        "prompt dialog never arrived"
    );
    assert!(
        window.attachedSheet().is_none(),
        "background tab prompt popped a native NSAlert sheet on the app window"
    );
    let dialog = current_dialog
        .lock()
        .expect("lock")
        .clone()
        .expect("prompt dialog");
    assert_eq!(dialog.kind, "prompt");
    assert_eq!(dialog.message, "Your name?");
    assert_eq!(dialog.default_text.as_deref(), Some("nobody"));

    answer_dialog(&tab_id, true, Some("kirio")).expect("answer prompt");
    wait_eval(
        "return JSON.stringify(window.__prompt ?? null);",
        "\"kirio\"",
    );

    // 3. window.alert() dismissed cleanly
    let _ = eval_json("setTimeout(() => { window.alert('Done'); window.__alerted = true; }, 10); return 'scheduled';");
    assert!(
        spin(
            &|| current_dialog.lock().expect("lock").is_some(),
            Duration::from_secs(5)
        ),
        "alert dialog never arrived"
    );
    assert!(
        window.attachedSheet().is_none(),
        "background tab alert popped a native NSAlert sheet on the app window"
    );
    answer_dialog(&tab_id, true, None).expect("answer alert");
    wait_eval("return JSON.stringify(window.__alerted ?? null);", "true");

    forget(&tab_id);
    println!("test {TEST} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    if listing() {
        return;
    }
    println!("test {TEST} ... ok");
}
