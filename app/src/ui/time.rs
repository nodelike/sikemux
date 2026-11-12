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

/// A total to the nearest minute: `5m`, `3h`, `3h 20m`, and whole hours from a hundred: `1,234h`.
pub fn total(ms: f64) -> String {
    let minutes = (ms / 60_000.).round() as i64;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours >= 100 {
        return format!("{}h", crate::ui::numbers::whole(hours as f64));
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
        0..60 => t!("now").into(),
        60..3600 => t!("{count}m", count = seconds / 60),
        3600..86_400 => t!("{count}h", count = seconds / 3600),
        86_400..2_592_000 => t!("{count}d", count = seconds / 86_400),
        2_592_000..31_536_000 => t!("{count}mo", count = seconds / 2_592_000),
        _ => t!("{count}y", count = seconds / 31_536_000),
    }
}

/// How long ago in words: `now`, `5m ago`, `3h ago`.
pub fn ago(at: u64) -> String {
    match age(at) {
        now if now == t!("now") => now,
        age => t!("{age} ago", age = age),
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

/// A moment as the person's clock reads it, in their 24 or 12-hour format: `16:40` or
/// `4:40 PM`, with the day once it isn't today.
pub fn clock(at: u64) -> String {
    use chrono::TimeZone;
    let Some(at) = chrono::Local.timestamp_millis_opt(at as i64).single() else {
        return String::new();
    };
    let time = at.format(if twelve_hour() { "%-I:%M %p" } else { "%H:%M" });
    match at.date_naive() == chrono::Local::now().date_naive() {
        true => time.to_string(),
        false => format!("{} {time}", at.format("%a")),
    }
}

/// Whether the person's Mac writes times with AM and PM, as its locale and Language & Region
/// settings say.
#[cfg(target_os = "macos")]
fn twelve_hour() -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{ns_string, NSString};
    static TWELVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TWELVE.get_or_init(|| {
        // SAFETY: both are class methods taking and returning objects of these kinds, and the
        // format's template and the locale outlive the call.
        let format: Option<Retained<NSString>> = unsafe {
            let locale: Retained<AnyObject> = msg_send![class!(NSLocale), currentLocale];
            msg_send![
                class!(NSDateFormatter),
                dateFormatFromTemplate: ns_string!("j"),
                options: 0usize,
                locale: &*locale
            ]
        };
        format.is_some_and(|format| format.to_string().contains('a'))
    })
}

#[cfg(not(target_os = "macos"))]
fn twelve_hour() -> bool {
    false
}

/// How long until, in the same tight column as [`age`], rounded up: `now`, `1m`, `3h`, `4d`.
pub fn until(at: u64) -> String {
    let seconds = at.saturating_sub(now_ms()) / 1000;
    match seconds {
        0 => t!("now").into(),
        1..3600 => t!("{count}m", count = seconds.div_ceil(60)),
        3600..86_400 => t!("{count}h", count = seconds / 3600),
        _ => t!("{count}d", count = seconds / 86_400),
    }
}

/// How long until `reset`, rounded up to the minute and to two parts, `3h 20m` or `2d 4h`;
/// a week or more off, the day it falls on.
pub fn countdown(reset: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(reset) = reset else {
        return t!("unknown").into();
    };
    let minutes = ((reset - chrono::Utc::now()).num_seconds().max(0) as f64 / 60.).ceil() as i64;
    let (hours, days) = (minutes / 60, minutes / 1440);
    match minutes {
        0 => t!("resetting now").into(),
        1..=59 => t!("{count}m", count = minutes),
        _ if hours < 24 => match minutes % 60 {
            0 => t!("{count}h", count = hours),
            rest => t!("{hours}h {minutes}m", hours = hours, minutes = rest),
        },
        _ if days < 7 => match hours % 24 {
            0 => t!("{count}d", count = days),
            rest => t!("{days}d {hours}h", days = days, hours = rest),
        },
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
        return t!("{count} min", count = minutes);
    }
    let hours = (minutes as f64 / 60.).round() as u64;
    match hours {
        1 => t!("1 hour").to_owned(),
        hours if hours < 48 => t!("{count} hours", count = hours),
        hours => t!("{count} days", count = (hours as f64 / 24.).round() as u64),
    }
}
