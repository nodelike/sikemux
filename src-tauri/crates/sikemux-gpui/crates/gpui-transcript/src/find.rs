//! Finding text in a transcript: every place a query is found, in reading order.

use std::ops::Range;

/// What a reader looks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
}

/// A place the query was found: the text it is in, by row, doc, block and text, and
/// its bytes there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub at: (usize, usize, usize, u32),
    pub range: Range<usize>,
}

/// The places `query` is found in `text`, in order, each a range of `text` itself.
pub(crate) fn ranges_in(text: &str, query: &Query) -> Vec<Range<usize>> {
    if query.text.is_empty() {
        return Vec::new();
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let whole = |range: &Range<usize>| {
        !query.whole_word
            || (!text[..range.start].chars().next_back().is_some_and(word)
                && !text[range.end..].chars().next().is_some_and(word))
    };
    let found: Vec<Range<usize>> = if query.match_case {
        found_in(text, &query.text)
    } else if text.is_ascii() && query.text.is_ascii() {
        found_in(&text.to_ascii_lowercase(), &query.text.to_ascii_lowercase())
    } else {
        folded_found_in(text, &query.text)
    };
    found.into_iter().filter(whole).collect()
}

fn found_in(hay: &str, needle: &str) -> Vec<Range<usize>> {
    hay.match_indices(needle)
        .map(|(at, found)| at..at + found.len())
        .collect()
}

/// Finds `needle` in `text` with both lowered a character at a time, as `É` lowers to `é`
/// but `İ` to two characters, and answers each place as the whole characters of `text` it covers.
fn folded_found_in(text: &str, needle: &str) -> Vec<Range<usize>> {
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    let mut hay = String::with_capacity(text.len());
    let mut from: Vec<Range<usize>> = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let source = at..at + c.len_utf8();
        hay.extend(c.to_lowercase());
        from.resize(hay.len(), source);
    }
    found_in(&hay, &needle)
        .into_iter()
        .filter_map(|found| {
            let (first, last) = (&from[found.start], &from[found.end - 1]);
            let starts = found.start == 0 || from[found.start - 1] != *first;
            let ends = from.get(found.end).is_none_or(|next| next != last);
            (starts && ends).then_some(first.start..last.end)
        })
        .collect()
}

/// The transcript came to find something else, or the reader moved to another place found.
pub struct FoundChanged;
