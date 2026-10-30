//! Finding words in a chat, as the Mac's find does: only what text parts say, as the reader
//! sees it, the words taken as written rather than as a pattern.

use serde_json::Value;
use sikemux_wire::chat::{ChatMessage, ChatPart};

/// What the reader looks for, and how.
#[derive(Clone, Debug)]
pub struct Query {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
}

/// One place a query was found: the part it is in and where in that part's text, counted in
/// UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub part: String,
    pub start: usize,
    pub end: usize,
}

/// The text of `part` a find looks through: a text part's words as they are drawn, without
/// their markup, each block on a line of its own. Other parts are never searched.
pub fn searchable(part: &ChatPart) -> Option<String> {
    let ChatPart::Text { text, md, .. } = part else {
        return None;
    };
    let Some(tree) = md else {
        return Some(text.clone());
    };
    let mut shown = String::new();
    if let Value::Array(blocks) = &tree.blocks {
        for block in blocks {
            if !shown.is_empty() {
                shown.push('\n');
            }
            visible(block, &mut shown);
        }
    }
    Some(shown)
}

/// The words a tree node draws.
fn visible(node: &Value, out: &mut String) {
    match node {
        Value::String(text) => out.push_str(text),
        Value::Object(element) => {
            match element.get("t").and_then(Value::as_str) {
                Some("code" | "pre") => {
                    out.push_str(element.get("v").and_then(Value::as_str).unwrap_or_default());
                    return;
                }
                Some("br") => {
                    out.push('\n');
                    return;
                }
                Some("img" | "hr" | "fnref") => return,
                _ => {}
            }
            let blocks = matches!(
                element.get("t").and_then(Value::as_str),
                Some("ul" | "ol" | "quote" | "details")
            );
            for (at, child) in children(element).enumerate() {
                if blocks && at > 0 {
                    out.push('\n');
                }
                visible(child, out);
            }
        }
        _ => {}
    }
}

fn children(element: &serde_json::Map<String, Value>) -> Box<dyn Iterator<Item = &Value> + '_> {
    if let Some(Value::Array(c)) = element.get("c") {
        return Box::new(c.iter());
    }
    if element.get("t").and_then(Value::as_str) == Some("table") {
        let head = element
            .get("head")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        let rows = element
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|row| row.as_array().into_iter().flatten());
        return Box::new(
            head.chain(rows)
                .flat_map(|cell| cell.as_array().into_iter().flatten()),
        );
    }
    Box::new(std::iter::empty())
}

/// Every place `query` is found in `row`'s text parts, in reading order.
pub fn found_in(row: &ChatMessage, query: &Query) -> Vec<Found> {
    row.parts
        .iter()
        .filter_map(|part| Some((part_id(part)?, searchable(part)?)))
        .flat_map(|(part, text)| {
            matches(&text, query)
                .into_iter()
                .map(move |(start, end)| Found {
                    part: part.clone(),
                    start,
                    end,
                })
        })
        .collect()
}

fn part_id(part: &ChatPart) -> Option<String> {
    match part {
        ChatPart::Text { id, .. } => Some(id.clone()),
        _ => None,
    }
}

/// Where `query` is in `text`, as UTF-16 ranges. The query is taken literally; ignoring case
/// folds each character simply, and a whole word has no ASCII letter, digit or `_` either
/// side, as JavaScript's `\b` sees it.
pub fn matches(text: &str, query: &Query) -> Vec<(usize, usize)> {
    let fold = |c: char| -> char {
        match query.match_case {
            true => c,
            false => {
                let mut lower = c.to_lowercase();
                match (lower.next(), lower.next()) {
                    (Some(single), None) => single,
                    _ => c,
                }
            }
        }
    };
    let needle: Vec<char> = query.text.chars().map(fold).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<char> = text.chars().collect();
    let folded: Vec<char> = hay.iter().copied().map(fold).collect();
    let mut units = Vec::with_capacity(hay.len() + 1);
    let mut unit = 0;
    for c in &hay {
        units.push(unit);
        unit += c.len_utf16();
    }
    units.push(unit);
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= folded.len() {
        if folded[at..at + needle.len()] == needle[..] {
            let end = at + needle.len();
            let before = at.checked_sub(1).and_then(|before| hay.get(before));
            let whole = !query.whole_word
                || (word(before) != word(hay.get(at))
                    && word(hay.get(end - 1)) != word(hay.get(end)));
            if whole {
                found.push((units[at], units[end]));
                at = end;
                continue;
            }
        }
        at += 1;
    }
    found
}
