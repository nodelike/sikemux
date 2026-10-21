// Secret Manager, by name only: secrets with their replication, rotation and
// versions, and enabling or disabling a version. Nothing here ever asks for a
// secret's value, so none can reach the pane or an agent.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::GcpResult;

const API: &str = "https://secretmanager.googleapis.com/v1";
const MAX_ITEMS: usize = 1000;

pub fn secret(value: &Value) -> Value {
    let replication = if value.pointer("/replication/automatic").is_some() {
        json!("automatic")
    } else {
        json!(at(value, "/replication/userManaged/replicas")
            .as_array()
            .into_iter()
            .flatten()
            .map(|replica| text(replica, "/location"))
            .collect::<Vec<_>>())
    };
    json!({
        "name": short(&text(value, "/name")),
        "replication": replication,
        "createdAt": at(value, "/createTime"),
        "expiresAt": at(value, "/expireTime"),
        "nextRotation": at(value, "/rotation/nextRotationTime"),
        "rotationPeriod": at(value, "/rotation/rotationPeriod"),
        "aliases": at(value, "/versionAliases"),
        "labels": at(value, "/labels"),
    })
}

pub fn version(value: &Value) -> Value {
    json!({
        "version": short(&text(value, "/name")),
        "state": at(value, "/state"),
        "createdAt": at(value, "/createTime"),
        "destroyedAt": at(value, "/destroyTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub query: Option<String>,
}

pub async fn secrets(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/secrets"),
        &[("pageSize", "250".into())],
        "secrets",
        MAX_ITEMS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    let mut read: Vec<Value> = listed
        .iter()
        .map(secret)
        .filter(|each| words.is_empty() || text(each, "/name").to_lowercase().contains(&words))
        .collect();
    read.sort_by_key(|each| text(each, "/name"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct SecretRef {
    #[serde(flatten)]
    pub target: Target,
    pub secret: String,
}

fn secret_url(input: &SecretRef) -> GcpResult<String> {
    let project = input.target.project()?;
    Ok(format!(
        "{API}/projects/{project}/secrets/{}",
        segment("secret", &input.secret)?
    ))
}

/// One secret with its versions, newest first, and never their values.
pub async fn secret_detail(input: SecretRef) -> GcpResult<Value> {
    let url = secret_url(&input)?;
    let (raw, versions) = futures::future::try_join(
        client::get(&input.target, &url, &[]),
        client::list(
            &input.target,
            &format!("{url}/versions"),
            &[("pageSize", "100".into())],
            "versions",
            200,
        ),
    )
    .await?;
    let mut read = secret(&raw);
    if let Some(object) = read.as_object_mut() {
        object.insert(
            "versions".into(),
            json!(versions.iter().map(version).collect::<Vec<_>>()),
        );
    }
    Ok(read)
}

#[derive(Deserialize)]
pub struct VersionChange {
    #[serde(flatten)]
    pub secret: SecretRef,
    pub version: String,
    /// Whether the version should be usable.
    pub enabled: bool,
}

/// Turns one version of a secret on or off; a version turned off can be turned on again.
pub async fn set_version(input: VersionChange) -> GcpResult<Value> {
    let verb = if input.enabled { "enable" } else { "disable" };
    let url = format!(
        "{}/versions/{}:{verb}",
        secret_url(&input.secret)?,
        segment("version", &input.version)?
    );
    let changed = client::post(&input.secret.target, &url, &json!({})).await?;
    Ok(version(&changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_reads_without_any_value() {
        let read = secret(
            &json!({ "name": "projects/1/secrets/db-password", "replication": { "userManaged": { "replicas": [{ "location": "us-central1" }, { "location": "europe-west1" }] } }, "rotation": { "rotationPeriod": "2592000s" } }),
        );
        assert_eq!(read["name"], "db-password");
        assert_eq!(read["replication"], json!(["us-central1", "europe-west1"]));
        assert_eq!(
            secret(&json!({ "name": "x", "replication": { "automatic": {} } }))["replication"],
            "automatic"
        );
        assert_eq!(
            version(
                &json!({ "name": "projects/1/secrets/db-password/versions/3", "state": "ENABLED" })
            )["version"],
            "3"
        );
    }

    #[test]
    fn nothing_here_asks_for_a_value() {
        let source = include_str!("secrets.rs");
        let asks = ["access", "\")"].concat();
        assert!(
            !source.contains(&format!(":{asks}")),
            "Secret Manager's :access call reads values"
        );
    }
}
