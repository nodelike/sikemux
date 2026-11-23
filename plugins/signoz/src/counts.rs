// Lines or traces counted per group of attribute values, by SigNoz itself
// rather than by paging through rows.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::checked_key;
use crate::query;

#[derive(Serialize, Debug, PartialEq)]
pub struct Count {
    pub group: BTreeMap<String, Value>,
    pub count: u64,
}

const DEFAULT_GROUPS: u32 = 50;
const MAX_GROUPS: u32 = 500;
const MAX_KEYS: usize = 5;

pub async fn count(
    data_dir: &Path,
    signal: &str,
    window: (u64, u64),
    expression: Option<String>,
    keys: &[String],
    limit: Option<u32>,
) -> SignozResult<Vec<Count>> {
    if keys.len() > MAX_KEYS {
        return Err(SignozError::BadArg(format!(
            "count by at most {MAX_KEYS} attributes"
        )));
    }
    let keys = keys
        .iter()
        .map(|key| checked_key(key).map(str::to_string))
        .collect::<SignozResult<Vec<_>>>()?;
    let spec = json!({
        "signal": signal,
        "aggregations": [{ "expression": "count()" }],
        "groupBy": keys.iter().map(|key| json!({ "name": key })).collect::<Vec<_>>(),
        "order": [{ "key": { "name": "count()" }, "direction": "desc" }],
        "limit": limit.unwrap_or(DEFAULT_GROUPS).clamp(1, MAX_GROUPS),
    });
    let result = client::query_range(
        data_dir,
        &query::builder("scalar", window, query::with_filter(spec, expression)),
    )
    .await?;
    Ok(parse(&result, &keys))
}

fn parse(result: &Value, keys: &[String]) -> Vec<Count> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let row = row.as_array()?;
            let count = row.get(keys.len())?.as_f64()?.max(0.0) as u64;
            let group = keys.iter().cloned().zip(row.iter().cloned()).collect();
            Some(Count { group, count })
        })
        .collect()
}
