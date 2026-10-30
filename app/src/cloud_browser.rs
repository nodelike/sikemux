//! The browser Settings › Single sign-on names for single sign-on: cloud sign-in pages open there, after
//! the shortcut that brings forward the desktop space it lives on.

use sikemux_process::user_environment::command;

/// Opens `url` in `app`, first activating it and pressing `shortcut`, such as `ctrl+3`.
pub fn open(url: &str, app: &str, shortcut: &str) {
    let mut script = format!(
        "tell application \"{}\" to activate\ndelay 0.4\n",
        quoted(app)
    );
    if let Some((held, key)) = keystroke(shortcut) {
        let press = match key_code(&key) {
            Some(code) => format!("key code {code}"),
            None => format!("keystroke \"{}\"", quoted(&key)),
        };
        script.push_str(&format!(
            "tell application \"System Events\" to tell process \"{}\" to {press} using {{{held}}}\ndelay 0.2",
            quoted(app)
        ));
    }
    let _ = command("/usr/bin/osascript").args(["-e", &script]).status();
    let _ = command("/usr/bin/open").args(["-a", app, url]).status();
}

fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `ctrl+3` as AppleScript's held modifiers and the key itself.
fn keystroke(shortcut: &str) -> Option<(String, String)> {
    let mut parts: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    let key = parts.pop().filter(|key| !key.is_empty())?.to_owned();
    if parts.is_empty() {
        return None;
    }
    let held = parts
        .into_iter()
        .map(|part| match part.to_lowercase().as_str() {
            "ctrl" | "control" => Some("control down"),
            "cmd" | "command" | "meta" => Some("command down"),
            "alt" | "opt" | "option" => Some("option down"),
            "shift" => Some("shift down"),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((held.join(", "), key))
}

/// The US keyboard's keys in the order of their codes, so a key's code is its place here. Codes
/// 10 and 36 are keys that type no character.
const US_KEYS: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.";

/// The US keyboard's code for `key`, which reaches browsers that read key codes rather than
/// characters, whatever the person's layout.
fn key_code(key: &str) -> Option<usize> {
    let mut chars = key.chars();
    let key = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() || key == '\0' {
        return None;
    }
    US_KEYS.chars().position(|named| named == key)
}
