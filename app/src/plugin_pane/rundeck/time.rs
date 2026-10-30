//! Reading Rundeck's times and saying them in the person's words.

use super::*;

fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// How long a run took, or has taken so far.
pub(super) fn run_secs(started: Option<&str>, ended: Option<&str>) -> Option<f64> {
    let started = parse(started?)?;
    let ended = ended.and_then(parse).unwrap_or_else(Utc::now);
    Some(((ended - started).num_milliseconds().max(0) as f64) / 1000.)
}

pub(super) fn since(started: Option<&str>) -> Option<f64> {
    run_secs(started, None)
}

/// A length of time as the pane writes it: `4.2s`, `4m 12s`, `1h 5m`.
pub(super) fn took(secs: f64) -> String {
    if secs <= 0. {
        return "0s".into();
    }
    if secs < 10. {
        let tenths = (secs * 10.).round() / 10.;
        return if tenths.fract() == 0. {
            format!("{tenths:.0}s")
        } else {
            format!("{tenths:.1}s")
        };
    }
    let seconds = secs.round() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return match seconds % 60 {
            0 => format!("{minutes}m"),
            rest => format!("{minutes}m {rest}s"),
        };
    }
    match minutes % 60 {
        0 => format!("{}h", minutes / 60),
        rest => format!("{}h {rest}m", minutes / 60),
    }
}

pub(super) fn day_of(at: &str) -> String {
    parse(at)
        .map(|at| at.with_timezone(&Local).format("%A %-d %B").to_string())
        .unwrap_or_else(|| "Unknown date".into())
}

pub(super) fn clock_of(at: &str) -> String {
    parse(at)
        .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

pub(super) fn stamp_of(at: &str) -> String {
    parse(at)
        .map(|at| {
            at.with_timezone(&Local)
                .format("%-d %b, %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| at.to_owned())
}

/// A time typed in the person's own zone, as ISO 8601 with its offset, the form Rundeck wants.
pub(super) fn local_to_iso(typed: &str) -> Option<String> {
    let typed = typed.trim().replace('T', " ");
    let naive = NaiveDateTime::parse_from_str(&typed, "%Y-%m-%d %H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(&typed, "%Y-%m-%d %H:%M:%S"))
        .ok()?;
    let local = Local.from_local_datetime(&naive).single()?;
    Some(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
}
