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

use std::time::Duration;

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
