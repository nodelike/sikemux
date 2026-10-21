// Cloud Scheduler: cron jobs with their schedule, target and last run, and
// running one now, pausing or resuming it.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::GcpResult;
use crate::run::location_of;

const API: &str = "https://cloudscheduler.googleapis.com/v1";

fn target_of(value: &Value) -> Value {
    if let Some(http) = value.get("httpTarget") {
        return json!({ "kind": "http", "method": at(http, "/httpMethod"), "uri": at(http, "/uri") });
    }
    if let Some(pubsub) = value.get("pubsubTarget") {
        return json!({ "kind": "pubsub", "topic": short(&text(pubsub, "/topicName")) });
    }
    if let Some(engine) = value.get("appEngineHttpTarget") {
        return json!({ "kind": "appEngine", "uri": at(engine, "/relativeUri") });
    }
    Value::Null
}

/// A job as the pane and agents read it, with how its last run went.
pub fn job(value: &Value) -> Value {
    let name = text(value, "/name");
    let code = at(value, "/status/code").as_i64().unwrap_or(0);
    json!({
        "name": short(&name),
        "location": location_of(&name),
        "description": at(value, "/description"),
        "schedule": at(value, "/schedule"),
        "timeZone": at(value, "/timeZone"),
        "state": at(value, "/state"),
        "target": target_of(value),
        "lastRunAt": at(value, "/lastAttemptTime"),
        "nextRunAt": at(value, "/scheduleTime"),
        "lastRunOk": if value.get("lastAttemptTime").is_none() { Value::Null } else { json!(code == 0) },
        "lastRunError": at(value, "/status/message"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Every Scheduler job in the project, in each region Scheduler runs in.
pub async fn jobs(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let locations = client::get(
        &input.target,
        &format!("{API}/projects/{project}/locations"),
        &[],
    )
    .await?;
    let regions: Vec<String> = at(&locations, "/locations")
        .as_array()
        .into_iter()
        .flatten()
        .map(|location| text(location, "/locationId"))
        .filter(|id| !id.is_empty())
        .collect();
    let lists = futures::future::join_all(regions.iter().map(|region| {
        let target = input.target.clone();
        let url = format!("{API}/projects/{project}/locations/{region}/jobs");
        async move {
            client::list(&target, &url, &[("pageSize", "500".into())], "jobs", 500)
                .await
                .unwrap_or_default()
        }
    }))
    .await;
    let mut read: Vec<Value> = lists.iter().flatten().map(job).collect();
    read.sort_by_key(|each| text(each, "/name"));
    Ok(read)
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Run,
    Pause,
    Resume,
}

#[derive(Deserialize)]
pub struct JobChange {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub job: String,
    pub action: Change,
}

/// Runs a job now, or pauses or resumes its schedule.
pub async fn change(input: JobChange) -> GcpResult<Value> {
    let project = input.target.project()?;
    let verb = match input.action {
        Change::Run => "run",
        Change::Pause => "pause",
        Change::Resume => "resume",
    };
    let url = format!(
        "{API}/projects/{project}/locations/{}/jobs/{}:{verb}",
        segment("region", &input.location)?,
        segment("job", &input.job)?
    );
    Ok(job(&client::post(&input.target, &url, &json!({})).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_reads_with_its_target_and_last_run() {
        let read = job(
            &json!({ "name": "projects/acme/locations/us-central1/jobs/nightly-export", "schedule": "0 3 * * *", "timeZone": "Europe/London", "state": "ENABLED",
            "httpTarget": { "uri": "https://nightly-export-x7k2q-uc.a.run.app", "httpMethod": "POST" }, "lastAttemptTime": "t", "status": { "code": 13, "message": "upstream timed out" } }),
        );
        assert_eq!(read["location"], "us-central1");
        assert_eq!(read["target"]["kind"], "http");
        assert_eq!(read["lastRunOk"], false);
        assert_eq!(read["lastRunError"], "upstream timed out");
        let pubsub = job(
            &json!({ "name": "x", "pubsubTarget": { "topicName": "projects/acme/topics/ticks" } }),
        );
        assert_eq!(pubsub["target"]["topic"], "ticks");
        assert_eq!(pubsub["lastRunOk"], Value::Null);
    }
}
