// A project's build settings: framework, commands, root directory and Node version,
// read and changed. Only these settings can be changed through here.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::{segment, ProjectRef};

/// The settings people change, as Vercel names them.
const COMMANDS: &[&str] = &[
    "buildCommand",
    "outputDirectory",
    "installCommand",
    "devCommand",
    "rootDirectory",
];

/// The Node versions Vercel builds with.
pub const NODE_VERSIONS: &[&str] = &["24.x", "22.x", "20.x"];

/// A project's settings as the pane and agents read them; an empty command is the framework's default.
pub fn settings_of(project: &Value) -> Value {
    let mut read = Map::new();
    read.insert("id".into(), at(project, "/id").clone());
    read.insert("name".into(), at(project, "/name").clone());
    read.insert("framework".into(), at(project, "/framework").clone());
    for key in COMMANDS {
        read.insert((*key).into(), at(project, &format!("/{key}")).clone());
    }
    read.insert("nodeVersion".into(), at(project, "/nodeVersion").clone());
    read.insert("nodeVersions".into(), json!(NODE_VERSIONS));
    Value::Object(read)
}

/// A project's cron jobs as its production deployment declared them in vercel.json.
pub fn crons_of(project: &Value) -> Value {
    let crons = at(project, "/crons");
    let jobs: Vec<Value> = at(crons, "/definitions")
        .as_array()
        .into_iter()
        .flatten()
        .map(|job| {
            json!({
                "path": at(job, "/path"),
                "schedule": at(job, "/schedule"),
                "host": at(job, "/host"),
            })
        })
        .collect();
    json!({
        "enabled": !crons.is_null() && at(crons, "/disabledAt").is_null(),
        "deploymentId": at(crons, "/deploymentId"),
        "updatedAt": at(crons, "/updatedAt"),
        "jobs": jobs,
    })
}

pub async fn crons(data_dir: &Path, input: ProjectRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let data = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    Ok(crons_of(&data))
}

pub async fn settings(data_dir: &Path, input: ProjectRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let data = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    Ok(settings_of(&data))
}

#[derive(Deserialize)]
pub struct Change {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    /// The settings to change. A command set to null or "" goes back to the framework's default.
    pub settings: Map<String, Value>,
}

/// The body Vercel is sent: only settings people may change, each checked.
pub fn patch_of(settings: &Map<String, Value>) -> VercelResult<Value> {
    let mut patch = Map::new();
    for (key, value) in settings {
        let value = match value {
            Value::String(text) if text.trim().is_empty() => Value::Null,
            Value::String(text) => json!(text.trim()),
            Value::Null => Value::Null,
            _ => return Err(VercelError::BadArg(format!("{key} is text"))),
        };
        match key.as_str() {
            "framework" => {}
            "nodeVersion" => {
                let version = value.as_str().unwrap_or_default();
                if !NODE_VERSIONS.contains(&version) {
                    return Err(VercelError::BadArg(format!(
                        "nodeVersion is one of {}",
                        NODE_VERSIONS.join(", ")
                    )));
                }
            }
            "rootDirectory" => {
                if value.as_str().is_some_and(|dir| {
                    dir.starts_with('/') || dir.split('/').any(|part| part == "..")
                }) {
                    return Err(VercelError::BadArg(
                        "rootDirectory is a folder inside the repository".into(),
                    ));
                }
            }
            known if COMMANDS.contains(&known) => {}
            other => {
                return Err(VercelError::BadArg(format!(
                    "{other} can't be changed from Sikemux"
                )))
            }
        }
        patch.insert(key.clone(), value);
    }
    if patch.is_empty() {
        return Err(VercelError::BadArg("no setting was changed".into()));
    }
    Ok(Value::Object(patch))
}

pub async fn update(data_dir: &Path, input: Change) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let patch = patch_of(&input.settings)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let data = session
        .send(
            Method::PATCH,
            &format!("/v9/projects/{project}"),
            &[],
            Some(patch),
        )
        .await?;
    Ok(settings_of(&data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_with_defaults_left_empty() {
        let raw = json!({
            "id": "prj_1", "name": "web", "framework": "nextjs", "buildCommand": null,
            "outputDirectory": "dist", "rootDirectory": "apps/web", "nodeVersion": "22.x",
            "env": [{ "key": "SECRET", "value": "hunter2" }]
        });
        let read = settings_of(&raw);
        assert_eq!(read["framework"], "nextjs");
        assert_eq!(read["buildCommand"], Value::Null);
        assert_eq!(read["outputDirectory"], "dist");
        assert_eq!(read["rootDirectory"], "apps/web");
        assert!(!read.to_string().contains("hunter2"));
    }

    #[test]
    fn crons_read_from_what_production_declared() {
        let raw = json!({ "crons": {
            "enabledAt": 1, "disabledAt": null, "updatedAt": 5, "deploymentId": "dpl_9",
            "definitions": [{ "host": "shop.acme.dev", "path": "/api/cron/sync", "schedule": "0 9 * * *" }]
        } });
        let read = crons_of(&raw);
        assert_eq!(read["enabled"], true);
        assert_eq!(read["jobs"][0]["path"], "/api/cron/sync");
        assert_eq!(read["jobs"][0]["schedule"], "0 9 * * *");
        let none = crons_of(&json!({}));
        assert_eq!(
            (none["enabled"].clone(), none["jobs"].clone()),
            (json!(false), json!([]))
        );
        let off = crons_of(&json!({ "crons": { "disabledAt": 7, "definitions": [] } }));
        assert_eq!(off["enabled"], false);
    }

    #[test]
    fn a_change_keeps_to_the_settings_people_change() {
        let settings = json!({ "buildCommand": "  pnpm build ", "devCommand": "", "framework": null, "nodeVersion": "20.x" });
        let patch = patch_of(settings.as_object().unwrap()).unwrap();
        assert_eq!(
            patch,
            json!({ "buildCommand": "pnpm build", "devCommand": null, "framework": null, "nodeVersion": "20.x" })
        );
        let name = json!({ "name": "other" });
        assert!(patch_of(name.as_object().unwrap()).is_err());
        let node = json!({ "nodeVersion": "12.x" });
        assert!(patch_of(node.as_object().unwrap()).is_err());
        let climb = json!({ "rootDirectory": "../secrets" });
        assert!(patch_of(climb.as_object().unwrap()).is_err());
        assert!(patch_of(&Map::new()).is_err());
    }
}
