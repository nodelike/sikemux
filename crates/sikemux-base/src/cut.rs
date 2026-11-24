//! How a tool's answer marks what was left out of it, so that text is recognised when an
//! agent sends it back as if it were whole.

use serde_json::Value;

/// Ends a text that was cut, and stands for the items left off the end of a list.
pub const MARK: &str = "…[cut by Sikemux]";
/// A text this short can be its author's own words ending in an ellipsis.
const LEAST_CUT_CHARS: usize = 200;

/// Whether any text in `value` carries [`MARK`].
pub fn marked(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains(MARK),
        Value::Array(items) => items.iter().any(marked),
        Value::Object(fields) => fields.values().any(marked),
        _ => false,
    }
}

/// Whether `text` is the start of `whole` with an ellipsis where the rest was cut.
pub fn cut_of(text: &str, whole: &str) -> bool {
    let start = text.strip_suffix(MARK).or_else(|| text.strip_suffix('…'));
    start.is_some_and(|start| {
        start.chars().count() >= LEAST_CUT_CHARS && start != whole && whole.starts_with(start)
    })
}
