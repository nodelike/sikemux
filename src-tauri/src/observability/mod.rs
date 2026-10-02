//! Lightweight, bounded process-local observability primitives.
//!
//! This module deliberately does not depend on a logging backend. It keeps a
//! small structured history that can be exposed through diagnostics, while
//! callers may independently forward snapshots to a tracing or telemetry
//! implementation. Values attached to events are scalar-only so diagnostics
//! cannot accidentally retain an arbitrary object graph.

mod activity;
mod heartbeat;
mod model;
mod spans;
mod store;
mod ui;
mod watchdog;

pub use activity::*;
pub use heartbeat::*;
pub use model::*;
pub use spans::*;
pub use store::*;
pub use ui::*;
pub use watchdog::*;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn next_monotonic_id(counter: &AtomicU64) -> u64 {
    loop {
        let current = counter.load(Ordering::Relaxed);
        let next = match current.checked_add(1) {
            Some(next) => next,
            // Exhausting a 64-bit process-local ID space is not recoverable:
            // wrapping would make traces ambiguous and violate the API.
            None => std::process::abort(),
        };
        if counter
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return current;
        }
    }
}

fn increment_monotonic(counter: &AtomicU64) -> u64 {
    loop {
        let current = counter.load(Ordering::Relaxed);
        let next = match current.checked_add(1) {
            Some(next) => next,
            None => std::process::abort(),
        };
        if counter
            .compare_exchange_weak(current, next, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}
