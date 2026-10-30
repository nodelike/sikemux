use serde_json::{json, Value};
use sikemux_plugin_api::process::clock::epoch_ms;
pub use sikemux_plugin_signoz_views::View;

pub const DEFAULT_MINUTES: u32 = 30;
const MAX_MINUTES: u32 = 7 * 24 * 60;

/// Milliseconds to the microsecond, which is finer than any span or request needs.
pub fn round_ms(ms: f64) -> f64 {
    (ms * 1_000.0).round() / 1_000.0
}

/// Start and end of the last `minutes`, clamped to a week.
pub fn window(minutes: Option<u32>) -> (u64, u64) {
    let minutes = minutes.unwrap_or(DEFAULT_MINUTES).clamp(1, MAX_MINUTES);
    let end = epoch_ms();
    (end.saturating_sub(u64::from(minutes) * 60_000), end)
}

/// "30 minutes", "24 hours", "7 days": a look-back as a person would say it.
pub fn minutes_label(minutes: u32) -> String {
    let (count, unit) = if minutes.is_multiple_of(24 * 60) {
        (minutes / (24 * 60), "day")
    } else if minutes.is_multiple_of(60) {
        (minutes / 60, "hour")
    } else {
        (minutes, "minute")
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// A value inside a filter expression, quoted so it can only ever be a value.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub fn all_of(parts: impl IntoIterator<Item = String>) -> Option<String> {
    let parts: Vec<String> = parts.into_iter().filter(|part| !part.is_empty()).collect();
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => Some(
            parts
                .into_iter()
                .map(|part| format!("({part})"))
                .collect::<Vec<_>>()
                .join(" AND "),
        ),
    }
}

pub fn builder(request_type: &str, (start, end): (u64, u64), mut spec: Value) -> Value {
    if let Some(object) = spec.as_object_mut() {
        object.insert("name".into(), json!("A"));
    }
    json!({
        "schemaVersion": "v1",
        "start": start,
        "end": end,
        "requestType": request_type,
        "compositeQuery": { "queries": [{ "type": "builder_query", "spec": spec }] },
    })
}

pub fn with_filter(mut spec: Value, expression: Option<String>) -> Value {
    if let (Some(object), Some(expression)) = (spec.as_object_mut(), expression) {
        object.insert("filter".into(), json!({ "expression": expression }));
    }
    spec
}

/// Unix nanoseconds from the RFC 3339 times SigNoz writes, like
/// `2026-09-24T08:59:25.187Z`.
pub fn parse_timestamp_ns(text: &str) -> Option<i128> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(time::OffsetDateTime::unix_timestamp_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_values_so_they_cannot_close_the_string() {
        assert_eq!(quote("api"), "'api'");
        assert_eq!(quote("can't"), "'can\\'t'");
        assert_eq!(quote("a\\' OR 1=1"), "'a\\\\\\' OR 1=1'");
    }

    #[test]
    fn says_a_look_back_the_way_a_person_would() {
        assert_eq!(minutes_label(1), "1 minute");
        assert_eq!(minutes_label(90), "90 minutes");
        assert_eq!(minutes_label(24 * 60), "1 day");
        assert_eq!(minutes_label(120), "2 hours");
    }

    #[test]
    fn joins_only_the_parts_that_are_there() {
        assert_eq!(all_of([]), None);
        assert_eq!(
            all_of(["a = 1".to_string(), String::new()]),
            Some("a = 1".into())
        );
        assert_eq!(
            all_of(["a = 1".to_string(), "b OR c".to_string()]),
            Some("(a = 1) AND (b OR c)".into())
        );
    }

    #[test]
    fn parses_signoz_timestamps() {
        assert_eq!(parse_timestamp_ns("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp_ns("2026-09-24T08:59:25.187Z"),
            Some(1_790_240_365_187_000_000)
        );
        assert_eq!(
            parse_timestamp_ns("2026-09-24T09:11:54.569674944Z"),
            Some(1_790_241_114_569_674_944)
        );
        assert_eq!(
            parse_timestamp_ns("2024-02-29T12:00:00Z"),
            Some(1_709_208_000_000_000_000)
        );
        assert_eq!(parse_timestamp_ns("yesterday"), None);
    }
}
