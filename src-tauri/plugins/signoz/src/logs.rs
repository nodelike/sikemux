use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};

use crate::client;
use crate::error::SignozResult;
use crate::filter::Scope;
use crate::query::{self, all_of, quote};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;
const TAIL_INTERVAL: Duration = Duration::from_secs(2);
const TAIL_MAX_BACKOFF: Duration = Duration::from_secs(30);
const TAIL_GIVE_UP_AFTER: u32 = 8;
const TAIL_REMEMBERED_IDS: usize = 4_000;

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogSearch {
    #[serde(flatten)]
    pub scope: Scope,
    pub text: Option<String>,
    #[serde(default)]
    pub severities: Vec<String>,
    pub trace_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub id: String,
    pub timestamp: String,
    pub service: Option<String>,
    pub severity: Option<String>,
    pub body: String,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    /// Where the cause usually is: bodies say "request failed", attributes say why.
    pub attributes: BTreeMap<String, Value>,
    pub resources: BTreeMap<String, Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    pub next_offset: Option<u32>,
}

#[derive(Serialize)]
struct TailTick {
    lines: Vec<LogLine>,
    error: Option<String>,
}

fn present(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn expression(search: &LogSearch) -> SignozResult<Option<String>> {
    let severities: Vec<String> = search
        .severities
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| quote(&s.trim().to_uppercase()))
        .collect();
    let own = [
        present(&search.text).map(|text| format!("body CONTAINS {}", quote(text))),
        (!severities.is_empty()).then(|| format!("severity_text IN ({})", severities.join(", "))),
        present(&search.trace_id).map(|trace| format!("trace_id = {}", quote(trace))),
    ];
    Ok(all_of(
        own.into_iter().flatten().chain(search.scope.clauses()?),
    ))
}

fn text_of(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn merge(into: &mut BTreeMap<String, Value>, from: Option<&Value>) {
    if let Some(map) = from.and_then(Value::as_object) {
        into.extend(map.iter().map(|(key, value)| (key.clone(), value.clone())));
    }
}

pub fn parse_line(row: &Value) -> Option<LogLine> {
    let data = row.get("data")?;
    let mut attributes = BTreeMap::new();
    for kind in ["attributes_string", "attributes_number", "attributes_bool"] {
        merge(&mut attributes, data.get(kind));
    }
    let mut resources = BTreeMap::new();
    merge(&mut resources, data.get("resources_string"));
    let service =
        text_of(resources.get("service.name")).or_else(|| text_of(data.get("service.name")));
    Some(LogLine {
        id: text_of(data.get("id"))?,
        timestamp: text_of(row.get("timestamp")).unwrap_or_default(),
        service,
        severity: text_of(data.get("severity_text")),
        body: data
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        trace_id: text_of(data.get("trace_id")),
        span_id: text_of(data.get("span_id")),
        attributes,
        resources,
    })
}

fn list_query(
    search: &LogSearch,
    window: (u64, u64),
    limit: u32,
    offset: u32,
    newest_first: bool,
) -> SignozResult<Value> {
    let direction = if newest_first { "desc" } else { "asc" };
    let spec = json!({
        "signal": "logs",
        "limit": limit,
        "offset": offset,
        "order": [
            { "key": { "name": "timestamp" }, "direction": direction },
            { "key": { "name": "id" }, "direction": direction },
        ],
    });
    Ok(query::builder(
        "raw",
        window,
        query::with_filter(spec, expression(search)?),
    ))
}

fn lines_of(result: &Value) -> Vec<LogLine> {
    result
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(parse_line).collect())
        .unwrap_or_default()
}

const DEFAULT_BUCKETS: u32 = 60;
const MAX_BUCKETS: u32 = 240;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VolumeQuery {
    #[serde(flatten)]
    pub search: LogSearch,
    pub buckets: Option<u32>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBucket {
    /// Unix milliseconds at the start of the bucket.
    pub start: u64,
    pub counts: BTreeMap<String, u64>,
}

/// How many lines of each level arrived in each slice of the window. The
/// severity toggles are left out on purpose: the chart shows the whole shape
/// of the traffic, and the reader's choice only dims part of it.
pub async fn volume(data_dir: &Path, request: VolumeQuery) -> SignozResult<Vec<VolumeBucket>> {
    let search = LogSearch {
        severities: Vec::new(),
        ..request.search
    };
    let (start, end) = search.scope.window()?;
    let buckets = request
        .buckets
        .unwrap_or(DEFAULT_BUCKETS)
        .clamp(1, MAX_BUCKETS);
    let step_seconds = ((end - start) / 1_000 / u64::from(buckets)).max(1);
    let spec = json!({
        "signal": "logs",
        "stepInterval": step_seconds,
        "aggregations": [{ "expression": "count()" }],
        "groupBy": [{ "name": "severity_text", "fieldContext": "log" }],
    });
    let request = query::builder(
        "time_series",
        (start, end),
        query::with_filter(spec, expression(&search)?),
    );
    let result = client::query_range(data_dir, &request).await?;
    Ok(bucket(&result, start, end, step_seconds * 1_000))
}

fn bucket(result: &Value, start: u64, end: u64, step_ms: u64) -> Vec<VolumeBucket> {
    let first = start - start % step_ms;
    let count = ((end.saturating_sub(first)) / step_ms + 1) as usize;
    let mut buckets: Vec<VolumeBucket> = (0..count)
        .map(|index| VolumeBucket {
            start: first + index as u64 * step_ms,
            counts: BTreeMap::new(),
        })
        .collect();
    let series = result
        .pointer("/aggregations/0/series")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for serie in &series {
        let level = serie
            .pointer("/labels/0/value")
            .and_then(Value::as_str)
            .map(|level| level.trim().to_uppercase())
            .filter(|level| !level.is_empty())
            .unwrap_or_else(|| "OTHER".into());
        for point in serie
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(at), Some(value)) = (
                point.get("timestamp").and_then(Value::as_u64),
                point.get("value").and_then(Value::as_f64),
            ) else {
                continue;
            };
            let index = (at.saturating_sub(first) / step_ms) as usize;
            if let Some(slot) = buckets.get_mut(index) {
                *slot.counts.entry(level.clone()).or_default() += value.max(0.0) as u64;
            }
        }
    }
    buckets
}

pub async fn search(data_dir: &Path, search: LogSearch) -> SignozResult<LogPage> {
    let limit = search.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = search.offset.unwrap_or(0);
    let request = list_query(&search, search.scope.window()?, limit, offset, true)?;
    let result = client::query_range(data_dir, &request).await?;
    let lines = lines_of(&result);
    let next_offset = (lines.len() as u32 == limit).then(|| offset.saturating_add(limit));
    Ok(LogPage { lines, next_offset })
}

struct Seen {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Seen {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    fn first_time(&mut self, id: &str) -> bool {
        if self.ids.contains(id) {
            return false;
        }
        self.ids.insert(id.to_string());
        self.order.push_back(id.to_string());
        if self.order.len() > TAIL_REMEMBERED_IDS {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(&oldest);
            }
        }
        true
    }
}

fn latest_ms(lines: &[LogLine]) -> Option<u64> {
    lines
        .iter()
        .filter_map(|line| query::parse_timestamp_ns(&line.timestamp))
        .max()
        .map(|ns| (ns / 1_000_000).max(0) as u64)
}

/// Sends the most recent lines, then every new one as it arrives. Each poll
/// looks back a second past the newest line it has, so a line written late
/// is still caught, and anything already sent is dropped.
pub async fn tail(data_dir: &Path, mut search: LogSearch, sink: StreamSink) -> PluginResult<()> {
    let backlog = search.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    search.offset = None;
    let mut seen = Seen::new();
    let mut since = query::window(search.scope.minutes.or(Some(15))).0;
    let mut first = true;
    let mut failures = 0u32;
    loop {
        let window = (since, query::now_ms());
        let outcome = async {
            let request = if first {
                list_query(&search, window, backlog, 0, true)?
            } else {
                list_query(&search, window, MAX_LIMIT, 0, false)?
            };
            client::query_range(data_dir, &request).await
        }
        .await;
        let wait = match outcome {
            Ok(result) => {
                failures = 0;
                let mut lines = lines_of(&result);
                if first {
                    lines.reverse();
                    first = false;
                }
                lines.retain(|line| seen.first_time(&line.id));
                if let Some(latest) = latest_ms(&lines) {
                    since = since.max(latest.saturating_sub(1_000));
                }
                if !lines.is_empty() {
                    sink.send(reply(TailTick { lines, error: None })?)?;
                }
                TAIL_INTERVAL
            }
            Err(error) => {
                failures += 1;
                sink.send(reply(TailTick {
                    lines: Vec::new(),
                    error: Some(error.to_string()),
                })?)?;
                if failures >= TAIL_GIVE_UP_AFTER {
                    return Ok(());
                }
                (TAIL_INTERVAL * 2u32.pow(failures.min(4))).min(TAIL_MAX_BACKOFF)
            }
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Value {
        json!({
            "timestamp": "2026-09-24T09:11:54.569674944Z",
            "data": {
                "id": "abc",
                "body": "Request completed with server error",
                "severity_text": "ERROR",
                "trace_id": "16db92378d1e0e36039248e129d148f5",
                "span_id": "",
                "attributes_string": { "path": "/upload", "error": "boom" },
                "attributes_number": { "status": 503 },
                "attributes_bool": {},
                "resources_string": { "service.name": "api-gateway" }
            }
        })
    }

    #[test]
    fn lifts_attributes_and_the_service_out_of_a_row() {
        let line = parse_line(&row()).unwrap();
        assert_eq!(line.service.as_deref(), Some("api-gateway"));
        assert_eq!(line.severity.as_deref(), Some("ERROR"));
        assert_eq!(line.span_id, None);
        assert_eq!(line.attributes.get("error"), Some(&json!("boom")));
        assert_eq!(line.attributes.get("status"), Some(&json!(503)));
    }

    #[test]
    fn builds_one_filter_from_the_fields_given() {
        let search = LogSearch {
            scope: Scope {
                service: Some("api".into()),
                ..Scope::default()
            },
            text: Some("can't".into()),
            severities: vec!["ERROR".into(), "FATAL".into()],
            ..LogSearch::default()
        };
        assert_eq!(
            expression(&search).unwrap().unwrap(),
            "(body CONTAINS 'can\\'t') AND (severity_text IN ('ERROR', 'FATAL')) AND (service.name = 'api')"
        );
        assert_eq!(expression(&LogSearch::default()).unwrap(), None);
    }

    #[test]
    fn buckets_every_level_and_leaves_quiet_slices_empty() {
        let result = json!({ "aggregations": [{ "series": [
            { "labels": [{ "value": "error" }], "values": [{ "timestamp": 60_000, "value": 2 }, { "timestamp": 180_000, "value": 1 }] },
            { "labels": [{ "value": "INFO" }], "values": [{ "timestamp": 60_000, "value": 40 }] },
            { "labels": [{ "value": "" }], "values": [{ "timestamp": 120_000, "value": 3 }] },
        ] }] });
        let buckets = bucket(&result, 60_000, 240_000, 60_000);
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0].counts.get("ERROR"), Some(&2));
        assert_eq!(buckets[0].counts.get("INFO"), Some(&40));
        assert_eq!(buckets[1].counts.get("OTHER"), Some(&3));
        assert_eq!(buckets[2].counts.get("ERROR"), Some(&1));
        assert!(buckets[3].counts.is_empty());
    }

    #[test]
    fn remembers_a_bounded_number_of_lines() {
        let mut seen = Seen::new();
        assert!(seen.first_time("a"));
        assert!(!seen.first_time("a"));
        for index in 0..TAIL_REMEMBERED_IDS {
            seen.first_time(&index.to_string());
        }
        assert!(seen.first_time("a"));
    }
}
