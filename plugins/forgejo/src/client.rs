// The HTTP side of the Forgejo and Gitea API: one warm client, the server and
// token each account uses, the API's paging, and its error shapes turned into ours.

use std::path::Path;

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::http::{self, Api, Calls, Http, Redirects};
use sikemux_plugin_api::store::{Accounts, Held};

use crate::config::{self, Account};
use crate::error::{Forgejo, ForgejoError, ForgejoResult};

/// The most a page holds on a server left at its default settings.
pub const PER_PAGE: u32 = 50;

pub static HTTP: Http<Forgejo> = Http::new(Redirects::SameHost, 8);
pub static API: Calls<Forgejo> = Calls::new();
static TOKENS: Held<String> = Held::secrets();

pub fn api_base(host: &str) -> String {
    format!("https://{host}/api/v1")
}

pub fn authorized(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.header("Authorization", format!("token {token}"))
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
    pub async fn current(data_dir: &Path) -> ForgejoResult<Session> {
        let account = config::load(data_dir)?
            .account(chosen().as_deref())
            .cloned()
            .ok_or(ForgejoError::Unconfigured)?;
        let token = TOKENS
            .secret(config::KEYCHAIN, &account.id, account.keychain_account())
            .await?;
        Ok(Session { account, token })
    }
}

/// The API says what went wrong as `message`, sometimes with a list of `errors` beside it.
pub fn error_message(bytes: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    let errors: Vec<&str> = body
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|text| !text.is_empty())
        .collect();
    match (message, errors.is_empty()) {
        (Some(message), true) => Some(message.to_string()),
        (Some(message), false) => Some(format!("{message}: {}", errors.join("; "))),
        (None, false) => Some(errors.join("; ")),
        (None, true) => None,
    }
}

impl Api for Forgejo {
    type Session = Session;

    fn http() -> &'static Http<Self> {
        &HTTP
    }

    async fn session(data_dir: &Path) -> ForgejoResult<Session> {
        Session::current(data_dir).await
    }

    fn account(session: &Session) -> &str {
        &session.account.id
    }

    fn request(session: &Session, client: &Client, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{path}", api_base(&session.account.host));
        authorized(client.request(method, url), &session.token)
    }

    async fn turned_down(session: &Session) {
        forget(Some(&session.account.id));
    }
}

fn total_of(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("x-total-count")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// One page of a list, with the page after it and the total when the server says them.
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<u32>,
    pub total: Option<u64>,
}

/// The page after `page`, when the total says there is one, or else when this one came full.
pub fn next_page(page: u32, per_page: u32, got: usize, total: Option<u64>) -> Option<u32> {
    let more = match total {
        Some(total) => u64::from(page) * u64::from(per_page) < total,
        None => got >= per_page as usize,
    };
    (more && got > 0).then_some(page + 1)
}

pub async fn get_page<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    page: u32,
    per_page: u32,
) -> ForgejoResult<Page<T>> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, PER_PAGE);
    let mut query = query.to_vec();
    query.push(("page", page.to_string()));
    query.push(("limit", per_page.to_string()));
    let answer = API.fetch(data_dir, Method::GET, path, &query, None).await?;
    let items: Vec<T> = answer.parsed()?;
    let total = total_of(&answer.headers);
    Ok(Page {
        next: next_page(page, per_page, items.len(), total),
        items,
        total,
    })
}

pub async fn get_all<T: DeserializeOwned>(
    data_dir: &Path,
    path: &str,
    query: &[(&str, String)],
    max_pages: u32,
) -> ForgejoResult<Vec<T>> {
    http::collect_pages(1, max_pages, |page| async move {
        let found: Page<T> = get_page(data_dir, path, query, page, PER_PAGE).await?;
        Ok((found.items, found.next))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_message_and_the_errors_beside_it() {
        assert_eq!(
            error_message(br#"{"message":"The target couldn't be found.","url":"x","errors":[]}"#)
                .as_deref(),
            Some("The target couldn't be found.")
        );
        assert_eq!(
            error_message(br#"{"message":"Validation Failed","errors":["Title cannot be empty"]}"#)
                .as_deref(),
            Some("Validation Failed: Title cannot be empty")
        );
        assert!(error_message(b"<html>").is_none());
    }

    #[test]
    fn a_page_has_a_next_while_the_total_says_so() {
        assert_eq!(next_page(1, 50, 50, Some(120)), Some(2));
        assert_eq!(next_page(3, 50, 20, Some(120)), None);
        assert_eq!(next_page(1, 50, 50, None), Some(2));
        assert_eq!(next_page(1, 50, 12, None), None);
    }
}
