//! What the SigNoz plugin's views hand a pane: the services with their health, one service's
//! page, logs, traces and one trace, and dashboards with what each panel draws.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unix milliseconds and a value.
pub type Point = (u64, f64);

/// One service in one environment, as SigNoz counts its entry spans.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    pub service: String,
    pub environment: Option<String>,
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p99_ms: f64,
}

/// One service across the environments shown: the counts added up and the worst p99.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRow {
    pub service: String,
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p99_ms: f64,
}

/// How the services are ordered.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ServiceSort {
    #[default]
    Errors,
    Calls,
    P99,
    Name,
}

/// The services in the environment picked, sorted, beside every environment there is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Services {
    pub rows: Vec<ServiceRow>,
    pub environments: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub per_minute: f64,
    pub p99_ms: f64,
    pub p50_ms: f64,
    pub requests: Vec<Point>,
    pub failures: Vec<Point>,
    pub p99: Vec<Point>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub name: String,
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p99_ms: f64,
    pub p50_ms: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorGroup {
    /// The message with its numbers and ids blanked, so one failure is one row.
    pub pattern: String,
    /// The most frequent message behind the pattern, as written.
    pub sample: String,
    pub count: u64,
    pub first_seen: u64,
    pub last_seen: u64,
}

/// One service's page: its health over time, its operations and its errors.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicePage {
    pub overview: Overview,
    pub operations: Vec<Operation>,
    pub errors: Vec<ErrorGroup>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    pub next_offset: Option<u32>,
}

/// What a live tail sends each time it reads: the lines new since the last, oldest first,
/// or why it could not read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TailTick {
    pub lines: Vec<LogLine>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBucket {
    /// Unix milliseconds at the start of the bucket.
    pub start: u64,
    /// Lines by severity.
    pub counts: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TracePage {
    pub traces: Vec<TraceSummary>,
    pub next_offset: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub panels: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub options: Vec<String>,
    pub selected: String,
}

/// Where a panel sits on SigNoz's grid of twelve columns.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub unit: String,
    pub layout: Layout,
    /// Whether Sikemux can draw it. Anything else links out to SigNoz.
    pub drawable: bool,
    /// The panel's saved query, handed back unchanged to run it.
    pub query: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub id: String,
    pub title: String,
    pub variables: Vec<Variable>,
    pub panels: Vec<Panel>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub label: String,
    pub points: Vec<Point>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub aggregation: bool,
}

/// What one panel draws: lines over time, a table, or one number.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "shape", rename_all = "camelCase")]
pub enum PanelData {
    Series {
        series: Vec<Series>,
    },
    Table {
        columns: Vec<Column>,
        rows: Vec<Vec<Value>>,
    },
    Value {
        value: Option<f64>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldKey {
    pub name: String,
    pub context: String,
    pub data_type: String,
}
