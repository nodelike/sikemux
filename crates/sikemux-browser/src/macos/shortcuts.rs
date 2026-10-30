//! App shortcuts pressed while a page owns the keyboard.

use super::*;

/// Command chords are the app's, not the page's, apart from the editing set
/// every text field expects to keep and the menu's own Quit, Hide and Minimize.
pub(super) fn forwards_chord(key: &str, flags: NSEventModifierFlags) -> bool {
    if flags.contains(NSEventModifierFlags::Control) || flags.contains(NSEventModifierFlags::Option)
    {
        return false;
    }
    let mut chars = key.chars();
    let (Some(char), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if !char.is_ascii_graphic() {
        return false;
    }
    !matches!(
        char.to_ascii_lowercase(),
        'a' | 'c' | 'v' | 'x' | 'z' | 'y' | 'q' | 'h' | 'm'
    )
}

pub(super) fn dom_code(key_code: u16, key: &str) -> String {
    let named = match key_code {
        36 => "Enter",
        48 => "Tab",
        49 => "Space",
        51 => "Backspace",
        53 => "Escape",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        23 => "Digit5",
        22 => "Digit6",
        26 => "Digit7",
        28 => "Digit8",
        25 => "Digit9",
        29 => "Digit0",
        33 => "BracketLeft",
        30 => "BracketRight",
        27 => "Minus",
        24 => "Equal",
        50 => "Backquote",
        _ => "",
    };
    if !named.is_empty() {
        return named.into();
    }
    let Some(char) = key.chars().next() else {
        return String::new();
    };
    match char {
        'a'..='z' | 'A'..='Z' => format!("Key{}", char.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{char}"),
        '[' => "BracketLeft".into(),
        ']' => "BracketRight".into(),
        ',' => "Comma".into(),
        '.' => "Period".into(),
        '/' => "Slash".into(),
        ';' => "Semicolon".into(),
        '\'' => "Quote".into(),
        '-' => "Minus".into(),
        '=' => "Equal".into(),
        '`' => "Backquote".into(),
        '\\' => "Backslash".into(),
        _ => String::new(),
    }
}

/// Tab under the key window's first responder, if any.
pub(super) fn focused_tab(event: &NSEvent, mtm: MainThreadMarker) -> Option<(String, String)> {
    let window = event.window(mtm)?;
    let responder = window.firstResponder()?;
    let view = responder.downcast_ref::<NSView>()?;
    TABS.with(|tabs| {
        tabs.borrow()
            .iter()
            .find(|(_, tab)| view.isDescendantOf(&tab.webview))
            .map(|(id, tab)| (id.clone(), tab.agent_id.clone()))
    })
}

/// The tab `view` belongs to, if it is part of one.
pub(crate) fn tab_holding(view: &NSView) -> Option<Retained<WKWebView>> {
    TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .find(|tab| view.isDescendantOf(&tab.webview))
            .map(|tab| tab.webview.clone())
    })
}

/// Watches the keyboard for app chords pressed while a page has focus, and
/// hands each one to `shortcut` instead of the page.
pub fn install_shortcuts(shortcut: impl Fn(BrowserShortcut) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let pass = event.as_ptr();
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        let event = unsafe { event.as_ref() };
        if crate::input::stop_returned_key(event) {
            return std::ptr::null_mut();
        }
        crate::input::note_person(event);
        if let Some((tab_id, _)) = focused_tab(event, mtm) {
            crate::person::typed_in(&tab_id);
        }
        let flags = event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return pass;
        }
        let key = event
            .charactersIgnoringModifiers()
            .map(|chars| chars.to_string())
            .unwrap_or_default();
        if !forwards_chord(&key, flags) {
            return pass;
        }
        let Some((tab_id, agent_id)) = focused_tab(event, mtm) else {
            return pass;
        };
        shortcut(BrowserShortcut {
            agent_id,
            tab_id,
            code: dom_code(event.keyCode(), &key),
            key,
            shift: flags.contains(NSEventModifierFlags::Shift),
            alt: false,
        });
        std::ptr::null_mut()
    });
    // SAFETY: main thread (checked above). AppKit copies the block, and the monitor it
    // returns is kept in SHORTCUT_MONITOR so it stays installed.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    SHORTCUT_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
    watch_the_person();
}
