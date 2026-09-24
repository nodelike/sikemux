//! Mouse and keyboard input for a tab, delivered as real AppKit events. A page
//! can tell a script-dispatched event from a person's (`isTrusted`), and rich
//! editors ignore values written straight into the DOM, so the agent's clicks
//! and keys take the same path through WebKit as the person's.
//!
//! Events go straight to the tab's view rather than through the window, so a
//! tab that is off screen still receives them and the person's keyboard focus
//! stays where it was.

use std::ffi::c_void;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSPoint, NSProcessInfo, NSString};
use objc2_web_kit::WKWebView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Down,
    Drag,
    Up,
}

/// One key as AppKit describes it, parsed from names like `Enter` or `Meta+a`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: u16,
    pub characters: String,
    pub unmodified: String,
    pub flags: NSEventModifierFlags,
}

fn webview_from(pointer: *mut c_void) -> Result<Retained<WKWebView>, String> {
    unsafe { Retained::retain(pointer.cast::<WKWebView>()) }.ok_or_else(|| "the tab is gone".into())
}

fn now() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// `x` and `y` are CSS pixels from the top left of the page's viewport.
pub fn mouse(
    pointer: *mut c_void,
    kind: Mouse,
    x: f64,
    y: f64,
    clicks: isize,
) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    let window = webview.window().ok_or("the tab is not in a window")?;
    let zoom = unsafe { webview.pageZoom() }.max(0.01);
    let height = webview.frame().size.height;
    let local = if webview.isFlipped() {
        NSPoint::new(x * zoom, y * zoom)
    } else {
        NSPoint::new(x * zoom, height - y * zoom)
    };
    let location = webview.convertPoint_toView(local, None);
    let kind_code = match kind {
        Mouse::Move => NSEventType::MouseMoved,
        Mouse::Down => NSEventType::LeftMouseDown,
        Mouse::Drag => NSEventType::LeftMouseDragged,
        Mouse::Up => NSEventType::LeftMouseUp,
    };
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind_code,
        location,
        NSEventModifierFlags::empty(),
        now(),
        window.windowNumber(),
        None,
        0,
        if kind == Mouse::Move { 0 } else { clicks.max(1) },
        if kind == Mouse::Up || kind == Mouse::Move { 0.0 } else { 1.0 },
    )
    .ok_or("AppKit refused the mouse event")?;
    match kind {
        Mouse::Move => webview.mouseMoved(&event),
        Mouse::Down => webview.mouseDown(&event),
        Mouse::Drag => webview.mouseDragged(&event),
        Mouse::Up => webview.mouseUp(&event),
    }
    Ok(())
}

pub fn key(pointer: *mut c_void, stroke: &KeyStroke) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    let window = webview.window().ok_or("the tab is not in a window")?;
    let event = |kind: NSEventType| {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            stroke.flags,
            now(),
            window.windowNumber(),
            None,
            &NSString::from_str(&stroke.characters),
            &NSString::from_str(&stroke.unmodified),
            false,
            stroke.code,
        )
        .ok_or_else(|| "AppKit refused the key event".to_string())
    };
    let down = event(NSEventType::KeyDown)?;
    let up = event(NSEventType::KeyUp)?;
    // AppKit hands command chords to performKeyEquivalent, never to keyDown.
    if !stroke.flags.contains(NSEventModifierFlags::Command) || !webview.performKeyEquivalent(&down)
    {
        webview.keyDown(&down);
    }
    webview.keyUp(&up);
    Ok(())
}

/// Inserts text at the page's caret the way a keyboard or input method would,
/// so editors that keep their own model of the document see it arrive.
pub fn insert_text(pointer: *mut c_void, text: &str) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    let text = NSString::from_str(text);
    let _: () = unsafe { msg_send![&*webview, insertText: &*text] };
    Ok(())
}

pub fn parse_key(name: &str) -> Result<KeyStroke, String> {
    let mut flags = NSEventModifierFlags::empty();
    let mut rest = name;
    while let Some((modifier, tail)) = rest.split_once('+').filter(|(_, tail)| !tail.is_empty()) {
        flags |= match modifier.to_ascii_lowercase().as_str() {
            "meta" | "cmd" | "command" => NSEventModifierFlags::Command,
            "control" | "ctrl" => NSEventModifierFlags::Control,
            "alt" | "option" => NSEventModifierFlags::Option,
            "shift" => NSEventModifierFlags::Shift,
            _ => return Err(format!("unknown modifier \"{modifier}\" in \"{name}\"")),
        };
        rest = tail;
    }
    let named =
        |code: u16, character: char| Some((code, character.to_string(), character.to_string()));
    let (code, characters, unmodified) = match rest {
        "Enter" | "Return" => named(36, '\r'),
        "Tab" => named(48, '\t'),
        "Escape" | "Esc" => named(53, '\u{1b}'),
        "Backspace" => named(51, '\u{7f}'),
        "Delete" => named(117, '\u{F728}'),
        "Space" | " " => named(49, ' '),
        "ArrowUp" => named(126, '\u{F700}'),
        "ArrowDown" => named(125, '\u{F701}'),
        "ArrowLeft" => named(123, '\u{F702}'),
        "ArrowRight" => named(124, '\u{F703}'),
        "Home" => named(115, '\u{F729}'),
        "End" => named(119, '\u{F72B}'),
        "PageUp" => named(116, '\u{F72C}'),
        "PageDown" => named(121, '\u{F72D}'),
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => {
                    let lower = character.to_lowercase().collect::<String>();
                    let shown = if flags.contains(NSEventModifierFlags::Shift) {
                        character.to_uppercase().collect()
                    } else {
                        character.to_string()
                    };
                    Some((key_code(&lower), shown, lower))
                }
                _ => None,
            }
        }
    }
    .ok_or_else(|| format!("unknown key \"{rest}\"; use a single character or a name like Enter, Tab, Escape, ArrowDown"))?;
    Ok(KeyStroke {
        code,
        characters,
        unmodified,
        flags,
    })
}

/// Virtual key codes on a US layout. Pages read them through `event.code`.
fn key_code(character: &str) -> u16 {
    const LAYOUT: &[(&str, u16)] = &[
        ("a", 0),
        ("s", 1),
        ("d", 2),
        ("f", 3),
        ("h", 4),
        ("g", 5),
        ("z", 6),
        ("x", 7),
        ("c", 8),
        ("v", 9),
        ("b", 11),
        ("q", 12),
        ("w", 13),
        ("e", 14),
        ("r", 15),
        ("y", 16),
        ("t", 17),
        ("1", 18),
        ("2", 19),
        ("3", 20),
        ("4", 21),
        ("6", 22),
        ("5", 23),
        ("=", 24),
        ("9", 25),
        ("7", 26),
        ("-", 27),
        ("8", 28),
        ("0", 29),
        ("]", 30),
        ("o", 31),
        ("u", 32),
        ("[", 33),
        ("i", 34),
        ("p", 35),
        ("l", 37),
        ("j", 38),
        ("'", 39),
        ("k", 40),
        (";", 41),
        ("\\", 42),
        (",", 43),
        ("/", 44),
        ("n", 45),
        ("m", 46),
        (".", 47),
        ("`", 50),
    ];
    LAYOUT
        .iter()
        .find(|(key, _)| *key == character)
        .map(|(_, code)| *code)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_carry_the_function_key_characters_webkit_expects() {
        let enter = parse_key("Enter").unwrap();
        assert_eq!((enter.code, enter.characters.as_str()), (36, "\r"));
        assert_eq!(parse_key("ArrowDown").unwrap().characters, "\u{F701}");
        assert!(parse_key("Hyper").is_err());
    }

    #[test]
    fn chords_split_into_modifiers_and_one_key() {
        let select_all = parse_key("Meta+a").unwrap();
        assert_eq!(select_all.flags, NSEventModifierFlags::Command);
        assert_eq!((select_all.code, select_all.unmodified.as_str()), (0, "a"));
        let back_tab = parse_key("Shift+Tab").unwrap();
        assert_eq!(back_tab.flags, NSEventModifierFlags::Shift);
        assert_eq!(back_tab.code, 48);
        assert_eq!(parse_key("Shift+k").unwrap().characters, "K");
        assert_eq!(parse_key("Control++").unwrap().characters, "+");
        assert!(parse_key("Super+a").is_err());
    }
}
