// The HTTP side of the GitLab API: one warm client, a cap on how much a single
// answer may be, the server and token each account uses, GitLab's paging, and
// its error shapes turned into ours.

use std::path::Path;

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Api, Calls, Http, Redirects};
use sikemux_plugin_api::ratelimit::LimitHeaders;
use sikemux_plugin_api::store::{self, Accounts, Held};

use crate::config::{self, Account};
use crate::error::{Gitlab, GitlabError, GitlabResult};

const PER_PAGE: u32 = 100;

pub static HTTP: Http<Gitlab> = Http::with_limits(
    Redirects::SameHost,
    8,
    LimitHeaders {
        remaining: Some("ratelimit-remaining"),
        reset: Some("ratelimit-reset"),
    },
);
pub static API: Calls<Gitlab> = Calls::new();
static TOKENS: Held<String> = Held::secrets();

/// Where a server's API lives.
pub fn api_base(host: &str) -> String {
    format!("https://{host}/api/v4")
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

impl Api for Gitlab {
    type Session = Session;

    fn http() -> &'static Http<Self> {
        &HTTP
    }

    async fn session(data_dir: &Path) -> GitlabResult<Session> {
        Session::current(data_dir).await
    }

    fn account(session: &Session) -> &str {
        &session.account.id
    }

    fn request(session: &Session, client: &Client, method: Method, path: &str) -> RequestBuilder {
        client
            .request(method, format!("{}{path}", session.base()))
            .bearer_auth(&session.token)
    }

    async fn turned_down(session: &Session) {
        forget(Some(&session.account.id));
    }
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
    let answer = API.fetch(data_dir, Method::GET, path, &query, None).await?;
    Ok(Page {
        items: answer.parsed()?,
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
}
