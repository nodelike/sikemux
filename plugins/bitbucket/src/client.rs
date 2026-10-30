// The HTTP side of the Bitbucket API: one warm client, a cap on how much a
// single answer may be, the credential each request carries, and Bitbucket's
// error shapes turned into ours.

use std::path::Path;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Deadline, Http, Redirects};
use sikemux_plugin_api::ratelimit::{LimitHeaders, RateLimits};
use sikemux_plugin_api::store::{self, Accounts, Held};

use crate::config::{self, Account, Method as AuthMethod};
use crate::error::{Bitbucket, BitbucketError, BitbucketResult};
use crate::oauth;

pub const API: &str = "https://api.bitbucket.org/2.0";
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// Logs and file contents are redirected to storage on another host, which
/// reqwest reaches without the credential.
pub static HTTP: Http<Bitbucket> = Http::new(Redirects::Follow, Deadline::WholeAnswer, 8);
/// Bitbucket counts requests over a rolling hour and only says whether few are left.
pub static LIMITS: RateLimits = RateLimits::new(LimitHeaders {
    limit: "x-ratelimit-limit",
    remaining: None,
    reset: None,
    near: Some("x-ratelimit-nearlimit"),
});
static API_TOKENS: Held<String> = Held::secrets();

#[derive(Clone)]
pub enum Credential {
    Bearer(String),
    Basic { email: String, token: String },
}

impl Credential {
    pub fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self {
            Self::Bearer(token) => request.bearer_auth(token),
            Self::Basic { email, token } => request.basic_auth(email, Some(token)),
        }
    }
}

/// Drops the credentials held in memory for one account, or for all of them,
/// so the next request reads the Keychain again.
pub async fn forget(account: Option<&str>) {
    oauth::ACCESS.forget(account).await;
    API_TOKENS.forget(account);
}

pub struct Session {
    pub account: Account,
    pub credential: Credential,
    /// What the OAuth grant allows. Empty for a pasted token, which says nothing about itself.
    pub scopes: Vec<String>,
}

impl Session {
    /// The account the call named, or the default one.
    pub async fn current(data_dir: &Path) -> BitbucketResult<Session> {
        let account = config::load(data_dir)?
            .account(chosen().as_deref())
            .cloned()
            .ok_or(BitbucketError::Unconfigured)?;
        Self::of(account).await
    }

    pub async fn of(account: Account) -> BitbucketResult<Session> {
        match account.method {
            AuthMethod::Token => {
                let token = api_token(&account).await?;
                let credential = match account.email.clone().filter(|email| !email.is_empty()) {
                    Some(email) => Credential::Basic { email, token },
                    None => Credential::Bearer(token),
                };
                Ok(Session {
                    account,
                    credential,
                    scopes: Vec::new(),
                })
            }
            AuthMethod::Oauth => {
                let access = oauth::ACCESS
                    .access(&account.id, oauth::entry(&account))
                    .await?;
                Ok(Session {
                    account,
                    credential: Credential::Bearer(access.token),
                    scopes: access.scopes,
                })
            }
        }
    }
}

async fn api_token(account: &Account) -> BitbucketResult<String> {
    if let Some(token) = API_TOKENS.get(&account.id) {
        return Ok(token);
    }
    let reading = account.keychain_account();
    let token = store::blocking(move || Ok(config::KEYCHAIN.read(&reading)?))
        .await?
        .ok_or(BitbucketError::Unconfigured)?;
    API_TOKENS.put(&account.id, token.clone());
    Ok(token)
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: Option<String>,
    detail: Option<Value>,
}

fn error_message(bytes: &[u8]) -> Option<String> {
    let detail = serde_json::from_slice::<ErrorBody>(bytes).ok()?.error?;
    let message = detail.message?;
    let extra = match detail.detail {
        Some(Value::String(text)) => Some(text),
        Some(Value::Object(fields)) => fields
            .values()
            .find_map(|value| value.as_str().map(str::to_string)),
        _ => None,
    };
    Some(match extra {
        Some(extra) if !extra.is_empty() && !message.contains(&extra) => {
            format!("{message}: {extra}")
        }
        _ => message,
    })
}

pub fn classify(status: StatusCode, bytes: &[u8]) -> BitbucketError {
    http::classify::<Bitbucket>(
        status,
        error_message(bytes),
        (status == StatusCode::TOO_MANY_REQUESTS).then(|| LIMITS.latest_wait()),
    )
}

pub enum Body<'a> {
    Json(&'a Value),
}

fn address(path: &str) -> String {
    if path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{API}{path}")
    }
}

/// A request to the API, sent again once with a fresh token when an OAuth
/// token turns out to have been revoked or expired early.
pub async fn send(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Body<'_>>,
) -> BitbucketResult<(StatusCode, Vec<u8>, bool)> {
    send_limited(
        data_dir,
        method,
        path,
        query,
        body,
        MAX_RESPONSE_BYTES,
        false,
    )
    .await
}

pub async fn send_limited(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Body<'_>>,
    limit: usize,
    keep_tail: bool,
) -> BitbucketResult<(StatusCode, Vec<u8>, bool)> {
    let mut refreshed = false;
    let mut waited = false;
    loop {
        let session = Session::current(data_dir).await?;
        let account = session.account.id.as_str();
        LIMITS.check(account)?;
        let mut request = session
            .credential
            .apply(HTTP.client()?.request(method.clone(), address(path)));
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(Body::Json(value)) = &body {
            request = request.json(value);
        }
        let response = HTTP.send(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::UNAUTHORIZED && !refreshed {
            refreshed = true;
            forget(Some(account)).await;
            if session.account.method == AuthMethod::Oauth {
                continue;
            }
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
        return Ok((status, bytes, cut));
    }
}

async fn fetch(
    data_dir: &Path,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Body<'_>>,
) -> BitbucketResult<Vec<u8>> {
    let (status, bytes, _) = send(data_dir, method, path, query, body).await?;
    if !status.is_success() {
        return Err(classify(status, &bytes));
    }
    Ok(bytes)
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> BitbucketResult<T> {
    Ok(http::parse(bytes)?)
}

pub async fn get<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
) -> BitbucketResult<T> {
    parse(&fetch(data_dir, Method::GET, path, query, None).await?)
}

/// Diffs, logs and files, which come back as plain text rather than JSON.
pub async fn get_text(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
) -> BitbucketResult<String> {
    let bytes = fetch(data_dir, Method::GET, path, query, None).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub async fn send_json<T: DeserializeOwned>(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: &Value,
) -> BitbucketResult<T> {
    parse(&fetch(data_dir, method, path, &[], Some(Body::Json(body))).await?)
}

/// A write whose answer nobody reads, such as approving or stopping something.
pub async fn post_empty(data_dir: &Path, path: &str, body: Option<&Value>) -> BitbucketResult<()> {
    fetch(data_dir, Method::POST, path, &[], body.map(Body::Json)).await?;
    Ok(())
}

/// One page of a list, as Bitbucket wraps every list.
#[derive(Deserialize)]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub values: Vec<T>,
    pub next: Option<String>,
    /// The total across every page, which some lists leave out.
    pub size: Option<u64>,
}

/// Every page of a list, up to `max_pages` of them, following the address
/// Bitbucket gives for the next one.
pub async fn get_all<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    max_pages: u32,
) -> BitbucketResult<Vec<T>> {
    http::collect_pages(None, max_pages, |later: Option<String>| async move {
        let page: Page<T> = match &later {
            Some(next) => get(data_dir, next, &[]).await?,
            None => get(data_dir, path, query).await?,
        };
        let next = page.next.filter(|next| next.starts_with(API));
        Ok((page.values, next.map(Some)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bitbuckets_error_shape() {
        let body = br#"{"type":"error","error":{"message":"Bad request","detail":"You can't merge until the build passes"}}"#;
        assert_eq!(
            error_message(body).as_deref(),
            Some("Bad request: You can't merge until the build passes")
        );
        let fields = br#"{"type":"error","error":{"message":"Bad request","fields":{},"detail":{"title":"branch not found"}}}"#;
        assert_eq!(
            error_message(fields).as_deref(),
            Some("Bad request: branch not found")
        );
        assert!(error_message(b"<html>").is_none());
    }

    #[test]
    fn an_absolute_next_page_address_is_used_as_it_is() {
        assert_eq!(address("/user"), "https://api.bitbucket.org/2.0/user");
        let next = "https://api.bitbucket.org/2.0/repositories?page=2";
        assert_eq!(address(next), next);
    }
}
