// The HTTP side of the GitHub API: one warm client, a cap on how much a
// single answer may be, and GitHub's error shapes turned into ours. Nothing
// here signs in again on its own; a refused token is reported as such.

use std::future::Future;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::{self, Account};
use crate::error::{GithubError, GithubResult};
use crate::ratelimit;

pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS_IN_FLIGHT: usize = 8;
const MAX_REDIRECTS: usize = 5;
/// Split across a host's addresses, so one that never answers, as happens on
/// some networks for one of GitHub's CDN addresses, costs a second before the
/// next is tried rather than the whole request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const API_VERSION: &str = "2022-11-28";
const JSON: &str = "application/vnd.github+json";

/// Runs `work` once one of the plugin-wide request slots is free, so a screen
/// that fans out over many jobs shares one bound with every other screen.
pub async fn limited<T>(work: impl Future<Output = T>) -> T {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    let _permit = PERMITS
        .get_or_init(|| Semaphore::new(MAX_REQUESTS_IN_FLIGHT))
        .acquire()
        .await
        .ok();
    work.await
}

/// GitHub answers for a renamed or moved repository with a redirect on its
/// own host, which keeps the token. A redirect anywhere else is storage, which
/// is left for the caller to ask without it.
fn same_host(next: &Url, first: &Url) -> bool {
    next.scheme() == "https"
        && next.host_str() == first.host_str()
        && next.port_or_known_default() == first.port_or_known_default()
}

/// Files can be large and slow, so a transfer has no overall deadline, only
/// one on connecting and one on going quiet.
fn build(transfer: bool) -> Option<Client> {
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
    let builder = Client::builder()
        .pool_idle_timeout(Duration::from_secs(25))
        .redirect(redirects)
        .user_agent("sikemux-github/0.1");
    let builder = builder.connect_timeout(CONNECT_TIMEOUT);
    let builder = if transfer {
        builder.read_timeout(Duration::from_secs(60))
    } else {
        builder.timeout(Duration::from_secs(30))
    };
    builder.build().ok()
}

fn no_client() -> GithubError {
    GithubError::Transport("could not start the HTTP client".into())
}

fn transfers() -> GithubResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build(true))
        .as_ref()
        .ok_or_else(no_client)
}

pub fn http() -> GithubResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build(false))
        .as_ref()
        .ok_or_else(no_client)
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

pub struct Session {
    pub account: Account,
    pub token: String,
}

impl Session {
    /// Read fresh every time, so a `gh auth login` in a terminal is picked up
    /// without restarting the app.
    pub async fn current(data_dir: &Path) -> GithubResult<Session> {
        let data_dir = data_dir.to_path_buf();
        let chosen = chosen();
        config::blocking(Box::new(move || {
            let config = config::load(&data_dir);
            let (account, token) =
                config::resolve(&config, chosen.as_deref())?.ok_or(GithubError::Unconfigured)?;
            Ok(Session { account, token })
        }))
        .await
    }
}

/// Reads an answer of up to `limit` bytes. Past that the read fails, unless
/// `keep_tail` is set: then the start is dropped, and the second value says so.
pub async fn read_body(
    response: Response,
    limit: usize,
    keep_tail: bool,
) -> GithubResult<(Vec<u8>, bool)> {
    let too_big = || GithubError::Response(format!("more than {} MiB came back", limit >> 20));
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

fn error_message(bytes: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let message = body.get("message")?.as_str()?.to_string();
    let detail = body
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(|first| {
            first
                .get("message")
                .or_else(|| first.get("code"))
                .and_then(Value::as_str)
        });
    Some(match detail {
        Some(detail) if !message.contains(detail) => format!("{message}: {detail}"),
        _ => message,
    })
}

pub fn classify(status: StatusCode, headers: &HeaderMap, bytes: &[u8]) -> GithubError {
    let message = error_message(bytes).unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("the request failed")
            .to_string()
    });
    match status.as_u16() {
        401 => GithubError::Auth(message),
        403 | 429 => match ratelimit::wait_for(status, headers, bytes) {
            Some(resets_in_secs) => GithubError::RateLimited { resets_in_secs },
            None => GithubError::Forbidden(message),
        },
        404 => GithubError::NotFound(message),
        status => GithubError::Http { status, message },
    }
}

fn api_request(
    client: &Client,
    session: &Session,
    method: Method,
    path: &str,
    accept: &str,
) -> RequestBuilder {
    client
        .request(
            method,
            format!("{}{path}", config::api_base(&session.account.host)),
        )
        .bearer_auth(&session.token)
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", API_VERSION)
}

/// A limit that resets within a few seconds is waited out and the request
/// sent once more; a longer one fails at once, and so does everything after it
/// until the limit resets.
async fn exchange(
    account: &str,
    request: RequestBuilder,
) -> GithubResult<(StatusCode, HeaderMap, Vec<u8>)> {
    let mut request = request;
    loop {
        ratelimit::check(account)?;
        let again = request.try_clone();
        let response = limited(request.send()).await?;
        let status = response.status();
        let headers = response.headers().clone();
        let (bytes, _) = read_body(response, MAX_RESPONSE_BYTES, false).await?;
        if status == StatusCode::UNAUTHORIZED {
            config::forget_token(Some(account));
        }
        let wait = ratelimit::wait_for(status, &headers, &bytes);
        match (wait, again) {
            (Some(secs), Some(retry)) if secs <= ratelimit::SHORT_WAIT_SECS => {
                tokio::time::sleep(Duration::from_secs(secs.max(1))).await;
                request = retry;
            }
            _ => {
                ratelimit::observe(account, status, &headers, &bytes);
                return Ok((status, headers, bytes));
            }
        }
    }
}

pub async fn send(
    session: &Session,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GithubResult<(StatusCode, HeaderMap, Vec<u8>)> {
    let mut request = api_request(http()?, session, method, path, JSON);
    if !query.is_empty() {
        request = request.query(query);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    exchange(&session.account.id, request).await
}

/// The body of a successful answer. The network half of every call lives in
/// these functions, which are not generic, so each shape of answer only adds
/// its own parsing to the app rather than another copy of the request.
async fn body_of(
    session: &Session,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GithubResult<Vec<u8>> {
    let (status, headers, bytes) = send(session, method, path, query, body).await?;
    if !status.is_success() {
        return Err(classify(status, &headers, &bytes));
    }
    Ok(bytes)
}

async fn fetch(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> GithubResult<Vec<u8>> {
    let session = Session::current(data_dir).await?;
    body_of(&session, method, path, query, body).await
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> GithubResult<T> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::from_slice(b"null")?);
    }
    Ok(serde_json::from_slice(bytes)?)
}

pub async fn get<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
) -> GithubResult<T> {
    parse(&fetch(data_dir, Method::GET, path, query, None).await?)
}

/// A write whose answer is the thing it made, such as a new pull request or issue.
pub async fn send_json<T: DeserializeOwned>(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: &Value,
) -> GithubResult<T> {
    parse(&fetch(data_dir, method, path, &[], Some(body)).await?)
}

/// Every page of a list GitHub splits into pages of a hundred, up to
/// `max_pages` of them. `items` takes the list out of a page, for the
/// endpoints that wrap it in an object.
pub async fn get_all<P: DeserializeOwned, T>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    max_pages: u32,
    items: impl Fn(P) -> Vec<T>,
) -> GithubResult<Vec<T>> {
    const PAGE: usize = 100;
    let mut all = Vec::new();
    for page in 1..=max_pages.max(1) {
        let mut paged = query.to_vec();
        paged.push(("per_page", PAGE.to_string()));
        paged.push(("page", page.to_string()));
        let batch = items(get::<P>(data_dir, path, &paged).await?);
        let short = batch.len() < PAGE;
        all.extend(batch);
        if short {
            break;
        }
    }
    Ok(all)
}

/// An answer to a conditional read. GitHub does not count a "nothing changed"
/// answer against the rate limit, which is what lets a run be watched closely.
pub struct Conditional<T> {
    /// Absent when nothing changed since `etag`.
    pub value: Option<T>,
    pub etag: Option<String>,
    /// Requests left before the rate limit, when GitHub said.
    pub remaining: Option<u64>,
}

pub async fn get_if_changed<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    etag: Option<&str>,
) -> GithubResult<Conditional<T>> {
    let (bytes, etag, remaining) = fetch_if_changed(data_dir, path, query, etag).await?;
    Ok(Conditional {
        value: bytes.as_deref().map(parse).transpose()?,
        etag,
        remaining,
    })
}

async fn fetch_if_changed(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    etag: Option<&str>,
) -> GithubResult<(Option<Vec<u8>>, Option<String>, Option<u64>)> {
    let session = Session::current(data_dir).await?;
    let mut request = api_request(http()?, &session, Method::GET, path, JSON).query(query);
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let (status, headers, bytes) = exchange(&session.account.id, request).await?;
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let remaining = header("x-ratelimit-remaining").and_then(|value| value.trim().parse().ok());
    if status == StatusCode::NOT_MODIFIED {
        return Ok((None, etag.map(str::to_string), remaining));
    }
    if !status.is_success() {
        return Err(classify(status, &headers, &bytes));
    }
    Ok((Some(bytes), header("etag").map(str::to_string), remaining))
}

pub async fn post_empty(data_dir: &Path, path: &str, body: Option<&Value>) -> GithubResult<()> {
    act(data_dir, Method::POST, path, body).await
}

/// Logs, artifacts and release files are served as a redirect to storage,
/// which is followed without the token: the signed address carries its own
/// permission, and storage refuses a request that sends both.
pub async fn open_download(data_dir: &Path, path: &str, accept: &str) -> GithubResult<Response> {
    let session = Session::current(data_dir).await?;
    let client = transfers()?;
    let account = session.account.id.as_str();
    ratelimit::check(account)?;
    let mut response =
        limited(api_request(client, &session, Method::GET, path, accept).send()).await?;
    ratelimit::observe(account, response.status(), response.headers(), b"");
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| GithubError::Response("the download redirect had no address".into()))?
            .to_string();
        response = limited(client.execute(from_storage(&location)?)).await?;
    }
    if !response.status().is_success() {
        return Err(failure(account, response).await);
    }
    Ok(response)
}

pub async fn failure(account: &str, response: Response) -> GithubError {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = read_body(response, MAX_RESPONSE_BYTES, false)
        .await
        .map(|(bytes, _)| bytes)
        .unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        config::forget_token(Some(account));
    }
    ratelimit::observe(account, status, &headers, &bytes);
    classify(status, &headers, &bytes)
}

/// A file GitHub hands over whole, such as a log or a workflow's YAML.
pub async fn download_as(
    data_dir: &Path,
    path: &str,
    accept: &str,
    keep_tail: bool,
) -> GithubResult<(Vec<u8>, bool)> {
    let response = open_download(data_dir, path, accept).await?;
    read_body(response, MAX_RESPONSE_BYTES, keep_tail).await
}

/// Storage answers 400 to any `Authorization` header, an empty one included.
fn from_storage(location: &str) -> GithubResult<reqwest::Request> {
    let url = Url::parse(location)
        .ok()
        .filter(|url| url.scheme() == "https")
        .ok_or_else(|| {
            GithubError::Response("the download redirect was not an https address".into())
        })?;
    Ok(reqwest::Request::new(Method::GET, url))
}

/// A call whose answer is only its status, which is how GitHub replies to the
/// buttons that start, stop, approve or switch something off.
pub async fn act(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> GithubResult<()> {
    let session = Session::current(data_dir).await?;
    let (status, headers, bytes) = send(&session, method, path, &[], body).await?;
    if status.is_success() {
        return Ok(());
    }
    Err(classify(status, &headers, &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            if let Ok(value) = HeaderValue::from_str(value) {
                map.insert(*name, value);
            }
        }
        map
    }

    fn category(status: u16, headers: &HeaderMap, body: &str) -> String {
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
        sikemux_plugin_api::PluginError::from(classify(status, headers, body.as_bytes())).category
    }

    #[test]
    fn tells_a_spent_rate_limit_apart_from_a_token_that_may_not() {
        let spent = headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "0")]);
        let allowed = headers(&[("x-ratelimit-remaining", "412")]);
        assert_eq!(category(403, &spent, "{}"), "rate-limited");
        assert_eq!(category(403, &allowed, "{}"), "forbidden");
        assert_eq!(category(403, &HeaderMap::new(), "{}"), "forbidden");
        assert_eq!(
            category(429, &headers(&[("retry-after", "30")]), "{}"),
            "rate-limited"
        );
    }

    #[test]
    fn maps_the_statuses_a_caller_branches_on() {
        let none = HeaderMap::new();
        assert_eq!(category(401, &none, "{}"), "auth");
        assert_eq!(category(404, &none, "{}"), "not-found");
        assert_eq!(category(422, &none, "{}"), "http");
        assert_eq!(category(500, &none, "{}"), "http");
    }

    #[test]
    fn error_messages_come_from_the_body_and_carry_the_first_detail() {
        let body = r#"{"message":"Validation Failed","errors":[{"message":"no ref named x"}]}"#;
        assert_eq!(
            error_message(body.as_bytes()).as_deref(),
            Some("Validation Failed: no ref named x")
        );
        assert_eq!(
            error_message(br#"{"message":"Not Found"}"#).as_deref(),
            Some("Not Found")
        );
        assert_eq!(error_message(b"<html>"), None);
    }

    #[test]
    fn a_download_from_storage_carries_no_authorization_header() {
        let request = from_storage("https://storage.example/log?sig=abc").expect("builds");
        assert!(request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
    }

    #[test]
    fn storage_is_only_reached_over_https() {
        assert!(from_storage("http://storage.example/log?sig=abc").is_err());
        assert!(from_storage("not an address").is_err());
    }

    #[test]
    fn only_a_redirect_on_the_same_host_is_followed_with_the_token() {
        let url = |raw: &str| Url::parse(raw).expect("parses");
        let api = url("https://api.github.com/repos/a/b/actions/runs");
        assert!(same_host(
            &url("https://api.github.com/repositories/9/actions/runs"),
            &api
        ));
        assert!(same_host(&url("https://api.github.com:443/x"), &api));
        assert!(!same_host(&url("http://api.github.com/x"), &api));
        assert!(!same_host(&url("https://api.github.com:8443/x"), &api));
        assert!(!same_host(
            &url("https://pipelines.actions.githubusercontent.com/x"),
            &api
        ));
        let company = url("https://ghe.corp:8443/api/v3/repos/a/b");
        assert!(same_host(
            &url("https://ghe.corp:8443/api/v3/repositories/9"),
            &company
        ));
        assert!(!same_host(
            &url("https://ghe.corp/api/v3/repositories/9"),
            &company
        ));
    }

    #[tokio::test]
    async fn a_long_answer_keeps_its_end_when_asked() {
        let body = |text: &'static str| Response::from(http_body(text));
        let (bytes, cut) = read_body(body("0123456789"), 4, true).await.expect("reads");
        assert_eq!((bytes.as_slice(), cut), (&b"6789"[..], true));
        let (bytes, cut) = read_body(body("0123"), 4, true).await.expect("reads");
        assert_eq!((bytes.as_slice(), cut), (&b"0123"[..], false));
        assert!(read_body(body("0123456789"), 4, false).await.is_err());
    }

    fn http_body(text: &'static str) -> http::Response<&'static str> {
        http::Response::new(text)
    }

    #[test]
    fn a_body_with_no_message_still_says_something_useful() {
        let error = classify(StatusCode::BAD_GATEWAY, &HeaderMap::new(), b"");
        assert!(error.to_string().contains("Bad Gateway"), "{error}");
    }
}
