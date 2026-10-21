// Cloud SQL: instances with their databases, users and backups, starting,
// stopping, restarting and backing one up, and the command that connects to it.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://sqladmin.googleapis.com/v1";

fn address(value: &Value, kind: &str) -> Value {
    at(value, "/ipAddresses")
        .as_array()
        .into_iter()
        .flatten()
        .find(|address| text(address, "/type") == kind)
        .map(|address| at(address, "/ipAddress").clone())
        .unwrap_or(Value::Null)
}

/// Running, stopped or busy, from the instance's state and whether it is set to run.
fn state_of(value: &Value) -> &'static str {
    match (
        text(value, "/state").as_str(),
        text(value, "/settings/activationPolicy").as_str(),
    ) {
        ("RUNNABLE", "NEVER") => "STOPPED",
        ("RUNNABLE", _) => "RUNNING",
        ("SUSPENDED", _) => "SUSPENDED",
        ("PENDING_CREATE", _) => "CREATING",
        ("MAINTENANCE", _) => "MAINTENANCE",
        ("FAILED", _) => "FAILED",
        _ => "UNKNOWN",
    }
}

/// An instance as the pane and agents read it.
pub fn instance(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "engine": at(value, "/databaseVersion"),
        "region": at(value, "/region"),
        "zone": at(value, "/gceZone"),
        "state": state_of(value),
        "tier": at(value, "/settings/tier"),
        "diskGb": at(value, "/settings/dataDiskSizeGb").as_str().and_then(|size| size.parse::<u64>().ok()),
        "highAvailability": text(value, "/settings/availabilityType") == "REGIONAL",
        "backups": at(value, "/settings/backupConfiguration/enabled").as_bool() == Some(true),
        "backupWindow": at(value, "/settings/backupConfiguration/startTime"),
        "publicIp": address(value, "PRIMARY"),
        "privateIp": address(value, "PRIVATE"),
        "connectionName": at(value, "/connectionName"),
        "createdAt": at(value, "/createTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

pub async fn instances(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let page = client::get(
        &input.target,
        &format!("{API}/projects/{project}/instances"),
        &[],
    )
    .await?;
    let mut listed: Vec<Value> = at(&page, "/items")
        .as_array()
        .into_iter()
        .flatten()
        .map(instance)
        .collect();
    listed.sort_by_key(|each| text(each, "/name"));
    Ok(listed)
}

#[derive(Deserialize)]
pub struct InstanceRef {
    #[serde(flatten)]
    pub target: Target,
    pub instance: String,
}

fn instance_url(input: &InstanceRef) -> GcpResult<String> {
    let project = input.target.project()?;
    Ok(format!(
        "{API}/projects/{project}/instances/{}",
        segment("instance", &input.instance)?
    ))
}

/// One instance with its databases, its users by name, its recent backups and the command that connects to it.
pub async fn instance_detail(input: InstanceRef) -> GcpResult<Value> {
    let url = instance_url(&input)?;
    let (raw, databases, users, backups) = futures::future::try_join4(
        client::get(&input.target, &url, &[]),
        client::get(&input.target, &format!("{url}/databases"), &[]),
        client::get(&input.target, &format!("{url}/users"), &[]),
        client::get(
            &input.target,
            &format!("{url}/backupRuns"),
            &[("maxResults", "20".into())],
        ),
    )
    .await?;
    let mut read = instance(&raw);
    if let Some(object) = read.as_object_mut() {
        object.insert(
            "databases".into(),
            json!(at(&databases, "/items")
                .as_array()
                .into_iter()
                .flatten()
                .map(|db| json!({ "name": at(db, "/name"), "charset": at(db, "/charset") }))
                .collect::<Vec<_>>()),
        );
        object.insert(
            "users".into(),
            json!(at(&users, "/items").as_array().into_iter().flatten().map(|user| json!({ "name": at(user, "/name"), "host": at(user, "/host"), "type": at(user, "/type") })).collect::<Vec<_>>()),
        );
        object.insert(
            "recentBackups".into(),
            json!(at(&backups, "/items").as_array().into_iter().flatten().map(|run| json!({ "id": at(run, "/id"), "status": at(run, "/status"), "type": at(run, "/type"), "startedAt": at(run, "/startTime"), "endedAt": at(run, "/endTime") })).collect::<Vec<_>>()),
        );
        object.insert("connect".into(), json!(connect_command(&input)?));
    }
    Ok(read)
}

/// `gcloud sql connect`, which opens the engine's own client through a short-lived allowlist, as an argument list.
pub fn connect_command(input: &InstanceRef) -> GcpResult<Vec<String>> {
    let project = input.target.project()?;
    let mut command = vec![
        "gcloud".to_string(),
        "sql".to_string(),
        "connect".to_string(),
        segment("instance", &input.instance)?,
        format!("--project={project}"),
    ];
    if let Some(account) = input.target.account() {
        command.push(format!("--account={}", crate::gcloud::value(account)?));
    }
    Ok(command)
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Start,
    Stop,
    Restart,
    Backup,
}

#[derive(Deserialize)]
pub struct InstanceChange {
    #[serde(flatten)]
    pub instance: InstanceRef,
    pub action: Change,
}

/// Starts, stops, restarts or backs up an instance, answering with the operation Google started.
pub async fn change(input: InstanceChange) -> GcpResult<Value> {
    let url = instance_url(&input.instance)?;
    let target = &input.instance.target;
    let operation = match input.action {
        Change::Start | Change::Stop => {
            let policy = if input.action == Change::Start {
                "ALWAYS"
            } else {
                "NEVER"
            };
            client::patch(
                target,
                &url,
                &[],
                &json!({ "settings": { "activationPolicy": policy } }),
            )
            .await?
        }
        Change::Restart => client::post(target, &format!("{url}/restart"), &json!({})).await?,
        Change::Backup => {
            client::post(
                target,
                &format!("{url}/backupRuns"),
                &json!({ "description": "Backed up from Sikemux" }),
            )
            .await?
        }
    };
    if operation.is_null() {
        return Err(GcpError::Response("Cloud SQL started nothing".into()));
    }
    Ok(
        json!({ "operation": at(&operation, "/name"), "status": at(&operation, "/status"), "type": at(&operation, "/operationType") }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_reads_with_its_state_addresses_and_backups() {
        let read = instance(&json!({
            "name": "orders", "databaseVersion": "POSTGRES_16", "region": "us-central1", "state": "RUNNABLE",
            "settings": { "tier": "db-custom-2-7680", "activationPolicy": "ALWAYS", "availabilityType": "REGIONAL", "dataDiskSizeGb": "100", "backupConfiguration": { "enabled": true, "startTime": "03:00" } },
            "ipAddresses": [{ "type": "PRIMARY", "ipAddress": "34.9.9.9" }, { "type": "PRIVATE", "ipAddress": "10.1.0.3" }],
            "connectionName": "acme:us-central1:orders"
        }));
        assert_eq!(read["state"], "RUNNING");
        assert_eq!(read["highAvailability"], true);
        assert_eq!(read["diskGb"], 100);
        assert_eq!(read["publicIp"], "34.9.9.9");
        assert_eq!(read["privateIp"], "10.1.0.3");
        assert_eq!(
            instance(&json!({ "state": "RUNNABLE", "settings": { "activationPolicy": "NEVER" } }))
                ["state"],
            "STOPPED"
        );
    }

    #[test]
    fn connecting_goes_through_gcloud_sql_connect() {
        let input = InstanceRef {
            target: Target {
                account: None,
                project: Some("acme".into()),
            },
            instance: "orders".into(),
        };
        assert_eq!(
            connect_command(&input).expect("a command"),
            ["gcloud", "sql", "connect", "orders", "--project=acme"]
        );
    }
}
