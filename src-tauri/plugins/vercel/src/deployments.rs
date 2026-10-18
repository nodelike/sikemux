// Deployments: a project's list, one with its build log, and building one again,
// promoting it to production or stopping it.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::segment;

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;
const DEFAULT_TAIL: usize = 200;
const MAX_TAIL: usize = 2000;
const GIT_PROVIDERS: [&str; 3] = ["github", "gitlab", "bitbucket"];

/// A git fact from a deployment's `meta`, whichever provider it came from.
fn git(meta: &Value, fact: &str) -> Value {
    GIT_PROVIDERS
        .iter()
        .find_map(|provider| meta.get(format!("{provider}Commit{fact}")))
        .filter(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        .cloned()
        .unwrap_or(Value::Null)
}

fn first_line(value: Value) -> Value {
    match value {
        Value::String(text) => json!(text.lines().next().unwrap_or_default()),
        other => other,
    }
}

/// A deployment as the pane and agents read it, from the list's shape or the single one's.
pub fn deployment(value: &Value) -> Value {
    let meta = at(value, "/meta");
    let id = value
        .get("uid")
        .or_else(|| value.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let url = text(value, "/url");
    let author = match git(meta, "AuthorName") {
        Value::Null => at(value, "/creator/username").clone(),
        name => name,
    };
    json!({
        "id": id,
        "project": at(value, "/name"),
        "projectId": at(value, "/projectId"),
        "url": if url.is_empty() { Value::Null } else { json!(format!("https://{url}")) },
        "state": value.get("state").or_else(|| value.get("readyState")).cloned().unwrap_or(Value::Null),
        "target": match at(value, "/target").as_str() { Some("production") => "production", _ => "preview" },
        "createdAt": value.get("created").or_else(|| value.get("createdAt")).cloned().unwrap_or(Value::Null),
        "readyAt": at(value, "/ready"),
        "buildMs": build_ms(value),
        "branch": git(meta, "Ref"),
        "commit": git(meta, "Sha"),
        "message": first_line(git(meta, "Message")),
        "author": author,
        "redeployOf": at(meta, "/originalDeploymentId"),
        "inspectorUrl": at(value, "/inspectorUrl"),
        "aliases": at(value, "/alias"),
        "error": value.get("errorMessage").cloned().unwrap_or(Value::Null),
    })
}

/// How long the build ran, from when it started building to when it finished.
fn build_ms(value: &Value) -> Value {
    let started = value.get("buildingAt").and_then(Value::as_u64);
    let finished = value.get("ready").and_then(Value::as_u64);
    match (started, finished) {
        (Some(started), Some(finished)) if finished >= started => json!(finished - started),
        _ => Value::Null,
    }
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn deployments(data_dir: &Path, input: Listing) -> VercelResult<Vec<Value>> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut query = vec![("limit", limit.to_string())];
    let named = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(project) = named(&input.project) {
        query.push(("projectId", project));
    }
    if let Some(target) = named(&input.target) {
        query.push(("target", target));
    }
    if let Some(state) = named(&input.state) {
        query.push(("state", state.to_ascii_uppercase()));
    }
    if let Some(branch) = named(&input.branch) {
        query.push(("branch", branch));
    }
    let data = session.get("/v6/deployments", &query).await?;
    Ok(at(&data, "/deployments")
        .as_array()
        .into_iter()
        .flatten()
        .map(deployment)
        .collect())
}

#[derive(Deserialize)]
pub struct DeploymentRef {
    pub id: String,
    #[serde(default)]
    pub team: Option<String>,
}

/// A deployment's id, or its address without the scheme.
fn reference(id: &str) -> VercelResult<String> {
    let id = id.trim();
    let id = id
        .strip_prefix("https://")
        .or_else(|| id.strip_prefix("http://"))
        .unwrap_or(id)
        .trim_end_matches('/');
    segment("deployment", id)
}

async fn read(session: &Session, id: &str) -> VercelResult<Value> {
    let id = reference(id)?;
    session
        .get(&format!("/v13/deployments/{id}"), &[])
        .await
        .map_err(|error| match error {
            VercelError::NotFound(_) => VercelError::NotFound(format!("no deployment {id}")),
            other => other,
        })
}

fn uid(raw: &Value) -> VercelResult<String> {
    segment("deployment", at(raw, "/id").as_str().unwrap_or_default())
}

pub async fn detail(data_dir: &Path, input: DeploymentRef) -> VercelResult<Value> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    Ok(deployment(&read(&session, &input.id).await?))
}

/// Drops the colour codes and other control characters a build prints, which mean nothing outside a terminal.
pub fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next_if_eq(&'[').is_some() {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        if c.is_control() && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

/// The lines of a build's events, each with when it was printed and whether it went to stderr.
pub fn log_lines(events: &Value) -> Vec<Value> {
    events
        .as_array()
        .into_iter()
        .flatten()
        .filter(|event| {
            matches!(
                at(event, "/type").as_str(),
                Some("stdout" | "stderr" | "command" | "fatal")
            )
        })
        .flat_map(|event| {
            let kind = text(event, "/type");
            let created = at(event, "/created").clone();
            let body = at(event, "/payload/text")
                .as_str()
                .or_else(|| at(event, "/text").as_str())
                .unwrap_or_default()
                .to_string();
            body.lines()
                .map(|line| json!({ "at": created, "kind": kind, "text": plain(line) }))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[derive(Deserialize)]
pub struct LogRef {
    pub id: String,
    #[serde(default)]
    pub tail: Option<usize>,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn logs(data_dir: &Path, input: LogRef) -> VercelResult<Value> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let raw = read(&session, &input.id).await?;
    let id = uid(&raw)?;
    let events = session
        .get(
            &format!("/v3/deployments/{id}/events"),
            &[
                ("builds", "1".into()),
                ("limit", "-1".into()),
                ("direction", "forward".into()),
            ],
        )
        .await?;
    let lines = log_lines(&events);
    let tail = input.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    let skipped = lines.len().saturating_sub(tail);
    Ok(json!({
        "deployment": deployment(&raw),
        "lines": lines.get(skipped..).unwrap_or_default(),
        "skipped": skipped,
    }))
}

#[derive(Deserialize)]
pub struct Redeploy {
    pub id: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn redeploy(data_dir: &Path, input: Redeploy) -> VercelResult<Value> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let raw = read(&session, &input.id).await?;
    let target = input
        .target
        .as_deref()
        .or_else(|| at(&raw, "/target").as_str())
        .filter(|target| *target == "production");
    let mut body = Map::new();
    body.insert("name".into(), at(&raw, "/name").clone());
    body.insert("deploymentId".into(), json!(uid(&raw)?));
    if let Some(target) = target {
        body.insert("target".into(), json!(target));
    }
    let created = session
        .send(
            Method::POST,
            "/v13/deployments",
            &[("forceNew", "1".into())],
            Some(Value::Object(body)),
        )
        .await?;
    Ok(deployment(&created))
}

pub async fn promote(data_dir: &Path, input: DeploymentRef) -> VercelResult<Value> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let raw = read(&session, &input.id).await?;
    let state = text(&raw, "/readyState");
    if state != "READY" {
        return Err(VercelError::BadArg(format!(
            "only a ready deployment can serve production; this one is {}",
            state.to_ascii_lowercase()
        )));
    }
    let project = segment(
        "project",
        at(&raw, "/projectId").as_str().unwrap_or_default(),
    )?;
    let id = uid(&raw)?;
    session
        .send(
            Method::POST,
            &format!("/v10/projects/{project}/promote/{id}"),
            &[],
            None,
        )
        .await?;
    Ok(deployment(&raw))
}

pub async fn cancel(data_dir: &Path, input: DeploymentRef) -> VercelResult<Value> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let raw = read(&session, &input.id).await?;
    let id = uid(&raw)?;
    let canceled = session
        .send(
            Method::PATCH,
            &format!("/v12/deployments/{id}/cancel"),
            &[],
            None,
        )
        .await?;
    Ok(deployment(&canceled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_deployment_reads_with_its_git_facts() {
        let raw = json!({
            "uid": "dpl_1", "name": "web", "url": "web-abc.vercel.app", "state": "ERROR", "target": null, "created": 10, "buildingAt": 1000, "ready": 46000,
            "creator": { "username": "ankit" },
            "meta": { "originalDeploymentId": "dpl_0", "githubCommitRef": "fix-nav", "githubCommitSha": "abc123", "githubCommitMessage": "Fix the nav\n\nlonger body", "githubCommitAuthorName": "Ankit" },
            "inspectorUrl": "https://vercel.com/acme/web/1"
        });
        let read = deployment(&raw);
        assert_eq!(read["id"], "dpl_1");
        assert_eq!(read["url"], "https://web-abc.vercel.app");
        assert_eq!(read["state"], "ERROR");
        assert_eq!(read["target"], "preview");
        assert_eq!(read["branch"], "fix-nav");
        assert_eq!(read["message"], "Fix the nav");
        assert_eq!(read["author"], "Ankit");
        assert_eq!(read["buildMs"], 45000);
        assert_eq!(read["redeployOf"], "dpl_0");
    }

    #[test]
    fn a_single_deployment_reads_from_its_own_shape() {
        let raw = json!({ "id": "dpl_2", "readyState": "READY", "target": "production", "createdAt": 3, "creator": { "username": "cli-user" }, "meta": { "gitlabCommitRef": "main" }, "errorMessage": null });
        let read = deployment(&raw);
        assert_eq!(
            (
                read["id"].as_str(),
                read["state"].as_str(),
                read["target"].as_str()
            ),
            (Some("dpl_2"), Some("READY"), Some("production"))
        );
        assert_eq!(read["branch"], "main");
        assert_eq!(read["author"], "cli-user");
    }

    #[test]
    fn a_build_log_keeps_printed_lines_without_colour() {
        let events = json!([
            { "type": "delimiter", "created": 1, "payload": { "text": "Build" } },
            { "type": "command", "created": 2, "payload": { "text": "npm run build" } },
            { "type": "stdout", "created": 3, "payload": { "text": "\u{1b}[32mok\u{1b}[0m\nnext" } },
            { "type": "stderr", "created": 4, "text": "Error: boom\u{7}\r" }
        ]);
        let lines = log_lines(&events);
        let texts: Vec<&str> = lines
            .iter()
            .filter_map(|line| line["text"].as_str())
            .collect();
        assert_eq!(texts, ["npm run build", "ok", "next", "Error: boom"]);
        assert_eq!(lines[3]["kind"], "stderr");
    }

    #[test]
    fn a_deployment_is_named_by_id_or_address() {
        assert_eq!(reference("dpl_abc").ok().as_deref(), Some("dpl_abc"));
        assert_eq!(
            reference("https://web-abc.vercel.app/").ok().as_deref(),
            Some("web-abc.vercel.app")
        );
        assert!(reference("https://web.vercel.app/../../v2/user").is_err());
    }
}
