//! Turns what a person does into the bytes a program reads, the way xterm
//! encodes them, under the modes the program turned on.

use alacritty_terminal::term::TermMode;
use sikemux_wire::grid::{Mods, MouseAction, MouseButton, TerminalInput};

use super::Terminal;
use crate::user_shell::configured_shell;

fn modifier(mods: Mods) -> u8 {
    1 + u8::from(mods.shift)
        + 2 * u8::from(mods.alt)
        + 4 * u8::from(mods.ctrl)
        + 8 * u8::from(mods.meta)
}

fn control(character: char) -> Option<u8> {
    Some(match character {
        'a'..='z' => character as u8 - b'a' + 1,
        'A'..='Z' => character as u8 - b'A' + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// A cursor key: `ESC [ X` normally, `ESC O X` in application mode, and
/// `ESC [ 1 ; m X` with modifiers.
fn cursor_key(out: &mut Vec<u8>, letter: u8, mods: Mods, application: bool) {
    let modifier = modifier(mods);
    if modifier > 1 {
        out.extend_from_slice(format!("\x1b[1;{modifier}").as_bytes());
        out.push(letter);
    } else if application {
        out.extend_from_slice(&[0x1b, b'O', letter]);
    } else {
        out.extend_from_slice(&[0x1b, b'[', letter]);
    }
}

fn tilde_key(out: &mut Vec<u8>, number: u8, mods: Mods) {
    let modifier = modifier(mods);
    if modifier > 1 {
        out.extend_from_slice(format!("\x1b[{number};{modifier}~").as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[{number}~").as_bytes());
    }
}

fn function_key(out: &mut Vec<u8>, number: u8, mods: Mods) {
    match number {
        1..=4 => {
            let letter = b'P' + number - 1;
            let modifier = modifier(mods);
            if modifier > 1 {
                out.extend_from_slice(format!("\x1b[1;{modifier}").as_bytes());
                out.push(letter);
            } else {
                out.extend_from_slice(&[0x1b, b'O', letter]);
            }
        }
        5..=12 => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde_key(out, CODES[usize::from(number - 5)], mods);
        }
        _ => {}
    }
}

fn key(out: &mut Vec<u8>, key: &str, mods: Mods, mode: TermMode) {
    let application = mode.contains(TermMode::APP_CURSOR);
    let only = |shift: bool, alt: bool, ctrl: bool, meta: bool| {
        mods == Mods {
            shift,
            alt,
            ctrl,
            meta,
        }
    };
    match key {
        "Enter" if only(true, false, false, false) => out.extend_from_slice(b"\x1b[13;2u"),
        "Enter" if mods.alt => out.extend_from_slice(b"\x1b\r"),
        "Enter" => out.push(b'\r'),
        "Backspace" if only(false, false, false, true) => out.push(0x15),
        "Backspace" if mods.ctrl => out.push(0x08),
        "Backspace" if mods.alt => out.extend_from_slice(b"\x1b\x7f"),
        "Backspace" => out.push(0x7f),
        "Tab" if mods.shift => out.extend_from_slice(b"\x1b[Z"),
        "Tab" if mods.alt => out.extend_from_slice(b"\x1b\t"),
        "Tab" => out.push(b'\t'),
        "Escape" if mods.alt => out.extend_from_slice(b"\x1b\x1b"),
        "Escape" => out.push(0x1b),
        "ArrowLeft" if only(false, false, false, true) => out.push(0x01),
        "ArrowRight" if only(false, false, false, true) => out.push(0x05),
        "ArrowLeft" if only(false, true, false, false) => out.extend_from_slice(b"\x1bb"),
        "ArrowRight" if only(false, true, false, false) => out.extend_from_slice(b"\x1bf"),
        "ArrowUp" => cursor_key(out, b'A', mods, application),
        "ArrowDown" => cursor_key(out, b'B', mods, application),
        "ArrowRight" => cursor_key(out, b'C', mods, application),
        "ArrowLeft" => cursor_key(out, b'D', mods, application),
        "Home" => cursor_key(out, b'H', mods, application),
        "End" => cursor_key(out, b'F', mods, application),
        "Insert" => tilde_key(out, 2, mods),
        "Delete" => tilde_key(out, 3, mods),
        "PageUp" => tilde_key(out, 5, mods),
        "PageDown" => tilde_key(out, 6, mods),
        _ => {
            if let Some(number) = key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
                function_key(out, number, mods);
                return;
            }
            let mut characters = key.chars();
            let (Some(character), None) = (characters.next(), characters.next()) else {
                return;
            };
            if mods.alt {
                out.push(0x1b);
            }
            match control(character).filter(|_| mods.ctrl) {
                Some(byte) => out.push(byte),
                None => {
                    let mut buffer = [0; 4];
                    out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
            }
        }
    }
}

fn mouse_report(out: &mut Vec<u8>, mode: TermMode, code: u8, col: u16, row: u16, release: bool) {
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        out.extend_from_slice(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).as_bytes());
        return;
    }
    let code = if release { (code & !0b11) | 3 } else { code };
    if mode.contains(TermMode::UTF8_MOUSE) {
        out.extend_from_slice(b"\x1b[M");
        for value in [
            u32::from(code) + 32,
            u32::from(col) + 33,
            u32::from(row) + 33,
        ] {
            if let Some(character) = char::from_u32(value) {
                let mut buffer = [0; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
        }
        return;
    }
    if col > 222 || row > 222 {
        return;
    }
    out.extend_from_slice(&[0x1b, b'[', b'M', code + 32, col as u8 + 33, row as u8 + 33]);
}

fn mouse_mods(mods: Mods) -> u8 {
    4 * u8::from(mods.shift) + 8 * u8::from(mods.alt) + 16 * u8::from(mods.ctrl)
}

fn paste(out: &mut Vec<u8>, text: &str, mode: TermMode) {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
        out.extend_from_slice(b"\x1b[201~");
    } else {
        out.extend_from_slice(text.as_bytes());
    }
}

/// `paths` as words `shell` reads back literally, evaluating none of them.
/// Nothing when a path holds a NUL, which no shell word can carry.
fn shell_words(paths: &[String], shell: &str) -> Option<String> {
    if paths.iter().any(|path| path.contains('\0')) {
        return None;
    }
    let powershell = reads_powershell(shell).unwrap_or(cfg!(windows));
    let quote = |path: &String| {
        if powershell {
            format!("'{}'", path.replace('\'', "''"))
        } else {
            format!("'{}'", path.replace('\'', r"'\''"))
        }
    };
    Some(paths.iter().map(quote).collect::<Vec<_>>().join(" "))
}

/// Whether the shell at `shell` quotes like PowerShell rather than a POSIX
/// shell, going by its name whatever the platform. Nothing for a shell it
/// does not know.
fn reads_powershell(shell: &str) -> Option<bool> {
    let shell = shell.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| shell.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(shell);
    let name = unquoted.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "powershell" | "pwsh" => Some(true),
        "sh" | "bash" | "zsh" | "dash" | "ash" | "ksh" | "mksh" | "fish" => Some(false),
        _ => None,
    }
}

impl Terminal {
    /// The bytes to write to the program for `input`, if it takes any.
    pub fn encode(&self, input: &TerminalInput) -> Vec<u8> {
        let mode = *self.term.mode();
        let mut out = Vec::new();
        match input {
            TerminalInput::Key { key: name, mods } => key(&mut out, name, *mods, mode),
            TerminalInput::Text { text } => out.extend_from_slice(text.as_bytes()),
            TerminalInput::Paste { text } => paste(&mut out, text, mode),
            TerminalInput::Paths { paths } => {
                if let Some(words) = shell_words(paths, &configured_shell()) {
                    paste(&mut out, &words, mode);
                }
            }
            TerminalInput::Focus { focused } => {
                if mode.contains(TermMode::FOCUS_IN_OUT) {
                    out.extend_from_slice(if *focused { b"\x1b[I" } else { b"\x1b[O" });
                }
            }
            TerminalInput::Mouse {
                action,
                button,
                col,
                row,
                mods,
            } => {
                let reports = match action {
                    MouseAction::Move => {
                        mode.contains(TermMode::MOUSE_MOTION)
                            || (mode.contains(TermMode::MOUSE_DRAG) && *button != MouseButton::None)
                    }
                    _ => mode.intersects(TermMode::MOUSE_MODE) && *button != MouseButton::None,
                };
                if !reports {
                    return out;
                }
                let mut code = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    MouseButton::None => 3,
                };
                if *action == MouseAction::Move {
                    code += 32;
                }
                code += mouse_mods(*mods);
                mouse_report(
                    &mut out,
                    mode,
                    code,
                    *col,
                    *row,
                    *action == MouseAction::Release,
                );
            }
            TerminalInput::Wheel {
                col,
                row,
                lines,
                mods,
            } => {
                let count = lines.unsigned_abs().min(64);
                if mode.intersects(TermMode::MOUSE_MODE) {
                    let code = if *lines < 0 { 64 } else { 65 } + mouse_mods(*mods);
                    for _ in 0..count {
                        mouse_report(&mut out, mode, code, *col, *row, false);
                    }
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                    let letter = if *lines < 0 { b'A' } else { b'B' };
                    let application = mode.contains(TermMode::APP_CURSOR);
                    for _ in 0..count {
                        cursor_key(&mut out, letter, Mods::default(), application);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{reads_powershell, shell_words};

    #[test]
    fn a_posix_word_keeps_every_metacharacter_inside_single_quotes() {
        let path =
            "-literal path\n$(touch nope)`whoami`;pipe|glob*?[x]>out<in\\slash'quote".to_owned();
        assert_eq!(
            shell_words(&[path], "/bin/zsh").as_deref(),
            Some("'-literal path\n$(touch nope)`whoami`;pipe|glob*?[x]>out<in\\slash'\\''quote'")
        );
    }

    #[test]
    fn a_powershell_word_doubles_its_single_quotes() {
        let path = "C:\\O'Brien\\-literal path\n$env:TEMP\\$(x)`x;pipe|glob*?>out<in".to_owned();
        assert_eq!(
            shell_words(&[path], "pwsh").as_deref(),
            Some("'C:\\O''Brien\\-literal path\n$env:TEMP\\$(x)`x;pipe|glob*?>out<in'")
        );
    }

    #[test]
    fn paths_are_words_apart_and_a_nul_drops_them_all() {
        let paths = ["/tmp/a b", "/tmp/O'Brien"].map(String::from);
        assert_eq!(
            shell_words(&paths, "/bin/bash").as_deref(),
            Some("'/tmp/a b' '/tmp/O'\\''Brien'")
        );
        let unsafe_paths = ["/tmp/safe", "/tmp/unsafe\0suffix"].map(String::from);
        assert_eq!(shell_words(&unsafe_paths, "/bin/bash"), None);
    }

    #[test]
    fn the_quoting_follows_the_shell_named_not_the_platform() {
        assert_eq!(reads_powershell("/bin/zsh"), Some(false));
        assert_eq!(
            reads_powershell("C:\\Program Files\\Git\\bin\\bash.exe"),
            Some(false)
        );
        assert_eq!(
            reads_powershell("\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\""),
            Some(true)
        );
        assert_eq!(
            reads_powershell("/opt/microsoft/powershell/7/PWSH"),
            Some(true)
        );
        assert_eq!(reads_powershell("/bin/custom-shell"), None);
    }
}
