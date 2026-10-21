// Issues: the list for a project, one issue with its latest event's stack trace,
// breadcrumbs and tags, and resolving, ignoring, assigning and commenting.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{self, at, number, text, Session};
use crate::error::{SentryError, SentryResult};

const MAX_LIMIT: u64 = 100;
const DEFAULT_LIMIT: u64 = 25;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Assignee {
    /// user or team.
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: String,
    /// Such as WEB-3F.
    pub short_id: String,
    pub title: String,
    /// Where it was thrown, such as `app/checkout.ts in capture`.
    pub culprit: String,
    /// The exception's type and message, when it is one.
    pub kind: Option<String>,
    pub message: Option<String>,
    pub level: String,
    /// unresolved, resolved or ignored.
    pub status: String,
    /// Such as new, ongoing, regressed or escalating.
    pub substatus: Option<String>,
    pub events: u64,
    pub users: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub project: String,
    pub link: String,
    pub assignee: Option<Assignee>,
    pub unhandled: bool,
    /// Events per hour or per day over the period asked for, oldest first.
    pub trend: Vec<u64>,
}

fn assignee_of(value: &Value) -> Option<Assignee> {
    if value.is_null() {
        return None;
    }
    Some(Assignee {
        kind: text(value, "/type").unwrap_or_else(|| "user".into()),
        id: text(value, "/id")?,
        name: text(value, "/name")
            .or_else(|| text(value, "/email"))
            .unwrap_or_default(),
    })
}

pub fn issue_of(item: &Value) -> Option<Issue> {
    let stats = at(item, "/stats");
    let trend = stats
        .as_object()
        .and_then(|periods| periods.values().next())
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|point| at(point, "/1").as_u64().unwrap_or(0))
        .collect();
    Some(Issue {
        id: text(item, "/id")?,
        short_id: text(item, "/shortId").unwrap_or_default(),
        title: text(item, "/title").unwrap_or_default(),
        culprit: text(item, "/culprit").unwrap_or_default(),
        kind: text(item, "/metadata/type"),
        message: text(item, "/metadata/value"),
        level: text(item, "/level").unwrap_or_else(|| "error".into()),
        status: text(item, "/status").unwrap_or_else(|| "unresolved".into()),
        substatus: text(item, "/substatus"),
        events: number(item, "/count"),
        users: number(item, "/userCount"),
        first_seen: text(item, "/firstSeen"),
        last_seen: text(item, "/lastSeen"),
        project: text(item, "/project/slug").unwrap_or_default(),
        link: text(item, "/permalink").unwrap_or_default(),
        assignee: assignee_of(at(item, "/assignedTo")),
        unhandled: at(item, "/isUnhandled").as_bool().unwrap_or(false),
        trend,
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListIssues {
    pub org: Option<String>,
    /// Numeric project ids; none for every project the token sees.
    pub projects: Vec<String>,
    /// A project by slug, as agents name it.
    pub project: Option<String>,
    /// Sentry search words, such as `is:unresolved release:4.2.0 checkout`.
    pub query: Option<String>,
    /// unresolved, resolved, ignored or all.
    pub status: Option<String>,
    /// date (last seen), new (first seen), freq (events) or user (users).
    pub sort: Option<String>,
    pub environment: Option<String>,
    /// 24h or 14d.
    pub period: Option<String>,
    pub limit: Option<u64>,
    pub cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueList {
    pub issues: Vec<Issue>,
    pub next: Option<String>,
}

/// The query string Sentry is sent, with the status filter before the person's words.
pub fn query_of(status: Option<&str>, words: Option<&str>) -> String {
    let status = match status.unwrap_or("unresolved") {
        "all" => None,
        "resolved" => Some("is:resolved"),
        "ignored" => Some("is:ignored"),
        "assigned" => Some("is:unresolved assigned:me"),
        _ => Some("is:unresolved"),
    };
    let words = words.map(str::trim).filter(|words| !words.is_empty());
    match (status, words) {
        (Some(status), Some(words)) => format!("{status} {words}"),
        (Some(status), None) => status.to_string(),
        (None, Some(words)) => words.to_string(),
        (None, None) => String::new(),
    }
}

pub fn list_query(input: &ListIssues) -> SentryResult<Vec<(&'static str, String)>> {
    let sort = match input.sort.as_deref().unwrap_or("date") {
        sort @ ("date" | "new" | "freq" | "user" | "trends") => sort,
        other => {
            return Err(SentryError::BadArg(format!(
                "sort {other:?}: use date, new, freq or user"
            )))
        }
    };
    let period = match input.period.as_deref().unwrap_or("24h") {
        period @ ("24h" | "14d") => period,
        other => {
            return Err(SentryError::BadArg(format!(
                "period {other:?}: use 24h or 14d"
            )))
        }
    };
    let mut query = vec![
        (
            "query",
            query_of(input.status.as_deref(), input.query.as_deref()),
        ),
        ("sort", sort.to_string()),
        ("statsPeriod", period.to_string()),
        (
            "limit",
            input
                .limit
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, MAX_LIMIT)
                .to_string(),
        ),
        ("shortIdLookup", "1".to_string()),
    ];
    for project in &input.projects {
        query.push(("project", client::slug("project", project)?.to_string()));
    }
    if let Some(environment) = input.environment.as_deref().filter(|e| !e.is_empty()) {
        query.push(("environment", environment.to_string()));
    }
    if let Some(cursor) = input.cursor.as_deref().filter(|c| !c.is_empty()) {
        query.push(("cursor", cursor.to_string()));
    }
    Ok(query)
}

pub async fn list(data_dir: &Path, input: ListIssues) -> SentryResult<IssueList> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?.to_string();
    let mut input = input;
    if let Some(project) = input.project.take().filter(|p| !p.is_empty()) {
        input
            .projects
            .push(client::project_id(&session, &org, &project).await?);
    }
    let query = list_query(&input)?;
    let page = session
        .page(&format!("/organizations/{org}/issues/"), &query)
        .await?;
    Ok(IssueList {
        issues: page
            .body
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(issue_of)
            .collect(),
        next: page.next,
    })
}

#[derive(Serialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub file: Option<String>,
    pub module: Option<String>,
    pub function: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    /// Code the app wrote, rather than a library's or the runtime's.
    pub in_app: bool,
    /// Lines around the one that ran, each with its number.
    pub context: Vec<(u64, String)>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Exception {
    pub kind: String,
    pub value: String,
    /// Whether the app caught it; none when the SDK did not say.
    pub handled: Option<bool>,
    /// Oldest call first, as Sentry sends them; the last one threw.
    pub frames: Vec<Frame>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Breadcrumb {
    pub at: Option<String>,
    pub category: String,
    pub level: String,
    pub message: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub at: Option<String>,
    pub message: Option<String>,
    /// The cause first, the one that reached the top last.
    pub exceptions: Vec<Exception>,
    pub breadcrumbs: Vec<Breadcrumb>,
    pub tags: Vec<(String, String)>,
    pub request: Option<String>,
    /// Browser, OS, runtime and device, in words such as `Chrome 128`.
    pub contexts: Vec<(String, String)>,
    pub release: Option<String>,
    pub environment: Option<String>,
}

fn frame_of(frame: &Value) -> Frame {
    Frame {
        file: text(frame, "/filename").or_else(|| text(frame, "/absPath")),
        module: text(frame, "/module"),
        function: text(frame, "/function"),
        line: at(frame, "/lineNo").as_u64(),
        column: at(frame, "/colNo").as_u64(),
        in_app: at(frame, "/inApp").as_bool().unwrap_or(false),
        context: at(frame, "/context")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pair| {
                Some((
                    at(pair, "/0").as_u64()?,
                    text(pair, "/1").unwrap_or_default(),
                ))
            })
            .collect(),
    }
}

fn entries<'a>(event: &'a Value, kind: &str) -> Vec<&'a Value> {
    at(event, "/entries")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| text(entry, "/type").as_deref() == Some(kind))
        .collect()
}

/// What a breadcrumb says, from its message or the fields its kind carries.
fn crumb_message(crumb: &Value) -> String {
    if let Some(message) = text(crumb, "/message") {
        return message;
    }
    let data = at(crumb, "/data");
    match (text(data, "/method"), text(data, "/url")) {
        (Some(method), Some(url)) => {
            let status = at(data, "/status_code")
                .as_u64()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default();
            format!("{method} {url}{status}")
        }
        _ => data
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .take(4)
                    .map(|(key, value)| {
                        format!(
                            "{key}: {}",
                            value
                                .as_str()
                                .map_or_else(|| value.to_string(), str::to_string)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
    }
}

const CONTEXTS: [(&str, &str); 4] = [
    ("browser", "Browser"),
    ("os", "OS"),
    ("runtime", "Runtime"),
    ("device", "Device"),
];

pub fn event_of(event: &Value) -> Event {
    let exceptions = entries(event, "exception")
        .into_iter()
        .flat_map(|entry| {
            at(entry, "/data/values")
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|value| Exception {
            kind: text(&value, "/type").unwrap_or_else(|| "Error".into()),
            value: text(&value, "/value").unwrap_or_default(),
            handled: at(&value, "/mechanism/handled").as_bool(),
            frames: at(&value, "/stacktrace/frames")
                .as_array()
                .into_iter()
                .flatten()
                .map(frame_of)
                .collect(),
        })
        .collect();
    let breadcrumbs = entries(event, "breadcrumbs")
        .into_iter()
        .flat_map(|entry| {
            at(entry, "/data/values")
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|crumb| Breadcrumb {
            at: text(&crumb, "/timestamp"),
            category: text(&crumb, "/category")
                .or_else(|| text(&crumb, "/type"))
                .unwrap_or_else(|| "default".into()),
            level: text(&crumb, "/level").unwrap_or_else(|| "info".into()),
            message: crumb_message(&crumb),
        })
        .collect();
    let tags: Vec<(String, String)> = at(event, "/tags")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| Some((text(tag, "/key")?, text(tag, "/value").unwrap_or_default())))
        .collect();
    let tag = |key: &str| {
        tags.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let request = entries(event, "request").first().and_then(|entry| {
        let url = text(entry, "/data/url")?;
        Some(match text(entry, "/data/method") {
            Some(method) => format!("{method} {url}"),
            None => url,
        })
    });
    let contexts = CONTEXTS
        .iter()
        .filter_map(|(key, label)| {
            let context = at(event, &format!("/contexts/{key}"));
            let name = text(context, "/name").or_else(|| text(context, "/model"))?;
            let version = text(context, "/version")
                .map(|v| format!(" {v}"))
                .unwrap_or_default();
            Some(((*label).to_string(), format!("{name}{version}")))
        })
        .collect();
    let message = entries(event, "message")
        .first()
        .and_then(|entry| text(entry, "/data/formatted").or_else(|| text(entry, "/data/message")))
        .or_else(|| text(event, "/message"));
    Event {
        id: text(event, "/eventID")
            .or_else(|| text(event, "/id"))
            .unwrap_or_default(),
        at: text(event, "/dateCreated"),
        message,
        exceptions,
        breadcrumbs,
        release: text(event, "/release/version").or_else(|| tag("release")),
        environment: tag("environment"),
        tags,
        request,
        contexts,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(flatten)]
    pub issue: Issue,
    pub first_release: Option<String>,
    pub last_release: Option<String>,
    pub comments: u64,
    pub event: Option<Event>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    #[serde(default)]
    pub org: Option<String>,
    /// The numeric id, or a short id such as WEB-3F.
    pub id: String,
}

/// The numeric id of an issue named by either id, asking Sentry for a short id's.
async fn issue_id(session: &Session, org: &str, id: &str) -> SentryResult<String> {
    let id = id.trim();
    if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
        return Ok(id.to_string());
    }
    let short = client::slug("issue", id)?.to_uppercase();
    let found = session
        .get(&format!("/organizations/{org}/shortids/{short}/"), &[])
        .await?;
    text(&found, "/groupId")
        .or_else(|| text(&found, "/group/id"))
        .ok_or_else(|| SentryError::NotFound(format!("no issue {short}")))
}

pub async fn detail(data_dir: &Path, input: IssueRef) -> SentryResult<Detail> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let id = issue_id(&session, org, &input.id).await?;
    let base = format!("/organizations/{org}/issues/{id}/");
    let latest = format!("{base}events/latest/");
    let (issue, event) = tokio::join!(session.get(&base, &[]), session.get(&latest, &[]));
    let issue = issue?;
    let event = match event {
        Ok(event) => Some(event_of(&event)),
        Err(SentryError::NotFound(_)) => None,
        Err(error) => return Err(error),
    };
    Ok(Detail {
        issue: issue_of(&issue)
            .ok_or_else(|| SentryError::Response("Sentry sent an issue without an id".into()))?,
        first_release: text(&issue, "/firstRelease/version"),
        last_release: text(&issue, "/lastRelease/version"),
        comments: number(&issue, "/numComments"),
        event,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    #[serde(default)]
    pub org: Option<String>,
    pub id: String,
    /// resolve, resolveInNextRelease, unresolve or ignore.
    #[serde(default)]
    pub action: Option<String>,
    /// `me`, an actor such as user:42 or team:7, or empty to unassign.
    #[serde(default)]
    pub assign: Option<String>,
}

/// The body that makes the change, checked first.
pub fn change_body(change: &Change, me: &str) -> SentryResult<Value> {
    let mut body = serde_json::Map::new();
    if let Some(action) = change.action.as_deref() {
        let (status, details) = match action {
            "resolve" => ("resolved", json!({})),
            "resolveInNextRelease" => ("resolved", json!({ "inNextRelease": true })),
            "unresolve" => ("unresolved", json!({})),
            "ignore" => ("ignored", json!({})),
            other => {
                return Err(SentryError::BadArg(format!(
                    "action {other:?}: use resolve, resolveInNextRelease, unresolve or ignore"
                )))
            }
        };
        body.insert("status".into(), json!(status));
        body.insert("statusDetails".into(), details);
        if action == "ignore" {
            body.insert("substatus".into(), json!("archived_until_escalating"));
        }
    }
    if let Some(assign) = change.assign.as_deref() {
        let actor = match assign.trim() {
            "" => String::new(),
            "me" if me.is_empty() => {
                return Err(SentryError::BadArg(
                    "Sentry did not say who signed in; name the person instead".into(),
                ))
            }
            "me" => format!("user:{me}"),
            actor if actor.starts_with("user:") || actor.starts_with("team:") => {
                let id = actor.split_once(':').map_or("", |(_, id)| id);
                client::slug("assignee", id)?;
                actor.to_string()
            }
            name => client::slug("assignee", name)?.to_string(),
        };
        body.insert("assignedTo".into(), json!(actor));
    }
    if body.is_empty() {
        return Err(SentryError::BadArg(
            "say what to change: an action or who to assign".into(),
        ));
    }
    Ok(Value::Object(body))
}

pub async fn change(data_dir: &Path, input: Change) -> SentryResult<Issue> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let body = change_body(&input, &session.settings.user_id)?;
    let id = issue_id(&session, org, &input.id).await?;
    let path = format!("/organizations/{org}/issues/{id}/");
    session.send(Method::PUT, &path, &body).await?;
    issue_of(&session.get(&path, &[]).await?)
        .ok_or_else(|| SentryError::Response("Sentry sent an issue without an id".into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    #[serde(default)]
    pub org: Option<String>,
    pub id: String,
    pub text: String,
}

pub async fn comment(data_dir: &Path, input: Comment) -> SentryResult<()> {
    let text = input.text.trim();
    if text.is_empty() {
        return Err(SentryError::BadArg("the comment is empty".into()));
    }
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let id = issue_id(&session, org, &input.id).await?;
    session
        .send(
            Method::POST,
            &format!("/organizations/{org}/issues/{id}/comments/"),
            &json!({ "text": text }),
        )
        .await?;
    Ok(())
}

/// The frames an agent is shown: every one the app wrote, and the frame that threw.
fn frames_for_agent(frames: &[Frame]) -> Vec<&Frame> {
    let last = frames.len().saturating_sub(1);
    frames
        .iter()
        .enumerate()
        .rev()
        .filter(|(index, frame)| frame.in_app || *index == last)
        .map(|(_, frame)| frame)
        .take(20)
        .collect()
}

fn where_of(frame: &Frame) -> String {
    let file = frame
        .file
        .clone()
        .or_else(|| frame.module.clone())
        .unwrap_or_else(|| "?".into());
    let line = frame
        .line
        .map(|line| format!(":{line}"))
        .unwrap_or_default();
    let function = frame.function.as_deref().unwrap_or("?");
    format!("{function} ({file}{line})")
}

/// The issue in plain text for an agent: what threw, where, and what led up to it.
pub fn brief(detail: &Detail) -> String {
    let issue = &detail.issue;
    let mut lines = vec![format!(
        "{} · {} · {} · {} events, {} users · first seen {}, last seen {}",
        issue.short_id,
        issue.title,
        issue.status,
        issue.events,
        issue.users,
        issue.first_seen.as_deref().unwrap_or("?"),
        issue.last_seen.as_deref().unwrap_or("?"),
    )];
    if let Some(release) = &detail.first_release {
        lines.push(format!("First seen in release {release}."));
    }
    lines.push(issue.link.clone());
    let Some(event) = &detail.event else {
        lines.push("Sentry kept no event for it.".into());
        return lines.join("\n");
    };
    for exception in event.exceptions.iter().rev() {
        let handled = match exception.handled {
            Some(false) => " (unhandled)",
            Some(true) => " (handled)",
            None => "",
        };
        lines.push(String::new());
        lines.push(format!("{}: {}{handled}", exception.kind, exception.value));
        for frame in frames_for_agent(&exception.frames) {
            lines.push(format!(
                "  at {}{}",
                where_of(frame),
                if frame.in_app { "" } else { " [library]" }
            ));
            if frame.in_app {
                if let Some((_, code)) = frame
                    .context
                    .iter()
                    .find(|(number, _)| Some(*number) == frame.line)
                {
                    lines.push(format!("      > {}", code.trim()));
                }
            }
        }
    }
    if event.exceptions.is_empty() {
        if let Some(message) = &event.message {
            lines.push(String::new());
            lines.push(message.clone());
        }
    }
    if let Some(request) = &event.request {
        lines.push(format!("\nRequest: {request}"));
    }
    let crumbs: Vec<&Breadcrumb> = event.breadcrumbs.iter().rev().take(10).collect();
    if !crumbs.is_empty() {
        lines.push("\nLast breadcrumbs, newest first:".into());
        for crumb in crumbs {
            lines.push(format!("- [{}] {}", crumb.category, crumb.message));
        }
    }
    let tags: Vec<String> = event
        .tags
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "environment"
                    | "release"
                    | "browser"
                    | "os"
                    | "runtime"
                    | "url"
                    | "transaction"
                    | "handled"
                    | "level"
            )
        })
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if !tags.is_empty() {
        lines.push(format!("\nTags: {}", tags.join(", ")));
    }
    lines.join("\n")
}

pub async fn brief_of(data_dir: &Path, input: IssueRef) -> SentryResult<String> {
    Ok(brief(&detail(data_dir, input).await?))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn issue_json() -> Value {
        json!({
            "id": "4512887211", "shortId": "WEB-3F", "title": "TypeError: Cannot read properties of undefined (reading 'total')",
            "culprit": "app/checkout/summary.ts in totalOf", "level": "error", "status": "unresolved", "substatus": "regressed",
            "count": "1284", "userCount": 312, "firstSeen": "2026-10-12T08:00:00Z", "lastSeen": "2026-10-19T12:30:00Z",
            "permalink": "https://acme.sentry.io/issues/4512887211/", "project": { "slug": "web" },
            "metadata": { "type": "TypeError", "value": "Cannot read properties of undefined (reading 'total')" },
            "assignedTo": { "type": "user", "id": "42", "name": "Ankit" }, "isUnhandled": true,
            "stats": { "24h": [[1, 3], [2, 0], [3, 41]] },
            "firstRelease": { "version": "web@4.13.0" }, "numComments": 2
        })
    }

    pub fn event_json() -> Value {
        json!({
            "eventID": "9fd3c1", "dateCreated": "2026-10-19T12:30:00Z",
            "tags": [{ "key": "environment", "value": "production" }, { "key": "release", "value": "web@4.13.0" }, { "key": "browser", "value": "Chrome 128" }],
            "contexts": { "browser": { "name": "Chrome", "version": "128.0" }, "os": { "name": "macOS", "version": "15.1" } },
            "entries": [
                { "type": "exception", "data": { "values": [{
                    "type": "TypeError", "value": "Cannot read properties of undefined (reading 'total')",
                    "mechanism": { "handled": false },
                    "stacktrace": { "frames": [
                        { "filename": "node_modules/react-dom/cjs/react-dom.js", "function": "commitRoot", "lineNo": 4012, "inApp": false },
                        { "filename": "app/checkout/Summary.tsx", "function": "Summary", "lineNo": 31, "colNo": 18, "inApp": true,
                          "context": [[30, "  const cart = useCart();"], [31, "  const total = totalOf(cart.order);"], [32, "  return <Total value={total} />;"]] },
                        { "filename": "app/checkout/summary.ts", "function": "totalOf", "lineNo": 12, "colNo": 22, "inApp": true,
                          "context": [[12, "  return order.lines.total;"]] }
                    ] }
                }] } },
                { "type": "breadcrumbs", "data": { "values": [
                    { "timestamp": "2026-10-19T12:29:58Z", "category": "navigation", "data": { "from": "/cart", "to": "/checkout" } },
                    { "timestamp": "2026-10-19T12:29:59Z", "category": "fetch", "type": "http", "data": { "method": "GET", "url": "/api/cart", "status_code": 404 } },
                    { "timestamp": "2026-10-19T12:30:00Z", "category": "console", "level": "error", "message": "cart missing" }
                ] } },
                { "type": "request", "data": { "url": "https://shop.acme.dev/checkout", "method": "GET" } }
            ]
        })
    }

    #[test]
    fn an_issue_reads_its_counts_assignee_and_trend() {
        let issue = issue_of(&issue_json()).expect("reads");
        assert_eq!(issue.short_id, "WEB-3F");
        assert_eq!((issue.events, issue.users), (1284, 312));
        assert_eq!(issue.trend, [3, 0, 41]);
        assert_eq!(issue.assignee.map(|a| a.name), Some("Ankit".into()));
        assert!(issue.unhandled);
        assert!(issue_of(&json!({ "title": "no id" })).is_none());
    }

    #[test]
    fn an_event_reads_its_stack_breadcrumbs_and_contexts() {
        let event = event_of(&event_json());
        let thrown = &event.exceptions[0];
        assert_eq!(thrown.handled, Some(false));
        assert_eq!(thrown.frames.len(), 3);
        assert!(thrown.frames[1].in_app);
        assert_eq!(
            thrown.frames[1].context[1],
            (31, "  const total = totalOf(cart.order);".into())
        );
        assert_eq!(event.breadcrumbs[0].message, "from: /cart, to: /checkout");
        assert_eq!(event.breadcrumbs[1].message, "GET /api/cart [404]");
        assert_eq!(event.release.as_deref(), Some("web@4.13.0"));
        assert_eq!(event.environment.as_deref(), Some("production"));
        assert_eq!(
            event.request.as_deref(),
            Some("GET https://shop.acme.dev/checkout")
        );
        assert_eq!(event.contexts[0], ("Browser".into(), "Chrome 128.0".into()));
    }

    #[test]
    fn the_list_query_puts_the_status_before_the_words() {
        assert_eq!(query_of(None, None), "is:unresolved");
        assert_eq!(query_of(Some("all"), Some(" checkout ")), "checkout");
        assert_eq!(
            query_of(Some("assigned"), None),
            "is:unresolved assigned:me"
        );
        let input = ListIssues {
            projects: vec!["11".into()],
            sort: Some("freq".into()),
            environment: Some("production".into()),
            limit: Some(500),
            ..ListIssues::default()
        };
        let query = list_query(&input).expect("valid");
        assert!(query.contains(&("limit", "100".into())));
        assert!(query.contains(&("project", "11".into())));
        assert!(query.contains(&("environment", "production".into())));
        assert!(list_query(&ListIssues {
            sort: Some("random".into()),
            ..ListIssues::default()
        })
        .is_err());
        assert!(list_query(&ListIssues {
            projects: vec!["../x".into()],
            ..ListIssues::default()
        })
        .is_err());
    }

    #[test]
    fn each_change_sends_just_what_sentry_needs() {
        let change = |action: Option<&str>, assign: Option<&str>| Change {
            org: None,
            id: "1".into(),
            action: action.map(str::to_string),
            assign: assign.map(str::to_string),
        };
        assert_eq!(
            change_body(&change(Some("resolveInNextRelease"), None), "42").ok(),
            Some(json!({ "status": "resolved", "statusDetails": { "inNextRelease": true } }))
        );
        assert_eq!(
            change_body(&change(Some("ignore"), None), "42").ok(),
            Some(
                json!({ "status": "ignored", "statusDetails": {}, "substatus": "archived_until_escalating" })
            )
        );
        assert_eq!(
            change_body(&change(None, Some("me")), "42").ok(),
            Some(json!({ "assignedTo": "user:42" }))
        );
        assert_eq!(
            change_body(&change(None, Some("")), "42").ok(),
            Some(json!({ "assignedTo": "" }))
        );
        assert!(change_body(&change(None, None), "42").is_err());
        assert!(change_body(&change(Some("delete"), None), "42").is_err());
        assert!(change_body(&change(None, Some("me")), "").is_err());
        assert!(change_body(&change(None, Some("user:../x")), "42").is_err());
    }

    #[test]
    fn an_agent_is_told_what_threw_where_and_what_led_up_to_it() {
        let detail = Detail {
            issue: issue_of(&issue_json()).expect("reads"),
            first_release: Some("web@4.13.0".into()),
            last_release: None,
            comments: 2,
            event: Some(event_of(&event_json())),
        };
        let text = brief(&detail);
        assert!(text.contains("WEB-3F · TypeError"));
        assert!(text.contains(
            "TypeError: Cannot read properties of undefined (reading 'total') (unhandled)"
        ));
        let top = text
            .find("at totalOf (app/checkout/summary.ts:12)")
            .expect("the frame that threw");
        let caller = text
            .find("at Summary (app/checkout/Summary.tsx:31)")
            .expect("its caller");
        assert!(top < caller);
        assert!(text.contains("> return order.lines.total;"));
        assert!(!text.contains("commitRoot"));
        assert!(text.contains("- [fetch] GET /api/cart [404]"));
        assert!(text.contains("environment=production"));
    }
}
