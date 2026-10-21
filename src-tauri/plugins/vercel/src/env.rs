// Adding, changing and removing a project's environment variables. A value only
// ever goes to Vercel; what comes back is stripped of it, as the listing is.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::{segment, variable};

const TARGETS: &[&str] = &["production", "preview", "development"];

fn checked_key(key: &str) -> VercelResult<String> {
    let key = key.trim();
    let mut chars = key.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(VercelError::BadArg(format!(
            "`{key}` is not a variable name: letters, digits and _, not starting with a digit"
        )));
    }
    Ok(key.to_string())
}

fn checked_targets(targets: &[String]) -> VercelResult<Vec<String>> {
    if targets.is_empty() {
        return Err(VercelError::BadArg(
            "pick at least one of production, preview and development".into(),
        ));
    }
    let mut checked = Vec::new();
    for target in targets {
        let target = target.trim().to_lowercase();
        if !TARGETS.contains(&target.as_str()) {
            return Err(VercelError::BadArg(format!(
                "`{target}` is not production, preview or development"
            )));
        }
        if !checked.contains(&target) {
            checked.push(target);
        }
    }
    Ok(checked)
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Readable by the team in Vercel's dashboard.
    #[default]
    Encrypted,
    /// Never readable again, even in Vercel's dashboard.
    Sensitive,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Set {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    /// The variable to change, by id; without one a new variable is added, or one of the same name and targets replaced.
    #[serde(default)]
    pub id: Option<String>,
    pub key: String,
    /// The new value. Left out when changing a variable, its value stays as it is.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    /// Only for deployments of this branch; the variable must target preview alone.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub kind: Kind,
}

/// The body Vercel is sent to add or change a variable, checked first.
pub fn body_of(input: &Set) -> VercelResult<Value> {
    let key = checked_key(&input.key)?;
    let targets = checked_targets(&input.targets)?;
    let branch = input
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty());
    if branch.is_some() && targets != ["preview"] {
        return Err(VercelError::BadArg(
            "a variable for one branch targets preview alone".into(),
        ));
    }
    if input.kind == Kind::Sensitive && targets.iter().any(|target| target == "development") {
        return Err(VercelError::BadArg(
            "a sensitive variable can't target development".into(),
        ));
    }
    let mut body = Map::new();
    body.insert("key".into(), json!(key));
    body.insert("target".into(), json!(targets));
    body.insert(
        "gitBranch".into(),
        branch.map_or(Value::Null, |branch| json!(branch)),
    );
    match (&input.value, &input.id) {
        (Some(value), _) => {
            body.insert("value".into(), json!(value));
        }
        (None, None) => return Err(VercelError::BadArg("a new variable needs a value".into())),
        (None, Some(_)) => {}
    }
    if input.id.is_none() {
        let kind = match input.kind {
            Kind::Encrypted => "encrypted",
            Kind::Sensitive => "sensitive",
        };
        body.insert("type".into(), json!(kind));
    }
    Ok(Value::Object(body))
}

/// The variable Vercel answered with, without its value, however Vercel wrapped it.
pub fn saved_of(answer: &Value) -> Value {
    let created = at(answer, "/created");
    let one = match created {
        Value::Array(many) => many.first().unwrap_or(&Value::Null),
        Value::Object(_) => created,
        _ => answer,
    };
    variable(one)
}

pub async fn set(data_dir: &Path, input: Set) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let body = body_of(&input)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let answer = match &input.id {
        Some(id) => {
            let id = segment("variable", id)?;
            session
                .send(
                    Method::PATCH,
                    &format!("/v9/projects/{project}/env/{id}"),
                    &[],
                    Some(body),
                )
                .await?
        }
        None => {
            session
                .send(
                    Method::POST,
                    &format!("/v10/projects/{project}/env"),
                    &[("upsert", "true".into())],
                    Some(body),
                )
                .await?
        }
    };
    Ok(saved_of(&answer))
}

#[derive(Deserialize)]
pub struct Remove {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    /// The variable, by its id.
    #[serde(default)]
    pub id: Option<String>,
    /// Or by its name, when one variable has it.
    #[serde(default)]
    pub key: Option<String>,
}

pub async fn remove(data_dir: &Path, input: Remove) -> VercelResult<()> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let id = match (input.id.as_deref(), input.key.as_deref()) {
        (Some(id), _) => segment("variable", id)?,
        (None, Some(key)) => id_of_key(&session, &project, &checked_key(key)?).await?,
        (None, None) => return Err(VercelError::BadArg("name the variable by id or key".into())),
    };
    session
        .send(
            Method::DELETE,
            &format!("/v9/projects/{project}/env/{id}"),
            &[],
            None,
        )
        .await?;
    Ok(())
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Set,
    Remove,
}

/// One call for agents, which set or remove by name.
pub async fn change(data_dir: &Path, input: Value) -> VercelResult<Value> {
    let action: Action = serde_json::from_value(at(&input, "/action").clone())
        .map_err(|_| VercelError::BadArg("action is set or remove".into()))?;
    match action {
        Action::Set => {
            let input: Set = serde_json::from_value(input)
                .map_err(|error| VercelError::BadArg(error.to_string()))?;
            set(data_dir, input).await
        }
        Action::Remove => {
            let input: Remove = serde_json::from_value(input)
                .map_err(|error| VercelError::BadArg(error.to_string()))?;
            remove(data_dir, input)
                .await
                .map(|()| json!({ "removed": true }))
        }
    }
}

/// The id of the one variable with this name; several with it is a question for the person.
async fn id_of_key(session: &Session, project: &str, key: &str) -> VercelResult<String> {
    let data = session
        .get(&format!("/v10/projects/{project}/env"), &[])
        .await?;
    let ids = ids_named(&data, key);
    match ids.as_slice() {
        [one] => segment("variable", one),
        [] => Err(VercelError::NotFound(format!(
            "{project} has no variable named {key}"
        ))),
        _ => Err(VercelError::BadArg(format!(
            "{project} has {} variables named {key}, one per environment; name one by its id",
            ids.len()
        ))),
    }
}

pub fn ids_named(listing: &Value, key: &str) -> Vec<String> {
    at(listing, "/envs")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|each| at(each, "/key").as_str() == Some(key))
        .filter_map(|each| at(each, "/id").as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(value: Value) -> Set {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_new_variable_is_checked_and_sent_with_its_value() {
        let body = body_of(&set(json!({ "project": "web", "key": " API_URL ", "value": "https://api", "targets": ["Production", "preview", "preview"] }))).unwrap();
        assert_eq!(
            body,
            json!({ "key": "API_URL", "value": "https://api", "target": ["production", "preview"], "gitBranch": null, "type": "encrypted" })
        );
        assert!(body_of(&set(
            json!({ "project": "web", "key": "API_URL", "targets": ["preview"] })
        ))
        .is_err());
        assert!(body_of(&set(
            json!({ "project": "web", "key": "1BAD", "value": "x", "targets": ["preview"] })
        ))
        .is_err());
        assert!(body_of(&set(
            json!({ "project": "web", "key": "A", "value": "x", "targets": [] })
        ))
        .is_err());
        assert!(body_of(&set(
            json!({ "project": "web", "key": "A", "value": "x", "targets": ["staging"] })
        ))
        .is_err());
    }

    #[test]
    fn a_branch_variable_targets_preview_alone_and_a_sensitive_one_never_development() {
        let branch = body_of(&set(json!({ "project": "web", "key": "A", "value": "x", "targets": ["preview"], "branch": "feat/x" }))).unwrap();
        assert_eq!(branch["gitBranch"], "feat/x");
        assert!(body_of(&set(json!({ "project": "web", "key": "A", "value": "x", "targets": ["production", "preview"], "branch": "feat/x" }))).is_err());
        assert!(body_of(&set(json!({ "project": "web", "key": "A", "value": "x", "targets": ["development"], "kind": "sensitive" }))).is_err());
    }

    #[test]
    fn changing_a_variable_keeps_its_value_unless_a_new_one_is_given() {
        let body = body_of(&set(
            json!({ "project": "web", "id": "env_1", "key": "A", "targets": ["production"] }),
        ))
        .unwrap();
        assert!(body.get("value").is_none());
        assert!(body.get("type").is_none());
    }

    #[test]
    fn what_comes_back_never_carries_the_value() {
        let created = json!({ "created": { "id": "env_9", "key": "A", "value": "hunter2", "target": ["preview"], "type": "plain" } });
        let saved = saved_of(&created);
        assert_eq!(saved["id"], "env_9");
        assert!(!saved.to_string().contains("hunter2"));
        let many = json!({ "created": [{ "id": "env_8", "key": "B", "value": "x", "target": "production" }] });
        assert_eq!(saved_of(&many)["id"], "env_8");
        let patched = json!({ "id": "env_7", "key": "C", "value": "y", "target": ["development"] });
        assert_eq!(saved_of(&patched)["id"], "env_7");
    }

    #[test]
    fn a_name_used_once_finds_its_id() {
        let listing = json!({ "envs": [
            { "id": "env_1", "key": "A" }, { "id": "env_2", "key": "B" }, { "id": "env_3", "key": "B" }
        ] });
        assert_eq!(ids_named(&listing, "A"), ["env_1"]);
        assert_eq!(ids_named(&listing, "B").len(), 2);
        assert!(ids_named(&listing, "C").is_empty());
    }
}
