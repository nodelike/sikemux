// A job's whole definition, as Rundeck exports it, and putting a changed one back in place.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{self, resolve};
use crate::client::{
    get_json_at_version, post_json_at_version, seg, Session, JOB_DEFINITION_API_VERSION,
};
use crate::error::{RundeckError, RundeckResult};
use crate::shape::is_production;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAsk {
    job: String,
    project: Option<String>,
}

#[derive(Serialize)]
pub struct Exported {
    project: String,
    path: String,
    definition: Value,
}

pub async fn export(session: &Session, ask: ExportAsk) -> RundeckResult<Exported> {
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let definition: Value = get_json_at_version(
        session,
        JOB_DEFINITION_API_VERSION,
        &format!("/job/{}", seg(&detail.id)?),
        &[("format", "json".into())],
    )
    .await?;
    Ok(Exported {
        path: agent::path_of(&detail),
        project: detail.project,
        definition: single(definition)?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAsk {
    job: String,
    project: Option<String>,
    definition: Value,
    #[serde(default)]
    production: bool,
}

#[derive(Serialize)]
pub struct Imported {
    project: String,
    path: String,
    id: String,
}

fn single(definition: Value) -> RundeckResult<Value> {
    match definition {
        Value::Array(mut items) if items.len() == 1 => Ok(items.remove(0)),
        Value::Array(_) => Err(RundeckError::BadArg(
            "a definition holds exactly one job".into(),
        )),
        Value::Object(_) => Ok(definition),
        _ => Err(RundeckError::BadArg(
            "definition is the object rundeck export answered with".into(),
        )),
    }
}

fn group_of(definition: &Value) -> Option<&str> {
    definition.get("group").and_then(Value::as_str)
}

pub async fn import(
    session: &Session,
    ask: ImportAsk,
    prod_envs: &[String],
) -> RundeckResult<Imported> {
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let mut definition = single(ask.definition)?;
    let named = ["uuid", "id"]
        .iter()
        .find_map(|key| definition.get(*key).and_then(Value::as_str));
    if named.is_some_and(|id| id != detail.id) {
        return Err(RundeckError::BadArg(format!(
            "the definition's uuid is not {}'s ({}); import changes that job in place",
            agent::path_of(&detail),
            detail.id
        )));
    }
    let production = is_production(&detail.project, detail.group.as_deref(), prod_envs)
        || is_production(&detail.project, group_of(&definition), prod_envs);
    if production && !ask.production {
        return Err(RundeckError::BadArg(format!(
            "{} is a production job. Ask the person first, and call again with production: true once they agree.",
            agent::path_of(&detail)
        )));
    }
    if let Value::Object(fields) = &mut definition {
        fields.insert("uuid".into(), Value::String(detail.id.clone()));
        fields.insert("id".into(), Value::String(detail.id.clone()));
    }
    let answer: Value = post_json_at_version(
        session,
        JOB_DEFINITION_API_VERSION,
        &format!("/project/{}/jobs/import", seg(&detail.project)?),
        &Value::Array(vec![definition]),
        &[
            ("fileformat", "json".into()),
            ("dupeOption", "update".into()),
            ("uuidOption", "preserve".into()),
        ],
    )
    .await?;
    if let Some(failed) = answer
        .get("failed")
        .and_then(Value::as_array)
        .and_then(|failed| failed.first())
    {
        let why = failed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Rundeck refused the definition");
        return Err(RundeckError::Api(why.to_owned()));
    }
    let job = answer
        .get("succeeded")
        .and_then(Value::as_array)
        .and_then(|done| done.first())
        .ok_or_else(|| RundeckError::Api("Rundeck skipped the definition".into()))?;
    let field = |key: &str| job.get(key).and_then(Value::as_str);
    let path = match (field("group"), field("name")) {
        (Some(group), Some(name)) if !group.is_empty() => format!("{group}/{name}"),
        (_, Some(name)) => name.to_owned(),
        _ => agent::path_of(&detail),
    };
    Ok(Imported {
        project: detail.project,
        path,
        id: field("id").unwrap_or(&detail.id).to_owned(),
    })
}
