//! The SnapShot shortcut, held with macOS as a hot key so it works while another app is in front.
//! Hot keys need no permission, unlike watching every key pressed.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::{Once, OnceLock};

use gpui_kit::{Context, Window};
use tokio::sync::mpsc;

use crate::workbench::Workbench;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        count: usize,
        kinds: *const EventTypeSpec,
        data: *mut c_void,
        installed: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        registered: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(registered: *mut c_void) -> i32;
}

const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;

static PRESSED: OnceLock<mpsc::UnboundedSender<()>> = OnceLock::new();

thread_local! {
    static REGISTERED: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
}

extern "C" fn pressed(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
    if let Some(pressed) = PRESSED.get() {
        let _ = pressed.send(());
    }
    0
}

/// Holds `keys`, in the shortcut table's spelling, as the hot key, in place of the last; none
/// lets it go. Keys macOS cannot hold, such as Enter, are left to the window alone.
pub fn register(keys: Option<&str>) {
    static HEARD: Once = Once::new();
    HEARD.call_once(|| {
        let kinds = [EventTypeSpec {
            class: KEYBOARD,
            kind: HOT_KEY_PRESSED,
        }];
        // SAFETY: on the main thread, where the app's event target is; the handler lives as
        // long as the app.
        unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                pressed,
                kinds.len(),
                kinds.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    });
    let last = REGISTERED.replace(std::ptr::null_mut());
    if !last.is_null() {
        // SAFETY: `last` is the hot key registered below, not yet let go.
        unsafe { UnregisterEventHotKey(last) };
    }
    let Some((code, modifiers)) = keys.and_then(carbon_keys) else {
        return;
    };
    let id = EventHotKeyId {
        signature: u32::from_be_bytes(*b"SKMX"),
        id: 1,
    };
    let mut registered = std::ptr::null_mut();
    // SAFETY: on the main thread, with a key code and modifiers Carbon defines.
    let status = unsafe {
        RegisterEventHotKey(
            code,
            modifiers,
            id,
            GetApplicationEventTarget(),
            0,
            &mut registered,
        )
    };
    if status == 0 {
        REGISTERED.set(registered);
    } else {
        eprintln!("sikemux: another app holds the SnapShot shortcut ({status})");
    }
}

/// Takes a SnapShot each time the hot key is pressed.
pub fn listen(window: &mut Window, cx: &mut Context<Workbench>) {
    let (pressed, mut presses) = mpsc::unbounded_channel();
    if PRESSED.set(pressed).is_err() {
        return;
    }
    cx.spawn_in(window, async move |bench, cx| {
        while presses.recv().await.is_some() {
            let taken = bench.update_in(cx, |bench, window, cx| {
                crate::keymap::counted(super::COMMAND, cx);
                bench.snapshot(None, true, window, cx)
            });
            if taken.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// The key code and Carbon modifiers of keys in the table's spelling, such as `Meta+Shift+Digit2`.
fn carbon_keys(keys: &str) -> Option<(u32, u32)> {
    let mut parts: Vec<&str> = keys.split('+').collect();
    let code = key_code(parts.pop()?)?;
    let modifiers = parts.into_iter().try_fold(0, |held, part| {
        let modifier = match part {
            "Meta" => 1 << 8,
            "Shift" => 1 << 9,
            "Alt" => 1 << 11,
            "Ctrl" => 1 << 12,
            _ => return None,
        };
        Some(held | modifier)
    })?;
    Some((code, modifiers))
}

/// macOS's code for the key at the place a US keyboard has `code`.
fn key_code(code: &str) -> Option<u32> {
    const CODES: &[(&str, u32)] = &[
        ("KeyA", 0x00),
        ("KeyS", 0x01),
        ("KeyD", 0x02),
        ("KeyF", 0x03),
        ("KeyH", 0x04),
        ("KeyG", 0x05),
        ("KeyZ", 0x06),
        ("KeyX", 0x07),
        ("KeyC", 0x08),
        ("KeyV", 0x09),
        ("KeyB", 0x0B),
        ("KeyQ", 0x0C),
        ("KeyW", 0x0D),
        ("KeyE", 0x0E),
        ("KeyR", 0x0F),
        ("KeyY", 0x10),
        ("KeyT", 0x11),
        ("Digit1", 0x12),
        ("Digit2", 0x13),
        ("Digit3", 0x14),
        ("Digit4", 0x15),
        ("Digit6", 0x16),
        ("Digit5", 0x17),
        ("Equal", 0x18),
        ("Digit9", 0x19),
        ("Digit7", 0x1A),
        ("Minus", 0x1B),
        ("Digit8", 0x1C),
        ("Digit0", 0x1D),
        ("BracketRight", 0x1E),
        ("KeyO", 0x1F),
        ("KeyU", 0x20),
        ("BracketLeft", 0x21),
        ("KeyI", 0x22),
        ("KeyP", 0x23),
        ("KeyL", 0x25),
        ("KeyJ", 0x26),
        ("Quote", 0x27),
        ("KeyK", 0x28),
        ("Semicolon", 0x29),
        ("Backslash", 0x2A),
        ("Comma", 0x2B),
        ("Slash", 0x2C),
        ("KeyN", 0x2D),
        ("KeyM", 0x2E),
        ("Period", 0x2F),
        ("Space", 0x31),
        ("Backquote", 0x32),
        ("F1", 0x7A),
        ("F2", 0x78),
        ("F3", 0x63),
        ("F4", 0x76),
        ("F5", 0x60),
        ("F6", 0x61),
        ("F7", 0x62),
        ("F8", 0x64),
        ("F9", 0x65),
        ("F10", 0x6D),
        ("F11", 0x67),
        ("F12", 0x6F),
    ];
    CODES
        .iter()
        .find(|(named, _)| *named == code)
        .map(|(_, key)| *key)
}
