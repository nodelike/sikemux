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
            let body = r##"<!doctype html>
<html>
<head>
<style>
  body { margin: 0; font-family: sans-serif; }
  .sticky-top { position: fixed; top: 0; left: 220px; right: 0; height: 60px; background: #222; color: #fff; z-index: 100; }
  #remove-saved { position: fixed; top: 0; left: 0; width: 220px; bottom: 0; z-index: 200; }
  #send-msg { position: absolute; top: 100px; left: 10px; width: 180px; height: 40px; }
  #partial-btn { position: absolute; top: 160px; left: 120px; width: 200px; height: 40px; }
  main { margin-left: 240px; padding-top: 80px; }
  .tall-spacer { height: 900px; }
  #sticky-target { display: block; margin-top: 10px; }
</style>
</head>
<body>
  <div class="sticky-top">Header</div>
  <button id="send-msg" type="button">Send message</button>
  <button id="partial-btn" type="button">Partial action</button>
  <button id="remove-saved" type="button">Remove from Saved</button>
  <main>
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
      <div>
        <input name="subject" class="subject-field" type="text" />
        <button id="submit-btn" type="submit">Save Draft</button>
      </div>
      <ul id="recipients">
        <li class="rc-input"><input id="to-chip" type="text" placeholder="To" /></li>
      </ul>
      <a id="channel-link" href="#general">#general</a>
    </form>
    <div class="tall-spacer"></div>
    <button id="sticky-target" type="button">Below Sticky</button>
    <div class="tall-spacer"></div>
  </main>
  <script>
    const toChip = document.getElementById("to-chip");
    const list = document.getElementById("recipients");
    toChip.addEventListener("change", () => {
      const val = toChip.value.trim();
      if (!val) return;
      const li = document.createElement("li");
      li.className = "recipient";
      li.textContent = val;
      list.insertBefore(li, toChip.parentElement);
      toChip.value = "";
    });
  </script>
</body></html>"##;
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
        r##"{PAGE_SCRIPT}
        const results = {{}};
        window.__sikemux.focus({{ text: "Where from?" }});
        results.sibling = document.activeElement && document.activeElement.id;
        window.__sikemux.focus({{ text: "Where to?" }});
        results.forAttr = document.activeElement && document.activeElement.id;
        window.__sikemux.focus({{ text: "Notes" }});
        results.wrapped = document.activeElement && document.activeElement.id;
        const sel = window.__sikemux.focus({{ text: "Cabin class" }});
        results.selectIsSelect = Boolean(sel && sel.isSelect);

        // Bug 1: covered button reports covering control instead of claiming uncovered;
        // partially covered button finds exposed point; element near sticky top bar scrolls clear.
        const cov = window.__sikemux.point({{ text: "Send message" }});
        results.coveredBy = cov.covered;
        results.coveredTag = cov.coveredTag;
        const partial = window.__sikemux.point({{ text: "Partial action" }});
        results.partialCleared = partial.covered === null && partial.x >= 220;
        const clear = window.__sikemux.point({{ text: "Below Sticky" }});
        results.stickyCleared = clear.covered === null;

        // Bug 3: CSS selector strings resolve for both clicks and types, with literal text fallback.
        const submitPoint = window.__sikemux.point({{ text: "button[type=submit]" }});
        results.selectorClickLabel = submitPoint.label;
        results.selectorMayNavigate = submitPoint.mayNavigate;
        window.__sikemux.focus({{ text: "input[name=subject]" }});
        results.selectorTypeName = document.activeElement && document.activeElement.name;
        const chanPoint = window.__sikemux.point({{ text: "#general" }});
        results.literalFallbackLabel = chanPoint.label;

        // Bug 5: auto-clearing chip input marks consumed: true and does not duplicate chips.
        window.__sikemux.focus({{ text: "#to-chip" }});
        const toEl = document.getElementById("to-chip");
        toEl.value = "alice@example.com";
        toEl.dispatchEvent(new Event("input", {{ bubbles: true }}));
        const committed = window.__sikemux.commitField({{ text: "#to-chip" }}, "alice@example.com", true, false);
        results.chipValue = committed.value;
        results.chipConsumed = Boolean(committed.consumed);
        results.chipCount = document.querySelectorAll("#recipients .recipient").length;

        return JSON.stringify(results);"##
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
    assert_eq!(
        parsed_focus.get("coveredBy").and_then(Value::as_str),
        Some("Remove from Saved")
    );
    assert_eq!(
        parsed_focus.get("coveredTag").and_then(Value::as_str),
        Some("button")
    );
    assert_eq!(
        parsed_focus.get("partialCleared").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        parsed_focus.get("stickyCleared").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        parsed_focus
            .get("selectorClickLabel")
            .and_then(Value::as_str),
        Some("Save Draft")
    );
    assert_eq!(
        parsed_focus
            .get("selectorMayNavigate")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        parsed_focus.get("selectorTypeName").and_then(Value::as_str),
        Some("subject")
    );
    assert_eq!(
        parsed_focus
            .get("literalFallbackLabel")
            .and_then(Value::as_str),
        Some("#general")
    );
    assert_eq!(
        parsed_focus.get("chipValue").and_then(Value::as_str),
        Some("")
    );
    assert_eq!(
        parsed_focus.get("chipConsumed").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        parsed_focus.get("chipCount").and_then(Value::as_u64),
        Some(1)
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
