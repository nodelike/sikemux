//! How the app writes lengths of time and moments, so every screen says them alike.

/// Now, in Unix milliseconds.
pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Whole seconds since `at` (Unix milliseconds); none counts as just started.
pub fn seconds_since(at: Option<i64>) -> i64 {
    at.map_or(0, |at| {
        (chrono::Utc::now().timestamp_millis() - at).max(0) / 1000
    })
}

/// A running length, clock style: `45s`, `3m 07s`, `2h 05m`.
pub fn elapsed(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// How long something took: milliseconds under a second, then as [`elapsed`], to the nearest second.
pub fn took(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms}ms"),
        _ => elapsed((ms + 500) / 1000),
    }
}

/// A length in words, leaving out a part that is zero: `45s`, `3m`, `3m 5s`, `2h`, `2h 5m`.
pub fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 if seconds.is_multiple_of(60) => format!("{}m", seconds / 60),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ if (seconds / 60).is_multiple_of(60) => format!("{}h", seconds / 3600),
        _ => format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// A total to the nearest minute: `5m`, `3h`, `3h 20m`, and whole hours from a hundred: `1,234h`.
pub fn total(ms: f64) -> String {
    let minutes = (ms / 60_000.).round() as i64;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours >= 100 {
        return format!("{}h", crate::home::whole(hours as f64));
    }
    match minutes % 60 {
        0 => format!("{hours}h"),
        rest => format!("{hours}h {rest}m"),
    }
}

/// How long ago, in a tight column: `now`, `5m`, `3h`, `4d`, `2mo`, `1y`.
pub fn age(at: u64) -> String {
    let seconds = now_ms().saturating_sub(at) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        2_592_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// How long ago in words: `now`, `5m ago`, `3h ago`.
pub fn ago(at: u64) -> String {
    match age(at) {
        now if now == "now" => now,
        age => format!("{age} ago"),
    }
}

/// [`ago`] of a moment a service wrote in RFC 3339, or with an offset that has no colon as Jira
/// writes it; nothing for one that does not read.
pub fn ago_written(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .or_else(|_| chrono::DateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map_or_else(
            |_| String::new(),
            |at| ago(at.timestamp_millis().max(0) as u64),
        )
}

/// How long until, in the same tight column as [`age`], rounded up: `now`, `1m`, `3h`, `4d`.
pub fn until(at: u64) -> String {
    let seconds = at.saturating_sub(now_ms()) / 1000;
    match seconds {
        0 => "now".into(),
        1..3600 => format!("{}m", seconds.div_ceil(60)),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// How long until `reset`, rounded up to the minute and to two parts, `3h 20m` or `2d 4h`;
/// a week or more off, the day it falls on.
pub fn countdown(reset: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(reset) = reset else {
        return "unknown".into();
    };
    let minutes = ((reset - chrono::Utc::now()).num_seconds().max(0) as f64 / 60.).ceil() as i64;
    let (hours, days) = (minutes / 60, minutes / 1440);
    let with = |big: i64, unit: &str, small: i64, small_unit: &str| match small {
        0 => format!("{big}{unit}"),
        small => format!("{big}{unit} {small}{small_unit}"),
    };
    match minutes {
        0 => "resetting now".into(),
        1..=59 => format!("{minutes}m"),
        _ if hours < 24 => with(hours, "h", minutes % 60, "m"),
        _ if days < 7 => with(days, "d", hours % 24, "h"),
        _ => reset
            .with_timezone(&chrono::Local)
            .format("%b %-d")
            .to_string(),
    }
}

/// How long until `until` (Unix milliseconds) in words, as a dialog says it: `5 min`, `3 hours`, `2 days`.
pub fn left(until: u64) -> String {
    let now = now_ms();
    let minutes = (until.saturating_sub(now) as f64 / 60_000.).round().max(1.) as u64;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = (minutes as f64 / 60.).round() as u64;
    match hours {
        1 => "1 hour".to_owned(),
        hours if hours < 48 => format!("{hours} hours"),
        hours => format!("{} days", (hours as f64 / 24.).round() as u64),
    }
}
