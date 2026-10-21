// Cloud Monitoring's alerting: alert policies with their conditions and
// where they notify, turned off or on; uptime checks; notification channels.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::GcpResult;

const API: &str = "https://monitoring.googleapis.com/v3";

fn condition(value: &Value) -> Value {
    let threshold = at(value, "/conditionThreshold");
    let absent = at(value, "/conditionAbsent");
    let query = at(value, "/conditionMonitoringQueryLanguage/query")
        .as_str()
        .or_else(|| at(value, "/conditionPrometheusQueryLanguage/query").as_str());
    let summary = if !threshold.is_null() {
        let comparison = match text(threshold, "/comparison").as_str() {
            "COMPARISON_GT" => ">",
            "COMPARISON_GE" => "≥",
            "COMPARISON_LT" => "<",
            "COMPARISON_LE" => "≤",
            "COMPARISON_EQ" => "=",
            "COMPARISON_NE" => "≠",
            _ => "?",
        };
        format!(
            "{} {comparison} {} for {}",
            metric_of(&text(threshold, "/filter")),
            at(threshold, "/thresholdValue"),
            text(threshold, "/duration")
        )
    } else if !absent.is_null() {
        format!(
            "{} absent for {}",
            metric_of(&text(absent, "/filter")),
            text(absent, "/duration")
        )
    } else {
        query.unwrap_or("custom condition").to_string()
    };
    json!({ "name": at(value, "/displayName"), "summary": summary })
}

/// The metric a filter names, such as `run.googleapis.com/request_count`.
pub fn metric_of(filter: &str) -> String {
    filter
        .split("metric.type")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap_or(filter)
        .to_string()
}

pub fn policy(value: &Value) -> Value {
    json!({
        "id": short(&text(value, "/name")),
        "name": at(value, "/displayName"),
        "enabled": at(value, "/enabled").as_bool() != Some(false),
        "conditions": at(value, "/conditions").as_array().into_iter().flatten().map(condition).collect::<Vec<_>>(),
        "combiner": at(value, "/combiner"),
        "channels": at(value, "/notificationChannels").as_array().map(|channels| channels.len()).unwrap_or(0),
        "documentation": at(value, "/documentation/content"),
    })
}

pub fn uptime_check(value: &Value) -> Value {
    let http = at(value, "/httpCheck");
    let host = text(value, "/monitoredResource/labels/host");
    let scheme = if at(http, "/useSsl").as_bool() == Some(true) {
        "https"
    } else {
        "http"
    };
    json!({
        "id": short(&text(value, "/name")),
        "name": at(value, "/displayName"),
        "target": if http.is_null() { json!(host) } else { json!(format!("{scheme}://{host}{}", text(http, "/path"))) },
        "period": at(value, "/period"),
        "timeout": at(value, "/timeout"),
        "regions": at(value, "/selectedRegions"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Alert policies, uptime checks and notification channels, together.
pub async fn alerting(input: Listing) -> GcpResult<Value> {
    let project = input.target.project()?;
    let base = format!("{API}/projects/{project}");
    let (policies, checks, channels) = futures::future::try_join3(
        client::list(
            &input.target,
            &format!("{base}/alertPolicies"),
            &[("pageSize", "500".into())],
            "alertPolicies",
            1000,
        ),
        client::list(
            &input.target,
            &format!("{base}/uptimeCheckConfigs"),
            &[("pageSize", "100".into())],
            "uptimeCheckConfigs",
            500,
        ),
        client::list(
            &input.target,
            &format!("{base}/notificationChannels"),
            &[("pageSize", "500".into())],
            "notificationChannels",
            500,
        ),
    )
    .await?;
    Ok(json!({
        "policies": policies.iter().map(policy).collect::<Vec<_>>(),
        "uptimeChecks": checks.iter().map(uptime_check).collect::<Vec<_>>(),
        "channels": channels.iter().map(|channel| json!({
            "name": at(channel, "/displayName"),
            "type": at(channel, "/type"),
            "enabled": at(channel, "/enabled").as_bool() != Some(false),
            "verified": text(channel, "/verificationStatus") != "UNVERIFIED",
        })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub struct PolicyChange {
    #[serde(flatten)]
    pub target: Target,
    pub policy: String,
    pub enabled: bool,
}

/// Turns an alert policy on or off.
pub async fn set_policy(input: PolicyChange) -> GcpResult<Value> {
    let project = input.target.project()?;
    let url = format!(
        "{API}/projects/{project}/alertPolicies/{}",
        segment("alert policy", &input.policy)?
    );
    let updated = client::patch(
        &input.target,
        &url,
        &[("updateMask", "enabled".into())],
        &json!({ "enabled": input.enabled }),
    )
    .await?;
    Ok(policy(&updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_reads_as_its_conditions_in_words() {
        let read = policy(&json!({
            "name": "projects/acme/alertPolicies/123", "displayName": "API 5xx rate", "enabled": true,
            "conditions": [{ "displayName": "5xx > 5/s", "conditionThreshold": { "filter": "resource.type = \"cloud_run_revision\" AND metric.type = \"run.googleapis.com/request_count\"", "comparison": "COMPARISON_GT", "thresholdValue": 5, "duration": "300s" } }],
            "notificationChannels": ["projects/acme/notificationChannels/9"]
        }));
        assert_eq!(read["id"], "123");
        assert_eq!(
            read["conditions"][0]["summary"],
            "run.googleapis.com/request_count > 5 for 300s"
        );
        assert_eq!(read["channels"], 1);
        assert_eq!(policy(&json!({ "enabled": false }))["enabled"], false);
    }

    #[test]
    fn an_uptime_check_names_what_it_checks() {
        let read = uptime_check(
            &json!({ "name": "projects/acme/uptimeCheckConfigs/shop", "displayName": "Shop home", "monitoredResource": { "labels": { "host": "shop.acme.dev" } }, "httpCheck": { "path": "/health", "useSsl": true }, "period": "60s" }),
        );
        assert_eq!(read["target"], "https://shop.acme.dev/health");
    }
}
