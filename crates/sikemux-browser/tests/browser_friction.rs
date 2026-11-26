//! Live WKWebView regression tests for:
//! 1. `didFailProvisionalNavigation` surfacing on unreachable `*.localhost` URLs
//! 2. `page.js` `focus` prioritizing fillable controls over `<label>` (sibling, `for=`, wrapped)
//! 3. `records.js` `pending()` reporting `readyState` and pending same-origin vs external URLs
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

const TEST: &str = "wkwebview_provisional_failure_label_focus_and_pending_network";

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
    use serde_json::Value;

    use sikemux_browser::documents::DocumentLog;
    use sikemux_browser::macos::{call_async, forget, open, Parent, Start, World};
    use sikemux_browser::{
        DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabStall, PAGE_SCRIPT,
        RECORDS_SCRIPT,
    };

    struct CaptureEvents {
        log: Arc<Mutex<DocumentLog>>,
    }
    impl TabEvents for CaptureEvents {
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
        fn document(&self, event: DocumentEvent) {
            self.log.lock().expect("log lock").note(event);
        }
        fn health(&self, _stall: Option<TabStall>) {}
    }

    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);
    // SAFETY: a fresh window on the main thread; kept alive to the end and never closed.
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

    // 1. Provisional navigation failure on unreachable *.localhost port.
    let closed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.local_addr().expect("addr").port()
    };
    let fail_url = format!("http://t1.localhost:{closed_port}/");
    let fail_log = Arc::new(Mutex::new(DocumentLog::default()));
    let fail_tab_id = "test-provisional-fail".to_owned();
    let _fail_view = open(
        Parent {
            view: &content,
            beneath: false,
        },
        "agent-test".into(),
        fail_tab_id.clone(),
        Start::Load(&fail_url),
        sikemux_browser::user_agent(),
        (800.0, 600.0),
        Arc::new(CaptureEvents {
            log: Arc::clone(&fail_log),
        }),
    );

    assert!(
        spin(
            &|| fail_log
                .lock()
                .expect("lock")
                .loads(true)
                .last()
                .is_some_and(|load| load.is_navigation_failure()),
            Duration::from_secs(10),
        ),
        "expected provisional navigation failure for unreachable {fail_url}, got: {:?}",
        fail_log.lock().expect("lock").loads(false)
    );
    {
        let loads = fail_log.lock().expect("lock").loads(true);
        let latest = loads.last().expect("latest load");
        assert!(latest.is_navigation_failure());
        assert!(
            latest.url.contains("t1.localhost"),
            "expected failing URL to retain t1.localhost, got {:?}",
            latest.url
        );
    }
    forget(&fail_tab_id);

    // 2 & 3. Label + fillable input resolution and pending() network diagnostics.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let read = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(buf.get(..read).unwrap_or_default());
            if req.starts_with("GET /slow") {
                std::thread::sleep(Duration::from_secs(5));
                let reply = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                let _ = stream.write_all(reply.as_bytes());
                continue;
            }
            let body = r#"<!doctype html>
<html><body>
  <form>
    <div>
      <label>Where from?</label>
      <input id="origin" type="text" placeholder="Where from?" />
    </div>
    <div>
      <label for="dest"><span>Where to?</span></label>
      <input id="dest" type="text" />
    </div>
    <label>Notes <textarea id="notes"></textarea></label>
    <div>
      <label for="cabin">Cabin class</label>
      <select id="cabin"><option value="eco">Economy</option><option value="biz">Business</option></select>
    </div>
  </form>
</body></html>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });

    let page_url = format!("http://127.0.0.1:{port}/");
    let page_tab_id = "test-label-and-network".to_owned();
    let page_log = Arc::new(Mutex::new(DocumentLog::default()));
    let webview = open(
        Parent {
            view: &content,
            beneath: false,
        },
        "agent-test".into(),
        page_tab_id.clone(),
        Start::Load(&page_url),
        sikemux_browser::user_agent(),
        (800.0, 600.0),
        Arc::new(CaptureEvents {
            log: Arc::clone(&page_log),
        }),
    );

    assert!(
        spin(
            &|| {
                // SAFETY: `isLoading` is a plain getter on a live WKWebView on the main thread.
                unsafe { !webview.isLoading() }
            },
            Duration::from_secs(10),
        ),
        "test form page did not finish loading"
    );

    let eval_in = |world: World, body: &str| -> String {
        let slot: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
        let sink = Arc::clone(&slot);
        call_async(
            &webview,
            body,
            world,
            Box::new(move |answer| {
                *sink.lock().expect("lock") = Some(answer);
            }),
        );
        assert!(
            spin(
                &|| slot.lock().expect("lock").is_some(),
                Duration::from_secs(5)
            ),
            "eval timed out"
        );
        let answer = slot
            .lock()
            .expect("lock")
            .take()
            .expect("result")
            .expect("eval succeeded");
        answer
    };

    let focus_script = format!(
        r#"{PAGE_SCRIPT}
        const results = {{}};
        window.__sikemux.focus({{ text: "Where from?" }});
        results.sibling = document.activeElement && document.activeElement.id;
        window.__sikemux.focus({{ text: "Where to?" }});
        results.forAttr = document.activeElement && document.activeElement.id;
        window.__sikemux.focus({{ text: "Notes" }});
        results.wrapped = document.activeElement && document.activeElement.id;
        const sel = window.__sikemux.focus({{ text: "Cabin class" }});
        results.selectIsSelect = Boolean(sel && sel.isSelect);
        return JSON.stringify(results);"#
    );
    let raw_focus = eval_in(World::Helper, &focus_script);
    let parsed_focus: Value = serde_json::from_str(&raw_focus).expect("valid JSON");
    assert_eq!(
        parsed_focus.get("sibling").and_then(Value::as_str),
        Some("origin")
    );
    assert_eq!(
        parsed_focus.get("forAttr").and_then(Value::as_str),
        Some("dest")
    );
    assert_eq!(
        parsed_focus.get("wrapped").and_then(Value::as_str),
        Some("notes")
    );
    assert_eq!(
        parsed_focus.get("selectIsSelect").and_then(Value::as_bool),
        Some(true)
    );

    let pending_script = format!(
        r#"fetch("/slow").catch(() => {{}});
        const api = ({RECORDS_SCRIPT});
        return JSON.stringify(api.pending());"#
    );
    let raw_pending = eval_in(World::Page, &pending_script);
    let parsed_pending: Value = serde_json::from_str(&raw_pending).expect("valid pending JSON");
    assert_eq!(
        parsed_pending.get("readyState").and_then(Value::as_str),
        Some("complete")
    );
    let same_origin = parsed_pending
        .get("sameOrigin")
        .and_then(Value::as_array)
        .expect("sameOrigin array");
    assert_eq!(
        same_origin.len(),
        1,
        "expected 1 pending same-origin fetch: {parsed_pending}"
    );
    assert!(
        same_origin
            .first()
            .and_then(|item| item.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("/slow"),
        "expected /slow url in pending list: {parsed_pending}"
    );

    forget(&page_tab_id);
    println!("test {TEST} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    if listing() {
        return;
    }
    println!("test {TEST} ... ok");
}
