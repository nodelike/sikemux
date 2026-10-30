// The HTTP side of the GitLab API: one warm client, a cap on how much a single
// answer may be, the server and token each account uses, GitLab's paging, and
// its error shapes turned into ours.

use std::path::Path;

use reqwest::header::HeaderMap;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Deadline, Http, Redirects};
use sikemux_plugin_api::ratelimit::{LimitHeaders, RateLimits};
use sikemux_plugin_api::store::{self, Accounts, Held};

use crate::config::{self, Account};
use crate::error::{Gitlab, GitlabError, GitlabResult};

pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const PER_PAGE: u32 = 100;

pub static HTTP: Http<Gitlab> = Http::new(Redirects::SameHost, Deadline::WholeAnswer, 8);
pub static LIMITS: RateLimits = RateLimits::new(LimitHeaders {
    limit: "ratelimit-limit",
    remaining: Some("ratelimit-remaining"),
    reset: Some("ratelimit-reset"),
    near: None,
});
static TOKENS: Held<String> = Held::secrets();

/// Where a server's API lives.
pub fn api_base(host: &str) -> String {
    format!("https://{host}/api/v4")
}

/// A token sent the way GitLab reads personal, group and project access tokens alike.
pub fn authorize(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.bearer_auth(token)
}

/// Drops the token held in memory for one account, or for all of them, so the
/// next request reads the Keychain again.
pub fn forget(account: Option<&str>) {
    TOKENS.forget(account);
}

pub struct Session {
    pub account: Account,
    pub token: String,
}

impl Session {
    /// The account the call named, or the default one.
    pub async fn current(data_dir: &Path) -> GitlabResult<Session> {
        let account = config::load(data_dir)?
            .account(chosen().as_deref())
            .cloned()
            .ok_or(GitlabError::Unconfigured)?;
        let token = token_of(&account).await?;
        Ok(Session { account, token })
    }

    pub fn base(&self) -> String {
        api_base(&self.account.host)
    }
}

async fn token_of(account: &Account) -> GitlabResult<String> {
    if let Some(token) = TOKENS.get(&account.id) {
        return Ok(token);
    }
    let reading = account.keychain_account();
    let token = store::blocking(move || Ok(config::KEYCHAIN.read(&reading)?))
        .await?
        .ok_or(GitlabError::Unconfigured)?;
    TOKENS.put(&account.id, token.clone());
    Ok(token)
}

/// GitLab says what went wrong as `message` (a sentence, a list, or a field
/// name with its complaints) or as `error` with `error_description`.
pub fn error_message(bytes: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let flatten = |value: &Value| -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Array(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            Value::Object(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, problems)| {
                        let said = match problems {
                            Value::Array(items) => items
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", "),
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        format!("{field} {said}")
                    })
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            _ => None,
        }
    };
    if let Some(message) = body.get("message").and_then(flatten) {
        return Some(message);
    }
    let error = body.get("error").and_then(Value::as_str)?;
    Some(
        match body.get("error_description").and_then(Value::as_str) {
            Some(description) if !description.is_empty() => format!("{error}: {description}"),
            _ => error.to_string(),
        },
    )
}

pub fn classify(status: StatusCode, bytes: &[u8]) -> GitlabError {
    http::classify::<Gitlab>(
        status,
        error_message(bytes),
        (status == StatusCode::TOO_MANY_REQUESTS).then(|| LIMITS.latest_wait()),
    )
}

/// An answer: its status, headers, body, and whether the body's start was cut.
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
    pub cut: bool,
}

/// A request to the API of the call's account, waiting out a short rate-limit pause once.
pub async fn send_limited(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
    limit: usize,
    keep_tail: bool,
) -> GitlabResult<Answer> {
    let mut waited = false;
    loop {
        let session = Session::current(data_dir).await?;
        let account = session.account.id.as_str();
        LIMITS.check(account)?;
        let mut request = authorize(
            HTTP.client()?
                .request(method.clone(), format!("{}{path}", session.base())),
            &session.token,
        );
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = HTTP.send(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::UNAUTHORIZED {
            forget(Some(account));
        }
        if let Some(pause) = LIMITS
            .short_wait(account, status, &headers, b"")
            .filter(|_| !waited)
        {
            waited = true;
            tokio::time::sleep(pause).await;
            continue;
        }
        LIMITS.observe(account, status, &headers, b"");
        let (bytes, cut) = if keep_tail {
            http::read_tail(response, limit).await?
        } else {
            (http::read_body(response, limit).await?, false)
        };
        return Ok(Answer {
            status,
            headers,
            bytes,
            cut,
        });
    }
}

async fn fetch(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GitlabResult<Answer> {
    let answer = send_limited(
        data_dir,
        method,
        path,
        query,
        body,
        MAX_RESPONSE_BYTES,
        false,
    )
    .await?;
    if !answer.status.is_success() {
        return Err(classify(answer.status, &answer.bytes));
    }
    Ok(answer)
}

pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> GitlabResult<T> {
    Ok(http::parse(bytes)?)
}

pub async fn get<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
) -> GitlabResult<T> {
    parse(&fetch(data_dir, Method::GET, path, query, None).await?.bytes)
}

/// Logs and files, which come back as plain text rather than JSON.
pub async fn get_text(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
) -> GitlabResult<String> {
    let answer = fetch(data_dir, Method::GET, path, query, None).await?;
    Ok(String::from_utf8_lossy(&answer.bytes).into_owned())
}

pub async fn send_json<T: DeserializeOwned>(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: &Value,
) -> GitlabResult<T> {
    parse(&fetch(data_dir, method, path, &[], Some(body)).await?.bytes)
}

/// A write whose answer nobody reads, such as approving or cancelling something.
pub async fn write(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> GitlabResult<()> {
    fetch(data_dir, method, path, &[], body).await?;
    Ok(())
}

fn header_number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// One page of a list, with the page after it and the total when GitLab says them.
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<u32>,
    pub total: Option<u64>,
}

pub async fn get_page<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    page: u32,
    per_page: u32,
) -> GitlabResult<Page<T>> {
    let mut query = query.to_vec();
    query.push(("page", page.max(1).to_string()));
    query.push(("per_page", per_page.clamp(1, PER_PAGE).to_string()));
    let answer = fetch(data_dir, Method::GET, path, &query, None).await?;
    Ok(Page {
        items: parse(&answer.bytes)?,
        next: header_number(&answer.headers, "x-next-page").map(|next| next as u32),
        total: header_number(&answer.headers, "x-total"),
    })
}

/// Every page of a list, up to `max_pages` of them.
pub async fn get_all<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    max_pages: u32,
) -> GitlabResult<Vec<T>> {
    http::collect_pages(1, max_pages, |page| async move {
        let found: Page<T> = get_page(data_dir, path, query, page, PER_PAGE).await?;
        Ok((found.items, found.next.filter(|next| *next > page)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_shape_gitlab_reports_errors_in() {
        assert_eq!(
            error_message(br#"{"message":"404 Project Not Found"}"#).as_deref(),
            Some("404 Project Not Found")
        );
        assert_eq!(
            error_message(br#"{"message":{"title":["can't be blank"],"source_branch":["is invalid","is missing"]}}"#).as_deref(),
            Some("title can't be blank; source_branch is invalid, is missing")
        );
        assert_eq!(
            error_message(br#"{"message":["Branch cannot be merged","Pipeline must succeed"]}"#)
                .as_deref(),
            Some("Branch cannot be merged; Pipeline must succeed")
        );
        assert_eq!(
            error_message(br#"{"error":"insufficient_scope","error_description":"The request requires higher privileges"}"#).as_deref(),
            Some("insufficient_scope: The request requires higher privileges")
        );
        assert!(error_message(b"<html>").is_none());
    }

    #[test]
    fn the_token_goes_in_the_authorization_header() -> GitlabResult<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let request = authorize(
            HTTP.client()?.get("https://gitlab.com/api/v4/user"),
            "glpat-x",
        )
        .build()
        .map_err(|error| GitlabError::Transport(error.to_string()))?;
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer glpat-x")
        );
        assert!(request.headers().get("private-token").is_none());
        Ok(())
    }
}
