//! How the host commands look in a terminal, matching the installer: the
//! accent for ticks and prompts, grey for detail. Plain text when the output is
//! not a terminal or `NO_COLOR` is set.

use std::io::IsTerminal;
use std::sync::OnceLock;

fn color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").is_ok_and(|term| term != "dumb")
    })
}

fn paint(code: &str, text: &str) -> String {
    if color() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

pub fn accent(text: &str) -> String {
    let truecolor =
        std::env::var("COLORTERM").is_ok_and(|value| value == "truecolor" || value == "24bit");
    paint(if truecolor { "38;2;162;119;255" } else { "35" }, text)
}

pub fn dim(text: &str) -> String {
    paint("2", text)
}

pub fn bold(text: &str) -> String {
    paint("1", text)
}

pub fn red(text: &str) -> String {
    paint("31", text)
}

/// A finished step: what happened, then the detail in grey.
pub fn done(what: &str, detail: &str) {
    if detail.is_empty() {
        println!("  {} {what}", accent("✓"));
    } else {
        println!("  {} {what:<32} {}", accent("✓"), dim(detail));
    }
}

pub fn note(text: &str) {
    println!("  {}", dim(text));
}
