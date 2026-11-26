use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, forget_zone, number, some_text, text, Session};
use crate::error::{CloudflareError, CloudflareResult};

const MAX_DNS_PAGES: u64 = 5;

pub fn expand_record_name(name: &str, zone_name: &str) -> String {
    let trimmed = name.trim().trim_end_matches('.');
    let zone = zone_name.trim().trim_end_matches('.').to_ascii_lowercase();
    if trimmed.is_empty() || trimmed == "@" {
        return zone;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == zone || lower.ends_with(&format!(".{zone}")) {
        return lower;
    }
    format!("{lower}.{zone}")
}

pub fn normalize_ttl_and_proxy(ttl: Option<u64>, proxied: Option<bool>) -> (u64, bool) {
    let proxied = proxied.unwrap_or(false);
    let ttl = if proxied { 1 } else { ttl.unwrap_or(1) };
    (ttl, proxied)
}

fn compact_record(item: &Value) -> Value {
    let mut map = Map::new();
    map.insert("id".into(), json!(text(item, "/id")));
    map.insert("type".into(), json!(text(item, "/type")));
    map.insert("name".into(), json!(text(item, "/name")));
    map.insert("content".into(), json!(text(item, "/content")));
    map.insert("ttl".into(), json!(number(item, "/ttl").unwrap_or(1)));
    map.insert(
        "proxied".into(),
        json!(at(item, "/proxied").as_bool().unwrap_or(false)),
    );
    if let Some(priority) = number(item, "/priority") {
        map.insert("priority".into(), json!(priority));
    }
    if let Some(comment) = some_text(item, "/comment") {
        map.insert("comment".into(), json!(comment));
    }
    Value::Object(map)
}

#[derive(Deserialize)]
pub struct DnsListInput {
    pub zone: String,
    #[serde(default, rename = "type")]
    pub record_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub page: Option<u64>,
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn dns_list(data_dir: &Path, input: DnsListInput) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let (zone_id, zone_name) = session
        .resolve_zone(&input.zone, input.account.as_deref())
        .await?;

    if input
        .format
        .as_deref()
        .is_some_and(|format| format.eq_ignore_ascii_case("bind"))
    {
        let bind = session
            .get_text(&format!("/zones/{zone_id}/dns_records/export"), &[])
            .await?;
        return Ok(json!({
            "zone": zone_name,
            "zoneId": zone_id,
            "format": "bind",
            "bind": bind,
        }));
    }

    let mut base_query = vec![("per_page", "100".into())];
    if let Some(record_type) = input
        .record_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        base_query.push(("type", record_type.to_ascii_uppercase()));
    }
    if let Some(name) = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        base_query.push(("name", expand_record_name(name, &zone_name)));
    }

    let mut records = Vec::new();
    let start_page = input.page.unwrap_or(1).max(1);
    let single_page = input.page.is_some();
    let mut total: u64 = 0;
    let mut has_more = false;
    let mut next_page: Option<u64> = None;

    for (fetched, page) in (0..MAX_DNS_PAGES).zip(start_page..) {
        let mut query = base_query.clone();
        query.push(("page", page.to_string()));
        let body = session
            .get(&format!("/zones/{zone_id}/dns_records"), &query)
            .await
            .map_err(|error| {
                if matches!(error, CloudflareError::NotFound(_)) {
                    forget_zone(&session.account.id, &zone_id);
                }
                error
            })?;
        for item in at(&body, "/result").as_array().into_iter().flatten() {
            records.push(compact_record(item));
        }
        total = at(&body, "/result_info/total_count")
            .as_u64()
            .unwrap_or(records.len() as u64);
        let total_pages = at(&body, "/result_info/total_pages").as_u64().unwrap_or(1);
        if page >= total_pages {
            break;
        }
        if single_page || fetched + 1 >= MAX_DNS_PAGES {
            has_more = true;
            next_page = Some(page + 1);
            break;
        }
    }

    Ok(json!({
        "zone": zone_name,
        "zoneId": zone_id,
        "records": records,
        "total": total,
        "hasMore": has_more,
        "nextPage": next_page,
    }))
}

#[derive(Deserialize)]
pub struct DnsUpsertInput {
    pub zone: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub record_type: String,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub ttl: Option<u64>,
    #[serde(default)]
    pub proxied: Option<bool>,
    #[serde(default)]
    pub priority: Option<u64>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
}

pub fn build_dns_record_payload(
    input: &DnsUpsertInput,
    zone_name: &str,
) -> CloudflareResult<Value> {
    let record_type = input.record_type.trim().to_ascii_uppercase();
    if record_type.is_empty() {
        return Err(CloudflareError::BadArg(
            "DNS record type is required".into(),
        ));
    }
    let content = input.content.trim();
    if content.is_empty() {
        return Err(CloudflareError::BadArg(
            "DNS record content is required".into(),
        ));
    }
    let name = expand_record_name(&input.name, zone_name);
    let (ttl, proxied) = normalize_ttl_and_proxy(input.ttl, input.proxied);
    let mut map = Map::new();
    map.insert("type".into(), json!(record_type));
    map.insert("name".into(), json!(name));
    map.insert("content".into(), json!(content));
    map.insert("ttl".into(), json!(ttl));
    map.insert("proxied".into(), json!(proxied));
    if let Some(priority) = input.priority {
        map.insert("priority".into(), json!(priority));
    }
    if let Some(comment) = &input.comment {
        map.insert("comment".into(), json!(comment));
    }
    Ok(Value::Object(map))
}

fn is_multi_value_type(record_type: &str) -> bool {
    matches!(record_type, "TXT" | "MX" | "NS" | "SRV" | "CAA")
}

pub async fn dns_upsert(data_dir: &Path, input: DnsUpsertInput) -> CloudflareResult<Value> {
    let session = Session::of(data_dir).await?;
    let (zone_id, zone_name) = session
        .resolve_zone(&input.zone, input.account.as_deref())
        .await?;
    let payload = build_dns_record_payload(&input, &zone_name)?;
    let record_type = text(&payload, "/type");
    let record_name = text(&payload, "/name");
    let record_content = text(&payload, "/content");

    let target_id = match input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(id.to_owned()),
        None => {
            let existing = session
                .get(
                    &format!("/zones/{zone_id}/dns_records"),
                    &[
                        ("type", record_type.clone()),
                        ("name", record_name.clone()),
                        ("per_page", "100".into()),
                    ],
                )
                .await?;
            let matches: Vec<&Value> = at(&existing, "/result")
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            if is_multi_value_type(&record_type) {
                matches
                    .iter()
                    .find(|item| text(item, "/content") == record_content)
                    .map(|item| text(item, "/id"))
            } else if matches.len() == 1 {
                matches.first().map(|item| text(item, "/id"))
            } else {
                matches
                    .iter()
                    .find(|item| text(item, "/content") == record_content)
                    .map(|item| text(item, "/id"))
            }
        }
    };

    let (action, body) = match target_id {
        Some(id) => (
            "updated",
            session
                .send(
                    Method::PUT,
                    &format!("/zones/{zone_id}/dns_records/{id}"),
                    &[],
                    Some(&payload),
                )
                .await?,
        ),
        None => (
            "created",
            session
                .send(
                    Method::POST,
                    &format!("/zones/{zone_id}/dns_records"),
                    &[],
                    Some(&payload),
                )
                .await?,
        ),
    };

    Ok(json!({
        "zone": zone_name,
        "zoneId": zone_id,
        "action": action,
        "record": compact_record(at(&body, "/result")),
    }))
}

#[derive(Deserialize)]
pub struct DnsDeleteInput {
    pub zone: String,
    pub id: String,
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn dns_delete(data_dir: &Path, input: DnsDeleteInput) -> CloudflareResult<Value> {
    let id = input.id.trim();
    if id.is_empty() {
        return Err(CloudflareError::BadArg("DNS record id is required".into()));
    }
    let session = Session::of(data_dir).await?;
    let (zone_id, zone_name) = session
        .resolve_zone(&input.zone, input.account.as_deref())
        .await?;
    let body = session
        .send(
            Method::DELETE,
            &format!("/zones/{zone_id}/dns_records/{id}"),
            &[],
            None,
        )
        .await?;
    Ok(json!({
        "id": some_text(&body, "/result/id").unwrap_or_else(|| id.to_owned()),
        "zone": zone_name,
        "zoneId": zone_id,
        "deleted": true,
    }))
}

#[derive(Deserialize)]
pub struct DnsBatchInput {
    pub zone: String,
    #[serde(default)]
    pub deletes: Vec<Value>,
    #[serde(default)]
    pub patches: Vec<Value>,
    #[serde(default)]
    pub puts: Vec<Value>,
    #[serde(default)]
    pub posts: Vec<Value>,
    #[serde(default)]
    pub account: Option<String>,
}

fn normalize_batch_record(mut record: Value, zone_name: &str, apply_defaults: bool) -> Value {
    let Some(map) = record.as_object_mut() else {
        return record;
    };
    if let Some(name) = map.get("name").and_then(Value::as_str).map(str::to_owned) {
        map.insert("name".into(), json!(expand_record_name(&name, zone_name)));
    }
    if let Some(record_type) = map.get("type").and_then(Value::as_str).map(str::to_owned) {
        map.insert(
            "type".into(),
            json!(record_type.trim().to_ascii_uppercase()),
        );
    }
    if apply_defaults {
        let proxied = map.get("proxied").and_then(Value::as_bool);
        let ttl = map.get("ttl").and_then(Value::as_u64);
        let (ttl, proxied) = normalize_ttl_and_proxy(ttl, proxied);
        map.insert("proxied".into(), json!(proxied));
        map.insert("ttl".into(), json!(ttl));
    } else if map.get("proxied").and_then(Value::as_bool) == Some(true) {
        map.insert("ttl".into(), json!(1));
    }
    record
}

pub async fn dns_batch(data_dir: &Path, input: DnsBatchInput) -> CloudflareResult<Value> {
    if input.deletes.is_empty()
        && input.patches.is_empty()
        && input.puts.is_empty()
        && input.posts.is_empty()
    {
        return Err(CloudflareError::BadArg(
            "dns_batch requires at least one operation in deletes, patches, puts or posts".into(),
        ));
    }
    let session = Session::of(data_dir).await?;
    let (zone_id, zone_name) = session
        .resolve_zone(&input.zone, input.account.as_deref())
        .await?;

    let mut payload = Map::new();
    if !input.deletes.is_empty() {
        payload.insert("deletes".into(), Value::Array(input.deletes));
    }
    if !input.patches.is_empty() {
        let patches = input
            .patches
            .into_iter()
            .map(|item| normalize_batch_record(item, &zone_name, false))
            .collect();
        payload.insert("patches".into(), Value::Array(patches));
    }
    if !input.puts.is_empty() {
        let puts = input
            .puts
            .into_iter()
            .map(|item| normalize_batch_record(item, &zone_name, true))
            .collect();
        payload.insert("puts".into(), Value::Array(puts));
    }
    if !input.posts.is_empty() {
        let posts = input
            .posts
            .into_iter()
            .map(|item| normalize_batch_record(item, &zone_name, true))
            .collect();
        payload.insert("posts".into(), Value::Array(posts));
    }

    let body = session
        .send(
            Method::POST,
            &format!("/zones/{zone_id}/dns_records/batch"),
            &[],
            Some(&Value::Object(payload)),
        )
        .await?;
    let result = at(&body, "/result");
    let map_list = |pointer: &str| -> Vec<Value> {
        at(result, pointer)
            .as_array()
            .into_iter()
            .flatten()
            .map(compact_record)
            .collect()
    };
    Ok(json!({
        "zone": zone_name,
        "zoneId": zone_id,
        "deletes": map_list("/deletes"),
        "patches": map_list("/patches"),
        "puts": map_list("/puts"),
        "posts": map_list("/posts"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_upsert_defaults_to_dns_only_auto_ttl_and_expands_apex() -> Result<(), String> {
        let input = DnsUpsertInput {
            zone: "sikemux.com".into(),
            id: None,
            record_type: "cname".into(),
            name: "@".into(),
            content: "cname.vercel-dns.com".into(),
            ttl: None,
            proxied: None,
            priority: None,
            comment: None,
            account: None,
        };
        let payload =
            build_dns_record_payload(&input, "sikemux.com").map_err(|error| error.to_string())?;
        assert_eq!(payload["type"], "CNAME");
        assert_eq!(payload["name"], "sikemux.com");
        assert_eq!(payload["proxied"], false);
        assert_eq!(payload["ttl"], 1);
        Ok(())
    }

    #[test]
    fn proxied_record_forces_auto_ttl() -> Result<(), String> {
        let input = DnsUpsertInput {
            zone: "sikemux.com".into(),
            id: None,
            record_type: "A".into(),
            name: "app".into(),
            content: "192.0.2.1".into(),
            ttl: Some(300),
            proxied: Some(true),
            priority: None,
            comment: None,
            account: None,
        };
        let payload =
            build_dns_record_payload(&input, "sikemux.com").map_err(|error| error.to_string())?;
        assert_eq!(payload["name"], "app.sikemux.com");
        assert_eq!(payload["proxied"], true);
        assert_eq!(payload["ttl"], 1);
        Ok(())
    }

    #[test]
    fn expand_record_name_handles_apex_relative_and_fqdn() {
        assert_eq!(expand_record_name("@", "sikemux.com"), "sikemux.com");
        assert_eq!(expand_record_name("api", "sikemux.com"), "api.sikemux.com");
        assert_eq!(
            expand_record_name("api.sikemux.com.", "sikemux.com"),
            "api.sikemux.com"
        );
    }
}
