use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::model::{
    EventKind, LatencyBuckets, LatencyHistogramSnapshot, Metadata, ObservabilityConfig,
    ObservabilitySnapshot, ScalarValue, SpanContext, TraceEvent, TraceId,
};
use super::{duration_us, next_monotonic_id};

static GLOBAL_OBSERVABILITY: OnceLock<Observability> = OnceLock::new();

#[derive(Debug)]
struct LatencySeries {
    samples_us: VecDeque<u64>,
    total_observations: u64,
    evicted_samples: u64,
}

impl LatencySeries {
    fn new() -> Self {
        Self {
            samples_us: VecDeque::new(),
            total_observations: 0,
            evicted_samples: 0,
        }
    }

    fn observe(&mut self, latency_us: u64, capacity: usize) {
        self.total_observations = self.total_observations.saturating_add(1);
        if self.samples_us.len() == capacity {
            self.samples_us.pop_front();
            self.evicted_samples = self.evicted_samples.saturating_add(1);
        }
        self.samples_us.push_back(latency_us);
    }

    fn snapshot(&self) -> LatencyHistogramSnapshot {
        let mut sorted = self.samples_us.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();

        let mut buckets = LatencyBuckets::default();
        for latency_us in &sorted {
            buckets.observe(*latency_us);
        }

        LatencyHistogramSnapshot {
            sample_count: usize_to_u64(sorted.len()),
            total_observations: self.total_observations,
            evicted_samples: self.evicted_samples,
            buckets,
            p50_us: percentile(&sorted, 50),
            p95_us: percentile(&sorted, 95),
            p99_us: percentile(&sorted, 99),
            max_us: sorted.last().copied(),
        }
    }
}

#[derive(Debug)]
struct State {
    reset_at_us: u64,
    events: VecDeque<TraceEvent>,
    dropped_events: u64,
    dropped_metric_series: u64,
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    latency_histograms: BTreeMap<String, LatencySeries>,
}

impl State {
    fn new() -> Self {
        Self {
            reset_at_us: 0,
            events: VecDeque::new(),
            dropped_events: 0,
            dropped_metric_series: 0,
            counters: BTreeMap::new(),
            gauges: BTreeMap::new(),
            latency_histograms: BTreeMap::new(),
        }
    }

    fn clear(&mut self, now_us: u64) {
        self.reset_at_us = now_us;
        self.events.clear();
        self.dropped_events = 0;
        self.dropped_metric_series = 0;
        self.counters.clear();
        self.gauges.clear();
        self.latency_histograms.clear();
    }
}

#[derive(Debug)]
pub(super) struct Inner {
    epoch: Instant,
    next_trace_id: AtomicU64,
    pub(super) next_span_id: AtomicU64,
    next_sequence: AtomicU64,
    config: ObservabilityConfig,
    state: Mutex<State>,
}

/// Thread-safe, cloneable handle to the process-local observability state.
#[derive(Clone, Debug)]
pub struct Observability {
    pub(super) inner: Arc<Inner>,
}

impl Default for Observability {
    fn default() -> Self {
        Self::new(ObservabilityConfig::default())
    }
}

/// Returns the lazily initialized process-global observability store.
///
/// The store's epoch begins on first use and remains stable until process
/// exit. Calling [`Observability::reset`] clears samples, not this epoch or the
/// monotonic trace, span, and event ID allocators.
pub fn global_observability() -> &'static Observability {
    GLOBAL_OBSERVABILITY.get_or_init(Observability::default)
}

impl Observability {
    pub fn new(config: ObservabilityConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                epoch: Instant::now(),
                next_trace_id: AtomicU64::new(1),
                next_span_id: AtomicU64::new(1),
                next_sequence: AtomicU64::new(1),
                config: config.normalized(),
                state: Mutex::new(State::new()),
            }),
        }
    }

    /// Allocates a trace ID. IDs are never reset with the sampled state.
    pub fn next_trace_id(&self) -> TraceId {
        TraceId(next_monotonic_id(&self.inner.next_trace_id))
    }

    /// Records an instantaneous event and returns its monotonic sequence.
    pub fn record_event(
        &self,
        name: impl Into<String>,
        context: Option<SpanContext>,
        metadata: Metadata,
    ) -> u64 {
        let sequence = self.next_sequence();
        self.push_event(TraceEvent {
            sequence,
            timestamp_us: self.now_us(),
            trace_id: context.map(|value| value.trace_id),
            span_id: context.map(|value| value.span_id),
            parent_span_id: None,
            kind: EventKind::Event,
            name: self.sanitize_text(name.into()),
            outcome: None,
            duration_us: None,
            metadata: self.sanitize_metadata(metadata),
        });
        sequence
    }

    /// Increments a named counter, saturating at `u64::MAX`.
    ///
    /// Returns `None` when the configured metric-series limit rejects a new
    /// name. Existing names continue to be updated at the limit.
    pub fn increment_counter(&self, name: impl Into<String>, delta: u64) -> Option<u64> {
        let name = self.sanitize_text(name.into());
        let mut state = self.lock_state();
        if !state.counters.contains_key(&name)
            && self.metric_series_count(&state) >= self.inner.config.metric_series_capacity
        {
            state.dropped_metric_series = state.dropped_metric_series.saturating_add(1);
            return None;
        }

        let counter = state.counters.entry(name).or_insert(0);
        *counter = counter.saturating_add(delta);
        Some(*counter)
    }

    /// Sets a finite gauge value.
    ///
    /// Non-finite values and new names above the metric-series limit are
    /// rejected because they cannot be represented reliably in JSON.
    pub fn set_gauge(&self, name: impl Into<String>, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }

        let name = self.sanitize_text(name.into());
        let mut state = self.lock_state();
        if !state.gauges.contains_key(&name)
            && self.metric_series_count(&state) >= self.inner.config.metric_series_capacity
        {
            state.dropped_metric_series = state.dropped_metric_series.saturating_add(1);
            return false;
        }
        state.gauges.insert(name, value);
        true
    }

    /// Adds a duration to a named bounded-window latency histogram.
    pub fn observe_latency(&self, name: impl Into<String>, latency: Duration) -> bool {
        self.observe_latency_us(name.into(), duration_us(latency))
    }

    /// Returns a consistent snapshot while retaining the current window.
    pub fn snapshot(&self) -> ObservabilitySnapshot {
        let now_us = self.now_us();
        let state = self.lock_state();
        self.snapshot_locked(&state, now_us)
    }

    /// Atomically returns the current snapshot and starts a fresh sample window.
    /// Trace, span, and event sequence IDs intentionally remain monotonic.
    pub fn reset(&self) -> ObservabilitySnapshot {
        let now_us = self.now_us();
        let mut state = self.lock_state();
        let snapshot = self.snapshot_locked(&state, now_us);
        state.clear(now_us);
        snapshot
    }

    pub(super) fn observe_latency_us(&self, name: String, latency_us: u64) -> bool {
        let name = self.sanitize_text(name);
        let mut state = self.lock_state();
        if !state.latency_histograms.contains_key(&name)
            && self.metric_series_count(&state) >= self.inner.config.metric_series_capacity
        {
            state.dropped_metric_series = state.dropped_metric_series.saturating_add(1);
            return false;
        }

        let histogram = state
            .latency_histograms
            .entry(name)
            .or_insert_with(LatencySeries::new);
        histogram.observe(latency_us, self.inner.config.latency_sample_capacity);
        true
    }

    pub(super) fn push_event(&self, event: TraceEvent) {
        let mut state = self.lock_state();
        if state.events.len() == self.inner.config.event_capacity {
            state.events.pop_front();
            state.dropped_events = state.dropped_events.saturating_add(1);
        }
        state.events.push_back(event);
    }

    fn snapshot_locked(&self, state: &State, now_us: u64) -> ObservabilitySnapshot {
        let latency_histograms = state
            .latency_histograms
            .iter()
            .map(|(name, histogram)| (name.clone(), histogram.snapshot()))
            .collect();

        ObservabilitySnapshot {
            process_uptime_us: now_us,
            window_duration_us: now_us.saturating_sub(state.reset_at_us),
            events: state.events.iter().cloned().collect(),
            dropped_events: state.dropped_events,
            dropped_metric_series: state.dropped_metric_series,
            counters: state.counters.clone(),
            gauges: state.gauges.clone(),
            latency_histograms,
        }
    }

    fn metric_series_count(&self, state: &State) -> usize {
        state
            .counters
            .len()
            .saturating_add(state.gauges.len())
            .saturating_add(state.latency_histograms.len())
    }

    pub(super) fn next_sequence(&self) -> u64 {
        next_monotonic_id(&self.inner.next_sequence)
    }

    pub(super) fn now_us(&self) -> u64 {
        duration_us(self.inner.epoch.elapsed())
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        match self.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(super) fn sanitize_metadata(&self, metadata: Metadata) -> Metadata {
        metadata
            .into_iter()
            .take(self.inner.config.max_metadata_entries)
            .map(|(key, value)| {
                let key = self.sanitize_text(key);
                let value = match value {
                    ScalarValue::String(value) => ScalarValue::String(self.sanitize_text(value)),
                    ScalarValue::F64(value) if !value.is_finite() => {
                        ScalarValue::String("non-finite".to_owned())
                    }
                    value => value,
                };
                (key, value)
            })
            .collect()
    }

    pub(super) fn sanitize_text(&self, mut value: String) -> String {
        let max_bytes = self.inner.config.max_string_bytes;
        if value.len() <= max_bytes {
            return value;
        }

        let mut boundary = max_bytes;
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
        value
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn percentile(sorted: &[u64], percentile: u128) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }

    let length = sorted.len() as u128;
    let rank = length
        .saturating_mul(percentile)
        .saturating_add(99)
        .saturating_div(100)
        .saturating_sub(1);
    match usize::try_from(rank) {
        Ok(index) => sorted.get(index).copied(),
        Err(_) => sorted.last().copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(event_capacity: usize, latency_sample_capacity: usize) -> ObservabilityConfig {
        ObservabilityConfig {
            event_capacity,
            latency_sample_capacity,
            metric_series_capacity: 16,
            max_metadata_entries: 4,
            max_string_bytes: 16,
        }
    }

    #[test]
    fn event_history_is_bounded_and_ordered() {
        let observer = Observability::new(config(2, 8));
        observer.record_event("one", None, Metadata::new());
        observer.record_event("two", None, Metadata::new());
        observer.record_event("three", None, Metadata::new());

        let snapshot = observer.snapshot();
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.dropped_events, 1);
        assert_eq!(snapshot.events[0].name, "two");
        assert_eq!(snapshot.events[1].name, "three");
        assert!(snapshot.events[0].sequence < snapshot.events[1].sequence);
    }

    #[test]
    fn histogram_uses_fixed_buckets_and_nearest_rank_percentiles() {
        let observer = Observability::new(config(8, 16));
        for latency_us in [
            0, 3_999, 4_000, 7_999, 8_000, 15_999, 16_000, 32_999, 33_000, 99_999, 100_000,
        ] {
            assert!(observer.observe_latency_us("input".to_owned(), latency_us));
        }

        let snapshot = observer.snapshot();
        let histogram = snapshot.latency_histograms.get("input").unwrap();
        assert_eq!(histogram.sample_count, 11);
        assert_eq!(histogram.buckets.under_4_ms, 2);
        assert_eq!(histogram.buckets.from_4_to_8_ms, 2);
        assert_eq!(histogram.buckets.from_8_to_16_ms, 2);
        assert_eq!(histogram.buckets.from_16_to_33_ms, 2);
        assert_eq!(histogram.buckets.from_33_to_100_ms, 2);
        assert_eq!(histogram.buckets.at_least_100_ms, 1);
        assert_eq!(histogram.p50_us, Some(15_999));
        assert_eq!(histogram.p95_us, Some(100_000));
        assert_eq!(histogram.p99_us, Some(100_000));
        assert_eq!(histogram.max_us, Some(100_000));
    }

    #[test]
    fn histogram_discards_oldest_samples_at_capacity() {
        let observer = Observability::new(config(8, 3));
        for latency_us in [1_000, 2_000, 3_000, 100_000] {
            observer.observe_latency_us("render".to_owned(), latency_us);
        }

        let snapshot = observer.snapshot();
        let histogram = snapshot.latency_histograms.get("render").unwrap();
        assert_eq!(histogram.sample_count, 3);
        assert_eq!(histogram.total_observations, 4);
        assert_eq!(histogram.evicted_samples, 1);
        assert_eq!(histogram.buckets.under_4_ms, 2);
        assert_eq!(histogram.buckets.at_least_100_ms, 1);
        assert_eq!(histogram.max_us, Some(100_000));
    }

    #[test]
    fn counters_gauges_and_reset_are_consistent() {
        let observer = Observability::default();
        assert_eq!(observer.increment_counter("writes", 2), Some(2));
        assert_eq!(observer.increment_counter("writes", 3), Some(5));
        assert!(observer.set_gauge("queue", 4.5));
        assert!(!observer.set_gauge("invalid", f64::NAN));
        let before_id = observer.next_trace_id();

        let previous = observer.reset();
        assert_eq!(previous.counters.get("writes"), Some(&5));
        assert_eq!(previous.gauges.get("queue"), Some(&4.5));

        let current = observer.snapshot();
        assert!(current.counters.is_empty());
        assert!(current.gauges.is_empty());
        assert!(current.events.is_empty());
        assert!(observer.next_trace_id() > before_id);
    }

    #[test]
    fn metadata_and_names_are_safely_bounded() {
        let observer = Observability::new(config(8, 8));
        let mut metadata = Metadata::new();
        for index in 0..8 {
            metadata.insert(
                format!("metadata-key-{index}"),
                ScalarValue::String("0123456789abcdefghijklmnop".to_owned()),
            );
        }
        observer.record_event("event-name-that-is-too-long", None, metadata);

        let snapshot = observer.snapshot();
        let event = &snapshot.events[0];
        assert!(event.name.len() <= 16);
        assert_eq!(event.metadata.len(), 4);
        assert!(event.metadata.keys().all(|key| key.len() <= 16));
        assert!(event.metadata.values().all(|value| match value {
            ScalarValue::String(value) => value.len() <= 16,
            _ => true,
        }));
    }
}
