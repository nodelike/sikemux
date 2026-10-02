use std::time::{Duration, Instant};

use super::model::{
    EventKind, Metadata, ScalarValue, SpanContext, SpanId, SpanOutcome, TraceEvent, TraceId,
};
use super::store::Observability;
use super::{duration_us, next_monotonic_id};

impl Observability {
    /// Starts a root span, or a child span when `parent` is supplied.
    #[must_use = "dropping the guard records a dropped span"]
    pub fn begin_span(
        &self,
        name: impl Into<String>,
        parent: Option<SpanContext>,
        metadata: Metadata,
    ) -> SpanGuard {
        let trace_id = parent
            .map(|context| context.trace_id)
            .unwrap_or_else(|| self.next_trace_id());
        let parent_span_id = parent.map(|context| context.span_id);
        self.begin_span_in_trace(trace_id, parent_span_id, name, metadata)
    }

    /// Starts a span in an existing trace, optionally beneath another span.
    #[must_use = "dropping the guard records a dropped span"]
    pub fn begin_span_in_trace(
        &self,
        trace_id: TraceId,
        parent_span_id: Option<SpanId>,
        name: impl Into<String>,
        metadata: Metadata,
    ) -> SpanGuard {
        let span_id = SpanId(next_monotonic_id(&self.inner.next_span_id));
        let name = self.sanitize_text(name.into());
        let started_at = Instant::now();
        let context = SpanContext { trace_id, span_id };

        self.push_event(TraceEvent {
            sequence: self.next_sequence(),
            timestamp_us: self.now_us(),
            trace_id: Some(trace_id),
            span_id: Some(span_id),
            parent_span_id,
            kind: EventKind::SpanStarted,
            name: name.clone(),
            outcome: None,
            duration_us: None,
            metadata: self.sanitize_metadata(metadata),
        });

        SpanGuard {
            observer: self.clone(),
            context,
            parent_span_id,
            name,
            started_at,
            completed: false,
        }
    }

    /// Creates a timer which records every duration in a latency histogram and
    /// emits a structured event only when it meets the supplied threshold.
    #[must_use = "dropping the guard records a dropped operation outcome"]
    pub fn slow_operation(
        &self,
        name: impl Into<String>,
        threshold: Duration,
        parent: Option<SpanContext>,
        metadata: Metadata,
    ) -> SlowOperationGuard {
        let trace_id = parent
            .map(|context| context.trace_id)
            .unwrap_or_else(|| self.next_trace_id());
        let parent_span_id = parent.map(|context| context.span_id);
        let span_id = SpanId(next_monotonic_id(&self.inner.next_span_id));

        SlowOperationGuard {
            observer: self.clone(),
            context: SpanContext { trace_id, span_id },
            parent_span_id,
            name: self.sanitize_text(name.into()),
            threshold_us: duration_us(threshold),
            metadata: self.sanitize_metadata(metadata),
            started_at: Instant::now(),
            completed: false,
        }
    }

    fn finish_span(
        &self,
        context: SpanContext,
        parent_span_id: Option<SpanId>,
        name: String,
        started_at: Instant,
        outcome: SpanOutcome,
        metadata: Metadata,
    ) -> u64 {
        let elapsed_us = duration_us(started_at.elapsed());
        self.push_event(TraceEvent {
            sequence: self.next_sequence(),
            timestamp_us: self.now_us(),
            trace_id: Some(context.trace_id),
            span_id: Some(context.span_id),
            parent_span_id,
            kind: EventKind::SpanEnded,
            name,
            outcome: Some(outcome),
            duration_us: Some(elapsed_us),
            metadata: self.sanitize_metadata(metadata),
        });
        elapsed_us
    }

    fn finish_slow_operation(&self, guard: &SlowOperationGuard, outcome: SpanOutcome) -> u64 {
        let elapsed_us = duration_us(guard.started_at.elapsed());
        self.observe_latency_us(guard.name.clone(), elapsed_us);

        if elapsed_us >= guard.threshold_us {
            let mut metadata = guard.metadata.clone();
            metadata.insert(
                "threshold_us".to_owned(),
                ScalarValue::U64(guard.threshold_us),
            );
            self.push_event(TraceEvent {
                sequence: self.next_sequence(),
                timestamp_us: self.now_us(),
                trace_id: Some(guard.context.trace_id),
                span_id: Some(guard.context.span_id),
                parent_span_id: guard.parent_span_id,
                kind: EventKind::SlowOperation,
                name: guard.name.clone(),
                outcome: Some(outcome),
                duration_us: Some(elapsed_us),
                metadata: self.sanitize_metadata(metadata),
            });
        }

        elapsed_us
    }
}

/// RAII timer for a structured span.
#[must_use = "dropping the guard records a dropped span"]
pub struct SpanGuard {
    observer: Observability,
    context: SpanContext,
    parent_span_id: Option<SpanId>,
    name: String,
    started_at: Instant,
    completed: bool,
}

impl SpanGuard {
    pub fn context(&self) -> SpanContext {
        self.context
    }

    /// Ends the span with an explicit outcome and no end metadata.
    pub fn finish(self, outcome: SpanOutcome) -> u64 {
        self.finish_with_metadata(outcome, Metadata::new())
    }

    /// Ends the span with an explicit outcome and scalar end metadata.
    pub fn finish_with_metadata(mut self, outcome: SpanOutcome, metadata: Metadata) -> u64 {
        self.completed = true;
        self.observer.finish_span(
            self.context,
            self.parent_span_id,
            self.name.clone(),
            self.started_at,
            outcome,
            metadata,
        )
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.completed = true;
            self.observer.finish_span(
                self.context,
                self.parent_span_id,
                self.name.clone(),
                self.started_at,
                SpanOutcome::Dropped,
                Metadata::new(),
            );
        }
    }
}

/// RAII timer which reports only operations at or above a slow threshold.
/// Every elapsed duration is still retained in the named latency histogram.
#[must_use = "dropping the guard records a dropped operation outcome"]
pub struct SlowOperationGuard {
    observer: Observability,
    context: SpanContext,
    parent_span_id: Option<SpanId>,
    name: String,
    threshold_us: u64,
    metadata: Metadata,
    started_at: Instant,
    completed: bool,
}

impl SlowOperationGuard {
    pub fn context(&self) -> SpanContext {
        self.context
    }

    /// Completes the timer with an explicit outcome.
    pub fn finish(mut self, outcome: SpanOutcome) -> u64 {
        self.completed = true;
        self.observer.finish_slow_operation(&self, outcome)
    }
}

impl Drop for SlowOperationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.completed = true;
            self.observer
                .finish_slow_operation(self, SpanOutcome::Dropped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_and_span_ids_are_monotonic_and_linked() {
        let observer = Observability::default();
        let root = observer.begin_span("root", None, Metadata::new());
        let root_context = root.context();
        let child = observer.begin_span("child", Some(root_context), Metadata::new());
        let child_context = child.context();

        assert!(child_context.span_id > root_context.span_id);
        assert_eq!(child_context.trace_id, root_context.trace_id);
        child.finish(SpanOutcome::Success);
        root.finish(SpanOutcome::Success);

        let starts = observer
            .snapshot()
            .events
            .into_iter()
            .filter(|event| event.kind == EventKind::SpanStarted)
            .collect::<Vec<_>>();
        assert_eq!(starts[1].parent_span_id, Some(root_context.span_id));
    }

    #[test]
    fn span_guard_records_explicit_and_drop_outcomes() {
        let observer = Observability::default();
        observer
            .begin_span("explicit", None, Metadata::new())
            .finish(SpanOutcome::Error);
        {
            let _dropped = observer.begin_span("dropped", None, Metadata::new());
        }

        let endings = observer
            .snapshot()
            .events
            .into_iter()
            .filter(|event| event.kind == EventKind::SpanEnded)
            .collect::<Vec<_>>();
        assert_eq!(endings.len(), 2);
        assert_eq!(endings[0].outcome, Some(SpanOutcome::Error));
        assert_eq!(endings[1].outcome, Some(SpanOutcome::Dropped));
        assert!(endings.iter().all(|event| event.duration_us.is_some()));
    }

    #[test]
    fn slow_operation_records_histogram_and_drop_fallback() {
        let observer = Observability::default();
        observer
            .slow_operation("git.status", Duration::ZERO, None, Metadata::new())
            .finish(SpanOutcome::Success);
        {
            let _dropped =
                observer.slow_operation("git.status", Duration::ZERO, None, Metadata::new());
        }

        let snapshot = observer.snapshot();
        let histogram = snapshot.latency_histograms.get("git.status").unwrap();
        assert_eq!(histogram.sample_count, 2);
        let slow_events = snapshot
            .events
            .iter()
            .filter(|event| event.kind == EventKind::SlowOperation)
            .collect::<Vec<_>>();
        assert_eq!(slow_events.len(), 2);
        assert_eq!(slow_events[0].outcome, Some(SpanOutcome::Success));
        assert_eq!(slow_events[1].outcome, Some(SpanOutcome::Dropped));
    }
}
