// Sentry's REST API: one warm client, the token from the Keychain, the next page
// Sentry names in its Link header, and its errors in words.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::{Client, Method, Response};
use serde_json::Value;

use crate::config::{self, Settings};
use crate::error::{SentryError, SentryResult};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const TOKEN_TTL: Duration = Duration::from_secs(60);

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> Option<String> {
    at(value, path)
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub fn number(value: &Value, path: &str) -> u64 {
    let found = at(value, path);
    found
        .as_u64()
        .or_else(|| found.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(0)
}

pub fn http() -> SentryResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-sentry/0.1")
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| SentryError::Transport("could not start the HTTP client".into()))
}

static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn forget_token() {
    if let Ok(mut held) = TOKEN.lock() {
        *held = None;
    }
}

async fn token() -> SentryResult<String> {
    if let Ok(held) = TOKEN.lock() {
        if let Some((token, _)) = held.as_ref().filter(|(_, at)| at.elapsed() < TOKEN_TTL) {
            return Ok(token.clone());
        }
    }
    let token = config::blocking(config::token_read)
        .await?
        .ok_or(SentryError::Unconfigured)?;
    if let Ok(mut held) = TOKEN.lock() {
        *held = Some((token.clone(), Instant::now()));
    }
    Ok(token)
}

/// A page of a list, and the cursor of the next one when Sentry says there is more.
pub struct Page {
    pub body: Value,
    pub next: Option<String>,
}

/// The cursor of the next page, from a Link header such as
/// `<…?cursor=0:100:0>; rel="next"; results="true"; cursor="0:100:0"`.
pub fn next_cursor(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let part = part.trim();
        let is_next = part.contains("rel=\"next\"");
        let has_results = part.contains("results=\"true\"");
        if !(is_next && has_results) {
            return None;
        }
        let start = part.find("cursor=\"")? + "cursor=\"".len();
        let rest = part.get(start..)?;
        let end = rest.find('"')?;
        rest.get(..end).map(str::to_string)
    })
}

/// Sentry's error in words, from a `detail` field or the start of the body.
pub fn detail_of(bytes: &[u8]) -> String {
    let parsed: Option<Value> = serde_json::from_slice(bytes).ok();
    parsed
        .as_ref()
        .and_then(|body| {
            text(body, "/detail")
                .or_else(|| text(body, "/detail/message"))
                .or_else(|| {
                    body.as_object()
                        .and_then(|fields| fields.values().next())
                        .and_then(Value::as_array)
                        .and_then(|reasons| reasons.first())
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).chars().take(200).collect())
}

pub fn checked(status: u16, bytes: &[u8]) -> SentryResult<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let detail = detail_of(bytes);
    Err(match status {
        401 => SentryError::Auth(if detail.is_empty() {
            "Sentry turned the token away".into()
        } else {
            detail
        }),
        403 => SentryError::Forbidden(format!(
            "{detail}. The token may be missing a scope this needs"
        )),
        404 => SentryError::NotFound("no such issue or project, or this token can't see it".into()),
        429 => SentryError::RateLimited,
        _ => SentryError::Http {
            status,
            message: detail,
        },
    })
}

async fn read_body(response: Response) -> SentryResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SentryError::Response("more than 16 MiB came back".into()));
        }
    }
    Ok(bytes)
}

/// Sends a request with the token given, without reading the Keychain.
pub async fn send_with(
    base: &str,
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> SentryResult<Page> {
    let mut request = http()?
        .request(method, format!("{base}/api/0{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let next = response
        .headers()
        .get("link")
        .and_then(|link| link.to_str().ok())
        .and_then(next_cursor);
    let bytes = read_body(response).await?;
    checked(status, &bytes)?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok(Page { body, next })
}

/// The settings and the token of the sign-in, read once per call.
pub struct Session {
    pub settings: Settings,
    token: String,
}

impl Session {
    pub async fn open(data_dir: &Path) -> SentryResult<Session> {
        let settings = config::load(data_dir);
        if !settings.is_signed_in() {
            return Err(SentryError::Unconfigured);
        }
        Ok(Session {
            settings,
            token: token().await?,
        })
    }

    /// The organisation named, or the one signed in to.
    pub fn org<'a>(&'a self, named: Option<&'a str>) -> &'a str {
        named
            .filter(|named| !named.is_empty())
            .unwrap_or(&self.settings.org)
    }

    pub async fn page(&self, path: &str, query: &[(&str, String)]) -> SentryResult<Page> {
        send_with(
            &self.settings.url,
            &self.token,
            Method::GET,
            path,
            query,
            None,
        )
        .await
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> SentryResult<Value> {
        Ok(self.page(path, query).await?.body)
    }

    pub async fn send(&self, method: Method, path: &str, body: &Value) -> SentryResult<Value> {
        Ok(send_with(
            &self.settings.url,
            &self.token,
            method,
            path,
            &[],
            Some(body),
        )
        .await?
        .body)
    }
}

/// A project's numeric id, which Sentry's lists filter by, from its slug.
pub async fn project_id(session: &Session, org: &str, project: &str) -> SentryResult<String> {
    let project = slug("project", project)?;
    let found = session
        .get(&format!("/projects/{org}/{project}/"), &[])
        .await?;
    text(&found, "/id").ok_or_else(|| SentryError::NotFound(format!("no project {project}")))
}

/// Keeps a slug or id to the letters Sentry uses, so it can't reach another path.
pub fn slug<'a>(what: &str, value: &'a str) -> SentryResult<&'a str> {
    let ok = !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !value.starts_with('.');
    if ok {
        Ok(value)
    } else {
        Err(SentryError::BadArg(format!(
            "{what} {value:?} is not one Sentry would use"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_page_is_read_from_the_link_header() {
        let link = r#"<https://sentry.io/api/0/organizations/acme/issues/?cursor=0:0:1>; rel="previous"; results="false"; cursor="0:0:1", <https://sentry.io/api/0/organizations/acme/issues/?cursor=0:25:0>; rel="next"; results="true"; cursor="0:25:0""#;
        assert_eq!(next_cursor(link).as_deref(), Some("0:25:0"));
        let last = r#"<…>; rel="next"; results="false"; cursor="0:50:0""#;
        assert_eq!(next_cursor(last), None);
    }

    #[test]
    fn sentrys_errors_read_in_words() {
        assert!(matches!(
            checked(401, br#"{"detail":"Invalid token"}"#),
            Err(SentryError::Auth(message)) if message == "Invalid token"
        ));
        assert!(matches!(
            checked(403, br#"{"detail":"You do not have permission to perform this action."}"#),
            Err(SentryError::Forbidden(message)) if message.contains("scope")
        ));
        assert!(matches!(checked(404, b"{}"), Err(SentryError::NotFound(_))));
        assert!(matches!(checked(429, b""), Err(SentryError::RateLimited)));
        assert!(matches!(
            checked(400, br#"{"status":["Invalid status"]}"#),
            Err(SentryError::Http { status: 400, message }) if message == "Invalid status"
        ));
        assert!(checked(204, b"").is_ok());
    }

    #[test]
    fn slugs_keep_to_sentrys_letters() {
        assert!(slug("project", "storefront-web").is_ok());
        assert!(slug("issue", "4512887211").is_ok());
        assert!(slug("project", "../users").is_err());
        assert!(slug("project", "a/b").is_err());
        assert!(slug("project", "").is_err());
    }

    #[tokio::test]
    async fn a_token_sentry_does_not_know_is_refused_as_a_sign_in() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        match send_with(
            config::SENTRY_IO,
            "sntryu_not-a-token",
            Method::GET,
            "/organizations/",
            &[],
            None,
        )
        .await
        {
            Err(SentryError::Auth(_)) | Err(SentryError::Unreachable(_)) => {}
            other => panic!("{:?}", other.map(|page| page.body)),
        }
    }
}
