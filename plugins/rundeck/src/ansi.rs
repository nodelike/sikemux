//! A line of output's terminal codes turned into styled stretches, read by the
//! same parser the terminal uses. Codes that move the cursor or set a title are
//! dropped.

use sikemux_plugin_rundeck_views::{StyledText, TermColor};
use vte::ansi::{Attr, Color, Handler, NamedColor, Processor};

#[derive(Default)]
struct Runs {
    runs: Vec<StyledText>,
    style: StyledText,
}

impl Runs {
    fn push(&mut self, text: &str) {
        match self.runs.last_mut() {
            Some(last) if same_style(last, &self.style) => last.text.push_str(text),
            _ => self.runs.push(StyledText {
                text: text.to_owned(),
                ..self.style.clone()
            }),
        }
    }
}

fn same_style(a: &StyledText, b: &StyledText) -> bool {
    (a.fg, a.bg, a.bold, a.dim, a.italic, a.underline)
        == (b.fg, b.bg, b.bold, b.dim, b.italic, b.underline)
}

fn color(color: Color) -> Option<TermColor> {
    match color {
        Color::Named(named) => {
            let index = named as usize;
            (index < 16).then_some(TermColor::Palette { index: index as u8 })
        }
        Color::Indexed(index) => Some(TermColor::Palette { index }),
        Color::Spec(rgb) => Some(TermColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
    }
}

impl Handler for Runs {
    fn input(&mut self, c: char) {
        let mut buffer = [0; 4];
        self.push(c.encode_utf8(&mut buffer));
    }

    fn put_tab(&mut self, count: u16) {
        self.push(&"\t".repeat(count as usize));
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        let style = &mut self.style;
        match attr {
            Attr::Reset => {
                *style = StyledText::default();
            }
            Attr::Bold => style.bold = true,
            Attr::Dim => style.dim = true,
            Attr::Italic => style.italic = true,
            Attr::Underline
            | Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline => style.underline = true,
            Attr::CancelBold => style.bold = false,
            Attr::CancelBoldDim => {
                style.bold = false;
                style.dim = false;
            }
            Attr::CancelItalic => style.italic = false,
            Attr::CancelUnderline => style.underline = false,
            Attr::Foreground(Color::Named(NamedColor::Foreground)) => style.fg = None,
            Attr::Background(Color::Named(NamedColor::Background)) => style.bg = None,
            Attr::Foreground(value) => style.fg = color(value),
            Attr::Background(value) => style.bg = color(value),
            _ => {}
        }
    }
}

/// `text` in its styles, or nothing when it carries no terminal codes.
pub fn styled(text: &str) -> Vec<StyledText> {
    if !text.contains('\u{1b}') {
        return Vec::new();
    }
    let mut runs = Runs::default();
    Processor::<vte::ansi::StdSyncHandler>::new().advance(&mut runs, text.as_bytes());
    runs.runs
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: &str = "\u{1b}";

    fn run(text: &str, change: impl FnOnce(&mut StyledText)) -> StyledText {
        let mut run = StyledText {
            text: text.to_owned(),
            ..StyledText::default()
        };
        change(&mut run);
        run
    }

    fn palette(index: u8) -> Option<TermColor> {
        Some(TermColor::Palette { index })
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert!(styled("hello").is_empty());
        assert!(styled("").is_empty());
    }

    #[test]
    fn maps_standard_bright_and_background_colours_and_bold() {
        assert_eq!(
            styled(&format!("{ESC}[31mred{ESC}[0m plain")),
            vec![run("red", |r| r.fg = palette(1)), run(" plain", |_| {})]
        );
        assert_eq!(
            styled(&format!("{ESC}[1;92mok{ESC}[22m!")),
            vec![
                run("ok", |r| {
                    r.fg = palette(10);
                    r.bold = true;
                }),
                run("!", |r| r.fg = palette(10)),
            ]
        );
        assert_eq!(
            styled(&format!("{ESC}[44mbg{ESC}[49m")),
            vec![run("bg", |r| r.bg = palette(4))]
        );
    }

    #[test]
    fn treats_an_empty_sgr_as_a_reset() {
        assert_eq!(
            styled(&format!("{ESC}[33my{ESC}[mn")),
            vec![run("y", |r| r.fg = palette(3)), run("n", |_| {})]
        );
    }

    #[test]
    fn reads_256_colour_and_truecolour() {
        assert_eq!(
            styled(&format!("{ESC}[38;5;31mx{ESC}[38;2;1;2;3;1my")),
            vec![
                run("x", |r| r.fg = palette(31)),
                run("y", |r| {
                    r.fg = Some(TermColor::Rgb { r: 1, g: 2, b: 3 });
                    r.bold = true;
                }),
            ]
        );
    }

    #[test]
    fn drops_cursor_erase_and_title_sequences() {
        assert_eq!(
            styled(&format!("a{ESC}[2Kb{ESC}[1Ac{ESC}]0;title\u{7}d")),
            vec![run("abcd", |_| {})]
        );
    }
}
