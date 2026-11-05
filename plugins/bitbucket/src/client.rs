// The HTTP side of the Bitbucket API: one warm client, a cap on how much a
// single answer may be, the credential each request carries, and Bitbucket's
// error shapes turned into ours.

use std::path::Path;

use reqwest::{Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Api, Calls, Http, Redirects};
use sikemux_plugin_api::ratelimit::LimitHeaders;
use sikemux_plugin_api::store::{self, Accounts, Held};

use crate::config::{self, Account, Method as AuthMethod};
use crate::error::{Bitbucket, BitbucketError, BitbucketResult};
use crate::oauth;

pub const BASE: &str = "https://api.bitbucket.org/2.0";
/// Logs and file contents are redirected to storage on another host, which
/// reqwest reaches without the credential. Bitbucket counts requests over a
/// rolling hour and says nothing of how many are left.
pub static HTTP: Http<Bitbucket> = Http::with_limits(
    Redirects::Follow,
    8,
    LimitHeaders {
        remaining: None,
        reset: None,
    },
);
pub static API: Calls<Bitbucket> = Calls::new();
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

pub fn error_message(bytes: &[u8]) -> Option<String> {
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

fn address(path: &str) -> String {
    if path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{BASE}{path}")
    }
}

impl Api for Bitbucket {
    type Session = Session;

    fn http() -> &'static Http<Self> {
        &HTTP
    }

    async fn session(data_dir: &Path) -> BitbucketResult<Session> {
        Session::current(data_dir).await
    }

    fn account(session: &Session) -> &str {
        &session.account.id
    }

    fn request(session: &Session, client: &Client, method: Method, path: &str) -> RequestBuilder {
        session
            .credential
            .apply(client.request(method, address(path)))
    }

    async fn turned_down(session: &Session) {
        forget(Some(&session.account.id)).await;
    }

    /// An OAuth token may have been revoked or expired early, and can be fetched again.
    fn renews(session: &Session) -> bool {
        session.account.method == AuthMethod::Oauth
    }
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
            Some(next) => API.get(data_dir, next, &[]).await?,
            None => API.get(data_dir, path, query).await?,
        };
        let next = page.next.filter(|next| next.starts_with(BASE));
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
}
