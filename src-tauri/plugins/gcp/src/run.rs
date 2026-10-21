// Cloud Run: services in every region with their revisions and how traffic
// splits between them, jobs with their executions, and the changes a person
// or an agent makes: sending traffic to a revision and running a job.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://run.googleapis.com/v2";
const MAX_ITEMS: usize = 500;

/// The region in a resource name such as `projects/p/locations/us-central1/services/api`.
pub fn location_of(name: &str) -> String {
    let parts: Vec<&str> = name.split('/').collect();
    parts
        .iter()
        .position(|part| *part == "locations")
        .and_then(|index| parts.get(index + 1))
        .map(|location| (*location).to_string())
        .unwrap_or_default()
}

/// Ready, failed or still settling, from a resource's terminal condition.
fn state_of(condition: &Value) -> &'static str {
    match at(condition, "/state").as_str() {
        Some("CONDITION_SUCCEEDED") => "READY",
        Some("CONDITION_FAILED") => "FAILED",
        Some("CONDITION_RECONCILING") | Some("CONDITION_PENDING") => "DEPLOYING",
        _ => "UNKNOWN",
    }
}

fn traffic_of(entries: &Value) -> Vec<Value> {
    entries
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let latest = text(entry, "/type") == "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST";
            json!({
                "revision": if latest && text(entry, "/revision").is_empty() { Value::Null } else { json!(short(&text(entry, "/revision"))) },
                "latest": latest,
                "percent": at(entry, "/percent").as_u64().unwrap_or(0),
                "tag": at(entry, "/tag"),
                "url": at(entry, "/uri"),
            })
        })
        .collect()
}

/// A service as the pane and agents read it.
pub fn service(value: &Value) -> Value {
    let name = text(value, "/name");
    let container = at(value, "/template/containers/0");
    let reconciling = at(value, "/reconciling").as_bool() == Some(true);
    json!({
        "name": short(&name),
        "location": location_of(&name),
        "url": at(value, "/uri"),
        "state": if reconciling { "DEPLOYING" } else { state_of(at(value, "/terminalCondition")) },
        "message": at(value, "/terminalCondition/message"),
        "latestRevision": short(&text(value, "/latestReadyRevision")),
        "latestCreatedRevision": short(&text(value, "/latestCreatedRevision")),
        "image": at(container, "/image"),
        "cpu": at(container, "/resources/limits/cpu"),
        "memory": at(container, "/resources/limits/memory"),
        "minInstances": at(value, "/template/scaling/minInstanceCount"),
        "maxInstances": at(value, "/template/scaling/maxInstanceCount"),
        "concurrency": at(value, "/template/maxInstanceRequestConcurrency"),
        "serviceAccount": at(value, "/template/serviceAccount"),
        "ingress": at(value, "/ingress"),
        "traffic": traffic_of(at(value, "/trafficStatuses")),
        "updatedAt": at(value, "/updateTime"),
        "updatedBy": at(value, "/lastModifier"),
        "createdAt": at(value, "/createTime"),
    })
}

/// A revision as the pane and agents read it, with the share of traffic it gets.
pub fn revision(value: &Value, traffic: &[Value]) -> Value {
    let name = short(&text(value, "/name"));
    let ready = at(value, "/conditions")
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| text(condition, "/type") == "Ready")
        .map(state_of)
        .unwrap_or("UNKNOWN");
    let percent: u64 = traffic
        .iter()
        .filter(|entry| entry.get("revision").and_then(Value::as_str) == Some(name.as_str()))
        .filter_map(|entry| entry.get("percent").and_then(Value::as_u64))
        .sum();
    json!({
        "name": name,
        "state": ready,
        "image": at(value, "/containers/0/image"),
        "createdAt": at(value, "/createTime"),
        "minInstances": at(value, "/scaling/minInstanceCount"),
        "maxInstances": at(value, "/scaling/maxInstanceCount"),
        "percent": percent,
        "tags": traffic.iter().filter(|entry| entry.get("revision").and_then(Value::as_str) == Some(name.as_str())).filter_map(|entry| entry.get("tag").and_then(Value::as_str).filter(|tag| !tag.is_empty())).collect::<Vec<_>>(),
    })
}

/// A job as the pane and agents read it.
pub fn job(value: &Value) -> Value {
    let name = text(value, "/name");
    json!({
        "name": short(&name),
        "location": location_of(&name),
        "state": if at(value, "/reconciling").as_bool() == Some(true) { "DEPLOYING" } else { state_of(at(value, "/terminalCondition")) },
        "image": at(value, "/template/template/containers/0/image"),
        "tasks": at(value, "/template/taskCount"),
        "executions": at(value, "/executionCount"),
        "latestExecution": short(&text(value, "/latestCreatedExecution/name")),
        "latestExecutionAt": at(value, "/latestCreatedExecution/createTime"),
        "latestExecutionState": at(value, "/latestCreatedExecution/completionStatus"),
        "updatedAt": at(value, "/updateTime"),
    })
}

pub fn execution(value: &Value) -> Value {
    let succeeded = at(value, "/succeededCount").as_u64().unwrap_or(0);
    let failed = at(value, "/failedCount").as_u64().unwrap_or(0);
    let running = at(value, "/runningCount").as_u64().unwrap_or(0);
    let state = if running > 0 {
        "RUNNING"
    } else if failed > 0 {
        "FAILED"
    } else if at(value, "/completionTime").is_null() {
        "PENDING"
    } else if succeeded > 0 {
        "SUCCEEDED"
    } else {
        "CANCELLED"
    };
    json!({
        "name": short(&text(value, "/name")),
        "state": state,
        "tasks": at(value, "/taskCount"),
        "succeeded": succeeded,
        "failed": failed,
        "running": running,
        "startedAt": at(value, "/startTime"),
        "completedAt": at(value, "/completionTime"),
        "createdAt": at(value, "/createTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub query: Option<String>,
}

fn matches(words: &str, value: &Value) -> bool {
    words.is_empty() || text(value, "/name").to_lowercase().contains(words)
}

/// Every Cloud Run service in the project, in every region.
pub async fn services(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/locations/-/services"),
        &[("pageSize", "100".into())],
        "services",
        MAX_ITEMS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    Ok(listed
        .iter()
        .map(service)
        .filter(|each| matches(&words, each))
        .collect())
}

#[derive(Deserialize)]
pub struct ServiceRef {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub service: String,
}

fn service_url(input: &ServiceRef) -> GcpResult<String> {
    let project = input.target.project()?;
    let location = segment("region", &input.location)?;
    let name = segment("service", &input.service)?;
    Ok(format!(
        "{API}/projects/{project}/locations/{location}/services/{name}"
    ))
}

/// One service with every revision, newest first.
pub async fn service_detail(input: ServiceRef) -> GcpResult<Value> {
    let url = service_url(&input)?;
    let (raw, revisions) = futures::future::try_join(
        client::get(&input.target, &url, &[]),
        client::list(
            &input.target,
            &format!("{url}/revisions"),
            &[("pageSize", "100".into())],
            "revisions",
            MAX_ITEMS,
        ),
    )
    .await?;
    let mut read = service(&raw);
    let traffic = read
        .get("traffic")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut revisions: Vec<Value> = revisions
        .iter()
        .map(|each| revision(each, &traffic))
        .collect();
    revisions.sort_by_key(|each| std::cmp::Reverse(text(each, "/createdAt")));
    if let Some(object) = read.as_object_mut() {
        object.insert("revisions".into(), json!(revisions));
        object.insert(
            "env".into(),
            json!(env_names(at(&raw, "/template/containers/0/env"))),
        );
    }
    Ok(read)
}

/// The names of a container's environment variables, and which come from Secret Manager. Never their values.
pub fn env_names(env: &Value) -> Vec<Value> {
    env.as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            json!({
                "name": at(entry, "/name"),
                "secret": at(entry, "/valueSource/secretKeyRef/secret").as_str().map(short),
            })
        })
        .collect()
}

#[derive(Deserialize, Clone, Debug)]
pub struct Share {
    /// The revision, or none for whichever revision is latest.
    #[serde(default)]
    pub revision: Option<String>,
    pub percent: u32,
}

#[derive(Deserialize)]
pub struct Traffic {
    #[serde(flatten)]
    pub service: ServiceRef,
    /// The revision to send every request to; none sends them to the latest ready one.
    #[serde(default)]
    pub revision: Option<String>,
    /// A split between revisions instead, such as 90% on one and 10% on a canary; the shares must add up to 100.
    #[serde(default)]
    pub split: Option<Vec<Share>>,
}

/// Cloud Run's traffic targets for a split, refusing one that does not add up to 100.
pub fn targets_of(split: &[Share]) -> GcpResult<Vec<Value>> {
    let total: u32 = split.iter().map(|share| share.percent).sum();
    if total != 100 {
        return Err(GcpError::BadArg(format!(
            "the shares add up to {total}%, not 100%"
        )));
    }
    split
        .iter()
        .filter(|share| share.percent > 0)
        .map(|share| match share.revision.as_deref().map(str::trim).filter(|revision| !revision.is_empty()) {
            Some(revision) => Ok(json!({ "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION", "revision": segment("revision", revision)?, "percent": share.percent })),
            None => Ok(json!({ "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST", "percent": share.percent })),
        })
        .collect()
}

/// Sends all of a service's traffic to one revision, as a rollback does, or back to the latest.
pub async fn set_traffic(input: Traffic) -> GcpResult<Value> {
    let url = service_url(&input.service)?;
    let targets = match &input.split {
        Some(split) => targets_of(split)?,
        None => targets_of(&[Share {
            revision: input.revision.clone(),
            percent: 100,
        }])?,
    };
    let operation = client::patch(
        &input.service.target,
        &url,
        &[("updateMask", "traffic".into())],
        &json!({ "traffic": targets }),
    )
    .await?;
    Ok(json!({ "operation": short(&text(&operation, "/name")), "done": at(&operation, "/done") }))
}

/// The service as Google sent it, with only the template's image and scaling changed, ready to send back.
pub fn changed_template(
    mut raw: Value,
    image: Option<&str>,
    min: Option<u32>,
    max: Option<u32>,
) -> GcpResult<Value> {
    let template = raw
        .get_mut("template")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| GcpError::Response("the service has no template".into()))?;
    template.remove("revision");
    if let Some(image) = image {
        let container = template
            .get_mut("containers")
            .and_then(Value::as_array_mut)
            .and_then(|containers| containers.first_mut())
            .and_then(Value::as_object_mut)
            .ok_or_else(|| GcpError::Response("the service has no container".into()))?;
        container.insert("image".into(), json!(image));
    }
    if min.is_some() || max.is_some() {
        let scaling = template.entry("scaling").or_insert_with(|| json!({}));
        if let Some(scaling) = scaling.as_object_mut() {
            if let Some(min) = min {
                scaling.insert("minInstanceCount".into(), json!(min));
            }
            if let Some(max) = max {
                scaling.insert("maxInstanceCount".into(), json!(max));
            }
        }
    }
    Ok(raw)
}

/// An image address a deploy will take: a registry path with a tag or digest, nothing that reads as a flag or a URL.
pub fn image_ref(image: &str) -> GcpResult<String> {
    let image = image.trim();
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@');
    if image.is_empty()
        || image.contains("://")
        || image.starts_with('-')
        || !image.chars().all(allowed)
        || !image.contains('/')
    {
        return Err(GcpError::BadArg(format!(
            "`{image}` is not an image address such as us-docker.pkg.dev/project/repo/image:tag"
        )));
    }
    Ok(image.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deploy {
    #[serde(flatten)]
    pub service: ServiceRef,
    /// The image the new revision runs; none keeps the current one.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub min_instances: Option<u32>,
    #[serde(default)]
    pub max_instances: Option<u32>,
}

/// Deploys a new revision of a service with another image or other scaling, keeping every other setting.
pub async fn deploy(input: Deploy) -> GcpResult<Value> {
    let image = input
        .image
        .as_deref()
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .map(image_ref)
        .transpose()?;
    if image.is_none() && input.min_instances.is_none() && input.max_instances.is_none() {
        return Err(GcpError::BadArg(
            "name an image or the instances to scale to".into(),
        ));
    }
    if let (Some(min), Some(max)) = (input.min_instances, input.max_instances) {
        if min > max {
            return Err(GcpError::BadArg(format!(
                "at least {min} instances cannot fit in at most {max}"
            )));
        }
    }
    let url = service_url(&input.service)?;
    let raw = client::get(&input.service.target, &url, &[]).await?;
    let body = changed_template(
        raw,
        image.as_deref(),
        input.min_instances,
        input.max_instances,
    )?;
    let operation = client::patch(&input.service.target, &url, &[], &body).await?;
    Ok(json!({ "operation": short(&text(&operation, "/name")), "done": at(&operation, "/done") }))
}

/// Every Cloud Run job in the project, in every region.
pub async fn jobs(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/locations/-/jobs"),
        &[("pageSize", "100".into())],
        "jobs",
        MAX_ITEMS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    Ok(listed
        .iter()
        .map(job)
        .filter(|each| matches(&words, each))
        .collect())
}

#[derive(Deserialize)]
pub struct JobRef {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub job: String,
}

fn job_url(input: &JobRef) -> GcpResult<String> {
    let project = input.target.project()?;
    let location = segment("region", &input.location)?;
    let name = segment("job", &input.job)?;
    Ok(format!(
        "{API}/projects/{project}/locations/{location}/jobs/{name}"
    ))
}

/// One job with its recent executions.
pub async fn job_detail(input: JobRef) -> GcpResult<Value> {
    let url = job_url(&input)?;
    let (raw, executions) = futures::future::try_join(
        client::get(&input.target, &url, &[]),
        client::list(
            &input.target,
            &format!("{url}/executions"),
            &[("pageSize", "50".into())],
            "executions",
            50,
        ),
    )
    .await?;
    let mut read = job(&raw);
    if let Some(object) = read.as_object_mut() {
        object.insert(
            "recent".into(),
            json!(executions.iter().map(execution).collect::<Vec<_>>()),
        );
    }
    Ok(read)
}

/// Starts a job's tasks now.
pub async fn run_job(input: JobRef) -> GcpResult<Value> {
    let url = job_url(&input)?;
    let operation = client::post(&input.target, &format!("{url}:run"), &json!({})).await?;
    let execution = text(&operation, "/metadata/name");
    if operation.is_null() {
        return Err(GcpError::Response("Cloud Run did not start the job".into()));
    }
    Ok(json!({ "operation": short(&text(&operation, "/name")), "execution": short(&execution) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_service() -> Value {
        json!({
            "name": "projects/acme/locations/us-central1/services/api",
            "uri": "https://api-abc-uc.a.run.app",
            "latestReadyRevision": "projects/acme/locations/us-central1/services/api/revisions/api-00012-xyz",
            "latestCreatedRevision": "projects/acme/locations/us-central1/services/api/revisions/api-00012-xyz",
            "terminalCondition": { "type": "Ready", "state": "CONDITION_SUCCEEDED" },
            "template": { "containers": [{ "image": "us-docker.pkg.dev/acme/api:1.4", "resources": { "limits": { "cpu": "1", "memory": "512Mi" } },
                "env": [{ "name": "MODE", "value": "prod" }, { "name": "DB_PASSWORD", "valueSource": { "secretKeyRef": { "secret": "projects/1/secrets/db-password", "version": "latest" } } }] }],
                "scaling": { "maxInstanceCount": 10 } },
            "trafficStatuses": [
                { "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION", "revision": "api-00012-xyz", "percent": 90 },
                { "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION", "revision": "api-00011-abc", "percent": 10, "tag": "canary", "uri": "https://canary---api-abc-uc.a.run.app" }
            ],
            "lastModifier": "ankit@example.com"
        })
    }

    #[test]
    fn a_service_reads_with_its_region_health_and_traffic() {
        let read = service(&raw_service());
        assert_eq!(read["name"], "api");
        assert_eq!(read["location"], "us-central1");
        assert_eq!(read["state"], "READY");
        assert_eq!(read["latestRevision"], "api-00012-xyz");
        assert_eq!(read["memory"], "512Mi");
        assert_eq!(read["traffic"][1]["tag"], "canary");
        assert_eq!(read["traffic"][1]["percent"], 10);
        let mut deploying = raw_service();
        deploying["reconciling"] = json!(true);
        assert_eq!(service(&deploying)["state"], "DEPLOYING");
    }

    #[test]
    fn a_revision_carries_its_share_of_traffic() {
        let traffic = traffic_of(at(&raw_service(), "/trafficStatuses"));
        let read = revision(
            &json!({ "name": "projects/acme/locations/us-central1/services/api/revisions/api-00011-abc", "conditions": [{ "type": "Ready", "state": "CONDITION_SUCCEEDED" }], "containers": [{ "image": "img:1.3" }] }),
            &traffic,
        );
        assert_eq!(read["percent"], 10);
        assert_eq!(read["tags"], json!(["canary"]));
        assert_eq!(read["state"], "READY");
    }

    #[test]
    fn environment_variables_come_back_by_name_only() {
        let names = env_names(at(&raw_service(), "/template/containers/0/env"));
        assert_eq!(names.len(), 2);
        assert_eq!(names[1]["secret"], "db-password");
        assert!(!json!(names).to_string().contains("prod"));
    }

    #[test]
    fn a_split_must_add_up_to_a_hundred() {
        let split = [
            Share {
                revision: Some("api-00012-xyz".into()),
                percent: 90,
            },
            Share {
                revision: Some("api-00013-abc".into()),
                percent: 10,
            },
            Share {
                revision: None,
                percent: 0,
            },
        ];
        let targets = targets_of(&split).expect("adds up");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[1]["percent"], 10);
        assert!(targets_of(&[Share {
            revision: None,
            percent: 90
        }])
        .is_err());
        assert_eq!(
            targets_of(&[Share {
                revision: None,
                percent: 100
            }])
            .expect("latest")[0]["type"],
            "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
        );
        assert!(targets_of(&[Share {
            revision: Some("../x".into()),
            percent: 100
        }])
        .is_err());
    }

    #[test]
    fn a_deploy_changes_only_the_image_and_scaling() {
        let raw = raw_service();
        let mut with_revision = raw.clone();
        with_revision["template"]["revision"] = json!("api-00012-xyz");
        let changed = changed_template(
            with_revision,
            Some("us-docker.pkg.dev/acme/api:1.5"),
            Some(1),
            None,
        )
        .expect("changed");
        assert_eq!(
            changed["template"]["containers"][0]["image"],
            "us-docker.pkg.dev/acme/api:1.5"
        );
        assert_eq!(changed["template"]["scaling"]["minInstanceCount"], 1);
        assert_eq!(changed["template"]["scaling"]["maxInstanceCount"], 10);
        assert_eq!(
            changed["template"]["containers"][0]["env"],
            raw["template"]["containers"][0]["env"]
        );
        assert!(changed["template"].get("revision").is_none());
        assert!(changed_template(json!({}), None, None, None).is_err());
    }

    #[test]
    fn an_image_address_is_a_registry_path() {
        assert!(image_ref("us-docker.pkg.dev/acme/web/storefront:2.4.0").is_ok());
        assert!(image_ref("gcr.io/acme/api@sha256:abc").is_ok());
        assert!(image_ref("https://evil.example/x").is_err());
        assert!(image_ref("--flag").is_err());
        assert!(image_ref("storefront").is_err());
        assert!(image_ref("us-docker.pkg.dev/acme/web/storefront:2.4.0; rm").is_err());
    }

    #[test]
    fn an_execution_says_how_it_went() {
        assert_eq!(execution(&json!({ "runningCount": 2 }))["state"], "RUNNING");
        assert_eq!(
            execution(&json!({ "failedCount": 1, "completionTime": "t" }))["state"],
            "FAILED"
        );
        assert_eq!(
            execution(&json!({ "succeededCount": 3, "completionTime": "t" }))["state"],
            "SUCCEEDED"
        );
        assert_eq!(
            location_of("projects/p/locations/europe-west1/jobs/nightly"),
            "europe-west1"
        );
    }
}
