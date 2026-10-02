use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::Scope;
use crate::query::{self, all_of, quote, View};

const DEFAULT_LIMIT: u32 = 30;
const MAX_LIMIT: u32 = 500;
const BODY_CHARS: usize = 1_000;
const ID_WINDOW_MS: u64 = 10 * 60_000;
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
    pub id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    #[serde(default)]
    pub view: View,
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

/// What every returned line has in common, written once instead of on each line.
#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, Value>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DigestLine {
    pub id: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, Value>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogDigest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,
    pub lines: Vec<DigestLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum LogReply {
    Lines(LogPage),
    Digest(LogDigest),
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
        present(&search.id).map(|id| format!("id = {}", quote(id))),
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

pub async fn search(data_dir: &Path, search: LogSearch) -> SignozResult<LogReply> {
    if search.view == View::Agent {
        if let Some(id) = present(&search.id) {
            return one_line(data_dir, &search, id).await.map(LogReply::Digest);
        }
    }
    let limit = search.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = search.offset.unwrap_or(0);
    let request = list_query(&search, search.scope.window()?, limit, offset, true)?;
    let result = client::query_range(data_dir, &request).await?;
    let lines = lines_of(&result);
    let next_offset = (lines.len() as u32 == limit).then(|| offset.saturating_add(limit));
    Ok(match search.view {
        View::Pane => LogReply::Lines(LogPage { lines, next_offset }),
        View::Agent => LogReply::Digest(digest(lines, next_offset, Some(BODY_CHARS))),
    })
}

/// One line, whole. A SigNoz log id is a KSUID stamped with the line's time,
/// so without a window from the agent the lookup reads only minutes around it.
async fn one_line(data_dir: &Path, search: &LogSearch, id: &str) -> SignozResult<LogDigest> {
    let scope = &search.scope;
    let window_given = scope.start.is_some() || scope.end.is_some() || scope.minutes.is_some();
    let stamped = ksuid_seconds(id).filter(|_| !window_given).map(|seconds| {
        let at = seconds * 1_000;
        (at.saturating_sub(ID_WINDOW_MS), at + ID_WINDOW_MS)
    });
    let mut windows = Vec::from_iter(stamped);
    windows.push(scope.window()?);
    for window in windows {
        let request = list_query(search, window, 1, 0, true)?;
        let lines = lines_of(&client::query_range(data_dir, &request).await?);
        if !lines.is_empty() {
            return Ok(digest(lines, None, None));
        }
    }
    Err(SignozError::NotFound(format!(
        "no log line {id} in the window; pass the minutes or start and end the search used"
    )))
}

fn ksuid_seconds(id: &str) -> Option<u64> {
    if id.len() != 27 {
        return None;
    }
    let mut bytes = [0u8; 20];
    for c in id.chars() {
        let mut carry = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            'a'..='z' => c as u32 - 'a' as u32 + 36,
            _ => return None,
        };
        for byte in bytes.iter_mut().rev() {
            let value = u32::from(*byte) * 62 + carry;
            *byte = (value & 0xff) as u8;
            carry = value >> 8;
        }
        if carry != 0 {
            return None;
        }
    }
    let stamp = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Some(u64::from(stamp) + 1_400_000_000)
}

fn uniform<'a>(mut values: impl Iterator<Item = Option<&'a String>>) -> Option<String> {
    let first = values.next()??;
    values
        .all(|value| value == Some(first))
        .then(|| first.clone())
}

fn common(maps: &[&BTreeMap<String, Value>]) -> BTreeMap<String, Value> {
    let Some((first, rest)) = maps.split_first() else {
        return BTreeMap::new();
    };
    first
        .iter()
        .filter(|(key, value)| rest.iter().all(|map| map.get(*key) == Some(*value)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn without(
    map: BTreeMap<String, Value>,
    shared: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    map.into_iter()
        .filter(|(key, _)| !shared.contains_key(key))
        .collect()
}

fn clipped(body: String, most: Option<usize>) -> String {
    let Some(most) = most else {
        return body;
    };
    let length = body.chars().count();
    if length <= most {
        return body;
    }
    let kept: String = body.chars().take(most).collect();
    format!(
        "{kept}… [{} more chars; search by this line's id for all of it]",
        length - most
    )
}

/// Lines as an agent reads them: anything every line repeats is said once in
/// `shared`, and each line keeps only what sets it apart.
pub fn digest(
    lines: Vec<LogLine>,
    next_offset: Option<u32>,
    body_chars: Option<usize>,
) -> LogDigest {
    let mut lines = lines;
    for line in &mut lines {
        if line.service.is_some()
            && line.resources.get("service.name").and_then(Value::as_str) == line.service.as_deref()
        {
            line.resources.remove("service.name");
        }
    }
    let shared = (lines.len() > 1).then(|| Shared {
        service: uniform(lines.iter().map(|line| line.service.as_ref())),
        severity: uniform(lines.iter().map(|line| line.severity.as_ref())),
        trace_id: uniform(lines.iter().map(|line| line.trace_id.as_ref())),
        attributes: common(
            &lines
                .iter()
                .map(|line| &line.attributes)
                .collect::<Vec<_>>(),
        ),
        resources: common(&lines.iter().map(|line| &line.resources).collect::<Vec<_>>()),
    });
    let hoisted = shared.as_ref();
    let lines = lines
        .into_iter()
        .map(|line| {
            let unless_shared = |value: Option<String>, pick: fn(&Shared) -> &Option<String>| {
                value.filter(|_| hoisted.and_then(|shared| pick(shared).as_ref()).is_none())
            };
            DigestLine {
                service: unless_shared(line.service, |shared| &shared.service),
                severity: unless_shared(line.severity, |shared| &shared.severity),
                trace_id: unless_shared(line.trace_id, |shared| &shared.trace_id),
                attributes: match hoisted {
                    Some(shared) => without(line.attributes, &shared.attributes),
                    None => line.attributes,
                },
                resources: match hoisted {
                    Some(shared) => without(line.resources, &shared.resources),
                    None => line.resources,
                },
                body: clipped(line.body, body_chars),
                id: line.id,
                timestamp: line.timestamp,
                span_id: line.span_id,
            }
        })
        .collect();
    LogDigest {
        shared: shared.filter(|shared| shared != &Shared::default()),
        lines,
        next_offset,
    }
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

    fn line(id: &str, pod: &str, status: u64, body: &str) -> LogLine {
        LogLine {
            id: id.into(),
            timestamp: format!("2026-09-24T09:11:5{id}Z"),
            service: Some("api".into()),
            severity: Some("ERROR".into()),
            body: body.into(),
            trace_id: Some(format!("trace-{id}")),
            span_id: None,
            attributes: BTreeMap::from([
                ("path".into(), json!("/upload")),
                ("status".into(), json!(status)),
            ]),
            resources: BTreeMap::from([
                ("service.name".into(), json!("api")),
                ("k8s.pod.name".into(), json!(pod)),
                ("host.name".into(), json!("node-1")),
            ]),
        }
    }

    #[test]
    fn says_once_what_every_line_repeats() {
        let digest = digest(
            vec![
                line("1", "api-a", 503, "failed"),
                line("2", "api-a", 500, "failed"),
            ],
            Some(30),
            Some(BODY_CHARS),
        );
        let shared = digest.shared.as_ref().unwrap();
        assert_eq!(shared.service.as_deref(), Some("api"));
        assert_eq!(shared.severity.as_deref(), Some("ERROR"));
        assert_eq!(shared.trace_id, None);
        assert_eq!(
            shared.attributes,
            BTreeMap::from([("path".into(), json!("/upload"))])
        );
        assert_eq!(
            shared.resources,
            BTreeMap::from([
                ("host.name".into(), json!("node-1")),
                ("k8s.pod.name".into(), json!("api-a")),
            ])
        );
        let first = &digest.lines[0];
        assert_eq!(first.service, None);
        assert_eq!(first.severity, None);
        assert_eq!(first.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(
            first.attributes,
            BTreeMap::from([("status".into(), json!(503))])
        );
        assert!(first.resources.is_empty());
        assert_eq!(
            serde_json::to_value(&digest).unwrap()["lines"][1],
            json!({ "id": "2", "timestamp": "2026-09-24T09:11:52Z", "body": "failed", "traceId": "trace-2", "attributes": { "status": 500 } })
        );
    }

    #[test]
    fn keeps_a_key_on_its_line_when_only_some_lines_have_it() {
        let mut other = line("2", "api-b", 503, "failed");
        other.attributes.remove("path");
        other.severity = Some("WARN".into());
        let digest = digest(vec![line("1", "api-a", 503, "failed"), other], None, None);
        let shared = digest.shared.unwrap();
        assert_eq!(
            shared.attributes,
            BTreeMap::from([("status".into(), json!(503))])
        );
        assert_eq!(shared.severity, None);
        assert_eq!(
            digest.lines[0].attributes.get("path"),
            Some(&json!("/upload"))
        );
        assert_eq!(
            digest.lines[0].resources.get("k8s.pod.name"),
            Some(&json!("api-a"))
        );
        assert_eq!(digest.lines[1].severity.as_deref(), Some("WARN"));
    }

    #[test]
    fn a_single_line_keeps_everything_but_its_repeated_service_name() {
        let digest = digest(vec![line("1", "api-a", 503, "failed")], None, None);
        assert_eq!(digest.shared, None);
        assert_eq!(digest.lines[0].service.as_deref(), Some("api"));
        assert_eq!(digest.lines[0].resources.len(), 2);
    }

    #[test]
    fn clips_long_bodies_and_says_how_to_read_the_rest() {
        let long = "x".repeat(BODY_CHARS + 25);
        let clipped = digest(vec![line("1", "p", 1, &long)], None, Some(BODY_CHARS));
        let body = &clipped.lines[0].body;
        assert!(body.starts_with(&"x".repeat(BODY_CHARS)));
        assert!(body.ends_with("[25 more chars; search by this line's id for all of it]"));
        let whole = digest(vec![line("1", "p", 1, &long)], None, None);
        assert_eq!(whole.lines[0].body, long);
    }

    #[test]
    fn reads_the_time_a_ksuid_was_stamped_with() {
        assert_eq!(
            ksuid_seconds("0ujtsYcgvSTl8PAuAdqWYSMnLOv"),
            Some(1_507_608_047)
        );
        assert_eq!(ksuid_seconds("not-a-ksuid"), None);
        assert_eq!(ksuid_seconds("zzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn looks_a_line_up_by_its_id() {
        let search = LogSearch {
            id: Some("abc".into()),
            ..LogSearch::default()
        };
        assert_eq!(expression(&search).unwrap().unwrap(), "id = 'abc'");
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
