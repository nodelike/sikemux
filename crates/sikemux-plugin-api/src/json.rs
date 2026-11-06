//! Reading the fields of an answer no plugin has a shape for.

use serde_json::Value;

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

pub fn some_text(value: &Value, path: &str) -> Option<String> {
    at(value, path)
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub fn number(value: &Value, path: &str) -> Option<u64> {
    at(value, path).as_u64()
}
