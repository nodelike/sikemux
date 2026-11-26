//! The WebKitGTK side of a tab's page: its scripts, its pictures, its history and what it
//! tells the app. Everything here runs on the GTK thread that owns the page.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::{c_uint, c_ulong};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk::{cairo, gdk, gdk_pixbuf, glib};
use javascriptcore::ValueExt;
use sikemux_process::sync::Locked;
use webkit2gtk::{
    DownloadExt, LoadEvent, SettingsExt, SnapshotOptions, SnapshotRegion, URIResponseExt,
    UserContentInjectedFrames, UserContentManagerExt, UserScript, UserScriptInjectionTime, WebView,
    WebViewExt,
};

use crate::{BrowserShortcut, TabEvents, TabStall};

/// The world the agents' helpers run in: it shares the page's DOM but none of its
/// JavaScript, so the page cannot tamper with what runs there.
const HELPER_WORLD: &str = "sikemux";
const JPEG_QUALITY: &str = "82";

type Shortcut = Box<dyn Fn(BrowserShortcut) + Send + Sync>;

static SHORTCUT: OnceLock<Shortcut> = OnceLock::new();
/// A page holds the keyboard, which only the GTK thread knows the window of.
static PAGE_HAS_KEYBOARD: AtomicBool = AtomicBool::new(false);

type EvalDone = Box<dyn FnOnce(Result<String, String>) + Send>;
type DownloadDone = Box<dyn FnOnce(Result<crate::downloads::DownloadedAsset, String>) + Send>;
type DownloadSlot = Arc<Mutex<Option<DownloadDone>>>;

struct PendingEval {
    cancellable: gtk::gio::Cancellable,
    done: Arc<Mutex<Option<EvalDone>>>,
}

thread_local! {
    /// The X11 window of the page holding the keyboard.
    static HOLDING: Cell<Option<c_ulong>> = const { Cell::new(None) };
    static PENDING_EVALS: RefCell<HashMap<usize, Vec<PendingEval>>> = RefCell::new(HashMap::new());
}

fn view_key(webview: &WebView) -> usize {
    webview.as_ptr() as usize
}

fn abort_evals(view: usize, reason: &str) {
    let entries = PENDING_EVALS.with(|pending| pending.borrow_mut().remove(&view));
    let Some(entries) = entries else {
        return;
    };
    for entry in entries {
        gtk::gio::prelude::CancellableExt::cancel(&entry.cancellable);
        if let Some(done) = entry.done.locked().take() {
            done(Err(reason.to_owned()));
        }
    }
}

#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

/// Wires a page WebKitGTK just made to the tab that holds it.
pub(crate) fn attach(webview: &WebView, agent_id: &str, tab_id: &str, events: Arc<dyn TabEvents>) {
    if let Some(manager) = webview.user_content_manager() {
        let script = UserScript::for_world(
            crate::PAGE_SCRIPT,
            UserContentInjectedFrames::TopFrame,
            UserScriptInjectionTime::Start,
            HELPER_WORLD,
            &[],
            &[],
        );
        manager.add_script(&script);
    }
    let crashed = events.clone();
    webview.connect_web_process_terminated(move |view, _| {
        abort_evals(
            view_key(view),
            "the tab's page crashed while the script ran",
        );
        crashed.health(Some(TabStall::Crashed));
    });
    let failed_events = events.clone();
    webview.connect_load_failed(move |_view, _event, uri, error| {
        failed_events.document(crate::DocumentEvent::Failed {
            navigation: 0,
            url: (!uri.is_empty()).then(|| uri.to_owned()),
            error: error.message().to_owned(),
            provisional: true,
        });
        false
    });
    webview.connect_load_changed(|view, event| {
        if matches!(event, LoadEvent::Started | LoadEvent::Committed) {
            abort_evals(view_key(view), crate::native::EVAL_NAVIGATED);
        }
    });
    webview.connect_destroy(|view| {
        abort_evals(view_key(view), "the tab went away");
    });
    crate::linux::dialogs::attach(webview, tab_id, events.clone());
    let hung = events;
    webview.connect_is_web_process_responsive_notify(move |view| {
        hung.health((!view.is_web_process_responsive()).then_some(TabStall::Unresponsive));
    });
    webview.connect_button_press_event(|view, _| {
        if !crate::linux::input::sending() {
            take_keyboard(view);
        }
        glib::Propagation::Proceed
    });
    let (agent_id, tab_id) = (agent_id.to_owned(), tab_id.to_owned());
    webview.connect_key_press_event(move |_, event| {
        let Some(shortcut) = chord_of(event) else {
            return glib::Propagation::Proceed;
        };
        if let Some(send) = SHORTCUT.get() {
            send(BrowserShortcut {
                agent_id: agent_id.clone(),
                tab_id: tab_id.clone(),
                ..shortcut
            });
        }
        glib::Propagation::Stop
    });
}

/// The page is an X11 window of another connection inside the app's, so X leaves the
/// keyboard on the app's window when the person clicks into the page; the page takes it.
fn take_keyboard(webview: &WebView) {
    let Some(window) = webview.toplevel().and_then(|toplevel| toplevel.window()) else {
        return;
    };
    let Ok(page) = window.downcast::<gdkx11::X11Window>() else {
        return;
    };
    let xid = page.xid();
    if focus(xid) {
        webview.grab_focus();
        HOLDING.set(Some(xid));
        PAGE_HAS_KEYBOARD.store(true, Ordering::Release);
    }
}

/// Gives the keyboard back to the app's window when a page holds it, as the person's click
/// in the app does.
pub fn give_keyboard_back() {
    if !PAGE_HAS_KEYBOARD.swap(false, Ordering::AcqRel) {
        return;
    }
    crate::linux::on_gtk(|| {
        // The page's window sits in its clip window, which sits in the app's.
        if let Some(app) = HOLDING.take().and_then(parent_of).and_then(parent_of) {
            focus(app);
        }
    });
}

pub(crate) fn xdisplay() -> Option<*mut x11::xlib::Display> {
    let display = gdk::Display::default()?
        .downcast::<gdkx11::X11Display>()
        .ok()?;
    // SAFETY: the display is GTK's own live X11 connection, which outlives the call.
    let xdisplay = unsafe { gdkx11::ffi::gdk_x11_display_get_xdisplay(display.to_glib_none().0) };
    (!xdisplay.is_null()).then_some(xdisplay)
}

fn focus(window: c_ulong) -> bool {
    let Some(display) = xdisplay() else {
        return false;
    };
    // SAFETY: a live connection, and X answers a window that is gone with an error event
    // rather than failing the call.
    unsafe {
        x11::xlib::XSetInputFocus(
            display,
            window,
            x11::xlib::RevertToParent,
            x11::xlib::CurrentTime,
        );
        x11::xlib::XFlush(display);
    }
    true
}

fn parent_of(window: c_ulong) -> Option<c_ulong> {
    let display = xdisplay()?;
    let (mut root, mut parent, mut children, mut count) = (0, 0, std::ptr::null_mut(), 0 as c_uint);
    // SAFETY: a live connection; X fills the out-pointers, and the children list it hands
    // back is freed here.
    let found = unsafe {
        let found = x11::xlib::XQueryTree(
            display,
            window,
            &mut root,
            &mut parent,
            &mut children,
            &mut count,
        );
        if !children.is_null() {
            x11::xlib::XFree(children.cast());
        }
        found
    };
    (found != 0 && parent != 0 && parent != root).then_some(parent)
}

/// Where command chords pressed in a page go: the app's keymap, not the page.
pub(crate) fn install_shortcuts(send: impl Fn(BrowserShortcut) + Send + Sync + 'static) {
    let _ = SHORTCUT.set(Box::new(send));
}

/// A Control chord is the app's, apart from the editing set every text field keeps.
fn chord_of(event: &gdk::EventKey) -> Option<BrowserShortcut> {
    let state = event.state();
    if !state.contains(gdk::ModifierType::CONTROL_MASK)
        || state.intersects(gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SUPER_MASK)
    {
        return None;
    }
    let key = event
        .keyval()
        .to_lower()
        .to_unicode()
        .filter(char::is_ascii_graphic)?;
    if matches!(key, 'a' | 'c' | 'v' | 'x' | 'z' | 'y') {
        return None;
    }
    Some(BrowserShortcut {
        agent_id: String::new(),
        tab_id: String::new(),
        key: key.to_string(),
        code: dom_code(key),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: false,
    })
}

fn dom_code(key: char) -> String {
    match key {
        'a'..='z' => format!("Key{}", key.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{key}"),
        '[' => "BracketLeft".into(),
        ']' => "BracketRight".into(),
        ',' => "Comma".into(),
        '.' => "Period".into(),
        '/' => "Slash".into(),
        ';' => "Semicolon".into(),
        '\'' => "Quote".into(),
        '\\' => "Backslash".into(),
        '-' => "Minus".into(),
        '=' => "Equal".into(),
        '`' => "Backquote".into(),
        _ => String::new(),
    }
}

/// Runs `body` as the body of an async function in the page, awaiting any promise it
/// returns. The body must return a string.
pub fn call_async(
    webview: &WebView,
    body: &str,
    world: World,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let world = match world {
        World::Page => None,
        World::Helper => Some(HELPER_WORLD),
    };
    let cancellable = gtk::gio::Cancellable::new();
    let slot = Arc::new(Mutex::new(Some(done)));
    let key = view_key(webview);
    PENDING_EVALS.with(|pending| {
        let mut pending = pending.borrow_mut();
        let entries = pending.entry(key).or_default();
        entries.retain(|existing| existing.done.locked().is_some());
        entries.push(PendingEval {
            cancellable: cancellable.clone(),
            done: Arc::clone(&slot),
        });
    });
    webview.call_async_javascript_function(
        body,
        None,
        world,
        None,
        Some(&cancellable),
        move |answer| {
            let Some(done) = slot.locked().take() else {
                return;
            };
            done(match answer {
                Ok(value) if value.is_string() => Ok(value.to_str().into()),
                Ok(_) => Err("the script returned nothing readable".into()),
                Err(error) => Err(error.message().to_owned()),
            })
        },
    );
}

/// The visible part of the page, or only `area` of it in CSS pixels.
pub fn snapshot_jpeg(
    webview: &WebView,
    area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let zoom = webview.zoom_level().max(0.01);
    snapshot(webview, SnapshotRegion::Visible, move |picture| {
        let crop = area.map(|(left, top, width, height)| {
            (left * zoom, top * zoom, width * zoom, height * zoom)
        });
        encode(
            picture.and_then(|(pixbuf, scale)| cropped(&pixbuf, scale, crop)),
            JPEG_QUALITY,
            done,
        );
    });
}

/// The whole page down to `height` CSS pixels, at most `most` of them.
pub fn full_page_jpeg(
    webview: &WebView,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let zoom = webview.zoom_level().max(0.01);
    snapshot(webview, SnapshotRegion::FullDocument, move |picture| {
        let crop = (height > most).then_some((0.0, 0.0, f64::MAX, most * zoom));
        encode(
            picture.and_then(|(pixbuf, scale)| cropped(&pixbuf, scale, crop)),
            JPEG_QUALITY,
            done,
        );
    });
}

/// The page's picture and how many image pixels make one of its own.
fn snapshot(
    webview: &WebView,
    region: SnapshotRegion,
    then: impl FnOnce(Result<(gdk_pixbuf::Pixbuf, f64), String>) + 'static,
) {
    webview.snapshot(
        region,
        SnapshotOptions::NONE,
        None::<&gtk::gio::Cancellable>,
        move |surface| {
            then(
                surface
                    .map_err(|error| error.message().to_owned())
                    .and_then(|surface| {
                        let picture = cairo::ImageSurface::try_from(surface)
                            .map_err(|_| "the page could not be captured".to_string())?;
                        let (scale, _) = picture.device_scale();
                        gdk::pixbuf_get_from_surface(
                            &picture,
                            0,
                            0,
                            picture.width(),
                            picture.height(),
                        )
                        .map(|pixbuf| (pixbuf, scale.max(1.0)))
                        .ok_or_else(|| "the page could not be captured".to_string())
                    }),
            )
        },
    );
}

/// `crop` is in the page's own pixels, before the screen's scale.
fn cropped(
    pixbuf: &gdk_pixbuf::Pixbuf,
    scale: f64,
    crop: Option<(f64, f64, f64, f64)>,
) -> Result<gdk_pixbuf::Pixbuf, String> {
    let Some((left, top, width, height)) = crop else {
        return Ok(pixbuf.clone());
    };
    let (most_x, most_y) = (pixbuf.width(), pixbuf.height());
    let at = |value: f64, most: i32| ((value * scale).round() as i32).clamp(0, most);
    let (x, y) = (at(left, most_x), at(top, most_y));
    let (width, height) = (
        at(width, most_x - x).max(1).min(most_x - x),
        at(height, most_y - y).max(1).min(most_y - y),
    );
    if width <= 0 || height <= 0 {
        return Err("that area is outside the page".into());
    }
    Ok(pixbuf.new_subpixbuf(x, y, width, height))
}

/// Compressing the picture takes milliseconds, so it happens off the GTK thread.
fn encode(
    picture: Result<gdk_pixbuf::Pixbuf, String>,
    quality: &'static str,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    // A cut-out shares its parent's pixels, so it is copied out whole before its bytes go.
    let pixbuf = match picture.map(|pixbuf| pixbuf.copy()) {
        Ok(Some(pixbuf)) => pixbuf,
        Ok(None) => return done(Err("could not encode the page image".into())),
        Err(error) => return done(Err(error)),
    };
    let (bytes, alpha, width, height, stride) = (
        pixbuf.read_pixel_bytes(),
        pixbuf.has_alpha(),
        pixbuf.width(),
        pixbuf.height(),
        pixbuf.rowstride(),
    );
    std::thread::spawn(move || {
        let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
            &bytes,
            gdk_pixbuf::Colorspace::Rgb,
            alpha,
            8,
            width,
            height,
            stride,
        );
        done(
            pixbuf
                .save_to_bufferv("jpeg", &[("quality", quality)])
                .map_err(|_| "could not encode the page image".to_string()),
        )
    });
}

pub fn history(webview: &WebView, delta: i32) {
    if delta < 0 {
        webview.go_back();
    } else {
        webview.go_forward();
    }
}

pub fn introduce_as(webview: &WebView, agent: &str) {
    if let Some(settings) = WebViewExt::settings(webview) {
        settings.set_user_agent(Some(agent));
    }
}

fn finish_download(slot: &DownloadSlot, result: Result<crate::downloads::DownloadedAsset, String>) {
    if let Some(cb) = slot.locked().take() {
        cb(result);
    }
}

pub(crate) fn download_asset(
    webview: &WebView,
    url: &str,
    dir: std::path::PathBuf,
    filename: Option<String>,
    max_bytes: u64,
    done: DownloadDone,
) {
    let Ok(parsed) = url::Url::parse(url) else {
        done(Err("invalid download url".into()));
        return;
    };
    let Some(download) = webview.download_uri(url) else {
        done(Err("WebKitGTK could not start the download".into()));
        return;
    };
    let done: DownloadSlot = Arc::new(Mutex::new(Some(done)));
    let state: Arc<Mutex<(Option<std::path::PathBuf>, String)>> =
        Arc::new(Mutex::new((None, "application/octet-stream".into())));

    let done_dest = Arc::clone(&done);
    let state_dest = Arc::clone(&state);
    download.connect_decide_destination(move |dl, suggested| {
        if let Some(response) = dl.response() {
            let status = response.status_code();
            if status >= 400 {
                dl.cancel();
                finish_download(
                    &done_dest,
                    Err(format!("download failed with HTTP {status}")),
                );
                return true;
            }
            if response.content_length() > max_bytes {
                dl.cancel();
                finish_download(
                    &done_dest,
                    Err(format!("download exceeds {max_bytes} byte size limit")),
                );
                return true;
            }
            if let Some(mime) = response.mime_type().filter(|m| !m.is_empty()) {
                state_dest.locked().1 = mime.into();
            }
        }
        match crate::downloads::contained_download_path(
            &dir,
            &parsed,
            filename.as_deref(),
            std::path::Path::new(suggested),
        ) {
            Ok(dest) => {
                state_dest.locked().0 = Some(dest.clone());
                dl.set_destination(&dest.to_string_lossy());
                true
            }
            Err(error) => {
                dl.cancel();
                finish_download(&done_dest, Err(error));
                true
            }
        }
    });

    let done_data = Arc::clone(&done);
    download.connect_received_data(move |dl, _| {
        if dl.received_data_length() > max_bytes {
            dl.cancel();
            finish_download(
                &done_data,
                Err(format!("download exceeds {max_bytes} byte size limit")),
            );
        }
    });

    let done_fail = Arc::clone(&done);
    let state_fail = Arc::clone(&state);
    download.connect_failed(move |_, error| {
        if let Some(dest) = state_fail.locked().0.as_deref() {
            let _ = std::fs::remove_file(dest);
        }
        finish_download(
            &done_fail,
            Err(format!("download failed: {}", error.message())),
        );
    });

    let done_ok = Arc::clone(&done);
    let state_ok = Arc::clone(&state);
    download.connect_finished(move |_| {
        let (dest, mime_type) = state_ok.locked().clone();
        let Some(dest) = dest else {
            finish_download(
                &done_ok,
                Err("download finished without a destination file".into()),
            );
            return;
        };
        let bytes = match std::fs::metadata(&dest) {
            Ok(meta) => meta.len(),
            Err(error) => {
                finish_download(
                    &done_ok,
                    Err(format!("could not read downloaded file: {error}")),
                );
                return;
            }
        };
        if bytes > max_bytes {
            let _ = std::fs::remove_file(&dest);
            finish_download(
                &done_ok,
                Err(format!("download exceeds {max_bytes} byte size limit")),
            );
            return;
        }
        finish_download(
            &done_ok,
            Ok(crate::downloads::DownloadedAsset {
                path: dest,
                mime_type,
                bytes,
            }),
        );
    });
}
