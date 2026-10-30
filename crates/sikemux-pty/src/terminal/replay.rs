use std::fmt::Write;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use sikemux_wire::grid::{self as wire, Color, SpanStyle};

use super::Terminal;

fn color(out: &mut String, color: Color, base: u16) {
    match color {
        Color::Named(index @ 0..=7) => {
            let _ = write!(out, ";{}", base + index);
        }
        Color::Named(index @ 8..=15) => {
            let _ = write!(out, ";{}", base + 60 + index - 8);
        }
        Color::Named(index @ 259..=266) => {
            let _ = write!(out, ";{}", base + index - 259);
        }
        Color::Named(_) => {}
        Color::Indexed(index) => {
            let _ = write!(out, ";{};5;{index}", base + 8);
        }
        Color::Rgb([r, g, b]) => {
            let _ = write!(out, ";{};2;{r};{g};{b}", base + 8);
        }
    }
}

/// The look of a span without its link, which is set apart.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    fg: Color,
    bg: Color,
    flags: u16,
    underline_color: Option<Color>,
}

impl Look {
    const PLAIN: Look = Look {
        fg: Color::Named(wire::NAMED_FOREGROUND),
        bg: Color::Named(wire::NAMED_BACKGROUND),
        flags: 0,
        underline_color: None,
    };

    fn of(style: &SpanStyle<'_>) -> Self {
        Look {
            fg: style.fg,
            bg: style.bg,
            flags: style.flags,
            underline_color: style.underline_color,
        }
    }

    fn sgr(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (flag, code) in [
            (wire::STYLE_BOLD, "1"),
            (wire::STYLE_DIM, "2"),
            (wire::STYLE_ITALIC, "3"),
            (wire::STYLE_UNDERLINE, "4"),
            (wire::STYLE_DOUBLE_UNDERLINE, "4:2"),
            (wire::STYLE_CURLY_UNDERLINE, "4:3"),
            (wire::STYLE_DOTTED_UNDERLINE, "4:4"),
            (wire::STYLE_DASHED_UNDERLINE, "4:5"),
            (wire::STYLE_INVERSE, "7"),
            (wire::STYLE_HIDDEN, "8"),
            (wire::STYLE_STRIKE, "9"),
        ] {
            if self.flags & flag != 0 {
                out.push(';');
                out.push_str(code);
            }
        }
        color(out, self.fg, 30);
        color(out, self.bg, 40);
        match self.underline_color {
            Some(Color::Indexed(index)) => {
                let _ = write!(out, ";58;5;{index}");
            }
            Some(Color::Rgb([r, g, b])) => {
                let _ = write!(out, ";58;2;{r};{g};{b}");
            }
            _ => {}
        }
        out.push('m');
    }
}

impl Terminal {
    /// Bytes that rebuild this screen, its history and modes in a terminal
    /// of the same size. On the alternate screen only that screen is kept.
    pub fn replay(&self) -> Vec<u8> {
        let mut out = String::new();
        let mode = *self.term.mode();
        let first = if mode.contains(TermMode::ALT_SCREEN) {
            out.push_str("\x1b[?1049h\x1b[H\x1b[2J");
            self.pushed
        } else {
            self.first_line()
        };
        let last = self.last_line();
        let cols = self.term.columns();
        let mut look = Look::PLAIN;
        let mut link: Option<String> = None;
        for line in first..=last {
            let mut width = 0;
            let wrapped = self
                .walk_line(line, |style, wide, cluster, text| {
                    let next = Look::of(style);
                    if next != look {
                        next.sgr(&mut out);
                        look = next;
                    }
                    if style.link != link.as_deref() {
                        let _ = write!(out, "\x1b]8;;{}\x1b\\", style.link.unwrap_or(""));
                        link = style.link.map(str::to_owned);
                    }
                    out.push_str(text);
                    let cells = if cluster { 1 } else { text.chars().count() };
                    width += cells * if wide { 2 } else { 1 };
                })
                .unwrap_or(false);
            if wrapped {
                if width < cols && look != Look::PLAIN {
                    out.push_str("\x1b[0m");
                    look = Look::PLAIN;
                }
                out.extend(std::iter::repeat_n(' ', cols.saturating_sub(width)));
                continue;
            }
            if line == last {
                break;
            }
            if link.take().is_some() {
                out.push_str("\x1b]8;;\x1b\\");
            }
            if look != Look::PLAIN {
                out.push_str("\x1b[0m");
                look = Look::PLAIN;
            }
            out.push_str("\r\n");
        }
        if link.is_some() {
            out.push_str("\x1b]8;;\x1b\\");
        }
        out.push_str("\x1b[0m");
        let cursor = self.term.grid().cursor.point;
        let _ = write!(out, "\x1b[{};{}H", cursor.line.0 + 1, cursor.column.0 + 1);
        for (flag, set) in [
            (TermMode::APP_CURSOR, "\x1b[?1h"),
            (TermMode::APP_KEYPAD, "\x1b="),
            (TermMode::BRACKETED_PASTE, "\x1b[?2004h"),
            (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
            (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
            (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
            (TermMode::SGR_MOUSE, "\x1b[?1006h"),
            (TermMode::UTF8_MOUSE, "\x1b[?1005h"),
            (TermMode::FOCUS_IN_OUT, "\x1b[?1004h"),
        ] {
            if mode.contains(flag) {
                out.push_str(set);
            }
        }
        for (flag, unset) in [
            (TermMode::SHOW_CURSOR, "\x1b[?25l"),
            (TermMode::LINE_WRAP, "\x1b[?7l"),
            (TermMode::ALTERNATE_SCROLL, "\x1b[?1007l"),
        ] {
            if !mode.contains(flag) {
                out.push_str(unset);
            }
        }
        let style = self.term.cursor_style();
        let shape = match style.shape {
            CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => 1,
            CursorShape::Underline => 3,
            CursorShape::Beam => 5,
        } + u8::from(!style.blinking);
        let _ = write!(out, "\x1b[{shape} q");
        out.into_bytes()
    }
}
