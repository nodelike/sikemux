use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::Scope;
use crate::query;

const MAX_ROWS: u32 = 500;
const MAX_OPERATIONS: u32 = 100;
const ERROR_BODIES_READ: u32 = 200;
const MAX_ERROR_GROUPS: usize = 20;
const CHART_POINTS: u64 = 60;
const ENTRY_SPANS: &str = "isRoot = true OR isEntryPoint = true";

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceQuery {
    /// Narrows by environment and window. A service or filter here would
    /// hide the very rows the list is for, so those are ignored.
    #[serde(flatten)]
    pub scope: Scope,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    pub service: String,
    pub environment: Option<String>,
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p99_ms: f64,
}

/// Counted from entry spans rather than SigNoz's service map, which leaves out
/// services that still send traces. One row per service in each environment,
/// since the same service often runs in several.
pub async fn health(data_dir: &Path, request: ServiceQuery) -> SignozResult<Vec<ServiceHealth>> {
    let scope = Scope {
        environment: request.scope.environment.clone(),
        ..Scope::default()
    };
    let expression =
        query::all_of(std::iter::once(ENTRY_SPANS.to_string()).chain(scope.clauses()?));
    let spec = json!({
        "signal": "traces",
        "aggregations": [
            { "expression": "count()" },
            { "expression": "countIf(hasError = true)" },
            { "expression": "p99(duration_nano)" },
        ],
        "groupBy": [
            { "name": "service.name", "fieldContext": "resource" },
            { "name": "deployment.environment", "fieldContext": "resource" },
        ],
        "order": [{ "key": { "name": "count()" }, "direction": "desc" }],
        "limit": MAX_ROWS,
    });
    let result = client::query_range(
        data_dir,
        &query::builder(
            "scalar",
            request.scope.window()?,
            query::with_filter(spec, expression),
        ),
    )
    .await?;
    Ok(parse(&result))
}

fn number(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

fn parse(result: &Value) -> Vec<ServiceHealth> {
    let rows = result
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rows.iter()
        .filter_map(|row| {
            let row = row.as_array()?;
            let service = row.first()?.as_str()?.to_string();
            let environment = row
                .get(1)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|environment| !environment.is_empty())
                .map(str::to_string);
            let calls = number(row.get(2)).max(0.0) as u64;
            let errors = number(row.get(3)).max(0.0) as u64;
            Some(ServiceHealth {
                service,
                environment,
                calls,
                errors,
                error_rate: if calls == 0 {
                    0.0
                } else {
                    errors as f64 / calls as f64
                },
                p99_ms: number(row.get(4)) / 1_000_000.0,
            })
        })
        .collect()
}

/// One service, in the environment and window of the scope around it.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRequest {
    #[serde(flatten)]
    pub scope: Scope,
}

impl ServiceRequest {
    fn expression(&self, base: &str) -> SignozResult<Option<String>> {
        if self
            .scope
            .service
            .as_deref()
            .is_none_or(|service| service.trim().is_empty())
        {
            return Err(SignozError::BadArg("a service is needed".into()));
        }
        Ok(query::all_of(
            std::iter::once(base.to_string()).chain(self.scope.clauses()?),
        ))
    }
}

pub type Point = (u64, f64);

#[derive(Serialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub per_minute: f64,
    pub p99_ms: f64,
    pub p50_ms: f64,
    pub requests: Vec<Point>,
    pub failures: Vec<Point>,
    pub p99: Vec<Point>,
}

const LATENCY_AND_ERRORS: [&str; 4] = [
    "count()",
    "countIf(hasError = true)",
    "p99(duration_nano)",
    "p50(duration_nano)",
];

fn aggregations() -> Value {
    Value::Array(
        LATENCY_AND_ERRORS
            .iter()
            .map(|expression| json!({ "expression": expression }))
            .collect(),
    )
}

/// Rates, errors and latency of a service's entry spans: the totals, and the
/// same four over time for its charts.
pub async fn overview(data_dir: &Path, request: ServiceRequest) -> SignozResult<Overview> {
    let expression = request.expression(ENTRY_SPANS)?;
    let (start, end) = request.scope.window()?;
    let step_seconds = ((end - start) / 1_000 / CHART_POINTS).max(60);
    let totals = query::builder(
        "scalar",
        (start, end),
        query::with_filter(
            json!({ "signal": "traces", "aggregations": aggregations() }),
            expression.clone(),
        ),
    );
    let series = query::builder(
        "time_series",
        (start, end),
        query::with_filter(
            json!({ "signal": "traces", "stepInterval": step_seconds, "aggregations": aggregations() }),
            expression,
        ),
    );
    let (totals, series) = tokio::try_join!(
        client::query_range(data_dir, &totals),
        client::query_range(data_dir, &series),
    )?;
    Ok(parse_overview(&totals, &series, end - start, step_seconds))
}

/// Points in time order, leaving out the half-filled buckets at either end,
/// which would otherwise read as a sudden drop. SigNoz lists a time series'
/// aggregations in no fixed order, so each is found by the index it carries.
fn points(result: &Value, index: usize, scale: f64) -> Vec<Point> {
    let mut points: Vec<Point> = result
        .get("aggregations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|aggregation| aggregation.get("index").and_then(Value::as_u64) == Some(index as u64))
        .and_then(|aggregation| aggregation.pointer("/series/0/values"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|point| point.get("partial").and_then(Value::as_bool) != Some(true))
        .filter_map(|point| {
            Some((
                point.get("timestamp")?.as_u64()?,
                point.get("value")?.as_f64()? * scale,
            ))
        })
        .collect();
    points.sort_by_key(|point| point.0);
    points
}

fn parse_overview(totals: &Value, series: &Value, span_ms: u64, step_seconds: u64) -> Overview {
    let row = totals.pointer("/data/0").and_then(Value::as_array);
    let cell = |index: usize| number(row.and_then(|row| row.get(index)));
    let calls = cell(0).max(0.0) as u64;
    let errors = cell(1).max(0.0) as u64;
    let per_step = 60.0 / step_seconds as f64;
    Overview {
        calls,
        errors,
        error_rate: if calls == 0 {
            0.0
        } else {
            errors as f64 / calls as f64
        },
        per_minute: calls as f64 / (span_ms.max(1) as f64 / 60_000.0),
        p99_ms: cell(2) / 1_000_000.0,
        p50_ms: cell(3) / 1_000_000.0,
        requests: points(series, 0, per_step),
        failures: points(series, 1, per_step),
        p99: points(series, 2, 1.0 / 1_000_000.0),
    }
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub name: String,
    pub calls: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p99_ms: f64,
    pub p50_ms: f64,
}

/// What a service is asked to do, one row per entry-span name, busiest first.
pub async fn operations(data_dir: &Path, request: ServiceRequest) -> SignozResult<Vec<Operation>> {
    let spec = json!({
        "signal": "traces",
        "aggregations": aggregations(),
        "groupBy": [{ "name": "name", "fieldContext": "span" }],
        "order": [{ "key": { "name": "count()" }, "direction": "desc" }],
        "limit": MAX_OPERATIONS,
    });
    let result = client::query_range(
        data_dir,
        &query::builder(
            "scalar",
            request.scope.window()?,
            query::with_filter(spec, request.expression(ENTRY_SPANS)?),
        ),
    )
    .await?;
    Ok(parse_operations(&result))
}

fn parse_operations(result: &Value) -> Vec<Operation> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let row = row.as_array()?;
            let calls = number(row.get(1)).max(0.0) as u64;
            let errors = number(row.get(2)).max(0.0) as u64;
            Some(Operation {
                name: row.first()?.as_str()?.to_string(),
                calls,
                errors,
                error_rate: if calls == 0 {
                    0.0
                } else {
                    errors as f64 / calls as f64
                },
                p99_ms: number(row.get(3)) / 1_000_000.0,
                p50_ms: number(row.get(4)) / 1_000_000.0,
            })
        })
        .collect()
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorGroup {
    /// The message with its numbers and ids blanked, so one failure is one row.
    pub pattern: String,
    /// The most frequent message behind the pattern, as written.
    pub sample: String,
    pub count: u64,
    pub first_seen: u64,
    pub last_seen: u64,
}

/// A service's error and fatal logs, grouped by what they say.
pub async fn errors(data_dir: &Path, request: ServiceRequest) -> SignozResult<Vec<ErrorGroup>> {
    let spec = json!({
        "signal": "logs",
        "aggregations": [
            { "expression": "count()" },
            { "expression": "min(timestamp)" },
            { "expression": "max(timestamp)" },
        ],
        "groupBy": [{ "name": "body", "fieldContext": "log" }],
        "order": [{ "key": { "name": "count()" }, "direction": "desc" }],
        "limit": ERROR_BODIES_READ,
    });
    let result = client::query_range(
        data_dir,
        &query::builder(
            "scalar",
            request.scope.window()?,
            query::with_filter(
                spec,
                request.expression("severity_text IN ('ERROR', 'FATAL')")?,
            ),
        ),
    )
    .await?;
    Ok(group_errors(&result))
}

fn group_errors(result: &Value) -> Vec<ErrorGroup> {
    let mut groups: Vec<ErrorGroup> = Vec::new();
    for row in result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        let Some(body) = row.first().and_then(Value::as_str) else {
            continue;
        };
        let count = number(row.get(1)).max(0.0) as u64;
        let first_seen = (number(row.get(2)) / 1_000_000.0) as u64;
        let last_seen = (number(row.get(3)) / 1_000_000.0) as u64;
        let pattern = pattern_of(body);
        match groups.iter_mut().find(|group| group.pattern == pattern) {
            Some(group) => {
                group.count += count;
                group.first_seen = group.first_seen.min(first_seen);
                group.last_seen = group.last_seen.max(last_seen);
            }
            None => groups.push(ErrorGroup {
                pattern,
                sample: body.to_string(),
                count,
                first_seen,
                last_seen,
            }),
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.count));
    groups.truncate(MAX_ERROR_GROUPS);
    groups
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn blank(word: &str) -> String {
    let digits = word.chars().filter(char::is_ascii_digit).count();
    if digits == 0 {
        return word.to_string();
    }
    let unit = word.trim_start_matches(|c: char| c.is_ascii_digit());
    if unit.len() < word.len() && unit.len() <= 3 && unit.chars().all(|c| c.is_ascii_alphabetic()) {
        return format!("<n>{unit}");
    }
    let hex_like = word.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if word.len() >= 8 && (hex_like || digits * 4 >= word.len()) {
        return "<id>".into();
    }
    word.to_string()
}

/// Numbers and ids blanked out: "took 2.3s" and "took 41ms" read the same.
pub fn pattern_of(body: &str) -> String {
    let body = body.lines().next().unwrap_or_default();
    let mut out = String::with_capacity(body.len());
    let mut word = String::new();
    for c in body.chars().chain(std::iter::once(' ')) {
        if is_word_char(c) {
            word.push(c);
            continue;
        }
        out.push_str(&blank(&word));
        word.clear();
        out.push(c);
    }
    out.pop();
    out.replace("<n>.<n>", "<n>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_row_per_service_and_environment() {
        let result = json!({ "data": [
            ["reel-worker", "production", 9066, 1, 1567176.99],
            ["reel-worker", "dev", 4874, 0, 0],
            ["no-env", "", 3, 0, 1000000],
            ["bad"],
        ] });
        let health = parse(&result);
        assert_eq!(health.len(), 4);
        assert_eq!(health[0].environment.as_deref(), Some("production"));
        assert!((health[0].p99_ms - 1.567_176_99).abs() < 1e-9);
        assert!((health[0].error_rate - 1.0 / 9066.0).abs() < 1e-12);
        assert_eq!(health[1].error_rate, 0.0);
        assert_eq!(health[2].environment, None);
        assert_eq!(health[3].calls, 0);
    }

    #[test]
    fn blanks_numbers_and_ids_but_keeps_words() {
        assert_eq!(pattern_of("slow query took 2.3s"), "slow query took <n>s");
        assert_eq!(pattern_of("took 41ms status=502"), "took <n>ms status=<n>");
        assert_eq!(
            pattern_of("order 3f2b9c1e-77aa-4c10-9d2e-0b1c2d3e4f50 failed"),
            "order <id> failed"
        );
        assert_eq!(
            pattern_of("user u_8812734 not found"),
            "user <id> not found"
        );
        assert_eq!(
            pattern_of("OAuth2 callback failed"),
            "OAuth2 callback failed"
        );
        assert_eq!(pattern_of("first line\nstack trace"), "first line");
    }

    #[test]
    fn groups_error_bodies_by_pattern() {
        let result = json!({ "data": [
            ["timed out after 30s", 10, 2_000_000_000u64, 9_000_000_000u64],
            ["worker poll failed", 7, 1_000_000_000u64, 5_000_000_000u64],
            ["timed out after 12s", 4, 1_000_000_000u64, 3_000_000_000u64],
        ] });
        let groups = group_errors(&result);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].pattern, "timed out after <n>s");
        assert_eq!(groups[0].sample, "timed out after 30s");
        assert_eq!(groups[0].count, 14);
        assert_eq!((groups[0].first_seen, groups[0].last_seen), (1_000, 9_000));
    }

    #[test]
    fn charts_find_each_series_by_its_index_and_leave_out_half_filled_buckets() {
        let totals = json!({ "data": [[600, 6, 2_000_000_000.0, 40_000_000.0]] });
        let series = json!({ "aggregations": [
            { "index": 1, "series": [{ "values": [{ "timestamp": 60_000, "value": 7 }] }] },
            { "index": 0, "series": [{ "values": [
                { "timestamp": 180_000, "value": 30, "partial": true },
                { "timestamp": 120_000, "value": 200 },
                { "timestamp": 60_000, "value": 100 },
            ] }] },
        ] });
        let overview = parse_overview(&totals, &series, 10 * 60_000, 120);
        assert_eq!(overview.requests, vec![(60_000, 50.0), (120_000, 100.0)]);
        assert!((overview.per_minute - 60.0).abs() < 1e-9);
        assert!((overview.error_rate - 0.01).abs() < 1e-12);
        assert!((overview.p99_ms - 2_000.0).abs() < 1e-9);
        assert_eq!(overview.failures, vec![(60_000, 3.5)]);
        assert!(overview.p99.is_empty());
    }

    #[test]
    fn a_service_page_needs_a_service() {
        assert!(ServiceRequest::default().expression(ENTRY_SPANS).is_err());
    }
}
