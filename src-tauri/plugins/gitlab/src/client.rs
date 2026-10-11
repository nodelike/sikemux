// The HTTP side of the GitLab API: one warm client, a cap on how much a single
// answer may be, the server and token each account uses, GitLab's paging, and
// its error shapes turned into ours.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::{self, Account};
use crate::error::{GitlabError, GitlabResult};
use crate::ratelimit;

pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS_IN_FLIGHT: usize = 8;
const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// A token read from the Keychain is held briefly, since starting `security`
/// for every request costs more than the request.
const TOKEN_TTL: Duration = Duration::from_secs(60);
const PER_PAGE: u32 = 100;

pub async fn limited<T>(work: impl Future<Output = T>) -> T {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    let _permit = PERMITS
        .get_or_init(|| Semaphore::new(MAX_REQUESTS_IN_FLIGHT))
        .acquire()
        .await
        .ok();
    work.await
}

/// A redirect keeps the token, so only one over https on the same server is followed.
fn same_host(next: &Url, first: &Url) -> bool {
    next.scheme() == "https"
        && next.host_str() == first.host_str()
        && next.port_or_known_default() == first.port_or_known_default()
}

pub fn http() -> GitlabResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let redirects = reqwest::redirect::Policy::custom(|attempt| {
                let follow = attempt.previous().len() <= MAX_REDIRECTS
                    && attempt
                        .previous()
                        .first()
                        .is_some_and(|first| same_host(attempt.url(), first));
                if follow {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            });
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .redirect(redirects)
                .user_agent("sikemux-gitlab/0.1")
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| GitlabError::Transport("could not start the HTTP client".into()))
}

/// Where a server's API lives.
pub fn api_base(host: &str) -> String {
    format!("https://{host}/api/v4")
}

/// A token sent the way GitLab reads personal, group and project access tokens alike.
pub fn authorize(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.bearer_auth(token)
}

struct HeldToken {
    token: String,
    at: Instant,
}

tokio::task_local! {
    /// The account a call named, held for everything the call does.
    static CHOSEN: Option<String>;
}

/// Runs a call as the account it named, or with none named, as the default one.
pub fn as_account<F: Future>(account: Option<String>, work: F) -> impl Future<Output = F::Output> {
    CHOSEN.scope(account, work)
}

pub fn chosen() -> Option<String> {
    CHOSEN.try_with(Clone::clone).ok().flatten()
}

static TOKENS: Mutex<BTreeMap<String, HeldToken>> = Mutex::new(BTreeMap::new());

/// Drops the token held in memory for one account, or for all of them, so the
/// next request reads the Keychain again.
pub fn forget(account: Option<&str>) {
    if let Ok(mut held) = TOKENS.lock() {
        match account {
            Some(id) => {
                held.remove(id);
            }
            None => held.clear(),
        }
    }
}

pub struct Session {
    pub account: Account,
    pub token: String,
}

impl Session {
    /// The account the call named, or the default one.
    pub async fn current(data_dir: &Path) -> GitlabResult<Session> {
        let account = config::load(data_dir)
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
    if let Ok(held) = TOKENS.lock() {
        if let Some(held) = held
            .get(&account.id)
            .filter(|held| held.at.elapsed() < TOKEN_TTL)
        {
            return Ok(held.token.clone());
        }
    }
    let reading = account.clone();
    let token = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(GitlabError::Unconfigured)?;
    if let Ok(mut held) = TOKENS.lock() {
        held.insert(
            account.id.clone(),
            HeldToken {
                token: token.clone(),
                at: Instant::now(),
            },
        );
    }
    Ok(token)
}

/// Reads an answer of up to `limit` bytes. Past that the read fails, unless
/// `keep_tail` is set: then the start is dropped, and the second value says so.
pub async fn read_body(
    response: Response,
    limit: usize,
    keep_tail: bool,
) -> GitlabResult<(Vec<u8>, bool)> {
    let too_big = || GitlabError::Response(format!("more than {} MiB came back", limit >> 20));
    if !keep_tail
        && response
            .content_length()
            .is_some_and(|size| size > limit as u64)
    {
        return Err(too_big());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut cut = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > limit {
            if !keep_tail {
                return Err(too_big());
            }
            cut = true;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() > 2 * limit {
            bytes.drain(..bytes.len() - limit);
        }
    }
    if bytes.len() > limit {
        bytes.drain(..bytes.len() - limit);
    }
    Ok((bytes, cut))
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
    let message = error_message(bytes).unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("the request failed")
            .to_string()
    });
    match status.as_u16() {
        401 => GitlabError::Auth(message),
        403 => GitlabError::Forbidden(message),
        404 => GitlabError::NotFound(message),
        429 => GitlabError::RateLimited {
            resets_in_secs: ratelimit::latest_wait(),
        },
        status => GitlabError::Http { status, message },
    }
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
        ratelimit::check(&session.account.id)?;
        let mut request = authorize(
            http()?.request(method.clone(), format!("{}{path}", session.base())),
            &session.token,
        );
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = limited(request.send()).await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::UNAUTHORIZED {
            forget(Some(&session.account.id));
        }
        if let Some(secs) = ratelimit::named_wait(status, &headers)
            .filter(|secs| *secs <= ratelimit::SHORT_WAIT_SECS && !waited)
        {
            waited = true;
            tokio::time::sleep(Duration::from_secs(secs.max(1))).await;
            continue;
        }
        ratelimit::observe(&session.account.id, status, &headers);
        let (bytes, cut) = read_body(response, limit, keep_tail).await?;
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
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::from_slice(b"null")?);
    }
    Ok(serde_json::from_slice(bytes)?)
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
    let mut all = Vec::new();
    let mut page = 1;
    for _ in 0..max_pages.max(1) {
        let mut found: Page<T> = get_page(data_dir, path, query, page, PER_PAGE).await?;
        all.append(&mut found.items);
        match found.next {
            Some(next) if next > page => page = next,
            _ => break,
        }
    }
    Ok(all)
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
    fn only_a_redirect_over_https_on_the_same_server_is_followed() {
        let url = |raw: &str| Url::parse(raw).expect("parses");
        let api = url("https://gitlab.acme.dev/api/v4/projects/a%2Fb");
        assert!(same_host(
            &url("https://gitlab.acme.dev/api/v4/projects/9"),
            &api
        ));
        assert!(same_host(&url("https://gitlab.acme.dev:443/x"), &api));
        assert!(!same_host(&url("http://gitlab.acme.dev/x"), &api));
        assert!(!same_host(&url("https://gitlab.acme.dev:8443/x"), &api));
        assert!(!same_host(&url("https://storage.acme.dev/x"), &api));
    }

    #[test]
    fn the_token_goes_in_the_authorization_header() -> GitlabResult<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let request = authorize(http()?.get("https://gitlab.com/api/v4/user"), "glpat-x")
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
