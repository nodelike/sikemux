//! What the SigNoz plugin's views hand a pane: the services with their health, one service's
//! page, logs, traces and one trace, and dashboards with what each panel draws.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// How the plugin signs in to SigNoz: a session from an email and password, or an API key.
#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum AuthMode {
    #[default]
    Session,
    ApiKey,
}

/// Which SigNoz the plugin is signed in to, and as whom.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Where {
    pub url: String,
    pub auth: AuthMode,
    pub email: String,
    pub key_from_environment: bool,
    pub version: Option<String>,
}

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

/// Who reads the answer. Agents get a compact shape by default; the SigNoz pane
/// asks for every field it draws.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum View {
    #[default]
    Agent,
    Pane,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    Exists,
    NotExists,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Filter {
    pub key: String,
    pub op: FilterOp,
    #[serde(default, deserialize_with = "plain_value")]
    pub value: String,
}

/// A value as written in a filter or a dashboard variable. Numbers and booleans
/// are read as their text, since SigNoz compares `status = '503'` and `status = 503` alike.
fn text_of(value: Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(format!("expected text or a number, not {other}")),
    }
}

pub fn plain_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    text_of(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

pub fn plain_values<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    BTreeMap::<String, Value>::deserialize(deserializer)?
        .into_iter()
        .map(|(name, value)| {
            text_of(value)
                .map(|text| (name.clone(), text))
                .map_err(|problem| serde::de::Error::custom(format!("`{name}`: {problem}")))
        })
        .collect()
}

/// What every query narrows by, whichever signal it reads.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    /// A filter in SigNoz's own query syntax, for anything the others cannot say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// A fixed range in Unix milliseconds. Without one, the last `minutes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
}

/// The services, as `servicesView` reads them.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServicesRequest {
    #[serde(flatten)]
    pub scope: Scope,
    #[serde(default)]
    pub sort: ServiceSort,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogSearch {
    #[serde(flatten)]
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub severities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// The pane never says who reads the answer: the view it asks decides that.
    #[serde(default, skip_serializing)]
    pub view: View,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VolumeQuery {
    #[serde(flatten)]
    pub search: LogSearch,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<u32>,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TraceOrder {
    #[default]
    Slowest,
    Recent,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSearch {
    #[serde(flatten)]
    pub scope: Scope,
    #[serde(default)]
    pub errors_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_duration_ms: Option<f64>,
    #[serde(default)]
    pub order: TraceOrder,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    pub trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_spans: Option<u32>,
    /// The pane never says who reads the answer: the view it asks decides that.
    #[serde(default, skip_serializing)]
    pub view: View,
}

#[derive(Serialize, Deserialize)]
pub struct DashboardViewRequest {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelRequest {
    pub kind: String,
    pub query: Value,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(flatten)]
    pub scope: Scope,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Signal {
    #[default]
    Logs,
    Traces,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Traces => "traces",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyQuery {
    #[serde(default)]
    pub signal: Signal,
    #[serde(default)]
    pub search: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueQuery {
    #[serde(default)]
    pub signal: Signal,
    pub name: String,
    #[serde(default)]
    pub search: String,
}
