//! Shiki's TextMate grammars, loaded lazily from one compressed bundle and
//! highlighted line by line on irosashi, whose tokens match vscode-textmate's.
//!
//! - [`Library`] reads the bundle and unpacks a language's grammars the first time
//!   it is highlighted.
//! - [`Styles`] turns a token's scopes into a [`Style`] the way a VS Code theme does.
//! - [`Document`] keeps each line's tokenizer state, so an edit retokenizes only until
//!   the state matches what it was.

mod document;
mod library;
mod styles;

pub use document::{Document, Span};
pub use library::Library;
pub use styles::{Style, Styles};

/// Every line of `text` in `language`, or `None` for a language the library lacks.
pub fn highlight(
    library: &Library,
    styles: &Styles,
    language: &str,
    text: &str,
) -> Option<Vec<Vec<Span>>> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut document = Document::new(library.session(language)?, None);
    document.reset(lines.len());
    document.tokenize(styles, |line| lines[line], lines.len());
    Some(
        (0..lines.len())
            .map(|line| document.spans(line).to_vec())
            .collect(),
    )
}
