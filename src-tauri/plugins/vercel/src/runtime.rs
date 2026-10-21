// A project's recent requests and what its functions logged while answering them,
// newest first, read from Vercel's observability API as `vercel logs` does. Only
// team projects keep them, and a Hobby team keeps function logs for a short while.

use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::segment;

const EVENTS: &str = "/v1/observability/events";
const REQUESTS: &str = "vercel.proxy_response";
const FUNCTION_LOGS: &str = "vercel.log";
/// Vercel records a request's background revalidation as a request of its own; `vercel logs` leaves them out too.
const BACKGROUND: &str = "NOT pathType:\"background_func\"";
const REASON: (&str, &str) = (
    "x-vercel-observability-query-reason",
    "sikemux/runtime-logs",
);
const DEFAULT_MINUTES: u64 = 15;
/// The furthest back a Hobby team's function logs can be read.
const MAX_MINUTES: u64 = 30;
const DEFAULT_LIMIT: u64 = 200;
const MAX_LIMIT: u64 = 1000;
/// A message longer than this is cut, so one runaway line can't fill the pane.
const MAX_MESSAGE_CHARS: usize = 4000;

const REQUEST_FIELDS: &[&str] = &[
    "requestId",
    "deploymentId",
    "requestHostname",
    "requestPath",
    "requestMethod",
    "httpStatus",
    "route",
    "pathType",
    "cacheResult",
];
const LOG_FIELDS: &[&str] = &[
    "requestId",
    "deploymentId",
    "requestPath",
    "requestMethod",
    "source",
    "level",
    "message",
    "truncated",
];

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// When Vercel saw it, as RFC 3339.
    pub at: String,
    /// "request" for a request Vercel answered, "log" for a line a function wrote.
    pub kind: &'static str,
    /// info, warning or error; a request reads as error from 500 and warning from 400.
    pub level: String,
    pub message: String,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status: Option<u16>,
    pub request_id: Option<String>,
    pub deployment_id: Option<String>,
}

fn some_text(value: &Value, path: &str) -> Option<String> {
    Some(text(value, path)).filter(|text| !text.is_empty())
}

fn cut(message: String) -> String {
    if message.chars().count() > MAX_MESSAGE_CHARS {
        message.chars().take(MAX_MESSAGE_CHARS).collect::<String>() + " …"
    } else {
        message
    }
}

pub fn request_of(event: &Value) -> LogLine {
    let status = at(event, "/httpStatus")
        .as_u64()
        .and_then(|status| u16::try_from(status).ok());
    let level = match status {
        Some(code) if code >= 500 => "error",
        Some(code) if code >= 400 => "warning",
        _ => "info",
    };
    let route = text(event, "/route");
    let how = [text(event, "/pathType"), text(event, "/cacheResult")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let message = match (route.is_empty(), how.is_empty()) {
        (false, false) => format!("{route} · {how}"),
        (false, true) => route,
        (true, _) => how,
    };
    LogLine {
        at: text(event, "/timestamp"),
        kind: "request",
        level: level.into(),
        message,
        method: some_text(event, "/requestMethod"),
        path: some_text(event, "/requestPath"),
        status,
        request_id: some_text(event, "/requestId"),
        deployment_id: some_text(event, "/deploymentId"),
    }
}

pub fn log_of(event: &Value) -> LogLine {
    LogLine {
        at: text(event, "/timestamp"),
        kind: "log",
        level: some_text(event, "/level").unwrap_or_else(|| "info".into()),
        message: cut(text(event, "/message")),
        method: some_text(event, "/requestMethod"),
        path: some_text(event, "/requestPath"),
        status: None,
        request_id: some_text(event, "/requestId"),
        deployment_id: some_text(event, "/deploymentId"),
    }
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    /// Only these levels: info, warning, error.
    pub levels: Vec<String>,
    /// Only requests answered with this status or above, such as 500, and what their functions logged.
    pub min_status: Option<u16>,
    /// Only requests whose path contains this.
    pub path: Option<String>,
    /// Only lines containing these words, ignoring case.
    pub query: Option<String>,
}

impl Filter {
    /// Keeps the lines asked for; a function's line follows the request it ran for.
    pub fn apply(&self, lines: Vec<LogLine>) -> (Vec<LogLine>, usize) {
        let failing: Option<HashSet<String>> = self.min_status.map(|min| {
            lines
                .iter()
                .filter(|line| line.status.is_some_and(|status| status >= min))
                .filter_map(|line| line.request_id.clone())
                .collect()
        });
        let words = self
            .query
            .as_deref()
            .map(str::to_lowercase)
            .filter(|words| !words.trim().is_empty());
        let path = self.path.as_deref().filter(|path| !path.is_empty());
        let before = lines.len();
        let kept: Vec<LogLine> = lines
            .into_iter()
            .filter(|line| {
                (self.levels.is_empty()
                    || self
                        .levels
                        .iter()
                        .any(|level| level.eq_ignore_ascii_case(&line.level)))
                    && failing.as_ref().is_none_or(|failing| {
                        line.request_id
                            .as_ref()
                            .is_some_and(|id| failing.contains(id))
                    })
                    && path.is_none_or(|path| {
                        line.path.as_deref().is_some_and(|own| own.contains(path))
                    })
                    && words.as_deref().is_none_or(|words| {
                        line.message.to_lowercase().contains(words)
                            || line
                                .path
                                .as_deref()
                                .is_some_and(|own| own.to_lowercase().contains(words))
                    })
            })
            .collect();
        let dropped = before - kept.len();
        (kept, dropped)
    }
}

/// Days since 1970 as a calendar date, for writing times without a date library.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

pub fn iso(seconds: i64) -> String {
    let (year, month, day) = civil(seconds.div_euclid(86_400));
    let rest = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}

pub struct Query<'a> {
    pub event: &'a str,
    pub owner: &'a str,
    pub project: &'a str,
    pub deployment: Option<&'a str>,
    pub minutes: u64,
    pub limit: u64,
    pub select: &'a [&'a str],
    pub end: i64,
}

pub fn body_of(query: &Query) -> Value {
    let start = query.end - i64::try_from(query.minutes * 60).unwrap_or(0);
    let mut body = json!({
        "event": query.event,
        "scope": { "ownerId": query.owner, "projectIds": [query.project] },
        "timeRange": { "start": iso(start), "end": iso(query.end) },
        "select": query.select,
        "order": "desc",
        "limit": query.limit,
    });
    let mut clauses = Vec::new();
    if let Some(deployment) = query.deployment {
        clauses.push(format!("deploymentId:{}", json!(deployment)));
    }
    if query.event == REQUESTS {
        clauses.push(BACKGROUND.to_string());
    }
    if let (false, Some(object)) = (clauses.is_empty(), body.as_object_mut()) {
        let filter = clauses
            .iter()
            .map(|clause| format!("({clause})"))
            .collect::<Vec<_>>()
            .join(" AND ");
        object.insert("filter".into(), json!(filter));
    }
    body
}

/// The team the project belongs to, by id: the one chosen, else the account's default
/// team, where newer accounts keep their projects. Vercel keeps logs only for teams.
async fn owner_of(session: &Session) -> VercelResult<String> {
    match session.team.as_deref() {
        Some(team) if team.starts_with("team_") => Ok(team.to_string()),
        Some(slug) => {
            let found = session
                .get(&format!("/v2/teams/{}", segment("team", slug)?), &[])
                .await?;
            Ok(text(&found, "/id"))
        }
        None => {
            let user = session.get("/v2/user", &[]).await?;
            some_text(&user, "/user/defaultTeamId").ok_or_else(|| {
                VercelError::BadArg(
                    "Vercel keeps runtime logs to read back only for projects in a team".into(),
                )
            })
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    /// The project's id or name.
    pub project: String,
    /// Only this deployment's.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
    /// How far back. Default 15, at most 30.
    #[serde(default)]
    pub minutes: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(flatten)]
    pub filter: Filter,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heard {
    pub lines: Vec<LogLine>,
    pub minutes: u64,
    /// Lines the filter left out.
    pub filtered: usize,
    pub note: Option<String>,
}

/// Requests and function lines together, newest first.
pub fn merged(mut requests: Vec<LogLine>, logs: Vec<LogLine>) -> Vec<LogLine> {
    requests.extend(logs);
    requests.sort_by(|a, b| b.at.cmp(&a.at));
    requests
}

fn events(answer: &Value) -> Vec<Value> {
    at(answer, "/events")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

pub async fn recent(data_dir: &Path, input: Recent) -> VercelResult<Heard> {
    let project = segment("project", &input.project)?;
    let deployment = input
        .id
        .as_deref()
        .map(|id| segment("deployment", id))
        .transpose()?;
    let minutes = input
        .minutes
        .unwrap_or(DEFAULT_MINUTES)
        .clamp(1, MAX_MINUTES);
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let owner = owner_of(&session).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    let project_id = text(&read, "/id");
    let end = now();
    let teams = [("teamId", owner.clone())];
    let query = |event, select| Query {
        event,
        owner: &owner,
        project: &project_id,
        deployment: deployment.as_deref(),
        minutes,
        limit,
        select,
        end,
    };
    let requests = session
        .post_with_header(
            EVENTS,
            &teams,
            REASON,
            body_of(&query(REQUESTS, REQUEST_FIELDS)),
        )
        .await?;
    let mut note = None;
    let logs = match session
        .post_with_header(
            EVENTS,
            &teams,
            REASON,
            body_of(&query(FUNCTION_LOGS, LOG_FIELDS)),
        )
        .await
    {
        Ok(logs) => logs,
        Err(VercelError::Api(message)) => {
            note = Some(format!("Function logs left out: {message}"));
            Value::Null
        }
        Err(other) => return Err(other),
    };
    let lines = merged(
        events(&requests).iter().map(request_of).collect(),
        events(&logs).iter().map(log_of).collect(),
    );
    let (lines, filtered) = input.filter.apply(lines);
    if lines.is_empty() && note.is_none() {
        note = Some(format!("Nothing in the last {minutes} minutes."));
    }
    Ok(Heard {
        lines,
        minutes,
        filtered,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILED: &str = r#"{"timestamp":"2026-10-19T05:55:47.787Z","requestId":"r1","deploymentId":"dpl_1","requestPath":"/api/checkout","requestMethod":"POST","httpStatus":500,"route":"/api/checkout","pathType":"streaming_func","cacheResult":"MISS"}"#;
    const SERVED: &str = r#"{"timestamp":"2026-10-19T05:55:40.000Z","requestId":"r2","requestPath":"/","requestMethod":"GET","httpStatus":200,"route":"/","pathType":"prerender","cacheResult":"PRERENDER"}"#;
    const THREW: &str = r#"{"timestamp":"2026-10-19T05:55:47.900Z","requestId":"r1","requestPath":"/api/checkout","source":"serverless","level":"error","message":"TypeError: cart is undefined"}"#;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn requests_read_with_their_status_route_and_cache() {
        let failed = request_of(&parse(FAILED));
        assert_eq!(
            (failed.kind, failed.level.as_str(), failed.status),
            ("request", "error", Some(500))
        );
        assert_eq!(failed.message, "/api/checkout · streaming_func · MISS");
        assert_eq!(request_of(&parse(SERVED)).level, "info");
        let missing = request_of(&json!({ "httpStatus": 404, "route": "/[slug]" }));
        assert_eq!(
            (missing.level.as_str(), missing.message.as_str()),
            ("warning", "/[slug]")
        );
    }

    #[test]
    fn a_failing_status_keeps_the_request_and_what_its_function_logged() {
        let lines = merged(
            vec![request_of(&parse(FAILED)), request_of(&parse(SERVED))],
            vec![log_of(&parse(THREW))],
        );
        assert_eq!(
            lines.iter().map(|line| line.kind).collect::<Vec<_>>(),
            ["log", "request", "request"]
        );
        let (kept, dropped) = Filter {
            min_status: Some(500),
            ..Filter::default()
        }
        .apply(lines.clone());
        assert_eq!((kept.len(), dropped), (2, 1));
        assert!(kept
            .iter()
            .all(|line| line.request_id.as_deref() == Some("r1")));
        let (words, _) = Filter {
            query: Some("typeerror".into()),
            ..Filter::default()
        }
        .apply(lines.clone());
        assert_eq!(words.len(), 1);
        let (path, _) = Filter {
            path: Some("/api".into()),
            ..Filter::default()
        }
        .apply(lines);
        assert_eq!(path.len(), 2);
    }

    #[test]
    fn a_query_names_its_event_window_and_deployment() {
        let query = Query {
            event: "vercel.log",
            owner: "team_1",
            project: "prj_1",
            deployment: Some("dpl_9"),
            minutes: 15,
            limit: 200,
            select: LOG_FIELDS,
            end: 1_792_389_347,
        };
        let body = body_of(&query);
        assert_eq!(
            body["scope"],
            json!({ "ownerId": "team_1", "projectIds": ["prj_1"] })
        );
        assert_eq!(body["timeRange"]["end"], "2026-10-19T05:55:47.000Z");
        assert_eq!(body["timeRange"]["start"], "2026-10-19T05:40:47.000Z");
        assert_eq!(body["filter"], "(deploymentId:\"dpl_9\")");
        let all = Query {
            deployment: None,
            ..query
        };
        assert!(body_of(&all).get("filter").is_none());
        let requests = Query {
            event: REQUESTS,
            deployment: None,
            ..query
        };
        assert_eq!(
            body_of(&requests)["filter"],
            "(NOT pathType:\"background_func\")"
        );
    }

    #[test]
    fn times_are_written_as_rfc_3339() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(1_792_389_347), "2026-10-19T05:55:47.000Z");
    }
}
