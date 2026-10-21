// Azure Monitor: what has happened to a subscription's resources lately, and the
// few numbers that say how one resource is doing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{self, at, maybe, text};
use crate::error::{AzureError, AzureResult};

const ACTIVITY_API: &str = "2015-04-01";
const METRICS_API: &str = "2019-07-01";
const MAX_EVENTS: u32 = 200;

async fn subscription_of(named: Option<&str>) -> AzureResult<String> {
    if let Some(named) = named.filter(|named| !named.is_empty()) {
        return client::segment("subscription", named);
    }
    crate::auth::status()
        .await
        .current
        .map(|subscription| subscription.id)
        .ok_or(AzureError::Unconfigured)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub at: String,
    /// What was done, such as Restart Virtual Machine.
    pub operation: String,
    /// Succeeded, Failed, Started or Accepted.
    pub status: String,
    /// Informational, Warning, Error or Critical.
    pub level: String,
    pub resource: String,
    pub group: String,
    pub kind: String,
    pub who: Option<String>,
    pub message: Option<String>,
    /// The correlation id, which ties the steps of one change together.
    pub correlation: Option<String>,
}

fn localized(value: &Value, path: &str) -> String {
    maybe(value, &format!("{path}/localizedValue"))
        .or_else(|| maybe(value, &format!("{path}/value")))
        .unwrap_or_default()
}

pub fn event_of(event: &Value) -> Option<Event> {
    let resource_id = text(event, "/resourceId");
    let at_time =
        maybe(event, "/eventTimestamp").or_else(|| maybe(event, "/submissionTimestamp"))?;
    Some(Event {
        at: at_time,
        operation: localized(event, "/operationName"),
        status: localized(event, "/status"),
        level: text(event, "/level"),
        resource: client::short(&resource_id),
        group: maybe(event, "/resourceGroupName").unwrap_or_else(|| client::group_of(&resource_id)),
        kind: text(event, "/resourceType/value").to_lowercase(),
        who: maybe(event, "/caller"),
        message: maybe(event, "/properties/statusMessage")
            .or_else(|| maybe(event, "/properties/message"))
            .or_else(|| maybe(event, "/description")),
        correlation: maybe(event, "/correlationId"),
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListActivity {
    pub subscription: Option<String>,
    pub group: Option<String>,
    /// How far back to look; default 24, at most 24 × 90 as Azure keeps.
    pub hours: Option<u32>,
    /// Only what failed.
    pub failed_only: bool,
    pub limit: Option<u32>,
}

/// The `$filter` Azure is sent, built only from values checked first.
pub fn activity_filter(since: &str, group: Option<&str>) -> AzureResult<String> {
    let mut filter = format!("eventTimestamp ge '{since}'");
    if let Some(group) = group.filter(|group| !group.is_empty()) {
        filter.push_str(&format!(
            " and resourceGroupName eq '{}'",
            client::segment("resource group", group)?
        ));
    }
    Ok(filter)
}

/// An ISO time `hours` before `now`, as Azure's filter wants it.
pub fn since(now_secs: i64, hours: u32) -> String {
    let seconds = now_secs - i64::from(hours) * 3600;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's civil_from_days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

pub async fn activity(input: ListActivity) -> AzureResult<Vec<Event>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let hours = input.hours.unwrap_or(24).clamp(1, 24 * 90);
    let filter = activity_filter(&since(now_secs(), hours), input.group.as_deref())?;
    let listed = client::list(
        &format!("/subscriptions/{subscription}/providers/Microsoft.Insights/eventtypes/management/values"),
        ACTIVITY_API,
        &[("$filter", filter)],
    )
    .await?;
    let mut events: Vec<Event> = listed.iter().filter_map(event_of).collect();
    if input.failed_only {
        events.retain(|event| {
            event.status == "Failed" || event.level == "Error" || event.level == "Critical"
        });
    }
    events.sort_by(|a, b| b.at.cmp(&a.at));
    events.truncate(input.limit.unwrap_or(MAX_EVENTS).clamp(1, MAX_EVENTS) as usize);
    Ok(events)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Metric {
    pub name: String,
    pub unit: String,
    /// One point per interval, oldest first; none where nothing was recorded.
    pub points: Vec<Option<f64>>,
    pub latest: Option<f64>,
    pub total: Option<f64>,
}

pub fn metric_of(metric: &Value) -> Option<Metric> {
    let points: Vec<Option<f64>> = at(metric, "/timeseries/0/data")
        .as_array()
        .into_iter()
        .flatten()
        .map(|point| {
            at(point, "/average")
                .as_f64()
                .or_else(|| at(point, "/total").as_f64())
                .or_else(|| at(point, "/count").as_f64())
                .or_else(|| at(point, "/maximum").as_f64())
        })
        .collect();
    let seen: Vec<f64> = points.iter().flatten().copied().collect();
    Some(Metric {
        name: localized(metric, "/name"),
        unit: text(metric, "/unit"),
        latest: seen.last().copied(),
        total: (!seen.is_empty()).then(|| seen.iter().sum()),
        points,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsFor {
    /// The resource's full id, as the lists give it.
    pub id: String,
    /// Metric names, such as `Percentage CPU`.
    pub names: Vec<String>,
    /// How far back to look, in hours; default 3.
    #[serde(default)]
    pub hours: Option<u32>,
    /// PT1M, PT5M, PT1H and the like; default PT5M.
    #[serde(default)]
    pub interval: Option<String>,
}

/// A resource id is used as a path, so it must be one Azure itself gave.
pub fn resource_path(id: &str) -> AzureResult<&str> {
    let id = id.trim();
    let ok = id.starts_with("/subscriptions/")
        && !id.contains("..")
        && !id.contains('?')
        && !id.contains('#')
        && id.len() <= 1024
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '(' | ')'));
    if ok {
        Ok(id)
    } else {
        Err(AzureError::BadArg(format!("`{id}` is not a resource id")))
    }
}

pub async fn metrics(input: MetricsFor) -> AzureResult<Vec<Metric>> {
    let id = resource_path(&input.id)?;
    if input.names.is_empty() {
        return Err(AzureError::BadArg("no metric was named".into()));
    }
    for name in &input.names {
        if name.len() > 60 || name.contains(',') || name.contains('&') {
            return Err(AzureError::BadArg(format!("`{name}` is not a metric name")));
        }
    }
    let hours = input.hours.unwrap_or(3).clamp(1, 24 * 30);
    let interval = match input.interval.as_deref().unwrap_or("PT5M") {
        interval @ ("PT1M" | "PT5M" | "PT15M" | "PT30M" | "PT1H" | "PT6H" | "P1D") => interval,
        other => return Err(AzureError::BadArg(format!("`{other}` is not an interval"))),
    };
    let now = now_secs();
    let answer = client::send(
        reqwest::Method::GET,
        &client::url(
            &format!("{id}/providers/Microsoft.Insights/metrics"),
            METRICS_API,
            &[
                ("metricnames", input.names.join(",")),
                (
                    "timespan",
                    format!("{}/{}", since(now, hours), since(now, 0)),
                ),
                ("interval", interval.to_string()),
            ],
        ),
        None,
    )
    .await?;
    Ok(at(&answer, "/value")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(metric_of)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_event_reads_what_was_done_and_whether_it_worked() {
        let event = event_of(&json!({
            "eventTimestamp": "2026-10-19T12:30:00.1234Z",
            "operationName": { "value": "Microsoft.Compute/virtualMachines/restart/action", "localizedValue": "Restart Virtual Machine" },
            "status": { "value": "Succeeded", "localizedValue": "Succeeded" },
            "level": "Informational",
            "resourceId": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Compute/virtualMachines/nine-mills",
            "resourceGroupName": "nine-mills",
            "resourceType": { "value": "Microsoft.Compute/virtualMachines" },
            "caller": "ankit@acme.dev",
            "correlationId": "c-1",
            "properties": { "statusMessage": "" }
        }))
        .expect("reads");
        assert_eq!(event.operation, "Restart Virtual Machine");
        assert_eq!(event.resource, "nine-mills");
        assert_eq!(event.kind, "microsoft.compute/virtualmachines");
        assert_eq!(event.who.as_deref(), Some("ankit@acme.dev"));
        assert!(event_of(&json!({ "resourceId": "/x" })).is_none());
    }

    #[test]
    fn the_window_is_written_as_azure_wants_it() {
        let noon = 1_792_411_200;
        assert_eq!(since(noon, 0), "2026-10-19T12:00:00Z");
        assert_eq!(since(noon, 24), "2026-10-18T12:00:00Z");
        assert_eq!(since(noon, 24 * 30), "2026-09-19T12:00:00Z");
        assert_eq!(
            activity_filter("2026-10-18T12:00:00Z", Some("nine-mills")).ok(),
            Some(
                "eventTimestamp ge '2026-10-18T12:00:00Z' and resourceGroupName eq 'nine-mills'"
                    .to_string()
            )
        );
        assert!(activity_filter("2026-10-18T12:00:00Z", Some("a' or '1'=='1")).is_err());
    }

    #[test]
    fn a_metric_reads_its_points_with_gaps_kept() {
        let metric = metric_of(&json!({
            "name": { "localizedValue": "Percentage CPU" }, "unit": "Percent",
            "timeseries": [{ "data": [{ "average": 2.5 }, { "timeStamp": "x" }, { "average": 7.5 }] }]
        }))
        .expect("reads");
        assert_eq!(metric.name, "Percentage CPU");
        assert_eq!(metric.points, [Some(2.5), None, Some(7.5)]);
        assert_eq!(metric.latest, Some(7.5));
        assert_eq!(metric.total, Some(10.0));
    }

    #[test]
    fn only_a_resource_id_azure_gave_is_used_as_a_path() {
        assert!(resource_path(
            "/subscriptions/s1/resourceGroups/g/providers/Microsoft.Compute/virtualMachines/vm"
        )
        .is_ok());
        assert!(resource_path("/subscriptions/s1/../../other").is_err());
        assert!(resource_path("https://evil.example/x").is_err());
        assert!(resource_path("/subscriptions/s1/x?a=b").is_err());
    }
}
