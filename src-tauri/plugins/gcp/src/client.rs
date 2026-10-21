// Google's REST APIs: one warm client, the account's token, pages followed
// to the end, and Google's error envelope turned into ours.

use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use reqwest::{Client, Method, Response};
use serde_json::Value;

use crate::auth::{self, Target};
use crate::error::{GcpError, GcpResult};

const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGES: usize = 20;

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

/// The last part of a resource name such as `projects/p/locations/l/services/s`.
pub fn short(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// A name, id or location as one part of an address, so it can never reach another endpoint.
pub fn segment(what: &str, value: &str) -> GcpResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GcpError::BadArg(format!("no {what} was named")));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@'))
        || value.starts_with('.')
    {
        return Err(GcpError::BadArg(format!("`{value}` is not a {what}")));
    }
    Ok(value.to_string())
}

fn http() -> GcpResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-gcp/0.1")
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(60))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| GcpError::Transport("could not start the HTTP client".into()))
}

async fn read_body(response: Response) -> GcpResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(GcpError::Response("more than 32 MiB came back".into()));
        }
    }
    Ok(bytes)
}

/// Google says a call failed with `{ "error": { "code", "message", "status", "details" } }`.
pub fn checked(status: u16, body: Value) -> GcpResult<Value> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let error = at(&body, "/error");
    let message = at(error, "/message")
        .as_str()
        .unwrap_or("Google Cloud refused the request");
    let message = message
        .split("\nHelp Token")
        .next()
        .unwrap_or(message)
        .trim()
        .to_string();
    if message.contains("Billing must be enabled") {
        return Err(GcpError::NoBilling);
    }
    let details = at(error, "/details")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let disabled = details
        .iter()
        .find(|detail| text(detail, "/reason") == "SERVICE_DISABLED");
    if let Some(detail) = disabled {
        let url = at(detail, "/metadata/activationUrl")
            .as_str()
            .map(str::to_string);
        let service = text(detail, "/metadata/service");
        return Err(GcpError::ServiceDisabled {
            service: if service.is_empty() {
                "needed".into()
            } else {
                service
            },
            url,
        });
    }
    Err(match status {
        401 => GcpError::Auth(message),
        403 if message.contains("has not been used in project")
            || message.contains("is disabled") =>
        {
            GcpError::ServiceDisabled {
                service: service_named_in(&message).unwrap_or_else(|| "needed".into()),
                url: None,
            }
        }
        403 => GcpError::Permission(message),
        404 => GcpError::NotFound(message),
        429 => GcpError::RateLimited,
        _ => GcpError::Api {
            status,
            message: message.chars().take(300).collect(),
        },
    })
}

/// The API in a message such as "... Enable it by visiting https://console.developers.google.com/apis/api/run.googleapis.com/overview?project=1".
fn service_named_in(message: &str) -> Option<String> {
    let (_, rest) = message.split_once("/apis/api/")?;
    let service = rest.split(['/', '?', ' ']).next()?;
    (!service.is_empty()).then(|| service.to_string())
}

async fn request(
    token: &str,
    method: Method,
    url: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GcpResult<(u16, Vec<u8>)> {
    request_range(token, method, url, query, body, None).await
}

async fn request_range(
    token: &str,
    method: Method,
    url: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
    range: Option<u64>,
) -> GcpResult<(u16, Vec<u8>)> {
    let mut builder = http()?.request(method, url).bearer_auth(token).query(query);
    if let Some(last) = range {
        builder = builder.header("Range", format!("bytes=0-{last}"));
    }
    if let Some(body) = body {
        builder = builder.json(body);
    }
    let response = builder.send().await?;
    let status = response.status().as_u16();
    Ok((status, read_body(response).await?))
}

fn parse(status: u16, bytes: &[u8]) -> GcpResult<Value> {
    let body = if bytes.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(bytes).map_err(|_| GcpError::Api {
            status,
            message: String::from_utf8_lossy(bytes).chars().take(300).collect(),
        })?
    };
    checked(status, body)
}

/// Calls Google as the target's account, asking gcloud for a fresh token once if the held one was refused.
pub async fn send(
    target: &Target,
    method: Method,
    url: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GcpResult<Value> {
    let account = target.account();
    let token = auth::token(account).await?;
    let (status, bytes) = request(&token, method.clone(), url, query, body).await?;
    if status == 401 {
        auth::forget_token(account);
        let token = auth::token(account).await?;
        let (status, bytes) = request(&token, method, url, query, body).await?;
        return parse(status, &bytes);
    }
    parse(status, &bytes)
}

pub async fn get(target: &Target, url: &str, query: &[(&str, String)]) -> GcpResult<Value> {
    send(target, Method::GET, url, query, None).await
}

pub async fn post(target: &Target, url: &str, body: &Value) -> GcpResult<Value> {
    send(target, Method::POST, url, &[], Some(body)).await
}

pub async fn patch(
    target: &Target,
    url: &str,
    query: &[(&str, String)],
    body: &Value,
) -> GcpResult<Value> {
    send(target, Method::PATCH, url, query, Some(body)).await
}

pub async fn put(target: &Target, url: &str, body: &Value) -> GcpResult<Value> {
    send(target, Method::PUT, url, &[], Some(body)).await
}

/// Calls Google for raw bytes, such as an object's contents, the first `limit` of them when there is one.
pub async fn bytes(
    target: &Target,
    url: &str,
    query: &[(&str, String)],
    limit: Option<u64>,
) -> GcpResult<Vec<u8>> {
    let account = target.account();
    let token = auth::token(account).await?;
    let range = limit.map(|limit| limit.saturating_sub(1));
    let (status, bytes) = request_range(&token, Method::GET, url, query, None, range).await?;
    if (200..300).contains(&status) {
        return Ok(bytes);
    }
    parse(status, &bytes).map(|_| Vec::new())
}

/// Sends raw bytes to Google, such as a file to store, answering with the JSON it sends back.
pub async fn upload(
    target: &Target,
    url: &str,
    query: &[(&str, String)],
    bytes: Vec<u8>,
    content_type: &str,
) -> GcpResult<Value> {
    let token = auth::token(target.account()).await?;
    let response = http()?
        .post(url)
        .bearer_auth(token)
        .query(query)
        .header("Content-Type", content_type)
        .timeout(Duration::from_secs(600))
        .body(bytes)
        .send()
        .await?;
    let status = response.status().as_u16();
    let body = read_body(response).await?;
    parse(status, &body)
}

/// A value as one percent-encoded part of an address, for names that may hold slashes and spaces.
pub fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Every item of a listing under `key`, following `nextPageToken` up to `limit` items.
pub async fn list(
    target: &Target,
    url: &str,
    query: &[(&str, String)],
    key: &str,
    limit: usize,
) -> GcpResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut page_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut query = query.to_vec();
        if let Some(token) = &page_token {
            query.push(("pageToken", token.clone()));
        }
        let page = get(target, url, &query).await?;
        items.extend(
            at(&page, &format!("/{key}"))
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        page_token = page
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        if page_token.is_none() || items.len() >= limit {
            break;
        }
    }
    items.truncate(limit);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn googles_errors_read_in_words() {
        let disabled = json!({ "error": { "code": 403, "message": "Cloud Run Admin API has not been used in project 1 before or it is disabled.", "status": "PERMISSION_DENIED",
            "details": [{ "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "SERVICE_DISABLED", "metadata": { "service": "run.googleapis.com", "activationUrl": "https://console.developers.google.com/apis/api/run.googleapis.com/overview?project=1" } }] } });
        assert!(matches!(
            checked(403, disabled),
            Err(GcpError::ServiceDisabled { service, url: Some(_) }) if service == "run.googleapis.com"
        ));
        let bare = json!({ "error": { "code": 403, "message": "Compute Engine API has not been used in project 1 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/compute.googleapis.com/overview?project=1 then retry." } });
        assert!(matches!(
            checked(403, bare),
            Err(GcpError::ServiceDisabled { service, url: None }) if service == "compute.googleapis.com"
        ));
        let unbilled = json!({ "error": { "code": 400, "message": "Billing account for project '1' is not found. Billing must be enabled for activation of service(s) 'compute.googleapis.com' to proceed.\nHelp Token: abc" } });
        assert!(matches!(checked(400, unbilled), Err(GcpError::NoBilling)));
        let noisy =
            json!({ "error": { "code": 500, "message": "Backend broke.\nHelp Token: abc" } });
        assert!(
            matches!(checked(500, noisy), Err(GcpError::Api { message, .. }) if message == "Backend broke.")
        );
        let denied = json!({ "error": { "code": 403, "message": "Permission 'run.services.list' denied", "status": "PERMISSION_DENIED" } });
        assert!(matches!(checked(403, denied), Err(GcpError::Permission(_))));
        assert!(matches!(checked(401, json!({})), Err(GcpError::Auth(_))));
        assert!(
            matches!(checked(404, json!({ "error": { "message": "Not found" } })), Err(GcpError::NotFound(m)) if m == "Not found")
        );
        assert!(matches!(
            checked(429, json!({})),
            Err(GcpError::RateLimited)
        ));
        assert!(matches!(
            checked(500, json!({})),
            Err(GcpError::Api { status: 500, .. })
        ));
        assert_eq!(
            checked(200, json!({ "a": 1 })).ok(),
            Some(json!({ "a": 1 }))
        );
    }

    #[test]
    fn a_name_is_one_part_of_an_address() {
        assert_eq!(
            segment("project", " acme-prod ").ok().as_deref(),
            Some("acme-prod")
        );
        assert!(segment("project", "acme/../x").is_err());
        assert!(segment("project", "..").is_err());
        assert!(segment("project", "").is_err());
        assert_eq!(short("projects/p/locations/us/services/api"), "api");
        assert_eq!(
            encoded("logs/2024 10/app.log"),
            "logs%2F2024%2010%2Fapp.log"
        );
    }
}
