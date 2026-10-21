// Azure's Resource Manager: one warm client, the CLI's token, pages followed to
// the end, and Azure's error envelope turned into ours.

use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use reqwest::{Client, Method, Response};
use serde_json::Value;

use crate::auth;
use crate::error::{AzureError, AzureResult};

pub const MANAGEMENT: &str = "https://management.azure.com";
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGES: usize = 20;

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

pub fn maybe(value: &Value, path: &str) -> Option<String> {
    at(value, path)
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The last part of a resource id such as `/subscriptions/s/resourceGroups/g/providers/…/sites/web`.
pub fn short(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_string()
}

/// The resource group a resource id belongs to, which Azure spells differently in different APIs.
pub fn group_of(id: &str) -> String {
    let mut parts = id.split('/');
    parts
        .by_ref()
        .find(|part| part.eq_ignore_ascii_case("resourceGroups"))
        .and_then(|_| parts.next())
        .unwrap_or_default()
        .to_string()
}

/// A name or id as one part of an address, so it can never reach another endpoint.
pub fn segment(what: &str, value: &str) -> AzureResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AzureError::BadArg(format!("no {what} was named")));
    }
    if value.len() > 260
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '(' | ')'))
        || value.starts_with('.')
    {
        return Err(AzureError::BadArg(format!("`{value}` is not a {what}")));
    }
    Ok(value.to_string())
}

fn http() -> AzureResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-azure/0.1")
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(60))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| AzureError::Transport("could not start the HTTP client".into()))
}

async fn read_body(response: Response) -> AzureResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(AzureError::Response("more than 32 MiB came back".into()));
        }
    }
    Ok(bytes)
}

/// Azure says a call failed with `{ "error": { "code", "message" } }`, and some
/// services wrap that in another `error`.
pub fn checked(status: u16, body: Value) -> AzureResult<Value> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let error = match at(&body, "/error") {
        Value::Null => &body,
        found => found,
    };
    let code = maybe(error, "/code").unwrap_or_else(|| format!("HTTP {status}"));
    let message = maybe(error, "/message")
        .or_else(|| maybe(&body, "/message"))
        .unwrap_or_else(|| body.to_string().chars().take(200).collect());
    Err(match status {
        401 => AzureError::Auth(message),
        403 => AzureError::Forbidden(message),
        404 => AzureError::NotFound(message),
        429 => AzureError::RateLimited,
        _ => AzureError::Api {
            status,
            code,
            message,
        },
    })
}

/// Sends one request to the Resource Manager, with the CLI's token.
pub async fn send(method: Method, url: &str, body: Option<&Value>) -> AzureResult<Value> {
    let token = auth::token().await?;
    let mut request = http()?.request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let bytes = read_body(response).await?;
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).chars().take(400).collect())
        })
    };
    checked(status, parsed)
}

/// The URL for a path under the Resource Manager, with the API version the service wants.
pub fn url(path: &str, api_version: &str, query: &[(&str, String)]) -> String {
    let mut url = format!("{MANAGEMENT}{path}?api-version={api_version}");
    for (key, value) in query {
        url.push_str(&format!(
            "&{key}={}",
            value.replace('&', "%26").replace(' ', "%20")
        ));
    }
    url
}

pub async fn get(path: &str, api_version: &str) -> AzureResult<Value> {
    send(Method::GET, &url(path, api_version, &[]), None).await
}

pub async fn post(path: &str, api_version: &str, body: &Value) -> AzureResult<Value> {
    send(Method::POST, &url(path, api_version, &[]), Some(body)).await
}

/// Reads a list, following Azure's `nextLink` to the end.
pub async fn list(
    path: &str,
    api_version: &str,
    query: &[(&str, String)],
) -> AzureResult<Vec<Value>> {
    let mut next = Some(url(path, api_version, query));
    let mut items = Vec::new();
    let mut pages = 0;
    while let Some(url) = next {
        let page = send(Method::GET, &url, None).await?;
        if let Some(values) = at(&page, "/value").as_array() {
            items.extend(values.iter().cloned());
        }
        pages += 1;
        next = maybe(&page, "/nextLink")
            .filter(|link| link.starts_with(MANAGEMENT) && pages < MAX_PAGES);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WEB: &str =
        "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Web/sites/shop";

    #[test]
    fn a_resource_id_gives_up_its_name_and_group() {
        assert_eq!(short(WEB), "shop");
        assert_eq!(group_of(WEB), "nine-mills");
        assert_eq!(
            group_of(
                "/subscriptions/s1/resourcegroups/NINE-MILLS/providers/Microsoft.Compute/disks/d1"
            ),
            "NINE-MILLS"
        );
        assert_eq!(group_of("/subscriptions/s1"), "");
    }

    #[test]
    fn a_name_cannot_reach_another_endpoint() {
        assert_eq!(
            segment("group", "nine-mills").ok().as_deref(),
            Some("nine-mills")
        );
        assert_eq!(
            segment("name", "vm_1.backup(old)").ok().as_deref(),
            Some("vm_1.backup(old)")
        );
        assert!(segment("group", "../subscriptions").is_err());
        assert!(segment("group", "a/b").is_err());
        assert!(segment("group", "").is_err());
    }

    #[test]
    fn azures_errors_read_in_words() {
        assert!(matches!(
            checked(403, json!({ "error": { "code": "AuthorizationFailed", "message": "no access" } })),
            Err(AzureError::Forbidden(message)) if message == "no access"
        ));
        assert!(matches!(
            checked(
                404,
                json!({ "error": { "code": "ResourceGroupNotFound", "message": "no such group" } })
            ),
            Err(AzureError::NotFound(_))
        ));
        assert!(matches!(
            checked(429, json!({})),
            Err(AzureError::RateLimited)
        ));
        assert!(matches!(
            checked(400, json!({ "error": { "code": "InvalidTemplate", "message": "bad request" } })),
            Err(AzureError::Api { code, status: 400, .. }) if code == "InvalidTemplate"
        ));
        assert_eq!(
            checked(200, json!({ "value": [] })).ok(),
            Some(json!({ "value": [] }))
        );
    }

    #[test]
    fn a_url_carries_the_api_version_and_the_query() {
        assert_eq!(
            url("/subscriptions/s1/resourcegroups", "2025-04-01", &[]),
            "https://management.azure.com/subscriptions/s1/resourcegroups?api-version=2025-04-01"
        );
        assert_eq!(
            url(
                "/x",
                "2024-01-01",
                &[("$top", "5".into()), ("$filter", "a eq b".into())]
            ),
            "https://management.azure.com/x?api-version=2024-01-01&$top=5&$filter=a%20eq%20b"
        );
    }
}
