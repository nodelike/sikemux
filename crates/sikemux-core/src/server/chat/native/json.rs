//! Reading the JSON the agents send.

use serde_json::Value;

/// `key`'s string.
pub(crate) fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// `key`'s string, when it says something.
pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    str_field(value, key).filter(|text| !text.is_empty())
}

/// `key`'s string, or nothing.
pub(crate) fn text_or_empty<'a>(value: &'a Value, key: &str) -> &'a str {
    str_field(value, key).unwrap_or_default()
}

/// The texts of a message or tool result's content: the string it is, or
/// its text blocks.
pub(crate) fn texts(content: &Value) -> Vec<&str> {
    match content {
        Value::String(text) => vec![text.as_str()],
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| str_field(block, "type") == Some("text"))
            .filter_map(|block| str_field(block, "text"))
            .collect(),
        _ => Vec::new(),
    }
}

/// The texts of `content`, one after another on their own lines.
pub(crate) fn joined_text(content: Option<&Value>) -> String {
    content.map(texts).unwrap_or_default().join("\n")
}

/// Whether JavaScript would call `value` true.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(_) => true,
    }
}

pub(crate) fn non_nullish(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| !v.is_null())
}

/// How a JavaScript template literal prints a value (`undefined` when absent).
pub(crate) fn js_str(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => match (number.as_i64(), number.as_u64()) {
            (Some(n), _) => n.to_string(),
            (_, Some(n)) => n.to_string(),
            _ => js_number(number.as_f64().unwrap_or(f64::NAN)),
        },
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| js_join_item(Some(item)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// How `Array.prototype.join` prints an element: null and undefined become empty.
pub(crate) fn js_join_item(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        other => js_str(other),
    }
}

pub(crate) fn js_number(number: f64) -> String {
    if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e21 {
        format!("{number:.0}")
    } else if number.is_nan() {
        "NaN".to_string()
    } else {
        number.to_string()
    }
}

/// The characters JavaScript's `trim` and `\s` treat as whitespace.
pub(crate) fn is_js_whitespace(ch: char) -> bool {
    ch.is_whitespace() || ch == '\u{feff}'
}

/// A map whose keys are not strings, kept as a list of pairs, since JSON
/// keys can only be strings.
pub(crate) mod pairs {
    use std::collections::HashMap;
    use std::hash::Hash;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        serializer.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Vec::<(K, V)>::deserialize(deserializer).map(|pairs| pairs.into_iter().collect())
    }
}
