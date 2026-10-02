use serde::Serialize;
use std::collections::BTreeMap;

const DEFAULT_EVENT_CAPACITY: usize = 512;
const DEFAULT_LATENCY_SAMPLE_CAPACITY: usize = 512;
const DEFAULT_METRIC_SERIES_CAPACITY: usize = 256;
const DEFAULT_METADATA_ENTRIES: usize = 16;
const DEFAULT_STRING_BYTES: usize = 256;

/// Runtime limits for an [`Observability`] instance.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityConfig {
    pub event_capacity: usize,
    pub latency_sample_capacity: usize,
    pub metric_series_capacity: usize,
    pub max_metadata_entries: usize,
    pub max_string_bytes: usize,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            event_capacity: DEFAULT_EVENT_CAPACITY,
            latency_sample_capacity: DEFAULT_LATENCY_SAMPLE_CAPACITY,
            metric_series_capacity: DEFAULT_METRIC_SERIES_CAPACITY,
            max_metadata_entries: DEFAULT_METADATA_ENTRIES,
            max_string_bytes: DEFAULT_STRING_BYTES,
        }
    }
}

impl ObservabilityConfig {
    pub(super) fn normalized(mut self) -> Self {
        self.event_capacity = self.event_capacity.max(1);
        self.latency_sample_capacity = self.latency_sample_capacity.max(1);
        self.metric_series_capacity = self.metric_series_capacity.max(1);
        self.max_metadata_entries = self.max_metadata_entries.max(1);
        self.max_string_bytes = self.max_string_bytes.max(1);
        self
    }
}

/// A trace identifier allocated monotonically for the lifetime of a process.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TraceId(pub(super) u64);

impl TraceId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A span identifier allocated monotonically for the lifetime of a process.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SpanId(pub(super) u64);

impl SpanId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Trace linkage that can be propagated across an IPC or task boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
}

/// Only scalar metadata is accepted by the event ring.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
}

impl From<bool> for ScalarValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for ScalarValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<i32> for ScalarValue {
    fn from(value: i32) -> Self {
        Self::I64(i64::from(value))
    }
}

impl From<u64> for ScalarValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<u32> for ScalarValue {
    fn from(value: u32) -> Self {
        Self::U64(u64::from(value))
    }
}

impl From<usize> for ScalarValue {
    fn from(value: usize) -> Self {
        match u64::try_from(value) {
            Ok(value) => Self::U64(value),
            Err(_) => Self::U64(u64::MAX),
        }
    }
}

impl From<f64> for ScalarValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}

impl From<String> for ScalarValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for ScalarValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

pub type Metadata = BTreeMap<String, ScalarValue>;

/// Terminal state of a timed operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanOutcome {
    Success,
    Error,
    Cancelled,
    Dropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Event,
    SpanStarted,
    SpanEnded,
    SlowOperation,
}

/// One structured record in the bounded event history.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub kind: EventKind,
    pub name: String,
    pub outcome: Option<SpanOutcome>,
    pub duration_us: Option<u64>,
    pub metadata: Metadata,
}

/// Counts for the fixed latency buckets. The upper bounds are exclusive.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyBuckets {
    pub under_4_ms: u64,
    pub from_4_to_8_ms: u64,
    pub from_8_to_16_ms: u64,
    pub from_16_to_33_ms: u64,
    pub from_33_to_100_ms: u64,
    pub at_least_100_ms: u64,
}

impl LatencyBuckets {
    pub(super) fn observe(&mut self, latency_us: u64) {
        match latency_us {
            0..=3_999 => self.under_4_ms = self.under_4_ms.saturating_add(1),
            4_000..=7_999 => self.from_4_to_8_ms = self.from_4_to_8_ms.saturating_add(1),
            8_000..=15_999 => self.from_8_to_16_ms = self.from_8_to_16_ms.saturating_add(1),
            16_000..=32_999 => self.from_16_to_33_ms = self.from_16_to_33_ms.saturating_add(1),
            33_000..=99_999 => self.from_33_to_100_ms = self.from_33_to_100_ms.saturating_add(1),
            _ => self.at_least_100_ms = self.at_least_100_ms.saturating_add(1),
        }
    }
}

/// A bounded-window latency summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyHistogramSnapshot {
    pub sample_count: u64,
    pub total_observations: u64,
    pub evicted_samples: u64,
    pub buckets: LatencyBuckets,
    pub p50_us: Option<u64>,
    pub p95_us: Option<u64>,
    pub p99_us: Option<u64>,
    pub max_us: Option<u64>,
}

/// Serializable, point-in-time process observability state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySnapshot {
    pub process_uptime_us: u64,
    pub window_duration_us: u64,
    pub events: Vec<TraceEvent>,
    pub dropped_events: u64,
    pub dropped_metric_series: u64,
    pub counters: BTreeMap<String, u64>,
    pub gauges: BTreeMap<String, f64>,
    pub latency_histograms: BTreeMap<String, LatencyHistogramSnapshot>,
}
