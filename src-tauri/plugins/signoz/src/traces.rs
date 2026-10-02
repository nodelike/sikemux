use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::Scope;
use crate::query::{self, quote, round_ms, View};

const MAX_SPANS: u32 = 5_000;
const DEFAULT_LOOKBACK_MINUTES: u32 = 24 * 60;
const DEFAULT_TRACE_LIMIT: u32 = 20;
const DEFAULT_OUTLINE_SPANS: u32 = 40;
const MAX_OUTLINE_SPANS: u32 = 500;
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
    pub span_id: Option<String>,
    pub max_spans: Option<u32>,
    #[serde(default)]
    pub view: View,
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

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutlineSpan {
    pub span_id: String,
    pub name: String,
    /// Left out when it is the same as the parent span's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub depth: usize,
    pub offset_ms: f64,
    pub duration_ms: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Spans below this one that the outline leaves out.
    #[serde(skip_serializing_if = "is_zero")]
    pub hidden: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceOutline {
    pub trace_id: String,
    pub start: String,
    pub duration_ms: f64,
    pub span_count: usize,
    pub error_count: usize,
    pub services: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub spans: Vec<OutlineSpan>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum TraceReply {
    Waterfall(Trace),
    Outline(TraceOutline),
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

/// The spans worth reading first, in waterfall order: the top of the tree, every
/// failing span, and the spans that spent the most time on their own work, each
/// with the spans above it so the path from the root stays readable.
fn outline(trace: Trace, from: Option<&str>, most: usize) -> SignozResult<TraceOutline> {
    let top = match from {
        Some(id) => trace
            .spans
            .iter()
            .position(|span| span.span_id == id)
            .ok_or_else(|| {
                SignozError::NotFound(format!("trace {} has no span {id}", trace.trace_id))
            })?,
        None => 0,
    };
    let base = trace.spans.get(top).map_or(0, |span| span.depth);
    let subtree: Vec<&TraceSpan> = trace
        .spans
        .iter()
        .skip(top)
        .enumerate()
        .take_while(|(index, span)| from.is_none() || *index == 0 || span.depth > base)
        .map(|(_, span)| span)
        .collect();
    let mut parent: Vec<Option<usize>> = Vec::with_capacity(subtree.len());
    let mut path: Vec<usize> = Vec::new();
    for (index, span) in subtree.iter().enumerate() {
        path.truncate(span.depth.saturating_sub(base));
        parent.push(path.last().copied());
        path.push(index);
    }
    let parent_of = |index: usize| parent.get(index).copied().flatten();
    let mut children_time = vec![0.0; subtree.len()];
    for (index, span) in subtree.iter().enumerate() {
        if let Some(time) = parent_of(index).and_then(|up| children_time.get_mut(up)) {
            *time += span.duration_ms;
        }
    }
    let own_time: Vec<f64> = subtree
        .iter()
        .zip(&children_time)
        .map(|(span, children)| (span.duration_ms - children).max(0.0))
        .collect();
    let depth_of = |index: usize| subtree.get(index).map_or(0, |span| span.depth);
    let own_of = |index: usize| own_time.get(index).copied().unwrap_or(0.0);
    let roots = (0..subtree.len()).filter(|&index| parent_of(index).is_none());
    let mut failing: Vec<usize> = (0..subtree.len())
        .filter(|&index| subtree.get(index).is_some_and(|span| span.error))
        .collect();
    failing.sort_by_key(|&index| std::cmp::Reverse(depth_of(index)));
    let mut busiest: Vec<usize> = (0..subtree.len()).collect();
    busiest.sort_by(|&left, &right| own_of(right).total_cmp(&own_of(left)));
    let mut shown = vec![false; subtree.len()];
    let is_shown = |shown: &[bool], index: usize| shown.get(index).copied().unwrap_or(false);
    let mut count = 0;
    for candidate in roots.chain(failing).chain(busiest) {
        let mut chain = Vec::new();
        let mut at = Some(candidate);
        while let Some(index) = at.filter(|&index| !is_shown(&shown, index)) {
            chain.push(index);
            at = parent_of(index);
        }
        if count + chain.len() > most {
            continue;
        }
        count += chain.len();
        for index in chain {
            if let Some(flag) = shown.get_mut(index) {
                *flag = true;
            }
        }
        if count == most {
            break;
        }
    }
    let mut hidden = vec![0; subtree.len()];
    for index in (0..subtree.len()).filter(|&index| !is_shown(&shown, index)) {
        let mut at = parent_of(index);
        while let Some(up) = at.filter(|&up| !is_shown(&shown, up)) {
            at = parent_of(up);
        }
        if let Some(slot) = at.and_then(|up| hidden.get_mut(up)) {
            *slot += 1;
        }
    }
    let spans = subtree
        .iter()
        .enumerate()
        .filter(|&(index, _)| is_shown(&shown, index))
        .map(|(index, span)| {
            let parent_service = parent_of(index)
                .and_then(|up| subtree.get(up))
                .map(|up| up.service.as_str());
            OutlineSpan {
                span_id: span.span_id.clone(),
                name: span.name.clone(),
                service: (parent_service != Some(span.service.as_str()))
                    .then(|| span.service.clone()),
                depth: span.depth,
                offset_ms: round_ms(span.offset_ms),
                duration_ms: round_ms(span.duration_ms),
                error: span.error,
                status: span.status.clone(),
                kind: span
                    .kind
                    .clone()
                    .filter(|kind| !matches!(kind.as_str(), "Internal" | "Unspecified")),
                hidden: hidden.get(index).copied().unwrap_or(0),
            }
        })
        .collect();
    Ok(TraceOutline {
        span_count: trace.spans.len(),
        trace_id: trace.trace_id,
        start: trace.start,
        duration_ms: round_ms(trace.duration_ms),
        error_count: trace.error_count,
        services: trace.services,
        truncated: trace.truncated,
        spans,
    })
}

pub async fn trace(data_dir: &Path, request: TraceRequest) -> SignozResult<TraceReply> {
    let view = request.view;
    let from = request
        .span_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let most = request
        .max_spans
        .unwrap_or(DEFAULT_OUTLINE_SPANS)
        .clamp(1, MAX_OUTLINE_SPANS) as usize;
    let trace = whole_trace(data_dir, request).await?;
    Ok(match view {
        View::Pane => TraceReply::Waterfall(trace),
        View::Agent => TraceReply::Outline(outline(trace, from.as_deref(), most)?),
    })
}

async fn whole_trace(data_dir: &Path, request: TraceRequest) -> SignozResult<Trace> {
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

    fn traced(raw: Vec<RawSpan>) -> Trace {
        Trace {
            trace_id: "t".into(),
            start: String::new(),
            duration_ms: 0.0,
            error_count: raw.iter().filter(|span| span.error).count(),
            services: Vec::new(),
            spans: waterfall(raw),
            truncated: false,
        }
    }

    fn wide() -> Vec<RawSpan> {
        let mut raw = vec![RawSpan {
            duration_ns: 900_000_000,
            ..span("root", None, 0)
        }];
        for index in 0..50 {
            let child = format!("child-{index}");
            raw.push(RawSpan {
                service: "db".into(),
                ..span(&child, Some("root"), index + 1)
            });
            raw.push(span(&format!("leaf-{index}"), Some(&child), index + 1));
        }
        raw.push(RawSpan {
            error: true,
            status: Some("connection reset".into()),
            ..span("broken", Some("child-30"), 31)
        });
        raw.push(RawSpan {
            duration_ns: 400_000_000,
            ..span("slow", Some("child-7"), 8)
        });
        raw
    }

    #[test]
    fn a_small_trace_is_shown_whole_and_says_each_service_once() {
        let outline = outline(
            traced(vec![
                span("root", None, 0),
                RawSpan {
                    service: "db".into(),
                    kind: Some("Client".into()),
                    ..span("query", Some("root"), 1)
                },
                RawSpan {
                    kind: Some("Internal".into()),
                    ..span("render", Some("root"), 2)
                },
            ]),
            None,
            40,
        )
        .unwrap();
        let services: Vec<Option<&str>> = outline
            .spans
            .iter()
            .map(|span| span.service.as_deref())
            .collect();
        assert_eq!(services, [Some("svc"), Some("db"), None]);
        assert_eq!(outline.spans[1].kind.as_deref(), Some("Client"));
        assert_eq!(outline.spans[2].kind, None);
        assert!(outline.spans.iter().all(|span| span.hidden == 0));
    }

    #[test]
    fn a_big_trace_keeps_its_failures_and_slowest_work_and_counts_the_rest() {
        let outline = outline(traced(wide()), None, 5).unwrap();
        let shown: Vec<&str> = outline
            .spans
            .iter()
            .map(|span| span.span_id.as_str())
            .collect();
        assert_eq!(outline.span_count, 103);
        assert_eq!(shown, ["root", "child-7", "slow", "child-30", "broken"]);
        assert_eq!(outline.spans[4].status.as_deref(), Some("connection reset"));
        let hidden: usize = outline.spans.iter().map(|span| span.hidden).sum();
        assert_eq!(hidden + shown.len(), outline.span_count);
        assert_eq!(outline.spans[1].hidden, 1);
    }

    #[test]
    fn opens_one_span_and_everything_under_it() {
        let outline = outline(traced(wide()), Some("child-30"), 40).unwrap();
        let shown: Vec<(&str, usize)> = outline
            .spans
            .iter()
            .map(|span| (span.span_id.as_str(), span.depth))
            .collect();
        assert_eq!(shown, [("child-30", 1), ("broken", 2), ("leaf-30", 2)]);
        assert_eq!(outline.spans[0].service.as_deref(), Some("db"));
        assert!(super::outline(traced(wide()), Some("nope"), 40).is_err());
    }

    #[test]
    fn draws_spans_caught_in_a_parent_cycle_instead_of_losing_them() {
        let ordered = waterfall(vec![span("a", Some("b"), 0), span("b", Some("a"), 1)]);
        assert_eq!(ordered.len(), 2);
    }
}
