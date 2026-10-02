// The HTTP side of the Bitbucket API: one warm client, a cap on how much a
// single answer may be, the credential each request carries, and Bitbucket's
// error shapes turned into ours.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::{self, Account, Method as AuthMethod};
use crate::error::{BitbucketError, BitbucketResult};
use crate::oauth;
use crate::ratelimit;

pub const API: &str = "https://api.bitbucket.org/2.0";
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS_IN_FLIGHT: usize = 8;
const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// A token read from the Keychain is held briefly, since starting `security`
/// for every request costs more than the request.
const TOKEN_TTL: Duration = Duration::from_secs(60);
/// An access token this close to expiring is replaced before it is used.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

pub async fn limited<T>(work: impl Future<Output = T>) -> T {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    let _permit = PERMITS
        .get_or_init(|| Semaphore::new(MAX_REQUESTS_IN_FLIGHT))
        .acquire()
        .await
        .ok();
    work.await
}

/// Logs and file contents are redirected to storage on another host, which
/// reqwest reaches without the credential.
pub fn http() -> BitbucketResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
                .user_agent("sikemux-bitbucket/0.1")
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| BitbucketError::Transport("could not start the HTTP client".into()))
}

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

struct Access {
    token: String,
    scopes: Vec<String>,
    expires: Instant,
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

static ACCESS: tokio::sync::Mutex<BTreeMap<String, Access>> =
    tokio::sync::Mutex::const_new(BTreeMap::new());
static API_TOKEN: Mutex<BTreeMap<String, HeldToken>> = Mutex::new(BTreeMap::new());

/// Drops the credentials held in memory for one account, or for all of them,
/// so the next request reads the Keychain again.
pub async fn forget(account: Option<&str>) {
    let mut access = ACCESS.lock().await;
    match account {
        Some(id) => access.remove(id),
        None => {
            access.clear();
            None
        }
    };
    if let Ok(mut held) = API_TOKEN.lock() {
        match account {
            Some(id) => held.remove(id),
            None => {
                held.clear();
                None
            }
        };
    }
}

pub async fn remember_access(account: &str, tokens: &oauth::Tokens) {
    ACCESS
        .lock()
        .await
        .insert(account.to_string(), access_of(tokens));
}

fn access_of(tokens: &oauth::Tokens) -> Access {
    Access {
        token: tokens.access_token.clone(),
        scopes: tokens
            .scopes
            .split([' ', ','])
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect(),
        expires: Instant::now() + Duration::from_secs(tokens.expires_in),
    }
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
        let config = config::load(data_dir);
        let account = config
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
                let (token, scopes) = access_token(&account).await?;
                Ok(Session {
                    account,
                    credential: Credential::Bearer(token),
                    scopes,
                })
            }
        }
    }
}

async fn api_token(account: &Account) -> BitbucketResult<String> {
    if let Ok(held) = API_TOKEN.lock() {
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
        .ok_or(BitbucketError::Unconfigured)?;
    if let Ok(mut held) = API_TOKEN.lock() {
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

/// One refresh at a time: the lock is held across it, so requests that find
/// the token stale together wait for the one refresh rather than each spending
/// the refresh token.
async fn access_token(account: &Account) -> BitbucketResult<(String, Vec<String>)> {
    let mut held = ACCESS.lock().await;
    if let Some(access) = held
        .get(&account.id)
        .filter(|access| access.expires > Instant::now() + EXPIRY_MARGIN)
    {
        return Ok((access.token.clone(), access.scopes.clone()));
    }
    let reading = account.clone();
    let refresh_token = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(BitbucketError::Unconfigured)?;
    let tokens = oauth::refresh(&refresh_token).await?;
    if tokens.refresh_token != refresh_token {
        let writing = account.clone();
        let replacement = tokens.refresh_token.clone();
        config::blocking(Box::new(move || {
            config::keychain_write(&writing, &replacement)
        }))
        .await?;
    }
    let access = access_of(&tokens);
    let answer = (access.token.clone(), access.scopes.clone());
    held.insert(account.id.clone(), access);
    Ok(answer)
}

/// Reads an answer of up to `limit` bytes. Past that the read fails, unless
/// `keep_tail` is set: then the start is dropped, and the second value says so.
pub async fn read_body(
    response: Response,
    limit: usize,
    keep_tail: bool,
) -> BitbucketResult<(Vec<u8>, bool)> {
    let too_big = || BitbucketError::Response(format!("more than {} MiB came back", limit >> 20));
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
    let message = error_message(bytes).unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("the request failed")
            .to_string()
    });
    match status.as_u16() {
        401 => BitbucketError::Auth(message),
        403 => BitbucketError::Forbidden(message),
        404 => BitbucketError::NotFound(message),
        429 => BitbucketError::RateLimited {
            resets_in_secs: ratelimit::latest_wait(),
        },
        status => BitbucketError::Http { status, message },
    }
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
        ratelimit::check(&session.account.id)?;
        let mut request = session
            .credential
            .apply(http()?.request(method.clone(), address(path)));
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(Body::Json(value)) = &body {
            request = request.json(value);
        }
        let response = limited(request.send()).await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::UNAUTHORIZED && !refreshed {
            refreshed = true;
            forget(Some(&session.account.id)).await;
            if session.account.method == AuthMethod::Oauth {
                continue;
            }
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
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::from_slice(b"null")?);
    }
    Ok(serde_json::from_slice(bytes)?)
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
    let mut all = Vec::new();
    let mut page: Page<T> = get(data_dir, path, query).await?;
    for _ in 1..max_pages {
        all.append(&mut page.values);
        let Some(next) = page.next.take().filter(|next| next.starts_with(API)) else {
            return Ok(all);
        };
        page = get(data_dir, &next, &[]).await?;
    }
    all.append(&mut page.values);
    Ok(all)
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
    fn a_refused_token_is_told_apart_from_a_missing_permission() {
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED, b""),
            BitbucketError::Auth(_)
        ));
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, b""),
            BitbucketError::Forbidden(_)
        ));
        assert!(matches!(
            classify(StatusCode::TOO_MANY_REQUESTS, b""),
            BitbucketError::RateLimited { .. }
        ));
    }

    #[test]
    fn an_absolute_next_page_address_is_used_as_it_is() {
        assert_eq!(address("/user"), "https://api.bitbucket.org/2.0/user");
        let next = "https://api.bitbucket.org/2.0/repositories?page=2";
        assert_eq!(address(next), next);
    }
}
