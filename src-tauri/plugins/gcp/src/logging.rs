// Cloud Logging: searching a project's logs by resource, severity, time and
// words, and following them as they arrive.

use std::collections::VecDeque;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::StreamSink;

use crate::auth::Target;
use crate::client::{self, at, text};
use crate::error::{GcpError, GcpResult};

const ENTRIES: &str = "https://logging.googleapis.com/v2/entries:list";
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;
const TAIL_EVERY: Duration = Duration::from_secs(3);
const TAIL_FIRST: usize = 100;
const REMEMBERED_IDS: usize = 2000;

const SEVERITIES: [&str; 9] = [
    "DEFAULT",
    "DEBUG",
    "INFO",
    "NOTICE",
    "WARNING",
    "ERROR",
    "CRITICAL",
    "ALERT",
    "EMERGENCY",
];

/// What the logs are of, so a person can ask for "this service's logs" without writing a filter.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Resource {
    RunService,
    RunJob,
    Instance,
    GkeCluster,
    Function,
    Build,
    Sql,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(flatten)]
    pub target: Target,
    /// A filter in Logging's own language, used as it is beside the others.
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub resource: Option<Resource>,
    /// The resource's name: a service, job, instance, cluster, function, build id or SQL instance.
    #[serde(default)]
    pub name: Option<String>,
    /// The lowest severity shown, such as WARNING.
    #[serde(default)]
    pub severity: Option<String>,
    /// How far back, such as 15m, 6h or 2d. Default 1h.
    #[serde(default)]
    pub since: Option<String>,
    /// Words to find anywhere in an entry.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A span such as `15m`, `6h` or `2d`, in seconds.
pub fn seconds(span: &str) -> GcpResult<u64> {
    let span = span.trim();
    let split = span
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(span.len());
    let (count, unit) = span.split_at(split);
    let count: u64 = count
        .parse()
        .map_err(|_| GcpError::BadArg(format!("`{span}` is not a span such as 15m, 6h or 2d")))?;
    let unit = match unit {
        "s" => 1,
        "m" | "" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(GcpError::BadArg(format!(
                "`{span}` is not a span such as 15m, 6h or 2d"
            )))
        }
    };
    Ok(count.saturating_mul(unit))
}

/// An RFC 3339 time `seconds` before now, in UTC.
pub fn rfc3339_before(now_secs: u64, seconds: u64) -> String {
    let at = now_secs.saturating_sub(seconds);
    let days = at / 86_400;
    let rest = at % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// A day count since 1970-01-01 as a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn resource_filter(resource: Resource, name: Option<&str>) -> String {
    let (kind, label) = match resource {
        Resource::RunService => ("cloud_run_revision", "service_name"),
        Resource::RunJob => ("cloud_run_job", "job_name"),
        Resource::Instance => ("gce_instance", "instance_id"),
        Resource::GkeCluster => ("k8s_container", "cluster_name"),
        Resource::Function => ("cloud_function", "function_name"),
        Resource::Build => ("build", "build_id"),
        Resource::Sql => ("cloudsql_database", "database_id"),
    };
    let mut filter = format!("resource.type={}", quoted(kind));
    if let Some(name) = name {
        if resource == Resource::Instance {
            filter.push_str(&format!(" AND (resource.labels.instance_id={0} OR labels.\"compute.googleapis.com/resource_name\"={0})", quoted(name)));
        } else if resource == Resource::Sql {
            filter.push_str(&format!(
                " AND resource.labels.database_id:{}",
                quoted(name)
            ));
        } else {
            filter.push_str(&format!(" AND resource.labels.{label}={}", quoted(name)));
        }
    }
    filter
}

/// The filter a query stands for, in Logging's own language.
pub fn filter_of(query: &Query, since_secs: u64, now: u64) -> GcpResult<String> {
    let mut parts = vec![format!(
        "timestamp>={}",
        quoted(&rfc3339_before(now, since_secs))
    )];
    if let Some(resource) = query.resource {
        let name = query
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        parts.push(resource_filter(resource, name));
    }
    if let Some(severity) = query
        .severity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let severity = severity.to_ascii_uppercase();
        if !SEVERITIES.contains(&severity.as_str()) {
            return Err(GcpError::BadArg(format!(
                "`{severity}` is not a severity; use one of {}",
                SEVERITIES.join(", ")
            )));
        }
        parts.push(format!("severity>={severity}"));
    }
    if let Some(words) = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|words| !words.is_empty())
    {
        parts.push(quoted(words));
    }
    if let Some(filter) = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|filter| !filter.is_empty())
    {
        parts.push(format!("({filter})"));
    }
    Ok(parts.join(" AND "))
}

/// The part of an entry a person reads first: its text, its JSON message, or what an audit entry did.
fn summary(entry: &Value) -> String {
    if let Some(text) = entry.get("textPayload").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(payload) = entry.get("jsonPayload") {
        for key in ["message", "msg", "textPayload", "error", "event"] {
            if let Some(text) = payload.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        return payload.to_string();
    }
    if let Some(payload) = entry.get("protoPayload") {
        let method = text(payload, "/methodName");
        let who = text(payload, "/authenticationInfo/principalEmail");
        let status = text(payload, "/status/message");
        return [method, who, status]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
    }
    let http = at(entry, "/httpRequest");
    if !http.is_null() {
        return format!(
            "{} {} {}",
            text(http, "/requestMethod"),
            at(http, "/status"),
            text(http, "/requestUrl")
        );
    }
    String::new()
}

/// A log entry as the pane and agents read it.
pub fn entry(value: &Value) -> Value {
    let log = text(value, "/logName");
    let log = log
        .rsplit("/logs/")
        .next()
        .unwrap_or(&log)
        .replace("%2F", "/");
    let http = at(value, "/httpRequest");
    json!({
        "id": at(value, "/insertId"),
        "at": at(value, "/timestamp"),
        "severity": value.get("severity").and_then(Value::as_str).unwrap_or("DEFAULT"),
        "text": summary(value),
        "log": log,
        "resource": at(value, "/resource/type"),
        "labels": at(value, "/resource/labels"),
        "http": if http.is_null() { Value::Null } else { json!({
            "method": at(http, "/requestMethod"),
            "url": at(http, "/requestUrl"),
            "status": at(http, "/status"),
            "latency": at(http, "/latency"),
        }) },
        "json": at(value, "/jsonPayload"),
        "trace": at(value, "/trace"),
    })
}

async fn fetch(
    target: &Target,
    project: &str,
    filter: &str,
    order: &str,
    limit: usize,
) -> GcpResult<Vec<Value>> {
    let body = json!({
        "resourceNames": [format!("projects/{project}")],
        "filter": filter,
        "orderBy": format!("timestamp {order}"),
        "pageSize": limit,
    });
    let page = client::post(target, ENTRIES, &body).await?;
    Ok(at(&page, "/entries")
        .as_array()
        .cloned()
        .unwrap_or_default())
}

/// The newest entries a query finds, newest first.
pub async fn search(query: Query) -> GcpResult<Vec<Value>> {
    let project = query.target.project()?;
    let since = seconds(query.since.as_deref().unwrap_or("1h"))?;
    let filter = filter_of(&query, since, now_secs())?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let entries = fetch(&query.target, &project, &filter, "desc", limit).await?;
    Ok(entries.iter().map(entry).collect())
}

/// Sends the query's latest entries, then each new one as it arrives, until the stream is closed.
pub async fn tail(query: Query, sink: StreamSink) -> GcpResult<()> {
    let project = query.target.project()?;
    let since = seconds(query.since.as_deref().unwrap_or("15m"))?;
    let mut seen: VecDeque<String> = VecDeque::new();
    let mut newest = String::new();
    let mut first = true;
    loop {
        let filter = if newest.is_empty() {
            filter_of(&query, since, now_secs())?
        } else {
            format!(
                "{} AND timestamp>={}",
                filter_of(&query, since, now_secs())?,
                quoted(&newest)
            )
        };
        let mut entries = fetch(
            &query.target,
            &project,
            &filter,
            "desc",
            if first { TAIL_FIRST } else { MAX_LIMIT },
        )
        .await?;
        entries.reverse();
        let fresh: Vec<Value> = entries
            .iter()
            .filter(|raw| {
                let id = text(raw, "/insertId");
                id.is_empty() || !seen.contains(&id)
            })
            .map(entry)
            .collect();
        for raw in &entries {
            let id = text(raw, "/insertId");
            if !id.is_empty() {
                seen.push_back(id);
            }
            let at = text(raw, "/timestamp");
            if at > newest {
                newest = at;
            }
        }
        while seen.len() > REMEMBERED_IDS {
            seen.pop_front();
        }
        if (!fresh.is_empty() || first) && sink.send(json!(fresh)).is_err() {
            return Ok(());
        }
        first = false;
        tokio::time::sleep(TAIL_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_read_as_seconds() {
        assert_eq!(seconds("15m").ok(), Some(900));
        assert_eq!(seconds("6h").ok(), Some(21_600));
        assert_eq!(seconds("2d").ok(), Some(172_800));
        assert_eq!(seconds("30").ok(), Some(1800));
        assert!(seconds("soon").is_err());
        assert!(seconds("5w").is_err());
    }

    #[test]
    fn times_are_utc_rfc3339() {
        assert_eq!(rfc3339_before(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_before(1_729_252_800, 3600), "2024-10-18T11:00:00Z");
        assert_eq!(rfc3339_before(951_782_400, 0), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn a_query_becomes_a_logging_filter() {
        let query = Query {
            resource: Some(Resource::RunService),
            name: Some("api".into()),
            severity: Some("warning".into()),
            text: Some("timeout \"db\"".into()),
            filter: Some("httpRequest.status>=500".into()),
            ..Query::default()
        };
        let filter = filter_of(&query, 3600, 1_729_252_800).expect("a filter");
        assert_eq!(
            filter,
            "timestamp>=\"2024-10-18T11:00:00Z\" AND resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"api\" AND severity>=WARNING AND \"timeout \\\"db\\\"\" AND (httpRequest.status>=500)"
        );
        let bad = Query {
            severity: Some("loud".into()),
            ..Query::default()
        };
        assert!(filter_of(&bad, 60, 0).is_err());
    }

    #[test]
    fn an_entry_reads_as_its_message() {
        let text_entry = entry(
            &json!({ "insertId": "1", "timestamp": "t", "severity": "ERROR", "textPayload": "boom", "logName": "projects/p/logs/run.googleapis.com%2Fstderr", "resource": { "type": "cloud_run_revision", "labels": { "service_name": "api" } } }),
        );
        assert_eq!(text_entry["text"], "boom");
        assert_eq!(text_entry["log"], "run.googleapis.com/stderr");
        let json_entry = entry(&json!({ "jsonPayload": { "message": "slow query", "ms": 900 } }));
        assert_eq!(json_entry["text"], "slow query");
        assert_eq!(json_entry["severity"], "DEFAULT");
        let audit = entry(
            &json!({ "protoPayload": { "methodName": "google.cloud.run.v2.Services.UpdateService", "authenticationInfo": { "principalEmail": "ankit@example.com" } } }),
        );
        assert_eq!(
            audit["text"],
            "google.cloud.run.v2.Services.UpdateService · ankit@example.com"
        );
        let request = entry(
            &json!({ "httpRequest": { "requestMethod": "GET", "status": 503, "requestUrl": "/api" } }),
        );
        assert_eq!(request["text"], "GET 503 /api");
        assert_eq!(request["http"]["status"], 503);
    }
}
