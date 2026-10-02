use serde::Serialize;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::heartbeat::{Heartbeat, HeartbeatSnapshot};
use super::model::{Metadata, ScalarValue};
use super::store::{global_observability, Observability};

const DEFAULT_WATCHDOG_SAMPLE_INTERVAL_MS: u64 = 25;
const DEFAULT_WATCHDOG_HANG_THRESHOLD_MS: u64 = 100;

const IDLE_WATCHDOG_WAIT: Duration = Duration::from_secs(60);

/// Notified once when a watchdog first sees a hang, so evidence can be
/// gathered while the process is still stuck.
pub trait HangListener: Send + Sync {
    fn on_hang(&self, signal: HangSignal);
}

/// What the watchdog knew at the moment a hang started.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HangSignal {
    pub watchdog: String,
    pub delay_us: u64,
    pub threshold_us: u64,
    pub heartbeat_sequence: u64,
    pub visible: bool,
}

/// Sampling policy for the process hang watchdog.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HangWatchdogConfig {
    /// Low-cardinality label used in metric names and event metadata.
    pub name: String,
    /// Delay between samples. Zero is normalized to one millisecond.
    pub sample_interval_ms: u64,
    /// A delay equal to or greater than this value is a hang.
    pub hang_threshold_ms: u64,
    /// When false, an invisible heartbeat is treated as intentionally idle.
    pub monitor_hidden: bool,
}

impl Default for HangWatchdogConfig {
    fn default() -> Self {
        Self {
            name: "ui".to_owned(),
            sample_interval_ms: DEFAULT_WATCHDOG_SAMPLE_INTERVAL_MS,
            hang_threshold_ms: DEFAULT_WATCHDOG_HANG_THRESHOLD_MS,
            monitor_hidden: false,
        }
    }
}

impl HangWatchdogConfig {
    fn normalized(mut self, observer: &Observability) -> Self {
        self.name = observer.sanitize_text(self.name);
        if self.name.is_empty() {
            self.name = "ui".to_owned();
        }
        self.sample_interval_ms = self.sample_interval_ms.max(1);
        self.hang_threshold_ms = self.hang_threshold_ms.max(1);
        self
    }
}

/// Result of classifying one watchdog sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatDelayClassification {
    Inactive,
    Healthy,
    HangStarted,
    HangOngoing,
    Recovered,
}

/// Pure watchdog state transition used by the production sampler and tests.
pub fn classify_heartbeat_delay(
    delay: Duration,
    threshold: Duration,
    previously_hung: bool,
    monitored: bool,
) -> HeartbeatDelayClassification {
    if !monitored {
        return HeartbeatDelayClassification::Inactive;
    }

    if delay >= threshold {
        if previously_hung {
            HeartbeatDelayClassification::HangOngoing
        } else {
            HeartbeatDelayClassification::HangStarted
        }
    } else if previously_hung {
        HeartbeatDelayClassification::Recovered
    } else {
        HeartbeatDelayClassification::Healthy
    }
}

#[derive(Debug, Default)]
struct WatchdogSignalState {
    stopped: bool,
    /// Set between the watchdog reading "nothing to watch" and it starting to
    /// wait. Without it that window would swallow the wakeup and the watchdog
    /// would sleep out its whole idle timeout.
    woken: bool,
}

#[derive(Debug)]
pub(super) struct WatchdogSignal {
    state: Mutex<WatchdogSignalState>,
    wake: Condvar,
}

impl WatchdogSignal {
    fn new() -> Self {
        Self {
            state: Mutex::new(WatchdogSignalState::default()),
            wake: Condvar::new(),
        }
    }

    fn stop(&self) {
        self.lock_state().stopped = true;
        self.wake.notify_all();
    }

    pub(super) fn wake(&self) {
        self.lock_state().woken = true;
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        self.lock_state().stopped
    }

    /// Returns true when stopped. The predicate closes the notify-before-wait
    /// race, allowing shutdown to interrupt even a very long sample interval.
    fn wait_until_stopped(&self, timeout: Duration) -> bool {
        let mut state = self.lock_state();
        if state.stopped || std::mem::take(&mut state.woken) {
            return state.stopped;
        }

        let mut state = match self
            .wake
            .wait_timeout_while(state, timeout, |state| !state.stopped && !state.woken)
        {
            Ok((state, _)) => state,
            Err(poisoned) => poisoned.into_inner().0,
        };
        state.woken = false;
        state.stopped
    }

    fn lock_state(&self) -> MutexGuard<'_, WatchdogSignalState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Debug)]
struct WatchdogMetricNames {
    samples: String,
    hangs: String,
    recoveries: String,
    delay_gauge: String,
    delay_histogram: String,
}

impl WatchdogMetricNames {
    fn new(name: &str) -> Self {
        let prefix = format!("watchdog.{name}");
        Self {
            samples: format!("{prefix}.samples"),
            hangs: format!("{prefix}.hangs"),
            recoveries: format!("{prefix}.recoveries"),
            delay_gauge: format!("{prefix}.heartbeat_delay_us"),
            delay_histogram: format!("{prefix}.heartbeat_delay"),
        }
    }
}

/// Error returned when the dedicated watchdog OS thread panics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatchdogJoinError;

impl fmt::Display for WatchdogJoinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("hang watchdog thread panicked")
    }
}

impl std::error::Error for WatchdogJoinError {}

/// Owner of a running watchdog thread.
///
/// Calling [`HangWatchdogHandle::stop`] interrupts its condition-variable wait
/// and joins it. Dropping the handle provides the same clean shutdown fallback.
#[must_use = "dropping the handle immediately stops the watchdog"]
pub struct HangWatchdogHandle {
    signal: Arc<WatchdogSignal>,
    thread: Option<JoinHandle<()>>,
}

impl HangWatchdogHandle {
    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
    }

    pub fn stop(mut self) -> Result<(), WatchdogJoinError> {
        self.signal.stop();
        self.join()
    }

    fn join(&mut self) -> Result<(), WatchdogJoinError> {
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| WatchdogJoinError),
            None => Ok(()),
        }
    }
}

impl Drop for HangWatchdogHandle {
    fn drop(&mut self) {
        self.signal.stop();
        // Drop cannot surface a panic. Explicit `stop` remains available when
        // the caller wants to distinguish a clean join from a panicked thread.
        let _ = self.join();
    }
}

/// Starts a dedicated OS thread which samples `heartbeat` and writes bounded
/// diagnostics to [`global_observability`].
pub fn start_hang_watchdog(
    heartbeat: Heartbeat,
    config: HangWatchdogConfig,
    on_hang: Option<Arc<dyn HangListener>>,
) -> std::io::Result<HangWatchdogHandle> {
    let observer = global_observability();
    let config = config.normalized(observer);
    let signal = Arc::new(WatchdogSignal::new());
    heartbeat.attach_waker(signal.clone());
    let thread_signal = signal.clone();
    let thread = thread::Builder::new()
        .name("sikemux-hang-watchdog".to_owned())
        .spawn(move || run_watchdog(heartbeat, config, thread_signal, on_hang))?;

    Ok(HangWatchdogHandle {
        signal,
        thread: Some(thread),
    })
}

fn run_watchdog(
    heartbeat: Heartbeat,
    config: HangWatchdogConfig,
    signal: Arc<WatchdogSignal>,
    on_hang: Option<Arc<dyn HangListener>>,
) {
    let observer = global_observability();
    let metric_names = WatchdogMetricNames::new(&config.name);
    let sample_interval = Duration::from_millis(config.sample_interval_ms);
    let threshold = Duration::from_millis(config.hang_threshold_ms);
    let mut previously_hung = false;

    while !signal.is_stopped() {
        let heartbeat_snapshot = heartbeat.snapshot();
        let now_us = observer.now_us();
        let delay_us = now_us.saturating_sub(heartbeat_snapshot.last_beat_us);
        let monitored =
            heartbeat_snapshot.armed && (config.monitor_hidden || heartbeat_snapshot.visible);
        let classification = classify_heartbeat_delay(
            Duration::from_micros(delay_us),
            threshold,
            previously_hung,
            monitored,
        );

        record_watchdog_sample(
            observer,
            &config,
            &metric_names,
            heartbeat_snapshot,
            delay_us,
            classification,
            on_hang.as_ref(),
        );
        previously_hung = matches!(
            classification,
            HeartbeatDelayClassification::HangStarted | HeartbeatDelayClassification::HangOngoing
        );

        // A hidden or disarmed window has no progress to miss, so wait for it
        // to come back rather than sampling four times a second all day. The
        // long timeout is only a backstop for a wakeup that never arrives.
        let wait = if monitored {
            sample_interval
        } else {
            IDLE_WATCHDOG_WAIT
        };
        if signal.wait_until_stopped(wait) {
            break;
        }
    }
}

fn record_watchdog_sample(
    observer: &Observability,
    config: &HangWatchdogConfig,
    metric_names: &WatchdogMetricNames,
    heartbeat: HeartbeatSnapshot,
    delay_us: u64,
    classification: HeartbeatDelayClassification,
    on_hang: Option<&Arc<dyn HangListener>>,
) {
    if classification == HeartbeatDelayClassification::Inactive {
        return;
    }

    let _ = observer.increment_counter(metric_names.samples.clone(), 1);
    observer.set_gauge(metric_names.delay_gauge.clone(), delay_us as f64);
    observer.observe_latency(
        metric_names.delay_histogram.clone(),
        Duration::from_micros(delay_us),
    );

    let (event_name, counter_name) = match classification {
        HeartbeatDelayClassification::HangStarted => {
            (Some("watchdog.hang_started"), Some(&metric_names.hangs))
        }
        HeartbeatDelayClassification::Recovered => (
            Some("watchdog.hang_recovered"),
            Some(&metric_names.recoveries),
        ),
        HeartbeatDelayClassification::Inactive
        | HeartbeatDelayClassification::Healthy
        | HeartbeatDelayClassification::HangOngoing => (None, None),
    };

    if let Some(counter_name) = counter_name {
        let _ = observer.increment_counter(counter_name.clone(), 1);
    }
    if let Some(event_name) = event_name {
        let mut metadata = Metadata::new();
        metadata.insert(
            "watchdog".to_owned(),
            ScalarValue::String(config.name.clone()),
        );
        metadata.insert("delay_us".to_owned(), ScalarValue::U64(delay_us));
        metadata.insert(
            "threshold_us".to_owned(),
            ScalarValue::U64(config.hang_threshold_ms.saturating_mul(1_000)),
        );
        metadata.insert(
            "heartbeat_sequence".to_owned(),
            ScalarValue::U64(heartbeat.sequence),
        );
        metadata.insert("visible".to_owned(), ScalarValue::Bool(heartbeat.visible));
        observer.record_event(event_name, None, metadata);
    }

    if classification == HeartbeatDelayClassification::HangStarted {
        if let Some(listener) = on_hang {
            listener.on_hang(HangSignal {
                watchdog: config.name.clone(),
                delay_us,
                threshold_us: config.hang_threshold_ms.saturating_mul(1_000),
                heartbeat_sequence: heartbeat.sequence,
                visible: heartbeat.visible,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::ObservabilityConfig;
    use super::super::ui::inactive_ui_heartbeat;
    use super::*;
    use std::time::Instant;

    #[test]
    fn heartbeat_delay_classification_has_deterministic_boundaries() {
        let threshold = Duration::from_millis(100);
        assert_eq!(
            classify_heartbeat_delay(Duration::from_secs(1), threshold, true, false),
            HeartbeatDelayClassification::Inactive
        );
        assert_eq!(
            classify_heartbeat_delay(Duration::from_millis(99), threshold, false, true),
            HeartbeatDelayClassification::Healthy
        );
        assert_eq!(
            classify_heartbeat_delay(Duration::from_millis(100), threshold, false, true),
            HeartbeatDelayClassification::HangStarted
        );
        assert_eq!(
            classify_heartbeat_delay(Duration::from_millis(101), threshold, true, true),
            HeartbeatDelayClassification::HangOngoing
        );
        assert_eq!(
            classify_heartbeat_delay(Duration::from_millis(1), threshold, true, true),
            HeartbeatDelayClassification::Recovered
        );
    }

    #[test]
    fn watchdog_sample_records_bounded_observability_data() {
        let observer = Observability::new(ObservabilityConfig {
            event_capacity: 8,
            latency_sample_capacity: 8,
            metric_series_capacity: 16,
            max_metadata_entries: 8,
            max_string_bytes: 128,
        });
        let watchdog_config = HangWatchdogConfig {
            name: "ui".to_owned(),
            sample_interval_ms: 25,
            hang_threshold_ms: 100,
            monitor_hidden: false,
        };
        let metric_names = WatchdogMetricNames::new(&watchdog_config.name);
        let heartbeat = HeartbeatSnapshot {
            sequence: 7,
            last_beat_us: 1,
            armed: true,
            visible: true,
        };

        record_watchdog_sample(
            &observer,
            &watchdog_config,
            &metric_names,
            heartbeat,
            125_000,
            HeartbeatDelayClassification::HangStarted,
            None,
        );

        let snapshot = observer.snapshot();
        assert_eq!(snapshot.counters.get("watchdog.ui.samples"), Some(&1));
        assert_eq!(snapshot.counters.get("watchdog.ui.hangs"), Some(&1));
        assert_eq!(
            snapshot.gauges.get("watchdog.ui.heartbeat_delay_us"),
            Some(&125_000.0)
        );
        assert_eq!(
            snapshot
                .latency_histograms
                .get("watchdog.ui.heartbeat_delay")
                .unwrap()
                .buckets
                .at_least_100_ms,
            1
        );
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].name, "watchdog.hang_started");
    }

    #[test]
    fn watchdog_stop_interrupts_a_long_wait_without_sleeping() {
        let heartbeat = Heartbeat::new();
        let handle = start_hang_watchdog(
            heartbeat,
            HangWatchdogConfig {
                sample_interval_ms: 60_000,
                ..HangWatchdogConfig::default()
            },
            None,
        )
        .unwrap();
        handle.stop().unwrap();
    }

    /// A hidden window leaves the watchdog parked; showing it again has to
    /// release the wait rather than let it run out its idle backstop.
    #[test]
    fn a_parked_watchdog_wakes_when_the_window_comes_back() {
        let heartbeat = inactive_ui_heartbeat();
        let signal = Arc::new(WatchdogSignal::new());
        heartbeat.attach_waker(signal.clone());

        // Woken before the wait starts: the wakeup is not lost.
        heartbeat.set_visible(true);
        let started = Instant::now();
        assert!(!signal.wait_until_stopped(IDLE_WATCHDOG_WAIT));
        assert!(started.elapsed() < Duration::from_secs(1));

        // Woken during the wait.
        let waker = heartbeat.clone();
        let waking = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            waker.set_armed(true);
        });
        let started = Instant::now();
        assert!(!signal.wait_until_stopped(IDLE_WATCHDOG_WAIT));
        assert!(started.elapsed() < Duration::from_secs(1));
        waking.join().unwrap();

        signal.stop();
        assert!(signal.wait_until_stopped(IDLE_WATCHDOG_WAIT));
    }

    #[derive(Debug, Default)]
    struct RecordingListener {
        signals: Mutex<Vec<HangSignal>>,
    }

    impl HangListener for RecordingListener {
        fn on_hang(&self, signal: HangSignal) {
            self.signals.lock().unwrap().push(signal);
        }
    }

    /// Only the transition into a hang is worth evidence. Every later sample of
    /// the same hang must stay silent so one stall cannot spawn a capture
    /// four times a second.
    #[test]
    fn only_the_start_of_a_hang_asks_for_a_capture() {
        let observer = Observability::default();
        let watchdog_config = HangWatchdogConfig::default();
        let metric_names = WatchdogMetricNames::new(&watchdog_config.name);
        let heartbeat = HeartbeatSnapshot {
            sequence: 7,
            last_beat_us: 1,
            armed: true,
            visible: true,
        };
        let listener = Arc::new(RecordingListener::default());
        let erased: Arc<dyn HangListener> = listener.clone();

        for classification in [
            HeartbeatDelayClassification::Healthy,
            HeartbeatDelayClassification::HangStarted,
            HeartbeatDelayClassification::HangOngoing,
            HeartbeatDelayClassification::Recovered,
            HeartbeatDelayClassification::Inactive,
        ] {
            record_watchdog_sample(
                &observer,
                &watchdog_config,
                &metric_names,
                heartbeat,
                125_000,
                classification,
                Some(&erased),
            );
        }

        let signals = listener.signals.lock().unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0],
            HangSignal {
                watchdog: "ui".to_owned(),
                delay_us: 125_000,
                threshold_us: 100_000,
                heartbeat_sequence: 7,
                visible: true,
            }
        );
    }
}
