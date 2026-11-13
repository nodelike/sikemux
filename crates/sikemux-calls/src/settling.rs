use serde::{Deserialize, Serialize};
use sikemux_corekit::wall_clock::{calendar, WallClock, Zone};
use sikemux_process::clock::epoch_ms;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeParams {
    pub agent_ids: Vec<String>,
    /// When it wakes, in Unix milliseconds; none wakes it now, unless `typed` names a time.
    #[serde(default)]
    pub until: Option<u64>,
    /// A time in words this host reads on its own clock, such as "fri 2pm" or "in 90m".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleParams {
    pub agent_ids: Vec<String>,
    pub settled: bool,
}

/// A time the snooze menu offers, on this computer's clock.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePreset {
    pub label: String,
    pub until: u64,
}

pub const MINUTE_MS: u64 = 60_000;
pub const HOUR_MS: u64 = 60 * MINUTE_MS;

/// `hour` o'clock `days_later` days from now on this computer's clock.
fn at_hour(now: u64, days_later: i32, hour: i32, minute: i32) -> Option<u64> {
    let clock = WallClock {
        zone: Zone::Host,
        date: None,
        hour,
        minute,
    };
    clock.on_day(now, days_later)
}

/// Days from today until `weekday`, 0 for Sunday, never 0 itself: today's weekday is a week on.
fn days_until(now: u64, weekday: i32) -> i32 {
    match calendar(now, Zone::Host).map(|today| (weekday - today.tm_wday).rem_euclid(7)) {
        Some(0) | None => 7,
        Some(days) => days,
    }
}

/// The snooze menu's times, soonest first, one per minute: in one and three hours, this
/// evening while it is still to come, tomorrow morning and next Monday morning.
pub fn presets() -> Vec<SnoozePreset> {
    let now = epoch_ms();
    let mut presets: Vec<SnoozePreset> = [
        ("In 1 hour", Some(now + HOUR_MS)),
        ("In 3 hours", Some(now + 3 * HOUR_MS)),
        ("This evening", at_hour(now, 0, 18, 0)),
        ("Tomorrow", at_hour(now, 1, 9, 0)),
        ("Next week", at_hour(now, days_until(now, 1), 9, 0)),
    ]
    .into_iter()
    .filter_map(|(label, until)| {
        let until = until.filter(|&until| until > now)?;
        Some(SnoozePreset {
            label: label.to_owned(),
            until,
        })
    })
    .collect();
    presets.sort_by_key(|preset| preset.until / MINUTE_MS);
    presets.dedup_by_key(|preset| preset.until / MINUTE_MS);
    presets
}

const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// When a time the person typed comes next, such as "in 90m", "2h", "tomorrow 8",
/// "fri 2pm" or "14:30"; none for words it cannot read or a time already past.
pub fn parse_time(typed: &str) -> Option<u64> {
    let now = epoch_ms();
    let typed = typed.trim().to_lowercase();
    let words: Vec<&str> = typed
        .strip_prefix("in ")
        .unwrap_or(&typed)
        .split_whitespace()
        .collect();
    if let Some(span) = span(&words) {
        return Some(now + span);
    }
    let (day, clock) = match words.as_slice() {
        [day, clock] => (Some(*day), Some(*clock)),
        [one] if day_offset(now, one).is_some() => (Some(*one), None),
        [one] => (None, Some(*one)),
        _ => return None,
    };
    let days = match day {
        Some(day) => day_offset(now, day)?,
        None => 0,
    };
    let (hour, minute) = match clock {
        Some(clock) => wall_clock(clock)?,
        None => (9, 0),
    };
    let until = at_hour(now, days, hour, minute)?;
    match until > now {
        true => Some(until),
        false if day.is_none() => at_hour(now, 1, hour, minute),
        false => None,
    }
}

/// "90m", "2 h", "3 days" as milliseconds.
fn span(words: &[&str]) -> Option<u64> {
    let joined = words.concat();
    let digits = joined.find(|c: char| !c.is_ascii_digit())?;
    let (count, unit) = joined.split_at(digits);
    let count: u64 = count.parse().ok().filter(|&count| count > 0)?;
    let unit = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => MINUTE_MS,
        "h" | "hr" | "hrs" | "hour" | "hours" => HOUR_MS,
        "d" | "day" | "days" => 24 * HOUR_MS,
        _ => return None,
    };
    count.checked_mul(unit)
}

fn day_offset(now: u64, word: &str) -> Option<i32> {
    match word {
        "today" => Some(0),
        "tomorrow" | "tmrw" => Some(1),
        word if word.len() >= 3 => WEEKDAYS
            .iter()
            .position(|day| word.starts_with(day))
            .map(|weekday| days_until(now, weekday as i32)),
        _ => None,
    }
}

/// "2pm", "2:30pm", "14:30" or "8" as the hour of the day and its minute.
fn wall_clock(word: &str) -> Option<(i32, i32)> {
    let (clock, pm) = match (word.strip_suffix("pm"), word.strip_suffix("am")) {
        (Some(clock), _) => (clock, Some(true)),
        (_, Some(clock)) => (clock, Some(false)),
        _ => (word, None),
    };
    let (hour, minute) = clock.split_once(':').unwrap_or((clock, "0"));
    let (hour, minute): (i32, i32) = (hour.parse().ok()?, minute.parse().ok()?);
    if !(0..60).contains(&minute) {
        return None;
    }
    match pm {
        Some(pm) if (1..=12).contains(&hour) => Some((hour % 12 + if pm { 12 } else { 0 }, minute)),
        None if (0..24).contains(&hour) => Some((hour, minute)),
        _ => None,
    }
}
