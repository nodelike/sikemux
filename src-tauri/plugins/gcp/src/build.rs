// Cloud Build: builds with their steps, timings and why one failed, a build's
// log, triggers, and running a trigger, retrying a build or cancelling one.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, encoded, segment, text};
use crate::error::GcpResult;
use crate::logging;

const API: &str = "https://cloudbuild.googleapis.com/v1";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
const MAX_LOG_LINES: usize = 3000;

fn substitution(value: &Value, key: &str) -> Value {
    at(value, &format!("/substitutions/{key}")).clone()
}

/// A build as the pane and agents read it.
pub fn build(value: &Value) -> Value {
    let repo = [
        substitution(value, "REPO_NAME"),
        at(value, "/source/repoSource/repoName").clone(),
    ]
    .into_iter()
    .find(|repo| repo.as_str().is_some_and(|repo| !repo.is_empty()))
    .unwrap_or(Value::Null);
    let branch = [
        substitution(value, "BRANCH_NAME"),
        at(value, "/source/repoSource/branchName").clone(),
    ]
    .into_iter()
    .find(|branch| branch.as_str().is_some_and(|branch| !branch.is_empty()))
    .unwrap_or(Value::Null);
    let commit = [
        substitution(value, "COMMIT_SHA"),
        at(value, "/source/repoSource/commitSha").clone(),
    ]
    .into_iter()
    .find(|commit| commit.as_str().is_some_and(|commit| !commit.is_empty()))
    .unwrap_or(Value::Null);
    let steps: Vec<Value> = at(value, "/steps")
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            json!({
                "id": at(step, "/id"),
                "image": at(step, "/name"),
                "status": at(step, "/status"),
                "startedAt": at(step, "/timing/startTime"),
                "endedAt": at(step, "/timing/endTime"),
            })
        })
        .collect();
    json!({
        "id": at(value, "/id"),
        "status": at(value, "/status"),
        "detail": at(value, "/statusDetail"),
        "failure": at(value, "/failureInfo/detail"),
        "trigger": substitution(value, "TRIGGER_NAME"),
        "triggerId": at(value, "/buildTriggerId"),
        "repo": repo,
        "branch": branch,
        "commit": commit,
        "steps": steps,
        "images": at(value, "/images"),
        "createdAt": at(value, "/createTime"),
        "startedAt": at(value, "/startTime"),
        "finishedAt": at(value, "/finishTime"),
        "logUrl": at(value, "/logUrl"),
        "region": at(value, "/name").as_str().map(crate::run::location_of).filter(|region| !region.is_empty()),
    })
}

pub fn trigger(value: &Value) -> Value {
    let github = at(value, "/github");
    let repo = if github.is_null() {
        at(value, "/triggerTemplate/repoName").clone()
    } else {
        json!(format!(
            "{}/{}",
            text(github, "/owner"),
            text(github, "/name")
        ))
    };
    let branch = [
        at(github, "/push/branch"),
        at(value, "/triggerTemplate/branchName"),
        at(value, "/sourceToBuild/ref"),
    ]
    .into_iter()
    .find(|branch| branch.as_str().is_some_and(|branch| !branch.is_empty()))
    .cloned()
    .unwrap_or(Value::Null);
    json!({
        "id": at(value, "/id"),
        "name": at(value, "/name"),
        "description": at(value, "/description"),
        "repo": repo,
        "branch": branch,
        "pullRequests": !at(github, "/pullRequest").is_null(),
        "file": at(value, "/filename"),
        "disabled": at(value, "/disabled").as_bool() == Some(true),
        "createdAt": at(value, "/createTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    /// A region for regional builds; none lists the global ones.
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn base(target: &Target, region: Option<&str>) -> GcpResult<String> {
    let project = target.project()?;
    Ok(
        match region.map(str::trim).filter(|region| !region.is_empty()) {
            Some(region) => format!(
                "{API}/projects/{project}/locations/{}",
                segment("region", region)?
            ),
            None => format!("{API}/projects/{project}"),
        },
    )
}

pub async fn builds(input: Listing) -> GcpResult<Vec<Value>> {
    let url = format!("{}/builds", base(&input.target, input.region.as_deref())?);
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let listed = client::list(
        &input.target,
        &url,
        &[("pageSize", limit.min(100).to_string())],
        "builds",
        limit,
    )
    .await?;
    Ok(listed.iter().map(build).collect())
}

pub async fn triggers(input: Listing) -> GcpResult<Vec<Value>> {
    let url = format!("{}/triggers", base(&input.target, input.region.as_deref())?);
    let listed = client::list(
        &input.target,
        &url,
        &[("pageSize", "100".into())],
        "triggers",
        500,
    )
    .await?;
    let mut read: Vec<Value> = listed.iter().map(trigger).collect();
    read.sort_by_key(|each| text(each, "/name"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct BuildRef {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub region: Option<String>,
    pub id: String,
}

fn build_url(input: &BuildRef) -> GcpResult<String> {
    Ok(format!(
        "{}/builds/{}",
        base(&input.target, input.region.as_deref())?,
        segment("build", &input.id)?
    ))
}

/// One build with the lines it printed: from its logs bucket, or from Cloud Logging when it has none.
pub async fn build_detail(input: BuildRef) -> GcpResult<Value> {
    let raw = client::get(&input.target, &build_url(&input)?, &[]).await?;
    let mut read = build(&raw);
    let bucket = text(&raw, "/logsBucket");
    let mut lines: Vec<String> = Vec::new();
    if let Some(bucket) = bucket
        .strip_prefix("gs://")
        .map(|b| b.split('/').next().unwrap_or(b))
        .filter(|b| !b.is_empty())
    {
        let url = format!(
            "https://storage.googleapis.com/storage/v1/b/{}/o/{}",
            segment("bucket", bucket)?,
            encoded(&format!("log-{}.txt", input.id))
        );
        if let Ok(bytes) =
            client::bytes(&input.target, &url, &[("alt", "media".into())], None).await
        {
            lines = String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_string)
                .collect();
        }
    }
    if lines.is_empty() {
        let entries = logging::search(logging::Query {
            target: input.target.clone(),
            resource: Some(logging::Resource::Build),
            name: Some(input.id.clone()),
            since: Some("30d".into()),
            limit: Some(1000),
            ..logging::Query::default()
        })
        .await
        .unwrap_or_default();
        lines = entries
            .iter()
            .rev()
            .map(|entry| text(entry, "/text"))
            .collect();
    }
    let skipped = lines.len().saturating_sub(MAX_LOG_LINES);
    if let Some(object) = read.as_object_mut() {
        object.insert(
            "log".into(),
            json!(lines.get(skipped..).unwrap_or_default()),
        );
        object.insert("logSkipped".into(), json!(skipped));
    }
    Ok(read)
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum BuildAction {
    Retry,
    Cancel,
}

#[derive(Deserialize)]
pub struct BuildChange {
    #[serde(flatten)]
    pub build: BuildRef,
    pub action: BuildAction,
}

/// Starts a build again from the same source, or stops one that is running.
pub async fn change(input: BuildChange) -> GcpResult<Value> {
    let verb = match input.action {
        BuildAction::Retry => "retry",
        BuildAction::Cancel => "cancel",
    };
    let answer = client::post(
        &input.build.target,
        &format!("{}:{verb}", build_url(&input.build)?),
        &json!({}),
    )
    .await?;
    let started = if at(&answer, "/metadata/build").is_null() {
        answer.clone()
    } else {
        at(&answer, "/metadata/build").clone()
    };
    Ok(build(&started))
}

#[derive(Deserialize)]
pub struct RunTrigger {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub region: Option<String>,
    pub trigger: String,
    /// The branch to build; none builds the trigger's own.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Starts a trigger's build now.
pub async fn run_trigger(input: RunTrigger) -> GcpResult<Value> {
    let url = format!(
        "{}/triggers/{}:run",
        base(&input.target, input.region.as_deref())?,
        segment("trigger", &input.trigger)?
    );
    let body = match input
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    {
        Some(branch) => json!({ "branchName": branch }),
        None => json!({}),
    };
    let operation = client::post(&input.target, &url, &body).await?;
    Ok(build(at(&operation, "/metadata/build")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_reads_with_its_source_steps_and_failure() {
        let read = build(&json!({
            "id": "b1", "status": "FAILURE", "statusDetail": "Build step failure", "failureInfo": { "type": "USER_BUILD_STEP", "detail": "step 1 exited with 2" },
            "substitutions": { "REPO_NAME": "storefront", "BRANCH_NAME": "main", "COMMIT_SHA": "abc123", "TRIGGER_NAME": "deploy-main" },
            "steps": [{ "id": "test", "name": "node:20", "status": "SUCCESS" }, { "id": "deploy", "name": "gcr.io/cloud-builders/gcloud", "status": "FAILURE" }],
            "name": "projects/acme/locations/us-central1/builds/b1"
        }));
        assert_eq!(read["repo"], "storefront");
        assert_eq!(read["branch"], "main");
        assert_eq!(read["trigger"], "deploy-main");
        assert_eq!(read["failure"], "step 1 exited with 2");
        assert_eq!(read["steps"][1]["status"], "FAILURE");
        assert_eq!(read["region"], "us-central1");
        assert_eq!(
            build(
                &json!({ "id": "b2", "source": { "repoSource": { "repoName": "api", "branchName": "dev" } } })
            )["branch"],
            "dev"
        );
    }

    #[test]
    fn a_trigger_names_its_repository_and_branch() {
        let github = trigger(
            &json!({ "id": "t1", "name": "deploy-main", "github": { "owner": "acme", "name": "storefront", "push": { "branch": "^main$" } }, "filename": "cloudbuild.yaml" }),
        );
        assert_eq!(github["repo"], "acme/storefront");
        assert_eq!(github["branch"], "^main$");
        assert_eq!(github["pullRequests"], false);
    }
}
