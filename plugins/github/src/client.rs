// The HTTP side of the GitHub API: one warm client, a cap on how much a
// single answer may be, and GitHub's error shapes turned into ours. Nothing
// here signs in again on its own; a refused token is reported as such.

use std::path::Path;
use std::sync::OnceLock;

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Deadline, Http, Redirects};
use sikemux_plugin_api::ratelimit::{LimitHeaders, RateLimits};
use sikemux_plugin_api::store;

use crate::config::{self, Account};
use crate::error::{Github, GithubError, GithubResult};

pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS_IN_FLIGHT: usize = 8;
const API_VERSION: &str = "2022-11-28";
const JSON: &str = "application/vnd.github+json";

/// GitHub answers for a renamed or moved repository with a redirect on its
/// own host, which keeps the token. A redirect anywhere else is storage, which
/// is left for the caller to ask without it.
pub static HTTP: Http<Github> = Http::new(
    Redirects::SameHost,
    Deadline::WholeAnswer,
    MAX_REQUESTS_IN_FLIGHT,
);
pub static LIMITS: RateLimits = RateLimits::new(LimitHeaders {
    limit: "x-ratelimit-limit",
    remaining: Some("x-ratelimit-remaining"),
    reset: Some("x-ratelimit-reset"),
    near: None,
});

/// Logs, artifacts and release files, which share `HTTP`'s request slots.
fn transfers() -> GithubResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            http::builder::<Github>(Redirects::SameHost, Deadline::QuietTransfer)
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| GithubError::Transport("could not start the HTTP client".into()))
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
        store::blocking(move || {
            let config = config::load(&data_dir)?;
            let (account, token) =
                config::resolve(&config, chosen.as_deref())?.ok_or(GithubError::Unconfigured)?;
            Ok(Session { account, token })
        })
        .await
    }
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

pub fn classify(
    account: &str,
    status: StatusCode,
    headers: &HeaderMap,
    bytes: &[u8],
) -> GithubError {
    let spent = matches!(status.as_u16(), 403 | 429)
        .then(|| LIMITS.wait_for(account, status, headers, bytes))
        .flatten();
    http::classify::<Github>(status, error_message(bytes), spent)
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
    let mut waited = false;
    loop {
        LIMITS.check(account)?;
        let again = request.try_clone();
        let response = HTTP.send(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = http::read_body(response, MAX_RESPONSE_BYTES).await?;
        if status == StatusCode::UNAUTHORIZED {
            config::forget_token(Some(account));
        }
        let pause = LIMITS
            .short_wait(account, status, &headers, &bytes)
            .filter(|_| !waited);
        match (pause, again) {
            (Some(pause), Some(retry)) => {
                waited = true;
                tokio::time::sleep(pause).await;
                request = retry;
            }
            _ => {
                LIMITS.observe(account, status, &headers, &bytes);
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
    let mut request = api_request(HTTP.client()?, session, method, path, JSON);
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
        return Err(classify(&session.account.id, status, &headers, &bytes));
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
    Ok(http::parse(bytes)?)
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
    let items = &items;
    http::collect_pages(1, max_pages, |page: u32| {
        let mut paged = query.to_vec();
        paged.push(("per_page", PAGE.to_string()));
        paged.push(("page", page.to_string()));
        async move {
            let batch = items(get::<P>(data_dir, path, &paged).await?);
            let next = (batch.len() >= PAGE).then_some(page + 1);
            Ok((batch, next))
        }
    })
    .await
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
    let mut request = api_request(HTTP.client()?, &session, Method::GET, path, JSON).query(query);
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
        return Err(classify(&session.account.id, status, &headers, &bytes));
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
    LIMITS.check(account)?;
    let mut response = HTTP
        .limited(api_request(client, &session, Method::GET, path, accept).send())
        .await?;
    LIMITS.observe(account, response.status(), response.headers(), b"");
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| GithubError::Response("the download redirect had no address".into()))?
            .to_string();
        response = HTTP
            .limited(client.execute(from_storage(&location)?))
            .await?;
    }
    if !response.status().is_success() {
        return Err(failure(account, response).await);
    }
    Ok(response)
}

pub async fn failure(account: &str, response: Response) -> GithubError {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = http::read_body(response, MAX_RESPONSE_BYTES)
        .await
        .unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        config::forget_token(Some(account));
    }
    LIMITS.observe(account, status, &headers, &bytes);
    classify(account, status, &headers, &bytes)
}

/// A file GitHub hands over whole, such as a log or a workflow's YAML.
pub async fn download_as(
    data_dir: &Path,
    path: &str,
    accept: &str,
    keep_tail: bool,
) -> GithubResult<(Vec<u8>, bool)> {
    let response = open_download(data_dir, path, accept).await?;
    Ok(if keep_tail {
        http::read_tail(response, MAX_RESPONSE_BYTES).await?
    } else {
        (http::read_body(response, MAX_RESPONSE_BYTES).await?, false)
    })
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
    Err(classify(&session.account.id, status, &headers, &bytes))
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
        let category = sikemux_plugin_api::PluginError::from(classify(
            "test",
            status,
            headers,
            body.as_bytes(),
        ))
        .category;
        serde_json::to_value(category)
            .ok()
            .and_then(|name| name.as_str().map(str::to_owned))
            .unwrap_or_default()
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
}
