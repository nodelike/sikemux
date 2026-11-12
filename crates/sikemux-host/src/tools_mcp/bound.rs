//! Keeps every answer an agent reads under [`BUDGET`] bytes, cutting the longest
//! texts and lists first and naming in `omitted` what went and how to get it.

use serde_json::{json, Value};

/// About five thousand tokens: well under what Claude Code shows the model, and
/// above nearly every answer the tools give.
pub(super) const BUDGET: usize = 20 * 1024;

/// What is kept of a text, at least, however far over the budget an answer is.
const LEAST_TEXT: usize = 400;
const MOST_CUTS: usize = 24;
/// Room kept for the `omitted` notes themselves.
const NOTES: usize = 512;

/// Arguments that ask for less, in the order the hint names them.
const NARROWING: [&str; 21] = [
    "selector", "query", "search", "grep", "jql", "filter", "filters", "fields", "card", "since",
    "around", "chars", "lines", "tail", "limit", "perPage", "page", "offset", "start", "cursor",
    "next",
];

/// `value` cut to fit the budget, with `omitted` saying what was left out. `takes`
/// says whether the tool takes an argument, for the hint on how to ask for less.
pub(super) fn bounded(mut value: Value, takes: impl Fn(&str) -> bool) -> Value {
    if size(&value) <= BUDGET {
        return value;
    }
    let mut omitted = Vec::new();
    for _ in 0..MOST_CUTS {
        let over = (size(&value) + NOTES).saturating_sub(BUDGET);
        if over == 0 {
            break;
        }
        match cut(&mut value, over, String::new()) {
            Some(note) => omitted.push(note),
            None => break,
        }
    }
    if size(&value) > BUDGET {
        let text = value.to_string();
        omitted = vec![format!("all after {BUDGET} of {} bytes", text.len())];
        value = Value::String(cut_text(&text, BUDGET).to_owned());
    }
    let narrowing: Vec<&str> = NARROWING.into_iter().filter(|name| takes(name)).collect();
    if !narrowing.is_empty() {
        omitted.push(format!(
            "to see what was cut, call again narrowed with {}",
            narrowing.join(", ")
        ));
    }
    match value {
        Value::Object(mut fields) => {
            let mut notes: Vec<Value> = match fields.remove("omitted") {
                Some(Value::Array(notes)) => notes,
                Some(note) => vec![note],
                None => Vec::new(),
            };
            notes.extend(omitted.into_iter().map(Value::String));
            fields.insert("omitted".into(), notes.into());
            Value::Object(fields)
        }
        other => json!({ "value": other, "omitted": omitted }),
    }
}

/// An error's words, cut to the budget.
pub(super) fn bounded_text(text: &str) -> String {
    if text.len() <= BUDGET {
        return text.to_owned();
    }
    format!(
        "{}\n({} more bytes left out)",
        cut_text(text, BUDGET),
        text.len() - BUDGET
    )
}

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

/// Cuts at least `over` bytes from where most of `value`'s bytes are, and says what
/// went. A list holding one item bigger than all its others is cut inside that item.
fn cut(value: &mut Value, over: usize, path: String) -> Option<String> {
    match value {
        Value::String(text) => {
            let length = text.chars().count();
            let keep = text.len().saturating_sub(over + 3).max(LEAST_TEXT);
            if keep >= text.len() {
                return None;
            }
            *text = format!("{}…", cut_text(text, keep));
            let kept = text.chars().count() - 1;
            Some(format!(
                "{} after {kept} of {length} characters",
                name(&path, "text")
            ))
        }
        Value::Object(fields) => {
            let (key, _) = biggest(fields.iter().map(|(key, value)| (key.clone(), value)))?;
            let inner = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            cut(fields.get_mut(&key)?, over, inner)
        }
        Value::Array(items) => {
            let total: usize = items.iter().map(size).sum();
            let (index, largest) = biggest(items.iter().enumerate())?;
            if items.len() == 1 || largest * 2 > total {
                return cut(&mut items[index], over, format!("{path}[{index}]"));
            }
            let count = items.len();
            let mut dropped = 0;
            while items.len() > 1 && dropped < over {
                dropped += items.pop().map_or(0, |item| size(&item) + 1);
            }
            Some(format!(
                "{} of {count} {} after the first {}",
                count - items.len(),
                name(&path, "items"),
                items.len()
            ))
        }
        _ => None,
    }
}

fn biggest<'a, K>(entries: impl Iterator<Item = (K, &'a Value)>) -> Option<(K, usize)> {
    entries
        .map(|(key, value)| (key, size(value)))
        .max_by_key(|(_, size)| *size)
}

fn name<'a>(path: &'a str, fallback: &'a str) -> &'a str {
    if path.is_empty() {
        fallback
    } else {
        path
    }
}

/// The start of `text`, at most `bytes` long, ending on a whole character.
fn cut_text(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
