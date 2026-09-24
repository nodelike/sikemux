// Shared HTTP plumbing. One process-wide reqwest::Client keeps connections
// warm across the matrix dashboard's parallel fan-out. A rejected token is
// reported as an `auth` error; nothing here logs in again on its own.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Client, Method, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::error::{RundeckError, RundeckResult};

use crate::config;

pub const API_VERSION: u32 = 41;
/// Job definitions are only served as JSON from API v44 on.
pub const JOB_DEFINITION_API_VERSION: u32 = 44;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS_IN_FLIGHT: usize = 12;
const ITEM_UNAUTHORIZED: &str = "api.error.item.unauthorized";
const REDIRECTED: &str = "Rundeck redirected the API call — a proxy or SSO login is in front of it";
const NOT_API_DATA: &str =
    "Rundeck answered with a web page instead of API data — a proxy or SSO login is in front of it";

const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Percent-encodes one value for use as a single URL path segment.
pub fn seg(value: &str) -> RundeckResult<String> {
    if matches!(value, "" | "." | "..") {
        return Err(RundeckError::BadArg("empty or dot-only path segment"));
    }
    Ok(utf8_percent_encode(value, PATH_SEGMENT).to_string())
}

/// Runs `work` once one of the plugin-wide request slots is free, so fan-outs
/// from different screens share one bound on concurrent Rundeck calls.
pub async fn limited<T>(work: impl Future<Output = T>) -> T {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    let _permit = PERMITS
        .get_or_init(|| Semaphore::new(MAX_REQUESTS_IN_FLIGHT))
        .acquire()
        .await
        .ok();
    work.await
}

fn http() -> RundeckResult<&'static Client> {
    static C: OnceLock<Option<Client>> = OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            // Servers behind corporate proxies sometimes drop idle keep-alive
            // after ~30s. 25s pool idle keeps reuse safe.
            .pool_idle_timeout(Duration::from_secs(25))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("sikemux-rundeck/0.1")
            .build()
            .ok()
    })
    .as_ref()
    .ok_or_else(|| RundeckError::Api("could not start the HTTP client".into()))
}

/// A warm client per pinned target. Building one per request threw away the
/// connection pool, so a private-HTTP install paid a fresh handshake on every
/// call of the dashboard's fan-out. The key is the pinned address set, so a
/// different DNS answer never reuses the old client.
fn pinned_http(transport: &config::ValidatedTransport) -> RundeckResult<Client> {
    const MAX_CACHED_CLIENTS: usize = 8;
    static CACHE: OnceLock<Mutex<HashMap<String, Client>>> = OnceLock::new();

    let key = transport.pin_key();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(client) = cache.get(&key) {
        return Ok(client.clone());
    }

    let client = transport
        .pin_dns(Client::builder())
        .pool_idle_timeout(Duration::from_secs(25))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("sikemux-rundeck/0.1")
        .build()?;
    if cache.len() >= MAX_CACHED_CLIENTS {
        cache.clear();
    }
    cache.insert(key, client.clone());
    Ok(client)
}

fn api_url(base: &str, version: u32, endpoint: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/api/{version}{endpoint}")
}

/// Where a request goes and which token it carries.
pub struct Session {
    pub url: String,
    pub token: String,
    pub allow_insecure_private_http: bool,
}

impl Session {
    /// The saved session. Re-read from disk on every request so a `rnd login`
    /// in a terminal is picked up without restarting the app.
    pub async fn current() -> RundeckResult<Session> {
        let cfg = config::refresh_from_disk()
            .await
            .unwrap_or_else(|_| config::RundeckConfig::default());
        if cfg.url.is_empty() || cfg.token.is_empty() {
            return Err(RundeckError::Unconfigured);
        }
        Ok(Session {
            url: cfg.url,
            token: cfg.token,
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        })
    }
}

async fn send(
    session: &Session,
    version: u32,
    method: Method,
    endpoint: &str,
    body: Option<&Value>,
    query: &[(&str, String)],
) -> RundeckResult<Response> {
    let transport =
        config::validate_transport(&session.url, session.allow_insecure_private_http).await?;

    // For acknowledged private HTTP, bind this client to the exact private
    // addresses that passed validation. This closes the re-resolution gap a
    // DNS rebinding response could otherwise exploit between policy and send.
    let private_client = if transport.pins_private_dns() {
        Some(pinned_http(&transport)?)
    } else {
        None
    };
    let client = match private_client.as_ref() {
        Some(client) => client,
        None => http()?,
    };
    let mut req = client
        .request(method, api_url(&session.url, version, endpoint))
        .header("X-Rundeck-Auth-Token", &session.token)
        .header("Accept", "application/json");
    if !query.is_empty() {
        req = req.query(query);
    }
    if let Some(b) = body {
        req = req.json(b);
    }
    Ok(req.send().await?)
}

pub async fn request_as<T: DeserializeOwned>(
    session: &Session,
    method: Method,
    endpoint: &str,
    body: Option<&Value>,
    query: &[(&str, String)],
) -> RundeckResult<T> {
    let resp = send(session, API_VERSION, method, endpoint, body, query).await?;
    decode(resp).await
}

pub async fn get_json<T: DeserializeOwned>(
    endpoint: &str,
    query: &[(&str, String)],
) -> RundeckResult<T> {
    let session = Session::current().await?;
    request_as(&session, Method::GET, endpoint, None, query).await
}

pub async fn get_json_at_version<T: DeserializeOwned>(
    version: u32,
    endpoint: &str,
    query: &[(&str, String)],
) -> RundeckResult<T> {
    let session = Session::current().await?;
    let resp = send(&session, version, Method::GET, endpoint, None, query).await?;
    decode(resp).await
}

pub async fn post_json<B: Serialize, T: DeserializeOwned>(
    endpoint: &str,
    body: &B,
) -> RundeckResult<T> {
    let val = serde_json::to_value(body).map_err(RundeckError::Json)?;
    let session = Session::current().await?;
    request_as(&session, Method::POST, endpoint, Some(&val), &[]).await
}

pub async fn post_empty_json<T: DeserializeOwned>(endpoint: &str) -> RundeckResult<T> {
    let session = Session::current().await?;
    request_as(&session, Method::POST, endpoint, None, &[]).await
}

async fn decode<T: DeserializeOwned>(resp: Response) -> RundeckResult<T> {
    let status = resp.status();
    if resp
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(RundeckError::Api("response exceeds 16 MiB limit".into()));
    }
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(RundeckError::Api("response exceeds 16 MiB limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = classify(status, &bytes)?;
    serde_json::from_value::<T>(value).map_err(RundeckError::Json)
}

fn classify(status: StatusCode, bytes: &[u8]) -> RundeckResult<Value> {
    if status.is_redirection() {
        return Err(RundeckError::Auth(REDIRECTED.into()));
    }
    if !status.is_success() {
        let body = String::from_utf8_lossy(bytes);
        let details = ErrorDetails::parse(&body);
        let message = details.message.unwrap_or_else(|| {
            if body.trim().is_empty() || looks_like_html(&body) {
                status.canonical_reason().unwrap_or("error").to_string()
            } else {
                body.to_string()
            }
        });
        return Err(match status.as_u16() {
            401 => RundeckError::Auth(message),
            403 if details.code.as_deref() == Some(ITEM_UNAUTHORIZED) => {
                RundeckError::Forbidden(message)
            }
            403 => RundeckError::Auth(message),
            status => RundeckError::Http { status, message },
        });
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes).map_err(|_| RundeckError::Auth(NOT_API_DATA.into()))
}

fn looks_like_html(body: &str) -> bool {
    body.trim_start().starts_with('<')
}

struct ErrorDetails {
    message: Option<String>,
    code: Option<String>,
}

impl ErrorDetails {
    fn parse(body: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            return Self {
                message: None,
                code: None,
            };
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(String::from);
        Self {
            message: ["message", "error", "errorMessage"]
                .into_iter()
                .find_map(text),
            code: text("errorCode"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(status: u16, body: &str) -> String {
        let error = classify(StatusCode::from_u16(status).unwrap(), body.as_bytes()).unwrap_err();
        sikemux_plugin_api::PluginError::from(error).category
    }

    #[test]
    fn a_denied_item_is_forbidden_but_a_rejected_token_is_auth() {
        let item = r#"{"error":true,"errorCode":"api.error.item.unauthorized","message":"Not authorized for action Run"}"#;
        let token = r#"{"error":true,"errorCode":"api.error.item.forbidden","message":"no"}"#;
        assert_eq!(category(403, item), "forbidden");
        assert_eq!(category(403, token), "auth");
        assert_eq!(category(403, ""), "auth");
        assert_eq!(category(401, "{}"), "auth");
    }

    #[test]
    fn redirects_and_web_pages_count_as_signed_out() {
        assert_eq!(category(302, ""), "auth");
        assert_eq!(category(200, "<!DOCTYPE html><html></html>"), "auth");
        assert_eq!(category(404, "<html>missing</html>"), "http");
    }

    #[test]
    fn error_messages_come_from_the_json_body() {
        let err = classify(
            StatusCode::BAD_REQUEST,
            br#"{"errorCode":"api.error.x","message":"bad job"}"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, RundeckError::Http { status: 400, ref message } if message == "bad job")
        );
    }

    #[test]
    fn empty_success_bodies_decode_as_null() {
        assert_eq!(classify(StatusCode::NO_CONTENT, b"").unwrap(), Value::Null);
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(seg("my project/x").unwrap(), "my%20project%2Fx");
        assert_eq!(seg("a?b#c%").unwrap(), "a%3Fb%23c%25");
        assert_eq!(seg("dev-api_1.x~").unwrap(), "dev-api_1.x~");
        assert!(seg("..").is_err());
        assert!(seg("").is_err());
    }
}
