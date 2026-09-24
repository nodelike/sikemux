// Attribute names and values SigNoz has seen, for filling in a filter as it
// is typed.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client;
use crate::error::SignozResult;

const SUGGESTIONS: u32 = 50;

#[derive(Deserialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Signal {
    #[default]
    Logs,
    Traces,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Traces => "traces",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyQuery {
    #[serde(default)]
    pub signal: Signal,
    #[serde(default)]
    pub search: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueQuery {
    #[serde(default)]
    pub signal: Signal,
    pub name: String,
    #[serde(default)]
    pub search: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldKey {
    pub name: String,
    pub context: String,
    pub data_type: String,
}

fn query_string(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

pub async fn keys(data_dir: &Path, request: KeyQuery) -> SignozResult<Vec<FieldKey>> {
    let limit = SUGGESTIONS.to_string();
    let query = query_string(&[
        ("signal", request.signal.name()),
        ("searchText", request.search.trim()),
        ("limit", &limit),
    ]);
    let answer = client::request(
        data_dir,
        Method::GET,
        &format!("/api/v1/fields/keys?{query}"),
        None,
    )
    .await?;
    Ok(parse_keys(&answer))
}

fn parse_keys(answer: &Value) -> Vec<FieldKey> {
    let Some(keys) = answer.pointer("/data/keys").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<FieldKey> = keys
        .iter()
        .filter_map(|(name, variants)| {
            let first = variants.as_array()?.first()?;
            let field = |key: &str| {
                first
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Some(FieldKey {
                name: name.clone(),
                context: field("fieldContext"),
                data_type: field("fieldDataType"),
            })
        })
        .collect();
    found.sort_by(|left, right| left.name.cmp(&right.name));
    found
}

pub async fn values(data_dir: &Path, request: ValueQuery) -> SignozResult<Vec<String>> {
    let limit = SUGGESTIONS.to_string();
    let query = query_string(&[
        ("signal", request.signal.name()),
        ("name", request.name.trim()),
        ("searchText", request.search.trim()),
        ("limit", &limit),
    ]);
    let answer = client::request(
        data_dir,
        Method::GET,
        &format!("/api/v1/fields/values?{query}"),
        None,
    )
    .await?;
    Ok(parse_values(&answer))
}

fn parse_values(answer: &Value) -> Vec<String> {
    let Some(values) = answer.pointer("/data/values").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<String> = values
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
        .collect();
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_attribute_names_with_their_kind() {
        let answer = json!({ "status": "success", "data": { "keys": {
            "path": [{ "name": "path", "signal": "logs", "fieldContext": "attribute", "fieldDataType": "string" }],
            "campaign_id": [{ "name": "campaign_id", "fieldContext": "attribute", "fieldDataType": "number" }],
            "broken": "nope",
        } } });
        let keys = parse_keys(&answer);
        assert_eq!(
            keys.iter().map(|key| key.name.as_str()).collect::<Vec<_>>(),
            ["campaign_id", "path"]
        );
        assert_eq!(keys[0].data_type, "number");
    }

    #[test]
    fn reads_values_of_every_type_once() {
        let answer = json!({ "data": { "values": { "stringValues": ["b", "a", "a", ""], "numberValues": [503], "boolValues": [true] }, "complete": false } });
        assert_eq!(parse_values(&answer), ["503", "a", "b", "true"]);
    }
}
