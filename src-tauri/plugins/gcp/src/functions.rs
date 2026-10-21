// Cloud Functions, both generations: each function's runtime, trigger, URL,
// size and state, with its environment variables by name only.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::GcpResult;
use crate::run::location_of;

const API: &str = "https://cloudfunctions.googleapis.com/v2";
const MAX_ITEMS: usize = 500;

fn trigger_of(value: &Value) -> Value {
    let event = at(value, "/eventTrigger");
    if event.is_null() {
        return json!({ "kind": "http" });
    }
    json!({
        "kind": "event",
        "event": at(event, "/eventType"),
        "topic": at(event, "/pubsubTopic").as_str().map(short),
        "retry": text(event, "/retryPolicy") == "RETRY_POLICY_RETRY",
    })
}

/// A function as the pane and agents read it.
pub fn function(value: &Value) -> Value {
    let name = text(value, "/name");
    let env: Vec<String> = at(value, "/serviceConfig/environmentVariables")
        .as_object()
        .map(|vars| vars.keys().cloned().collect())
        .unwrap_or_default();
    let secrets: Vec<String> = at(value, "/serviceConfig/secretEnvironmentVariables")
        .as_array()
        .into_iter()
        .flatten()
        .map(|secret| text(secret, "/key"))
        .collect();
    json!({
        "name": short(&name),
        "location": location_of(&name),
        "state": at(value, "/state"),
        "generation": if text(value, "/environment") == "GEN_1" { "1st gen" } else { "2nd gen" },
        "runtime": at(value, "/buildConfig/runtime"),
        "entryPoint": at(value, "/buildConfig/entryPoint"),
        "url": at(value, "/serviceConfig/uri").as_str().or_else(|| at(value, "/url").as_str()),
        "memory": at(value, "/serviceConfig/availableMemory"),
        "timeoutSeconds": at(value, "/serviceConfig/timeoutSeconds"),
        "maxInstances": at(value, "/serviceConfig/maxInstanceCount"),
        "serviceAccount": at(value, "/serviceConfig/serviceAccountEmail"),
        "runService": at(value, "/serviceConfig/service").as_str().map(short),
        "trigger": trigger_of(value),
        "env": env,
        "secretEnv": secrets,
        "updatedAt": at(value, "/updateTime"),
        "message": at(value, "/stateMessages/0/message"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub query: Option<String>,
}

pub async fn functions(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/locations/-/functions"),
        &[("pageSize", "500".into())],
        "functions",
        MAX_ITEMS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    let mut read: Vec<Value> = listed
        .iter()
        .map(function)
        .filter(|each| words.is_empty() || text(each, "/name").to_lowercase().contains(&words))
        .collect();
    read.sort_by_key(|each| text(each, "/name"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct FunctionRef {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub function: String,
}

pub async fn function_detail(input: FunctionRef) -> GcpResult<Value> {
    let project = input.target.project()?;
    let url = format!(
        "{API}/projects/{project}/locations/{}/functions/{}",
        segment("region", &input.location)?,
        segment("function", &input.function)?
    );
    Ok(function(&client::get(&input.target, &url, &[]).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_function_reads_with_its_trigger_and_variable_names() {
        let read = function(&json!({
            "name": "projects/acme/locations/us-central1/functions/resize-image", "state": "ACTIVE", "environment": "GEN_2",
            "buildConfig": { "runtime": "nodejs20", "entryPoint": "resize" },
            "serviceConfig": { "uri": "https://resize-image-abc.a.run.app", "availableMemory": "512M", "environmentVariables": { "BUCKET": "acme-images" }, "secretEnvironmentVariables": [{ "key": "API_KEY", "secret": "api-key" }], "service": "projects/acme/locations/us-central1/services/resize-image" },
            "eventTrigger": { "eventType": "google.cloud.pubsub.topic.v1.messagePublished", "pubsubTopic": "projects/acme/topics/uploads", "retryPolicy": "RETRY_POLICY_RETRY" }
        }));
        assert_eq!(read["location"], "us-central1");
        assert_eq!(read["generation"], "2nd gen");
        assert_eq!(read["trigger"]["topic"], "uploads");
        assert_eq!(read["trigger"]["retry"], true);
        assert_eq!(read["env"], json!(["BUCKET"]));
        assert_eq!(read["secretEnv"], json!(["API_KEY"]));
        assert!(!read.to_string().contains("acme-images"));
        assert_eq!(function(&json!({ "name": "x" }))["trigger"]["kind"], "http");
    }
}
