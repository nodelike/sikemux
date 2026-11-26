use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{at, some_text, text, Session};
use crate::error::CloudflareResult;

#[derive(Deserialize, Default)]
pub struct WorkersInput {
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub zone: Option<String>,
}

pub async fn workers(data_dir: &Path, input: WorkersInput) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let account_id = session.resolve_account_id(input.account.as_deref()).await?;

    let scripts_body = session
        .get(&format!("/accounts/{account_id}/workers/scripts"), &[])
        .await?;

    let domains_body = session
        .get(&format!("/accounts/{account_id}/workers/domains"), &[])
        .await
        .ok();
    let mut domains_by_script: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(domains_body) = domains_body {
        for item in at(&domains_body, "/result")
            .as_array()
            .into_iter()
            .flatten()
        {
            let service = text(item, "/service");
            let hostname = text(item, "/hostname");
            if !service.is_empty() && !hostname.is_empty() {
                domains_by_script.entry(service).or_default().push(hostname);
            }
        }
    }

    let mut routes_out = Vec::new();
    let mut routes_by_script: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(zone) = input
        .zone
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (zone_id, zone_name) = session.resolve_zone(zone, input.account.as_deref()).await?;
        let routes_body = session
            .get(&format!("/zones/{zone_id}/workers/routes"), &[])
            .await?;
        for item in at(&routes_body, "/result").as_array().into_iter().flatten() {
            let pattern = text(item, "/pattern");
            let script = text(item, "/script");
            if !script.is_empty() && !pattern.is_empty() {
                routes_by_script
                    .entry(script.clone())
                    .or_default()
                    .push(pattern.clone());
            }
            routes_out.push(json!({
                "id": text(item, "/id"),
                "zone": zone_name,
                "pattern": pattern,
                "script": script,
            }));
        }
    }

    let scripts: Vec<Value> = at(&scripts_body, "/result")
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let id = text(item, "/id");
            let domains = domains_by_script.remove(&id).unwrap_or_default();
            let routes = routes_by_script.remove(&id).unwrap_or_default();
            json!({
                "id": id,
                "modifiedOn": some_text(item, "/modified_on"),
                "createdOn": some_text(item, "/created_on"),
                "domains": domains,
                "routes": routes,
            })
        })
        .collect();

    Ok(json!({
        "accountId": account_id,
        "scripts": scripts,
        "routes": routes_out,
    }))
}

#[derive(Deserialize, Default)]
pub struct PagesInput {
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn pages(data_dir: &Path, input: PagesInput) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let account_id = session.resolve_account_id(input.account.as_deref()).await?;
    let body = session
        .get(&format!("/accounts/{account_id}/pages/projects"), &[])
        .await?;
    let projects: Vec<Value> = at(&body, "/result")
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let domains: Vec<String> = at(item, "/domains")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let latest = at(item, "/latest_deployment");
            let latest_deployment = (!latest.is_null()).then(|| {
                json!({
                    "id": text(latest, "/id"),
                    "environment": text(latest, "/environment"),
                    "url": text(latest, "/url"),
                    "status": text(latest, "/latest_stage/status"),
                    "stage": text(latest, "/latest_stage/name"),
                    "createdOn": some_text(latest, "/created_on"),
                })
            });
            json!({
                "name": text(item, "/name"),
                "subdomain": text(item, "/subdomain"),
                "productionBranch": text(item, "/production_branch"),
                "domains": domains,
                "latestDeployment": latest_deployment,
            })
        })
        .collect();
    Ok(json!({
        "accountId": account_id,
        "projects": projects,
    }))
}
