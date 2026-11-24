//! Prefixes bare agent names in prose with `@` so every client draws their pill,
//! skipping code blocks, inline code, links, images, HTML and URLs.

use std::ops::Range;

use pulldown_cmark::{Event, Options as ParserOptions, Parser, Tag, TagEnd};

/// Rewrites whole, capital-exact occurrences of any name in `names` in markdown prose to `@Name`,
/// leaving code, links, images, HTML, URLs and names already prefixed with `@` alone.
/// Returns `None` when nothing in `text` changes.
pub fn prefix_bare_names(text: &str, names: &[&str]) -> Option<String> {
    let mut wanted: Vec<&str> = names
        .iter()
        .copied()
        .map(str::trim)
        .filter(|name| {
            name.chars().count() >= 2 && *name != "Chief" && name.chars().all(char::is_alphanumeric)
        })
        .collect();
    if wanted.is_empty() {
        return None;
    }
    wanted.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    wanted.dedup();
    if !wanted.iter().any(|name| text.contains(name)) {
        return None;
    }

    let flags = ParserOptions::ENABLE_TABLES
        | ParserOptions::ENABLE_STRIKETHROUGH
        | ParserOptions::ENABLE_TASKLISTS
        | ParserOptions::ENABLE_FOOTNOTES
        | ParserOptions::ENABLE_MATH;
    let mut skip = 0usize;
    let mut insertions: Vec<usize> = Vec::new();
    let mut taken: Vec<Range<usize>> = Vec::new();

    for (event, range) in Parser::new_ext(text, flags).into_offset_iter() {
        match &event {
            Event::Start(
                Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. } | Tag::HtmlBlock,
            ) => skip += 1,
            Event::End(TagEnd::CodeBlock | TagEnd::Link | TagEnd::Image | TagEnd::HtmlBlock) => {
                skip = skip.saturating_sub(1);
            }
            Event::Text(words) if skip == 0 => {
                let Some(slice) = text
                    .get(range.clone())
                    .filter(|slice| *slice == words.as_ref())
                else {
                    continue;
                };
                taken.clear();
                for name in &wanted {
                    for (rel, _) in slice.match_indices(name) {
                        let start = range.start + rel;
                        let end = start + name.len();
                        let hit = start..end;
                        if taken
                            .iter()
                            .any(|held| held.start < hit.end && hit.start < held.end)
                        {
                            continue;
                        }
                        if is_bare_prose_name(text, start, end) {
                            taken.push(hit);
                            insertions.push(start);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if insertions.is_empty() {
        return None;
    }
    insertions.sort_unstable();
    insertions.dedup();
    let mut out = String::with_capacity(text.len() + insertions.len());
    let mut cursor = 0;
    for at in insertions {
        out.push_str(text.get(cursor..at).unwrap_or_default());
        out.push('@');
        cursor = at;
    }
    out.push_str(text.get(cursor..).unwrap_or_default());
    Some(out)
}

fn is_bare_prose_name(text: &str, start: usize, end: usize) -> bool {
    let before = text.get(..start).and_then(|head| head.chars().next_back());
    if before.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '@' | '/' | '.' | '-')) {
        return false;
    }
    let tail = text.get(end..).unwrap_or_default();
    let mut chars = tail.chars();
    if let Some(after) = chars.next() {
        if after.is_alphanumeric() || matches!(after, '_' | '/' | '-') {
            return false;
        }
        if after == '.' && chars.next().is_some_and(char::is_alphanumeric) {
            return false;
        }
    }
    !in_url_token(text, start, end)
}

fn in_url_token(text: &str, start: usize, end: usize) -> bool {
    let token_start = text
        .get(..start)
        .and_then(|head| head.rfind(char::is_whitespace))
        .map_or(0, |at| at + 1);
    let token_end = text
        .get(end..)
        .and_then(|tail| tail.find(char::is_whitespace))
        .map_or(text.len(), |rel| end + rel);
    text.get(token_start..token_end)
        .is_some_and(|token| token.contains("://"))
}
