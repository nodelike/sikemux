//! Numbers, durations and times said the way the pane shows them.

/// A clock time, with the day when the window spans more than one.
pub(super) fn time_label(ms: u64, span: u64) -> String {
    let Some(at) = chrono::DateTime::from_timestamp_millis(ms as i64) else {
        return String::new();
    };
    let at = at.with_timezone(&chrono::Local);
    if span > 24 * 3_600_000 {
        at.format("%b %-d %H:%M").to_string()
    } else {
        at.format("%H:%M").to_string()
    }
}

pub(super) fn percent(rate: f64) -> String {
    match rate {
        0. => "0%".into(),
        rate if rate < 0.001 => "<0.1%".into(),
        rate if rate >= 0.1 => format!("{:.0}%", rate * 100.),
        rate => format!("{:.1}%", rate * 100.),
    }
}

pub(super) fn per_minute(count: u64, minutes: f64) -> String {
    let rate = count as f64 / minutes.max(1.);
    match rate {
        rate if rate >= 10_000. => format!("{:.0}k/min", rate / 1_000.),
        rate if rate >= 1_000. => format!("{:.1}k/min", rate / 1_000.),
        rate if rate >= 10. => format!("{}/min", rate.round()),
        rate => format!("{}/min", significant(rate, 2)),
    }
}

/// `value` to `digits` significant figures, without trailing zeros.
fn significant(value: f64, digits: i32) -> String {
    if value == 0. {
        return "0".into();
    }
    let places = (digits - 1 - value.abs().log10().floor() as i32).max(0) as usize;
    let text = format!("{value:.places$}");
    match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text,
    }
}

fn compact(value: f64) -> String {
    let magnitude = value.abs();
    match magnitude {
        magnitude if magnitude >= 1e9 => format!("{:.1}B", value / 1e9),
        magnitude if magnitude >= 1e6 => format!("{:.1}M", value / 1e6),
        magnitude if magnitude >= 1e4 => format!("{:.1}k", value / 1e3),
        magnitude if magnitude >= 100. || value.fract() == 0. => format!("{}", value.round()),
        magnitude => significant(value, if magnitude >= 1. { 3 } else { 2 }),
    }
}

pub(super) fn duration(ms: f64) -> String {
    match ms {
        ms if ms >= 60_000. => format!("{:.1}m", ms / 60_000.),
        ms if ms >= 1_000. => format!("{}s", significant(ms / 1_000., 3)),
        ms if ms >= 1. => format!("{}ms", significant(ms, 3)),
        ms => format!("{}µs", significant(ms * 1_000., 3)),
    }
}

fn bytes(value: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value;
    let mut unit = 0;
    while value.abs() >= 1024. && unit + 1 < units.len() {
        value /= 1024.;
        unit += 1;
    }
    format!("{} {}", compact(value), units[unit])
}

/// Values in the unit a SigNoz panel declares; anything unknown is a plain number.
pub(super) fn value_in(value: Option<f64>, unit: &str) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "–".into();
    };
    match unit {
        "ns" => duration(value * 1e-6),
        "µs" | "us" => duration(value * 1e-3),
        "ms" => duration(value),
        "s" => duration(value * 1_000.),
        "bytes" | "decbytes" => bytes(value),
        "percent" => format!("{}%", compact(value)),
        "percentunit" => format!("{}%", compact(value * 100.)),
        "reqps" | "rps" => format!("{}/s", compact(value)),
        _ => compact(value),
    }
}

/// A log line's or trace's time to the millisecond.
pub(super) fn clock(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp).map_or_else(
        |_| timestamp.to_owned(),
        |at| {
            at.with_timezone(&chrono::Local)
                .format("%H:%M:%S%.3f")
                .to_string()
        },
    )
}
