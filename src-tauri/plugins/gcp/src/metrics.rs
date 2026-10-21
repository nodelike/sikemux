// Cloud Monitoring: a few charts per resource, aligned into even steps, for
// the console's sparklines and for an agent asking how a service is doing.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, text};
use crate::error::{GcpError, GcpResult};
use crate::logging::{rfc3339_before, seconds};

const API: &str = "https://monitoring.googleapis.com/v3";
const MAX_POINTS: u64 = 120;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Chart {
    RunRequests,
    RunErrors,
    RunLatency,
    RunInstances,
    InstanceCpu,
    SqlCpu,
    SqlMemory,
    SqlConnections,
    SubscriptionBacklog,
    SubscriptionOldest,
    FunctionExecutions,
}

struct Spec {
    metric: &'static str,
    resource: &'static str,
    label: &'static str,
    extra: &'static str,
    aligner: &'static str,
    reducer: &'static str,
    unit: &'static str,
    scale: f64,
}

fn spec(chart: Chart) -> Spec {
    let spec = |metric, resource, label, aligner, reducer, unit| Spec {
        metric,
        resource,
        label,
        extra: "",
        aligner,
        reducer,
        unit,
        scale: 1.0,
    };
    match chart {
        Chart::RunRequests => spec(
            "run.googleapis.com/request_count",
            "cloud_run_revision",
            "service_name",
            "ALIGN_RATE",
            "REDUCE_SUM",
            "req/s",
        ),
        Chart::RunErrors => Spec {
            extra: " AND metric.labels.response_code_class=\"5xx\"",
            ..spec(
                "run.googleapis.com/request_count",
                "cloud_run_revision",
                "service_name",
                "ALIGN_RATE",
                "REDUCE_SUM",
                "req/s",
            )
        },
        Chart::RunLatency => spec(
            "run.googleapis.com/request_latencies",
            "cloud_run_revision",
            "service_name",
            "ALIGN_DELTA",
            "REDUCE_PERCENTILE_95",
            "ms",
        ),
        Chart::RunInstances => spec(
            "run.googleapis.com/container/instance_count",
            "cloud_run_revision",
            "service_name",
            "ALIGN_MAX",
            "REDUCE_SUM",
            "instances",
        ),
        Chart::InstanceCpu => Spec {
            scale: 100.0,
            ..spec(
                "compute.googleapis.com/instance/cpu/utilization",
                "gce_instance",
                "instance_id",
                "ALIGN_MEAN",
                "REDUCE_MEAN",
                "%",
            )
        },
        Chart::SqlCpu => Spec {
            scale: 100.0,
            ..spec(
                "cloudsql.googleapis.com/database/cpu/utilization",
                "cloudsql_database",
                "database_id",
                "ALIGN_MEAN",
                "REDUCE_MEAN",
                "%",
            )
        },
        Chart::SqlMemory => Spec {
            scale: 100.0,
            ..spec(
                "cloudsql.googleapis.com/database/memory/utilization",
                "cloudsql_database",
                "database_id",
                "ALIGN_MEAN",
                "REDUCE_MEAN",
                "%",
            )
        },
        Chart::SqlConnections => spec(
            "cloudsql.googleapis.com/database/network/connections",
            "cloudsql_database",
            "database_id",
            "ALIGN_MEAN",
            "REDUCE_SUM",
            "connections",
        ),
        Chart::SubscriptionBacklog => spec(
            "pubsub.googleapis.com/subscription/num_undelivered_messages",
            "pubsub_subscription",
            "subscription_id",
            "ALIGN_MAX",
            "REDUCE_SUM",
            "messages",
        ),
        Chart::SubscriptionOldest => spec(
            "pubsub.googleapis.com/subscription/oldest_unacked_message_age",
            "pubsub_subscription",
            "subscription_id",
            "ALIGN_MAX",
            "REDUCE_MAX",
            "s",
        ),
        Chart::FunctionExecutions => spec(
            "cloudfunctions.googleapis.com/function/execution_count",
            "cloud_function",
            "function_name",
            "ALIGN_RATE",
            "REDUCE_SUM",
            "calls/s",
        ),
    }
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The resource label a chart's name is matched against; Cloud SQL's database id is `project:instance`.
fn label_value(chart: Chart, project: &str, name: &str) -> String {
    match chart {
        Chart::SqlCpu | Chart::SqlMemory | Chart::SqlConnections => format!("{project}:{name}"),
        _ => name.to_string(),
    }
}

/// The filter a chart stands for, in Monitoring's own language.
fn filter_of(chart: Chart, project: &str, name: &str) -> String {
    let spec = spec(chart);
    format!(
        "metric.type={} AND resource.type={} AND resource.labels.{}={}{}",
        quoted(spec.metric),
        quoted(spec.resource),
        spec.label,
        quoted(&label_value(chart, project, name)),
        spec.extra
    )
}

/// A step that fits the span into at most 120 points, at least a minute.
pub fn step_for(span_secs: u64) -> u64 {
    (span_secs / MAX_POINTS).max(60).div_ceil(60) * 60
}

fn number(value: &Value) -> Option<f64> {
    value
        .get("doubleValue")
        .and_then(Value::as_f64)
        .or_else(|| {
            value.get("int64Value").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| v.as_f64())
            })
        })
        .or_else(|| {
            value
                .pointer("/distributionValue/mean")
                .and_then(Value::as_f64)
        })
}

/// The points of the series Monitoring answered with, oldest first.
pub fn points_of(answer: &Value, scale: f64) -> Vec<Value> {
    let mut points: Vec<(String, f64)> = at(answer, "/timeSeries")
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|series| {
            at(series, "/points")
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|point| {
            Some((
                text(&point, "/interval/endTime"),
                number(at(&point, "/value"))? * scale,
            ))
        })
        .collect();
    points.sort_by(|a, b| a.0.cmp(&b.0));
    points
        .into_iter()
        .map(|(at, value)| json!({ "at": at, "value": (value * 1000.0).round() / 1000.0 }))
        .collect()
}

#[derive(Deserialize)]
pub struct Query {
    #[serde(flatten)]
    pub target: Target,
    pub chart: Chart,
    /// The service, instance id, SQL instance, subscription or function charted.
    pub name: String,
    /// How far back, such as 1h, 6h or 7d. Default 6h.
    #[serde(default)]
    pub since: Option<String>,
}

pub async fn chart(input: Query) -> GcpResult<Value> {
    let project = input.target.project()?;
    let name = input.name.trim();
    if name.is_empty() {
        return Err(GcpError::BadArg("no resource was named".into()));
    }
    let span = seconds(input.since.as_deref().unwrap_or("6h"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|e| e.as_secs())
        .unwrap_or(0);
    let spec = spec(input.chart);
    let step = step_for(span);
    let query = vec![
        ("filter", filter_of(input.chart, &project, name)),
        ("interval.startTime", rfc3339_before(now, span)),
        ("interval.endTime", rfc3339_before(now, 0)),
        ("aggregation.alignmentPeriod", format!("{step}s")),
        ("aggregation.perSeriesAligner", spec.aligner.to_string()),
        ("aggregation.crossSeriesReducer", spec.reducer.to_string()),
    ];
    let answer = client::get(
        &input.target,
        &format!("{API}/projects/{project}/timeSeries"),
        &query,
    )
    .await?;
    Ok(json!({ "unit": spec.unit, "step": step, "points": points_of(&answer, spec.scale) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chart_becomes_a_monitoring_filter() {
        assert_eq!(
            filter_of(Chart::RunErrors, "acme", "api"),
            "metric.type=\"run.googleapis.com/request_count\" AND resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"api\" AND metric.labels.response_code_class=\"5xx\""
        );
        assert!(filter_of(Chart::SqlCpu, "acme", "orders")
            .ends_with("resource.labels.database_id=\"acme:orders\""));
    }

    #[test]
    fn steps_fit_the_span() {
        assert_eq!(step_for(3600), 60);
        assert_eq!(step_for(6 * 3600), 180);
        assert_eq!(step_for(7 * 86_400), 5040);
    }

    #[test]
    fn points_come_oldest_first_and_scaled() {
        let answer = json!({ "timeSeries": [{ "points": [
            { "interval": { "endTime": "2024-10-18T11:02:00Z" }, "value": { "doubleValue": 0.4567 } },
            { "interval": { "endTime": "2024-10-18T11:01:00Z" }, "value": { "int64Value": "2" } },
            { "interval": { "endTime": "2024-10-18T11:03:00Z" }, "value": { "distributionValue": { "mean": 120.5 } } }
        ] }] });
        let points = points_of(&answer, 100.0);
        assert_eq!(points[0]["value"], 200.0);
        assert_eq!(points[1]["value"], 45.67);
        assert_eq!(points[2]["at"], "2024-10-18T11:03:00Z");
    }
}
