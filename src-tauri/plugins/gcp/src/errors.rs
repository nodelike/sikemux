// Error Reporting: the errors a project's services throw, grouped by where
// they come from, with how often and for how many users, an hour-by-hour
// timeline, recent occurrences, and marking a group resolved, acknowledged or muted.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://clouderrorreporting.googleapis.com/v1beta1";
const STATUSES: [&str; 4] = ["OPEN", "ACKNOWLEDGED", "RESOLVED", "MUTED"];

/// How far back a listing looks, as Error Reporting names its periods.
pub fn period_of(since: Option<&str>) -> GcpResult<(&'static str, &'static str)> {
    Ok(match since.map(str::trim).unwrap_or("1d") {
        "1h" => ("PERIOD_1_HOUR", "300s"),
        "6h" => ("PERIOD_6_HOURS", "1800s"),
        "1d" | "" => ("PERIOD_1_DAY", "3600s"),
        "7d" => ("PERIOD_1_WEEK", "21600s"),
        "30d" => ("PERIOD_30_DAYS", "86400s"),
        other => {
            return Err(GcpError::BadArg(format!(
                "`{other}` is not a period; use 1h, 6h, 1d, 7d or 30d"
            )))
        }
    })
}

fn representative(value: &Value) -> Value {
    let location = at(value, "/context/reportLocation");
    json!({
        "message": at(value, "/message"),
        "at": at(value, "/eventTime"),
        "service": at(value, "/serviceContext/service"),
        "version": at(value, "/serviceContext/version"),
        "file": at(location, "/filePath"),
        "line": at(location, "/lineNumber"),
        "function": at(location, "/functionName"),
        "request": if at(value, "/context/httpRequest").is_null() { Value::Null } else { json!({
            "method": at(value, "/context/httpRequest/method"),
            "url": at(value, "/context/httpRequest/url"),
            "status": at(value, "/context/httpRequest/responseStatusCode"),
        }) },
        "user": at(value, "/context/user"),
    })
}

/// The first line of an error's message, which names it.
fn headline(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(300)
        .collect()
}

/// An error group as the pane and agents read it.
pub fn group(value: &Value) -> Value {
    let first = representative(at(value, "/representative"));
    let message = text(&first, "/message");
    json!({
        "id": at(value, "/group/groupId"),
        "title": headline(&message),
        "status": value.pointer("/group/resolutionStatus").and_then(Value::as_str).unwrap_or("OPEN"),
        "count": at(value, "/count").as_str().and_then(|count| count.parse::<u64>().ok()).or_else(|| at(value, "/count").as_u64()),
        "users": at(value, "/affectedUsersCount").as_str().and_then(|count| count.parse::<u64>().ok()),
        "firstSeen": at(value, "/firstSeenTime"),
        "lastSeen": at(value, "/lastSeenTime"),
        "services": at(value, "/affectedServices").as_array().into_iter().flatten().map(|service| text(service, "/service")).filter(|service| !service.is_empty()).collect::<std::collections::BTreeSet<_>>(),
        "timeline": at(value, "/timedCounts").as_array().into_iter().flatten().map(|count| at(count, "/count").as_str().and_then(|c| c.parse::<u64>().ok()).unwrap_or(0)).collect::<Vec<_>>(),
        "issue": at(value, "/group/trackingIssues/0/url"),
        "sample": first,
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    /// 1h, 6h, 1d, 7d or 30d. Default 1d.
    #[serde(default)]
    pub since: Option<String>,
    /// Only one service's errors.
    #[serde(default)]
    pub service: Option<String>,
}

/// The project's error groups in a period, the most frequent first.
pub async fn groups(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let (period, step) = period_of(input.since.as_deref())?;
    let mut query = vec![
        ("timeRange.period", period.to_string()),
        ("timedCountDuration", step.to_string()),
        ("order", "COUNT_DESC".to_string()),
        ("pageSize", "100".to_string()),
    ];
    if let Some(service) = input
        .service
        .as_deref()
        .map(str::trim)
        .filter(|service| !service.is_empty())
    {
        query.push(("serviceFilter.service", service.to_string()));
    }
    let page = client::get(
        &input.target,
        &format!("{API}/projects/{project}/groupStats"),
        &query,
    )
    .await?;
    Ok(at(&page, "/errorGroupStats")
        .as_array()
        .into_iter()
        .flatten()
        .map(group)
        .collect())
}

#[derive(Deserialize)]
pub struct GroupRef {
    #[serde(flatten)]
    pub target: Target,
    pub group: String,
}

/// A group's recent occurrences, newest first.
pub async fn events(input: GroupRef) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let query = [
        ("groupId", segment("error group", &input.group)?),
        ("timeRange.period", "PERIOD_30_DAYS".to_string()),
        ("pageSize", "20".to_string()),
    ];
    let page = client::get(
        &input.target,
        &format!("{API}/projects/{project}/events"),
        &query,
    )
    .await?;
    Ok(at(&page, "/errorEvents")
        .as_array()
        .into_iter()
        .flatten()
        .map(representative)
        .collect())
}

#[derive(Deserialize)]
pub struct StatusChange {
    #[serde(flatten)]
    pub group: GroupRef,
    pub status: String,
}

/// Marks a group open, acknowledged, resolved or muted.
pub async fn set_status(input: StatusChange) -> GcpResult<Value> {
    let status = input.status.trim().to_ascii_uppercase();
    if !STATUSES.contains(&status.as_str()) {
        return Err(GcpError::BadArg(format!(
            "`{status}` is not a status; use {}",
            STATUSES.join(", ")
        )));
    }
    let project = input.group.target.project()?;
    let id = segment("error group", &input.group.group)?;
    let name = format!("projects/{project}/groups/{id}");
    let updated = client::put(
        &input.group.target,
        &format!("{API}/{name}"),
        &json!({ "name": name, "groupId": id, "resolutionStatus": status }),
    )
    .await?;
    Ok(json!({ "id": id, "status": at(&updated, "/resolutionStatus") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_read_as_error_reportings_own() {
        assert_eq!(period_of(None).ok(), Some(("PERIOD_1_DAY", "3600s")));
        assert_eq!(
            period_of(Some("7d")).ok(),
            Some(("PERIOD_1_WEEK", "21600s"))
        );
        assert!(period_of(Some("2h")).is_err());
    }

    #[test]
    fn a_group_reads_with_its_headline_counts_and_where_it_was_thrown() {
        let read = group(&json!({
            "group": { "groupId": "CJ3kq", "resolutionStatus": "ACKNOWLEDGED", "trackingIssues": [{ "url": "https://github.com/acme/api/issues/9" }] },
            "count": "1290", "affectedUsersCount": "41", "firstSeenTime": "t1", "lastSeenTime": "t2",
            "affectedServices": [{ "service": "api" }, { "service": "api" }, { "service": "worker" }],
            "timedCounts": [{ "count": "3" }, { "count": "12" }],
            "representative": { "message": "TypeError: Cannot read properties of undefined (reading 'taxRate')\n    at Totals (totals.js:42:18)", "serviceContext": { "service": "api", "version": "1.18.2" },
                "context": { "reportLocation": { "filePath": "totals.js", "lineNumber": 42, "functionName": "Totals" }, "httpRequest": { "method": "POST", "url": "/checkout", "responseStatusCode": 500 } } }
        }));
        assert_eq!(
            read["title"],
            "TypeError: Cannot read properties of undefined (reading 'taxRate')"
        );
        assert_eq!(
            (read["count"].as_u64(), read["users"].as_u64()),
            (Some(1290), Some(41))
        );
        assert_eq!(read["services"], json!(["api", "worker"]));
        assert_eq!(read["timeline"], json!([3, 12]));
        assert_eq!(read["sample"]["line"], 42);
        assert_eq!(read["sample"]["request"]["status"], 500);
        assert_eq!(read["status"], "ACKNOWLEDGED");
        assert_eq!(group(&json!({}))["status"], "OPEN");
    }
}
