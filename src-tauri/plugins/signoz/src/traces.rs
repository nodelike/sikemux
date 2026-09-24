use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::Scope;
use crate::query::{self, quote};

const MAX_SPANS: u32 = 5_000;
const DEFAULT_LOOKBACK_MINUTES: u32 = 24 * 60;
const DEFAULT_TRACE_LIMIT: u32 = 100;
const MAX_TRACE_LIMIT: u32 = 500;

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TraceOrder {
    #[default]
    Slowest,
    Recent,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraceSearch {
    #[serde(flatten)]
    pub scope: Scope,
    #[serde(default)]
    pub errors_only: bool,
    pub min_duration_ms: Option<f64>,
    #[serde(default)]
    pub order: TraceOrder,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSummary {
    pub trace_id: String,
    pub timestamp: String,
    pub service: String,
    pub name: String,
    pub duration_ms: f64,
    pub error: bool,
    pub status_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracePage {
    pub traces: Vec<TraceSummary>,
    pub next_offset: Option<u32>,
}

fn search_expression(search: &TraceSearch) -> SignozResult<Option<String>> {
    let own = [
        Some("isRoot = true".to_string()),
        search.errors_only.then(|| "hasError = true".to_string()),
        search
            .min_duration_ms
            .filter(|ms| ms.is_finite() && *ms > 0.0)
            .map(|ms| format!("duration_nano >= {}", (ms * 1_000_000.0) as u64)),
    ];
    Ok(query::all_of(
        own.into_iter().flatten().chain(search.scope.clauses()?),
    ))
}

fn summary_of(row: &Value) -> Option<TraceSummary> {
    let data = row.get("data")?;
    Some(TraceSummary {
        trace_id: text(data, "trace_id")?,
        timestamp: text(row, "timestamp")
            .or_else(|| text(data, "timestamp"))
            .unwrap_or_default(),
        service: text(data, "service.name").unwrap_or_default(),
        name: text(data, "name").unwrap_or_default(),
        duration_ms: data
            .get("duration_nano")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            / 1_000_000.0,
        error: data
            .get("has_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status_code: text(data, "response_status_code"),
    })
}

/// A trace per row, read from its root span: where it started and how long the whole of it took.
pub async fn search(data_dir: &Path, search: TraceSearch) -> SignozResult<TracePage> {
    let limit = search
        .limit
        .unwrap_or(DEFAULT_TRACE_LIMIT)
        .clamp(1, MAX_TRACE_LIMIT);
    let offset = search.offset.unwrap_or(0);
    let fields: Vec<Value> = [
        "trace_id",
        "name",
        "duration_nano",
        "has_error",
        "response_status_code",
        "timestamp",
    ]
    .iter()
    .map(|name| json!({ "name": name }))
    .chain(std::iter::once(
        json!({ "name": "service.name", "fieldContext": "resource" }),
    ))
    .collect();
    let order_by = match search.order {
        TraceOrder::Slowest => "duration_nano",
        TraceOrder::Recent => "timestamp",
    };
    let spec = json!({
        "signal": "traces",
        "selectFields": fields,
        "order": [{ "key": { "name": order_by }, "direction": "desc" }],
        "limit": limit,
        "offset": offset,
    });
    let request = query::builder(
        "raw",
        search.scope.window()?,
        query::with_filter(spec, search_expression(&search)?),
    );
    let result = client::query_range(data_dir, &request).await?;
    let traces: Vec<TraceSummary> = result
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(summary_of).collect())
        .unwrap_or_default();
    let next_offset = (traces.len() as u32 == limit).then(|| offset.saturating_add(limit));
    Ok(TracePage {
        traces,
        next_offset,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    pub trace_id: String,
    pub minutes: Option<u32>,
}

#[derive(Clone, Debug)]
struct RawSpan {
    span_id: String,
    parent_id: Option<String>,
    name: String,
    service: String,
    start_ns: i128,
    duration_ns: i128,
    error: bool,
    status: Option<String>,
    kind: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpan {
    pub span_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub service: String,
    pub depth: usize,
    pub offset_ms: f64,
    pub duration_ms: f64,
    pub error: bool,
    pub status: Option<String>,
    pub kind: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Trace {
    pub trace_id: String,
    pub start: String,
    pub duration_ms: f64,
    pub error_count: usize,
    pub services: Vec<String>,
    /// In the order a waterfall draws them: each span followed by its children, earliest first.
    pub spans: Vec<TraceSpan>,
    pub truncated: bool,
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_span(row: &Value) -> Option<RawSpan> {
    let data = row.get("data")?;
    let timestamp = text(data, "timestamp").or_else(|| text(row, "timestamp"))?;
    Some(RawSpan {
        span_id: text(data, "span_id")?,
        parent_id: text(data, "parent_span_id"),
        name: text(data, "name").unwrap_or_default(),
        service: text(data, "service.name").unwrap_or_default(),
        start_ns: query::parse_timestamp_ns(&timestamp)?,
        duration_ns: data
            .get("duration_nano")
            .and_then(Value::as_i64)
            .map(i128::from)
            .unwrap_or(0),
        error: data
            .get("has_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: text(data, "status_message"),
        kind: text(data, "kind_string"),
    })
}

fn nanos_to_ms(nanos: i128) -> f64 {
    nanos as f64 / 1_000_000.0
}

/// Depth-first, children earliest first. A span whose parent never arrived
/// is drawn as a root rather than dropped.
fn waterfall(mut spans: Vec<RawSpan>) -> Vec<TraceSpan> {
    spans.sort_by(|left, right| {
        left.start_ns
            .cmp(&right.start_ns)
            .then_with(|| left.span_id.cmp(&right.span_id))
    });
    let trace_start = spans.first().map(|span| span.start_ns).unwrap_or(0);
    let known: std::collections::HashSet<&str> =
        spans.iter().map(|span| span.span_id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        match span
            .parent_id
            .as_deref()
            .filter(|parent| known.contains(parent) && *parent != span.span_id)
        {
            Some(parent) => children.entry(parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    let mut ordered = Vec::with_capacity(spans.len());
    let mut visited = vec![false; spans.len()];
    let mut stack: Vec<(usize, usize)> = roots.iter().rev().map(|&index| (index, 0)).collect();
    let mut unreached = 0;
    while let Some((index, depth)) = stack.pop().or_else(|| {
        while unreached < spans.len() && visited.get(unreached).copied().unwrap_or(true) {
            unreached += 1;
        }
        (unreached < spans.len()).then_some((unreached, 0))
    }) {
        let Some(span) = spans.get(index) else {
            continue;
        };
        if visited.get(index).copied().unwrap_or(true) {
            continue;
        }
        if let Some(flag) = visited.get_mut(index) {
            *flag = true;
        }
        ordered.push(TraceSpan {
            span_id: span.span_id.clone(),
            parent_id: span.parent_id.clone(),
            name: span.name.clone(),
            service: span.service.clone(),
            depth,
            offset_ms: nanos_to_ms(span.start_ns - trace_start),
            duration_ms: nanos_to_ms(span.duration_ns),
            error: span.error,
            status: span.status.clone(),
            kind: span.kind.clone(),
        });
        if let Some(kids) = children.get(span.span_id.as_str()) {
            stack.extend(kids.iter().rev().map(|&child| (child, depth + 1)));
        }
    }
    ordered
}

pub async fn trace(data_dir: &Path, request: TraceRequest) -> SignozResult<Trace> {
    let trace_id = request.trace_id.trim().to_string();
    if trace_id.is_empty() || !trace_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SignozError::BadArg("a trace id is hexadecimal".into()));
    }
    let fields: Vec<Value> = [
        "span_id",
        "parent_span_id",
        "name",
        "duration_nano",
        "has_error",
        "status_message",
        "kind_string",
        "timestamp",
    ]
    .iter()
    .map(|name| json!({ "name": name }))
    .chain(std::iter::once(
        json!({ "name": "service.name", "fieldContext": "resource" }),
    ))
    .collect();
    let spec = json!({
        "signal": "traces",
        "selectFields": fields,
        "order": [{ "key": { "name": "timestamp" }, "direction": "asc" }],
        "limit": MAX_SPANS,
    });
    let lookback = request.minutes.unwrap_or(DEFAULT_LOOKBACK_MINUTES);
    let window = query::window(Some(lookback));
    let result = client::query_range(
        data_dir,
        &query::builder(
            "raw",
            window,
            query::with_filter(spec, Some(format!("trace_id = {}", quote(&trace_id)))),
        ),
    )
    .await?;
    let raw: Vec<RawSpan> = result
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(parse_span).collect())
        .unwrap_or_default();
    if raw.is_empty() {
        return Err(SignozError::NotFound(format!(
            "no spans for trace {trace_id} in the last {}; it may be older, or was never traced",
            query::minutes_label(lookback)
        )));
    }
    let truncated = raw.len() as u32 >= MAX_SPANS;
    let start_ns = raw.iter().map(|span| span.start_ns).min().unwrap_or(0);
    let end_ns = raw
        .iter()
        .map(|span| span.start_ns + span.duration_ns)
        .max()
        .unwrap_or(start_ns);
    let start = result
        .pointer("/rows/0/timestamp")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let mut services: Vec<String> = raw
        .iter()
        .map(|span| span.service.clone())
        .filter(|service| !service.is_empty())
        .collect();
    services.sort();
    services.dedup();
    let error_count = raw.iter().filter(|span| span.error).count();
    Ok(Trace {
        trace_id,
        start,
        duration_ms: nanos_to_ms(end_ns - start_ns),
        error_count,
        services,
        spans: waterfall(raw),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, parent: Option<&str>, start_ms: i128) -> RawSpan {
        RawSpan {
            span_id: id.into(),
            parent_id: parent.map(str::to_string),
            name: id.into(),
            service: "svc".into(),
            start_ns: start_ms * 1_000_000,
            duration_ns: 1_000_000,
            error: false,
            status: None,
            kind: None,
        }
    }

    #[test]
    fn orders_each_span_before_its_children_and_indents_them() {
        let ordered = waterfall(vec![
            span("child-late", Some("root"), 5),
            span("grandchild", Some("child-early"), 3),
            span("root", None, 0),
            span("child-early", Some("root"), 2),
        ]);
        let shape: Vec<(&str, usize)> = ordered
            .iter()
            .map(|span| (span.span_id.as_str(), span.depth))
            .collect();
        assert_eq!(
            shape,
            [
                ("root", 0),
                ("child-early", 1),
                ("grandchild", 2),
                ("child-late", 1)
            ]
        );
        assert_eq!(ordered[3].offset_ms, 5.0);
    }

    #[test]
    fn keeps_spans_whose_parent_is_missing_and_survives_self_parents() {
        let ordered = waterfall(vec![
            span("orphan", Some("gone"), 1),
            span("root", None, 0),
            span("loop", Some("loop"), 2),
        ]);
        assert_eq!(ordered.len(), 3);
        assert!(ordered.iter().all(|span| span.depth == 0));
    }

    #[test]
    fn draws_spans_caught_in_a_parent_cycle_instead_of_losing_them() {
        let ordered = waterfall(vec![span("a", Some("b"), 0), span("b", Some("a"), 1)]);
        assert_eq!(ordered.len(), 2);
    }
}
