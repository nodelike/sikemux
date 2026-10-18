// Projects: the list, and each one's domains and the names of its environment variables.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};

/// A name or id as one part of an address, so it can never reach another endpoint.
pub fn segment(what: &str, value: &str) -> VercelResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(VercelError::BadArg(format!("no {what} was named")));
    }
    let starts_plain = value.starts_with(|c: char| c.is_ascii_alphanumeric());
    if !starts_plain
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(VercelError::BadArg(format!("`{value}` is not a {what}")));
    }
    Ok(value.to_string())
}

fn repo_of(link: &Value) -> Value {
    let owner = ["/org", "/projectNamespace", "/owner", "/workspaceSlug"]
        .iter()
        .map(|path| text(link, path))
        .find(|owner| !owner.is_empty());
    let name = ["/repo", "/projectName", "/slug"]
        .iter()
        .map(|path| text(link, path))
        .find(|name| !name.is_empty());
    match (owner, name) {
        (Some(owner), Some(name)) => json!(format!("{owner}/{name}")),
        (None, Some(name)) => json!(name),
        _ => Value::Null,
    }
}

/// The shortest of a deployment's aliases, which is the domain people use; the
/// longer ones are the branch and scope addresses Vercel adds to every deployment.
pub fn shortest_alias(deployment: &Value) -> Option<String> {
    at(deployment, "/alias")
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .min_by_key(|alias| alias.len())
        .map(str::to_string)
}

/// A deployment serving, or meant to serve, production: where it answers and how it stands.
fn production_of(deployment: &Value) -> Value {
    let url = shortest_alias(deployment).or_else(|| {
        deployment
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    json!({
        "id": at(deployment, "/id"),
        "url": url.map(|url| format!("https://{url}")),
        "state": at(deployment, "/readyState"),
        "createdAt": at(deployment, "/createdAt"),
    })
}

/// A project as the pane and agents read it.
pub fn project(value: &Value) -> Value {
    let production = at(value, "/targets/production");
    let link = at(value, "/link");
    json!({
        "id": at(value, "/id"),
        "name": at(value, "/name"),
        "framework": at(value, "/framework"),
        "updatedAt": at(value, "/updatedAt"),
        "repo": repo_of(link),
        "gitProvider": at(link, "/type"),
        "productionBranch": at(link, "/productionBranch"),
        "production": if production.is_null() { Value::Null } else { production_of(production) },
        "latestProduction": Value::Null,
    })
}

/// The production domain people visit: verified, and neither a redirect nor a branch's.
fn production_domain(domains: &Value) -> Option<String> {
    at(domains, "/domains")
        .as_array()?
        .iter()
        .find(|domain| {
            at(domain, "/verified").as_bool() == Some(true)
                && at(domain, "/redirect").is_null()
                && at(domain, "/gitBranch").is_null()
        })
        .and_then(|domain| domain.get("name").and_then(Value::as_str))
        .map(str::to_string)
}

/// The deployment the production domain answers with. Vercel names the newest
/// production build as the project's production one even while it builds or after
/// it fails, though the domain still serves the build before it.
async fn serving(session: &Session, project_id: &str) -> Option<Value> {
    let project_id = segment("project", project_id).ok()?;
    let domains = session
        .get(&format!("/v9/projects/{project_id}/domains"), &[])
        .await
        .ok()?;
    let domain = production_domain(&domains)?;
    let alias = session
        .get(
            &format!("/v4/aliases/{}", segment("domain", &domain).ok()?),
            &[],
        )
        .await
        .ok()?;
    let id = segment("deployment", at(&alias, "/deploymentId").as_str()?).ok()?;
    let deployment = session
        .get(&format!("/v13/deployments/{id}"), &[])
        .await
        .ok()?;
    let mut serving = production_of(&deployment);
    if let Some(object) = serving.as_object_mut() {
        object.insert("url".into(), json!(format!("https://{domain}")));
    }
    Some(serving)
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn projects(data_dir: &Path, input: Listing) -> VercelResult<Vec<Value>> {
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let mut query = vec![("limit", "100".to_string())];
    if let Some(words) = input
        .query
        .as_deref()
        .map(str::trim)
        .filter(|words| !words.is_empty())
    {
        query.push(("search", words.to_string()));
    }
    let data = session.get("/v9/projects", &query).await?;
    let listed: Vec<Value> = at(&data, "/projects")
        .as_array()
        .into_iter()
        .flatten()
        .map(project)
        .collect();
    Ok(futures::future::join_all(
        listed
            .into_iter()
            .map(|project| with_serving(&session, project)),
    )
    .await)
}

/// Swaps in the deployment actually serving when the newest production build is not ready,
/// keeping that build as the latest one.
async fn with_serving(session: &Session, mut project: Value) -> Value {
    let state = text(&project, "/production/state");
    if state.is_empty() || state == "READY" {
        return project;
    }
    let Some(serving) = serving(session, &text(&project, "/id")).await else {
        return project;
    };
    if let Some(object) = project.as_object_mut() {
        if let Some(latest) = object.insert("production".into(), serving) {
            object.insert("latestProduction".into(), latest);
        }
    }
    project
}

#[derive(Deserialize)]
pub struct ProjectRef {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn domains(data_dir: &Path, input: ProjectRef) -> VercelResult<Vec<Value>> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let data = session
        .get(
            &format!("/v9/projects/{project}/domains"),
            &[("limit", "100".into())],
        )
        .await?;
    Ok(at(&data, "/domains")
        .as_array()
        .into_iter()
        .flatten()
        .map(|domain| {
            json!({
                "name": at(domain, "/name"),
                "verified": at(domain, "/verified"),
                "redirect": at(domain, "/redirect"),
                "gitBranch": at(domain, "/gitBranch"),
                "createdAt": at(domain, "/createdAt"),
            })
        })
        .collect())
}

/// An environment variable without its value, which never leaves Vercel through here.
pub fn variable(value: &Value) -> Value {
    let target = match at(value, "/target") {
        Value::String(one) => json!([one]),
        Value::Array(many) => json!(many),
        _ => json!([]),
    };
    json!({
        "id": at(value, "/id"),
        "key": at(value, "/key"),
        "target": target,
        "type": at(value, "/type"),
        "gitBranch": at(value, "/gitBranch"),
        "updatedAt": at(value, "/updatedAt"),
    })
}

pub async fn env(data_dir: &Path, input: ProjectRef) -> VercelResult<Vec<Value>> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let data = session
        .get(&format!("/v10/projects/{project}/env"), &[])
        .await?;
    Ok(at(&data, "/envs")
        .as_array()
        .into_iter()
        .flatten()
        .map(variable)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_reads_with_its_repository_and_production_address() {
        let raw = json!({
            "id": "prj_1", "name": "web", "framework": "nextjs", "updatedAt": 5,
            "link": { "type": "github", "org": "acme", "repo": "web", "productionBranch": "main" },
            "targets": { "production": { "id": "dpl_9", "url": "web-abc.vercel.app", "alias": ["web-git-main-acme.vercel.app", "web.acme.dev", "web-acme.vercel.app"], "readyState": "READY", "createdAt": 4 } }
        });
        let read = project(&raw);
        assert_eq!(read["repo"], "acme/web");
        assert_eq!(read["productionBranch"], "main");
        assert_eq!(read["production"]["url"], "https://web.acme.dev");
        assert_eq!(read["production"]["state"], "READY");
        assert_eq!(
            project(&json!({ "id": "prj_2", "name": "empty" }))["production"],
            Value::Null
        );
    }

    #[test]
    fn the_production_domain_is_verified_and_neither_a_redirect_nor_a_branchs() {
        let domains = json!({ "domains": [
            { "name": "staging.acme.dev", "verified": true, "redirect": null, "gitBranch": "staging" },
            { "name": "www.acme.dev", "verified": true, "redirect": "acme.dev", "gitBranch": null },
            { "name": "new.acme.dev", "verified": false, "redirect": null, "gitBranch": null },
            { "name": "acme.dev", "verified": true, "redirect": null, "gitBranch": null }
        ] });
        assert_eq!(production_domain(&domains).as_deref(), Some("acme.dev"));
        assert_eq!(production_domain(&json!({ "domains": [] })), None);
    }

    #[test]
    fn a_variable_never_carries_its_value() {
        let raw = json!({ "id": "env_1", "key": "DATABASE_URL", "value": "postgres://secret", "target": ["production", "preview"], "type": "encrypted" });
        let read = variable(&raw);
        assert_eq!(read["key"], "DATABASE_URL");
        assert_eq!(read["target"], json!(["production", "preview"]));
        assert!(!read.to_string().contains("secret"));
        assert_eq!(
            variable(&json!({ "key": "A", "target": "preview" }))["target"],
            json!(["preview"])
        );
    }

    #[test]
    fn a_name_cannot_reach_another_address() {
        assert_eq!(
            segment("project", " web-app ").ok().as_deref(),
            Some("web-app")
        );
        assert!(segment("project", "../v2/user").is_err());
        assert!(segment("project", "").is_err());
    }
}
