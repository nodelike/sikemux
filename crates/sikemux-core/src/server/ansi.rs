//! A line a program printed for a terminal, as text with its colours and
//! weights read off its escape codes.

use std::sync::LazyLock;

use regex::Regex;
use sikemux_plugin_api::codehost::{LogColor, LogSpan};

/// Control sequences, operating system commands and lone two-byte escapes.
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[([0-9;:?]*)([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?|\x1b[@-Z\\-_]?")
        .unwrap()
});

#[derive(Clone, Default, PartialEq)]
struct Style {
    fg: Option<LogColor>,
    bg: Option<LogColor>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

/// `5;n` or `2;r;g;b` after a 38 or 48, and how many codes it took.
fn extended(codes: &[u32]) -> (Option<LogColor>, usize) {
    let byte = |at: usize| codes.get(at).copied().unwrap_or_default().min(255) as u8;
    match codes.first() {
        Some(5) => (
            codes.get(1).map(|_| LogColor::Palette { index: byte(1) }),
            2,
        ),
        Some(2) => (
            Some(LogColor::Rgb {
                hex: format!("#{:02x}{:02x}{:02x}", byte(1), byte(2), byte(3)),
            }),
            4,
        ),
        _ => (None, 1),
    }
}

fn apply(style: &mut Style, params: &str) {
    let codes: Vec<u32> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split([';', ':'])
            .map(|part| part.parse().unwrap_or_default())
            .collect()
    };
    let mut at = 0;
    while at < codes.len() {
        let palette = |index: u32| Some(LogColor::Palette { index: index as u8 });
        match codes[at] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => (style.bold, style.dim) = (false, false),
            23 => style.italic = false,
            24 => style.underline = false,
            code @ 30..=37 => style.fg = palette(code - 30),
            code @ 90..=97 => style.fg = palette(code - 90 + 8),
            39 => style.fg = None,
            code @ 40..=47 => style.bg = palette(code - 40),
            code @ 100..=107 => style.bg = palette(code - 100 + 8),
            49 => style.bg = None,
            code @ (38 | 48) => {
                let (color, used) = extended(&codes[at + 1..]);
                if code == 38 {
                    style.fg = color;
                } else {
                    style.bg = color;
                }
                at += used;
            }
            _ => {}
        }
        at += 1;
    }
}

/// `line` without its escape codes, and its styled stretches, which are none
/// when it has no style at all.
pub(crate) fn spans_of(line: &str) -> (String, Vec<LogSpan>) {
    if !line.contains('\x1b') {
        return (line.to_owned(), Vec::new());
    }
    let mut plain = String::with_capacity(line.len());
    let mut spans: Vec<LogSpan> = Vec::new();
    let mut style = Style::default();
    let mut push = |text: &str, style: &Style, spans: &mut Vec<LogSpan>| {
        if text.is_empty() {
            return;
        }
        plain.push_str(text);
        if let Some(last) = spans.last_mut().filter(|last| same(last, style)) {
            last.text.push_str(text);
            return;
        }
        spans.push(LogSpan {
            text: text.to_owned(),
            fg: style.fg.clone(),
            bg: style.bg.clone(),
            bold: style.bold,
            dim: style.dim,
            italic: style.italic,
            underline: style.underline,
        });
    };
    let mut last = 0;
    for found in ESCAPE.captures_iter(line) {
        let whole = found.get(0).expect("a match has its whole");
        push(&line[last..whole.start()], &style, &mut spans);
        last = whole.end();
        if found.get(2).is_some_and(|end| end.as_str() == "m") {
            apply(
                &mut style,
                found.get(1).map_or("", |params| params.as_str()),
            );
        }
    }
    push(&line[last..], &style, &mut spans);
    if spans.iter().all(|span| same(span, &Style::default())) {
        spans.clear();
    }
    (plain, spans)
}

fn same(span: &LogSpan, style: &Style) -> bool {
    span.fg == style.fg
        && span.bg == style.bg
        && span.bold == style.bold
        && span.dim == style.dim
        && span.italic == style.italic
        && span.underline == style.underline
}
