// Signing in through the browser with an Atlassian account. Atlassian sends the
// browser back to this machine with a one-time code, traded for an access token
// and a refresh token. Atlassian has no flow for apps that cannot keep a secret,
// so the client secret is compiled in, as Bitbucket's is.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_loopback::{redirect_uri, Callback, LoopbackError};

use crate::client;
use crate::config::{self, Site};
use crate::error::{JiraError, JiraResult};

const CLIENT_ID: &str = match option_env!("JIRA_OAUTH_KEY") {
    Some(key) => key,
    None => "aJY3c50sMiGWv0fJpveLHOJ5Os2Z7rkt",
};
const CLIENT_SECRET: Option<&str> = option_env!("JIRA_OAUTH_SECRET");
/// Registered as the app's callback, so it cannot move without changing it there too.
const CALLBACK_PATH: &str = "/jira/callback";
const AUTHORIZE_URL: &str = "https://auth.atlassian.com/authorize";
const TOKEN_URL: &str = "https://auth.atlassian.com/oauth/token";
const SITES_URL: &str = "https://api.atlassian.com/oauth/token/accessible-resources";
/// A site signed in through the browser is reached here, by its id, rather than at its own host.
pub const API_URL: &str = "https://api.atlassian.com/ex/jira";
/// Boards are Jira Software's API, which a browser sign-in reaches only through its own scopes.
const SCOPES: &str = "read:jira-work write:jira-work read:jira-user \
    read:board-scope:jira-software read:board-scope.admin:jira-software \
    read:sprint:jira-software read:issue:jira-software read:project:jira \
    read:issue-details:jira read:jql:jira read:filter:jira offline_access";
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

pub fn available() -> bool {
    CLIENT_SECRET.is_some_and(|secret| !secret.is_empty())
}

fn secret() -> JiraResult<&'static str> {
    CLIENT_SECRET
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            JiraError::Auth(
                "this build cannot sign in through the browser; use an API token instead".into(),
            )
        })
}

fn refused(error: LoopbackError) -> JiraError {
    JiraError::Auth(error.0)
}

pub fn new_state() -> JiraResult<String> {
    sikemux_loopback::new_state().map_err(refused)
}

pub async fn listen() -> JiraResult<Callback> {
    Callback::bind(CALLBACK_PATH, "Jira").await.map_err(refused)
}

pub async fn code(callback: &Callback, state: &str) -> JiraResult<String> {
    callback.code(state).await.map_err(refused)
}

pub fn authorize_url(state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("audience", "api.atlassian.com")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("scope", SCOPES)
        .append_pair("redirect_uri", &redirect_uri(CALLBACK_PATH))
        .append_pair("response_type", "code")
        .append_pair("prompt", "consent")
        .append_pair("state", state)
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_lifetime")]
    pub expires_in: u64,
}

fn default_lifetime() -> u64 {
    3600
}

async fn token_request(mut body: Value) -> JiraResult<Tokens> {
    if let Some(fields) = body.as_object_mut() {
        fields.insert("client_id".into(), CLIENT_ID.into());
        fields.insert("client_secret".into(), secret()?.into());
    }
    let response = client::http()?.post(TOKEN_URL).json(&body).send().await?;
    let status = response.status();
    let bytes = client::read_limited(response).await?;
    if status.is_success() {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let refused: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let reason = ["error_description", "error"]
        .iter()
        .find_map(|key| refused.get(key).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| format!("http {status}"));
    Err(JiraError::Auth(reason))
}

pub async fn exchange(code: &str) -> JiraResult<Tokens> {
    token_request(json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri(CALLBACK_PATH),
    }))
    .await
}

/// Atlassian hands back a new refresh token every time, and the old one stops working.
async fn refresh(refresh_token: &str) -> JiraResult<Tokens> {
    token_request(json!({ "grant_type": "refresh_token", "refresh_token": refresh_token })).await
}

/// A Jira site the signed-in account may use.
#[derive(Deserialize)]
pub struct CloudSite {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// The Jira sites one browser sign-in reaches. Other Atlassian products, such as
/// Confluence, come back too, without Jira's scopes.
pub async fn sites(access_token: &str) -> JiraResult<Vec<CloudSite>> {
    let response = client::http()?
        .get(SITES_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = response.status();
    let bytes = client::read_limited(response).await?;
    if !status.is_success() {
        return Err(JiraError::Auth(format!(
            "Atlassian would not list the sites (http {status})"
        )));
    }
    Ok(jira_sites(serde_json::from_slice(&bytes)?))
}

fn jira_sites(all: Vec<CloudSite>) -> Vec<CloudSite> {
    all.into_iter()
        .filter(|site| site.scopes.iter().any(|scope| scope == "read:jira-work"))
        .collect()
}

struct Access {
    token: String,
    expires: Instant,
}

static ACCESS: tokio::sync::Mutex<BTreeMap<String, Access>> =
    tokio::sync::Mutex::const_new(BTreeMap::new());

pub async fn remember(account_id: &str, tokens: &Tokens) {
    ACCESS
        .lock()
        .await
        .insert(account_id.to_string(), access_of(tokens));
}

pub async fn forget(account_id: &str) {
    ACCESS.lock().await.remove(account_id);
}

fn access_of(tokens: &Tokens) -> Access {
    Access {
        token: tokens.access_token.clone(),
        expires: Instant::now() + Duration::from_secs(tokens.expires_in),
    }
}

/// One refresh at a time: the lock is held across it, because spending a refresh
/// token twice would sign the account out.
pub async fn access_token(site: &Site) -> JiraResult<String> {
    let mut held = ACCESS.lock().await;
    if let Some(access) = held
        .get(&site.account_id)
        .filter(|access| access.expires > Instant::now() + EXPIRY_MARGIN)
    {
        return Ok(access.token.clone());
    }
    let reading = site.clone();
    let refresh_token = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(JiraError::Unconfigured)?;
    let tokens = refresh(&refresh_token).await?;
    let writing = site.clone();
    let replacement = tokens.refresh_token.clone();
    config::blocking(Box::new(move || {
        config::keychain_write(&writing, &replacement)
    }))
    .await?;
    let access = access_of(&tokens);
    let token = access.token.clone();
    held.insert(site.account_id.clone(), access);
    Ok(token)
}

/// Jira refused `rejected`, so it is dropped and a fresh one fetched. A request that
/// already renewed it in the meantime is answered with that one rather than refreshing again.
pub async fn renew(site: &Site, rejected: &str) -> JiraResult<String> {
    {
        let mut held = ACCESS.lock().await;
        if held
            .get(&site.account_id)
            .is_some_and(|access| access.token == rejected)
        {
            held.remove(&site.account_id);
        }
    }
    access_token(site).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_asks_for_jira_and_a_refresh_token_and_comes_back_to_jiras_path() -> JiraResult<()>
    {
        let state = new_state()?;
        let url = authorize_url(&state);
        assert!(url.starts_with("https://auth.atlassian.com/authorize?audience=api.atlassian.com&"));
        assert!(url.contains("scope=read%3Ajira-work+write%3Ajira-work+read%3Ajira-user+"));
        assert!(url.contains("+read%3Aboard-scope%3Ajira-software+"));
        assert!(url.contains("+offline_access&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A47123%2Fjira%2Fcallback"));
        assert!(url.ends_with(&format!("state={state}")));
        Ok(())
    }

    #[test]
    fn only_sites_that_grant_jira_are_kept() -> JiraResult<()> {
        let all: Vec<CloudSite> = serde_json::from_value(json!([
            { "id": "1", "url": "https://acme.atlassian.net", "name": "acme", "scopes": ["read:jira-work", "write:jira-work"] },
            { "id": "2", "url": "https://acme.atlassian.net", "name": "acme", "scopes": ["read:confluence-content.all"] }
        ]))?;
        let kept: Vec<_> = jira_sites(all).into_iter().map(|site| site.id).collect();
        assert_eq!(kept, ["1"]);
        Ok(())
    }
}
