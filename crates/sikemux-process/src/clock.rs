use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since 1970 by the wall clock, which may jump.
pub fn epoch_ms() -> u64 {
    epoch_ms_of(SystemTime::now())
}

pub fn epoch_secs() -> u64 {
    epoch_ms() / 1000
}

pub fn epoch_ms_of(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}
