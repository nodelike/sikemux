//! The agent's mouse and keys for a tab on Linux, delivered to the page's widget as GDK
//! events, so the page takes them as trusted input the way it takes a person's.

use gtk::gdk;
use gtk::glib::translate::{IntoGlib, ToGlibPtr};
use gtk::prelude::*;
use webkit2gtk::{WebView, WebViewExt};

use crate::keys::{Key, Named};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Down,
    Drag,
    Up,
}

/// No key is ever bound to this one.
const UNUSED_KEYVAL: u32 = 0xffffff;

thread_local! {
    /// Set while an event of the agent's is on its way to a page.
    static SENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the event a page is handling now is the agent's, not the person's.
pub(crate) fn sending() -> bool {
    SENDING.get()
}

fn deliver(webview: &WebView, event: *mut gdk::ffi::GdkEvent) {
    SENDING.set(true);
    // SAFETY: `event` is a live event its caller filled in and still owns, and the widget
    // is the live page; `gtk_widget_event` only reads the event.
    unsafe {
        gtk::ffi::gtk_widget_event(webview.upcast_ref::<gtk::Widget>().to_glib_none().0, event);
    }
    SENDING.set(false);
}

/// `x` and `y` are CSS pixels from the top left of the page's viewport.
pub fn mouse(webview: &WebView, kind: Mouse, x: f64, y: f64, clicks: isize) -> Result<(), String> {
    let window = webview.window().ok_or("the tab is not drawn yet")?;
    let zoom = webview.zoom_level().max(0.01);
    let (mut x, mut y) = (x * zoom, y * zoom);
    if !webview.has_window() {
        let held = webview.allocation();
        x += f64::from(held.x());
        y += f64::from(held.y());
    }
    let (_, left, top) = window.origin();
    let (root_x, root_y) = (x + f64::from(left), y + f64::from(top));
    let pointer = window
        .display()
        .default_seat()
        .and_then(|seat| seat.pointer())
        .ok_or("no pointer to send the mouse from")?;
    let pressed = gdk::ModifierType::BUTTON1_MASK.bits();
    let send = |kind: gdk::EventType, state: u32| {
        // SAFETY: `gdk_event_new` hands back an event of `kind`, so the variant written is
        // the one it holds. The event takes its own reference to the window, which
        // `gdk_event_free` gives back.
        unsafe {
            let event = gdk::ffi::gdk_event_new(kind.into_glib());
            let window: *mut gdk::ffi::GdkWindow = window.to_glib_full();
            if kind == gdk::EventType::MotionNotify {
                let motion = &mut (*event).motion;
                motion.window = window;
                motion.time = now();
                (motion.x, motion.y, motion.x_root, motion.y_root) = (x, y, root_x, root_y);
                motion.state = state;
            } else {
                let button = &mut (*event).button;
                button.window = window;
                button.time = now();
                (button.x, button.y, button.x_root, button.y_root) = (x, y, root_x, root_y);
                button.state = state;
                button.button = 1;
            }
            gdk::ffi::gdk_event_set_device(event, pointer.to_glib_none().0);
            deliver(webview, event);
            gdk::ffi::gdk_event_free(event);
        }
    };
    match kind {
        Mouse::Move => send(gdk::EventType::MotionNotify, 0),
        Mouse::Drag => send(gdk::EventType::MotionNotify, pressed),
        Mouse::Down => {
            send(gdk::EventType::ButtonPress, 0);
            match clicks {
                2 => send(gdk::EventType::DoubleButtonPress, 0),
                3.. => send(gdk::EventType::TripleButtonPress, 0),
                _ => {}
            }
        }
        Mouse::Up => send(gdk::EventType::ButtonRelease, pressed),
    }
    Ok(())
}

/// The agents name chords the Mac's way, with Meta for the command key; Linux's command
/// key is Control.
pub fn key(webview: &WebView, name: &str) -> Result<(), String> {
    let chord = crate::keys::parse(name)?;
    let held = chord.modifiers;
    let mut state = gdk::ModifierType::empty();
    for (on, mask) in [
        (held.meta || held.control, gdk::ModifierType::CONTROL_MASK),
        (held.alt, gdk::ModifierType::MOD1_MASK),
        (held.shift, gdk::ModifierType::SHIFT_MASK),
    ] {
        if on {
            state |= mask;
        }
    }
    let keyval = keyval_of(chord.key, held.shift);
    press(webview, keyval, state.bits(), true)?;
    press(webview, keyval, state.bits(), false)
}

fn keyval_of(key: Key, shift: bool) -> u32 {
    use gdk::keys::constants as keys;

    let named = match key {
        Key::Named(Named::Enter) => keys::Return,
        Key::Named(Named::Tab) if shift => keys::ISO_Left_Tab,
        Key::Named(Named::Tab) => keys::Tab,
        Key::Named(Named::Escape) => keys::Escape,
        Key::Named(Named::Backspace) => keys::BackSpace,
        Key::Named(Named::Delete) => keys::Delete,
        Key::Named(Named::Space) => keys::space,
        Key::Named(Named::ArrowUp) => keys::Up,
        Key::Named(Named::ArrowDown) => keys::Down,
        Key::Named(Named::ArrowLeft) => keys::Left,
        Key::Named(Named::ArrowRight) => keys::Right,
        Key::Named(Named::Home) => keys::Home,
        Key::Named(Named::End) => keys::End,
        Key::Named(Named::PageUp) => keys::Page_Up,
        Key::Named(Named::PageDown) => keys::Page_Down,
        Key::Char(character) => {
            let shown = match shift {
                true => character.to_uppercase().next().unwrap_or(character),
                false => character,
            };
            return *gdk::keys::Key::from_unicode(shown);
        }
    };
    *named
}

fn press(webview: &WebView, keyval: u32, state: u32, down: bool) -> Result<(), String> {
    let window = webview.window().ok_or("the tab is not drawn yet")?;
    let display = window.display();
    let keyboard = display
        .default_seat()
        .and_then(|seat| seat.keyboard())
        .ok_or("no keyboard to send the key from")?;
    let code = gdk::Keymap::for_display(&display)
        .and_then(|keymap| {
            keymap
                .entries_for_keyval(keyval)
                .first()
                .map(|key| key.keycode())
        })
        .unwrap_or(0);
    let kind = if down {
        gdk::EventType::KeyPress
    } else {
        gdk::EventType::KeyRelease
    };
    // SAFETY: as in `mouse`, the event is the kind written and holds its own reference to
    // the window until it is freed.
    unsafe {
        let event = gdk::ffi::gdk_event_new(kind.into_glib());
        let key = &mut (*event).key;
        key.window = window.to_glib_full();
        key.time = now();
        key.state = state;
        key.keyval = keyval;
        key.hardware_keycode = code as u16;
        gdk::ffi::gdk_event_set_device(event, keyboard.to_glib_none().0);
        deliver(webview, event);
        gdk::ffi::gdk_event_free(event);
    }
    Ok(())
}

/// Inserts text at the page's caret the way a keyboard or input method would, so
/// editors that keep their own model of the document see it arrive.
pub fn insert_text(webview: &WebView, text: &str) -> Result<(), String> {
    webview.execute_editing_command_with_argument("InsertText", text);
    Ok(())
}

/// WebKit only notices a hung page when input it sent goes unanswered, and a script call
/// is not input. This sends a key release no key matches, which pages ignore.
pub fn probe_responsiveness(webview: &WebView) -> Result<(), String> {
    press(webview, UNUSED_KEYVAL, 0, false)
}

fn now() -> u32 {
    (gtk::glib::monotonic_time() / 1000) as u32
}
