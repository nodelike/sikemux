//! How markdown reads in the app: its parsing options and the look of text meant for reading.

use gpui_kit::{px, App, Hsla};
use gpui_transcript::{Markdown, Style};
use serde_json::Value;
use sikemux_markdown::{Doc, MarkdownOptions, MdOp};

use crate::theme::{self, Tokens};

pub(crate) const OPTIONS: MarkdownOptions = MarkdownOptions {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: true,
    html_layout: true,
    references: false,
    file_paths: false,
};

/// `text` as the tree the transcript draws.
pub(crate) fn render(text: &str) -> Markdown {
    let blocks = match Doc::read(text, OPTIONS).into_iter().next() {
        Some(MdOp::List { c, .. }) => Value::Array(c),
        _ => Value::Array(Vec::new()),
    };
    Markdown::new(&blocks, text.encode_utf16().count())
}

/// How the app reads markdown at `text_size` points at the default text size, growing and
/// shrinking with the person's choice as the rest of the interface's text does.
pub(crate) fn reading_style(text_size: f32, cx: &App) -> Style {
    let t = Tokens::get(cx);
    let mut mono = gpui_kit::font(theme::MONO);
    mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
        theme::MONO_ICONS.into()
    ]));
    Style {
        font: theme::UI.into(),
        mono,
        text_size: px(text_size * theme::text_scale(cx)),
        line_height: 1.72,
        ink: t.text_primary,
        ink_dim: t.text_tertiary,
        ink_faint: t.text_disabled,
        accent: t.accent,
        line: Hsla { a: 0.8, ..t.line },
        line_soft: t.line_soft,
        selection: Hsla { a: 0.3, ..t.accent },
        mark_ground: Hsla { a: 0.1, ..t.accent },
        table_ground: Hsla {
            a: 0.72,
            ..t.line_soft
        },
        code_ground: Hsla {
            a: 0.72,
            ..t.surface_sunken
        },
        ..Style::default()
    }
}

#[cfg(test)]
mod tests {
    use gpui_transcript::{DocFind, Query};

    use super::*;

    fn query(text: &str, match_case: bool, whole_word: bool) -> Query {
        Query {
            text: text.to_owned(),
            match_case,
            whole_word,
        }
    }

    const TEXT: &str = "# Notes\n\nFoo the food, then foo again.\n\n- one foo\n- `foo()` in code\n";

    #[test]
    fn find_counts_every_place_in_the_preview_as_it_reads() {
        let markdown = render(TEXT);
        let count = |query: Query| DocFind::new(&markdown, &query).found().1;
        assert_eq!(count(query("foo", false, false)), 5);
        assert_eq!(count(query("Foo", true, false)), 1);
        assert_eq!(count(query("foo", false, true)), 4);
        assert_eq!(count(query("absent", false, false)), 0);
        assert_eq!(count(query("#", false, false)), 0);
    }

    #[test]
    fn stepping_goes_round_the_places_found() {
        let markdown = render(TEXT);
        let mut found = DocFind::new(&markdown, &query("foo", false, true));
        found.step(false);
        assert_eq!(found.found(), (3, 4));
        found.step(true);
        assert_eq!(found.found(), (0, 4));
        found.step(true);
        assert_eq!(found.found(), (1, 4));
    }
}
