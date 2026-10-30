//! The WebKitGTK side of a tab's page: its scripts, its pictures, its history and what it
//! tells the app. Everything here runs on the GTK thread that owns the page.

use std::cell::Cell;
use std::os::raw::{c_uint, c_ulong};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk::{cairo, gdk, gdk_pixbuf, glib};
use javascriptcore::ValueExt;
use webkit2gtk::{SettingsExt, SnapshotOptions, SnapshotRegion, WebView, WebViewExt};

use crate::{BrowserShortcut, TabEvents, TabStall};

/// The world the agents' helpers run in: it shares the page's DOM but none of its
/// JavaScript, so the page cannot tamper with what runs there.
const HELPER_WORLD: &str = "sikemux";
const JPEG_QUALITY: &str = "82";

type Shortcut = Box<dyn Fn(BrowserShortcut) + Send + Sync>;

static SHORTCUT: OnceLock<Shortcut> = OnceLock::new();
/// A page holds the keyboard, which only the GTK thread knows the window of.
static PAGE_HAS_KEYBOARD: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The X11 window of the page holding the keyboard.
    static HOLDING: Cell<Option<c_ulong>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

/// Wires a page WebKitGTK just made to the tab that holds it.
pub(crate) fn attach(webview: &WebView, agent_id: &str, tab_id: &str, events: Arc<dyn TabEvents>) {
    let crashed = events.clone();
    webview.connect_web_process_terminated(move |_, _| crashed.health(Some(TabStall::Crashed)));
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
    webview.call_async_javascript_function(
        body,
        None,
        world,
        None,
        None::<&gtk::gio::Cancellable>,
        move |answer| {
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
