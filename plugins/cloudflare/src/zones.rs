use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{self, at, cache_zone, normalize_domain, text, Session};
use crate::error::{CloudflareError, CloudflareResult};

const MAX_ZONE_PAGES: u64 = 5;

pub async fn accounts(data_dir: &Path) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let body = session
        .get("/accounts", &[("per_page", "50".into())])
        .await?;
    let accounts: Vec<Value> = at(&body, "/result")
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            json!({
                "id": text(item, "/id"),
                "name": text(item, "/name"),
                "type": text(item, "/type"),
            })
        })
        .collect();
    Ok(json!({ "accounts": accounts }))
}

#[derive(Deserialize, Default)]
pub struct ZonesInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn zones(data_dir: &Path, input: ZonesInput) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let account_id = match input
        .account
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(account) => Some(session.resolve_account_id(Some(account)).await?),
        None => None,
    };
    let name_filter = input
        .name
        .as_deref()
        .map(normalize_domain)
        .filter(|name| !name.is_empty());

    let mut zones = Vec::new();
    let mut page: u64 = 1;
    let mut has_more = false;
    loop {
        let mut query = vec![("per_page", "50".into()), ("page", page.to_string())];
        if let Some(account_id) = &account_id {
            query.push(("account.id", account_id.clone()));
        }
        if let Some(name) = &name_filter {
            query.push(("name", name.clone()));
        }
        let body = session.get("/zones", &query).await?;
        for item in at(&body, "/result").as_array().into_iter().flatten() {
            let id = text(item, "/id");
            let name = normalize_domain(&text(item, "/name"));
            cache_zone(&session.account.id, &id, &name);
            let name_servers: Vec<String> = at(item, "/name_servers")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            zones.push(json!({
                "id": id,
                "name": name,
                "status": text(item, "/status"),
                "paused": at(item, "/paused").as_bool().unwrap_or(false),
                "plan": text(item, "/plan/name"),
                "account": {
                    "id": text(item, "/account/id"),
                    "name": text(item, "/account/name"),
                },
                "nameServers": name_servers,
            }));
        }
        let total_pages = at(&body, "/result_info/total_pages").as_u64().unwrap_or(1);
        if page >= total_pages {
            break;
        }
        if page >= MAX_ZONE_PAGES {
            has_more = true;
            break;
        }
        page += 1;
    }
    Ok(json!({
        "zones": zones,
        "hasMore": has_more,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneCreateInput {
    pub name: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default, rename = "type")]
    pub zone_type: Option<String>,
    #[serde(default)]
    pub jump_start: Option<bool>,
}

pub async fn zone_create(data_dir: &Path, input: ZoneCreateInput) -> CloudflareResult<Value> {
    let domain = normalize_domain(&input.name);
    if domain.is_empty() || !domain.contains('.') {
        return Err(CloudflareError::BadArg(
            "zone name must be a valid domain such as sikemux.com".into(),
        ));
    }
    let zone_type = input.zone_type.as_deref().unwrap_or("full");
    if !matches!(zone_type, "full" | "partial" | "secondary") {
        return Err(CloudflareError::BadArg(format!(
            "unsupported zone type '{zone_type}'; expected full, partial or secondary"
        )));
    }
    let session = Session::of(data_dir).await?;
    let account_id = session.resolve_account_id(input.account.as_deref()).await?;
    let mut payload = serde_json::Map::new();
    payload.insert("name".into(), json!(domain));
    payload.insert("account".into(), json!({ "id": account_id }));
    payload.insert("type".into(), json!(zone_type));
    if let Some(jump_start) = input.jump_start {
        payload.insert("jump_start".into(), json!(jump_start));
    }
    let body = session
        .send(Method::POST, "/zones", &[], Some(&Value::Object(payload)))
        .await?;
    let result = at(&body, "/result");
    let id = text(result, "/id");
    let name = normalize_domain(&text(result, "/name"));
    cache_zone(&session.account.id, &id, &name);
    let name_servers: Vec<String> = at(result, "/name_servers")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Ok(json!({
        "id": id,
        "name": name,
        "status": text(result, "/status"),
        "nameServers": name_servers,
        "account": {
            "id": text(result, "/account/id"),
            "name": text(result, "/account/name"),
        },
    }))
}

#[derive(Deserialize)]
pub struct PurgeCacheInput {
    pub zone: String,
    #[serde(default)]
    pub everything: Option<bool>,
    #[serde(default)]
    pub files: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    #[serde(default)]
    pub prefixes: Option<Vec<String>>,
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn purge_cache(data_dir: &Path, input: PurgeCacheInput) -> CloudflareResult<Value> {
    let mut payload = serde_json::Map::new();
    if input.everything == Some(true) {
        payload.insert("purge_everything".into(), Value::Bool(true));
    } else {
        if let Some(files) = input.files.filter(|items| !items.is_empty()) {
            payload.insert("files".into(), json!(files));
        }
        if let Some(tags) = input.tags.filter(|items| !items.is_empty()) {
            payload.insert("tags".into(), json!(tags));
        }
        if let Some(hosts) = input.hosts.filter(|items| !items.is_empty()) {
            payload.insert("hosts".into(), json!(hosts));
        }
        if let Some(prefixes) = input.prefixes.filter(|items| !items.is_empty()) {
            payload.insert("prefixes".into(), json!(prefixes));
        }
    }
    if payload.is_empty() {
        return Err(CloudflareError::BadArg(
            "purge_cache requires everything: true or at least one of files, tags, hosts, prefixes"
                .into(),
        ));
    }
    let session = Session::of(data_dir).await?;
    let (zone_id, zone_name) = session
        .resolve_zone(&input.zone, input.account.as_deref())
        .await?;
    let body = session
        .send(
            Method::POST,
            &format!("/zones/{zone_id}/purge_cache"),
            &[],
            Some(&Value::Object(payload)),
        )
        .await?;
    Ok(json!({
        "id": text(&body, "/result/id"),
        "zone": zone_name,
        "zoneId": zone_id,
        "purged": true,
    }))
}

#[derive(Deserialize)]
pub struct ResolveZoneInput {
    pub zone: String,
    #[serde(default)]
    pub account: Option<String>,
}

/// Resolves a domain name or 32-char zone ID to its canonical domain name for production gating.
pub async fn resolve_zone(data_dir: &Path, input: ResolveZoneInput) -> CloudflareResult<Value> {
    let raw = input.zone.trim();
    if raw.is_empty() {
        return Err(CloudflareError::BadArg("zone is required".into()));
    }
    if !client::is_cf_id(raw) {
        let name = normalize_domain(raw);
        return Ok(json!({ "name": name }));
    }
    let session = Session::of(data_dir).await?;
    let (id, name) = session.resolve_zone(raw, input.account.as_deref()).await?;
    Ok(json!({ "id": id, "name": name }))
}
