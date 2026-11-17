//! Input that reaches a page the way a person's does, as trusted events, sent through the
//! page's DevTools methods. Events go in order, so each call returns once it is sent.

use serde_json::json;

use super::{devtools, Page};
use crate::keys::{Key, Named};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Down,
    Drag,
    Up,
}

pub fn mouse(page: &Page, kind: Mouse, x: f64, y: f64, clicks: isize) -> Result<(), String> {
    let (kind, button, buttons) = match kind {
        Mouse::Move => ("mouseMoved", "none", 0),
        Mouse::Down => ("mousePressed", "left", 1),
        Mouse::Drag => ("mouseMoved", "left", 1),
        Mouse::Up => ("mouseReleased", "left", 0),
    };
    send(
        page,
        "Input.dispatchMouseEvent",
        json!({
            "type": kind,
            "x": x,
            "y": y,
            "button": button,
            "buttons": buttons,
            "clickCount": clicks.max(0),
        }),
    );
    Ok(())
}

/// The agents name chords the Mac's way, with Meta for the command key; Windows' command key
/// is Control.
pub fn key(page: &Page, name: &str) -> Result<(), String> {
    let chord = crate::keys::parse(name)?;
    let held = chord.modifiers;
    let control = held.meta || held.control;
    let modifiers = i32::from(held.alt) | i32::from(control) << 1 | i32::from(held.shift) << 3;
    let (key, code, virtual_key, text) = match chord.key {
        Key::Named(named) => {
            let (key, code, virtual_key) = named_key(named);
            let text = match named {
                Named::Enter => "\r",
                Named::Space => " ",
                _ => "",
            };
            (
                key.to_owned(),
                code.to_owned(),
                virtual_key,
                text.to_owned(),
            )
        }
        Key::Char(character) => {
            let upper = character.to_ascii_uppercase();
            let code = match character {
                'a'..='z' | 'A'..='Z' => format!("Key{upper}"),
                '0'..='9' => format!("Digit{character}"),
                _ => String::new(),
            };
            let virtual_key = if upper.is_ascii_alphanumeric() {
                upper as u32
            } else {
                0
            };
            (
                character.to_string(),
                code,
                virtual_key,
                character.to_string(),
            )
        }
    };
    let typed = if control || held.alt {
        String::new()
    } else {
        text
    };
    let down = if typed.is_empty() {
        "rawKeyDown"
    } else {
        "keyDown"
    };
    for kind in [down, "keyUp"] {
        send(
            page,
            "Input.dispatchKeyEvent",
            json!({
                "type": kind,
                "modifiers": modifiers,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": virtual_key,
                "text": if kind == "keyUp" { "" } else { typed.as_str() },
            }),
        );
    }
    Ok(())
}

fn named_key(named: Named) -> (&'static str, &'static str, u32) {
    match named {
        Named::Enter => ("Enter", "Enter", 0x0D),
        Named::Tab => ("Tab", "Tab", 0x09),
        Named::Escape => ("Escape", "Escape", 0x1B),
        Named::Backspace => ("Backspace", "Backspace", 0x08),
        Named::Delete => ("Delete", "Delete", 0x2E),
        Named::Space => (" ", "Space", 0x20),
        Named::ArrowUp => ("ArrowUp", "ArrowUp", 0x26),
        Named::ArrowDown => ("ArrowDown", "ArrowDown", 0x28),
        Named::ArrowLeft => ("ArrowLeft", "ArrowLeft", 0x25),
        Named::ArrowRight => ("ArrowRight", "ArrowRight", 0x27),
        Named::Home => ("Home", "Home", 0x24),
        Named::End => ("End", "End", 0x23),
        Named::PageUp => ("PageUp", "PageUp", 0x21),
        Named::PageDown => ("PageDown", "PageDown", 0x22),
    }
}

pub fn insert_text(page: &Page, text: &str) -> Result<(), String> {
    send(page, "Input.insertText", json!({ "text": text }));
    Ok(())
}

/// WebView2 reports a hung page by itself, so there is nothing to nudge.
pub fn probe_responsiveness(_page: &Page) -> Result<(), String> {
    Ok(())
}

fn send(page: &Page, method: &'static str, params: serde_json::Value) {
    devtools(page, method, &params, move |answer| {
        if let Err(why) = answer {
            eprintln!("sikemux: the page did not take {method}: {why}");
        }
    });
}
