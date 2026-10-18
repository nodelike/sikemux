// Signing in through the browser by installing the Sikemux integration. Vercel asks
// which team or account it goes on and which projects it may see, then sends the
// browser back to this machine with a one-time code, traded for a token that does
// not expire. Vercel's integrations have no flow for apps that cannot keep a
// secret, so the client secret is compiled in, as with Bitbucket.

use serde::Deserialize;
use serde_json::Value;
use sikemux_loopback::{redirect_uri, Callback, LoopbackError};

use crate::client;
use crate::error::{VercelError, VercelResult};

const CLIENT_ID: &str = "oac_ShIPirtpIwfcsyfvCWka3qNU";
const CLIENT_SECRET: Option<&str> = option_env!("VERCEL_OAUTH_SECRET");
/// Registered as the integration's redirect URL, so it cannot move without changing it there too.
const CALLBACK_PATH: &str = "/vercel/callback";
const INSTALL_URL: &str = "https://vercel.com/integrations/sikemux/new";
const TOKEN_URL: &str = "https://api.vercel.com/v2/oauth/access_token";

pub fn available() -> bool {
    CLIENT_SECRET.is_some_and(|secret| !secret.is_empty())
}

fn secret() -> VercelResult<&'static str> {
    CLIENT_SECRET
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            VercelError::Auth(
                "this build cannot sign in through the browser; use an access token instead".into(),
            )
        })
}

fn refused(error: LoopbackError) -> VercelError {
    VercelError::Auth(error.0)
}

pub fn new_state() -> VercelResult<String> {
    sikemux_loopback::new_state().map_err(refused)
}

pub async fn listen() -> VercelResult<Callback> {
    secret()?;
    Callback::bind(CALLBACK_PATH, "Vercel")
        .await
        .map_err(refused)
}

pub async fn code(callback: &Callback, state: &str) -> VercelResult<String> {
    callback.code(state).await.map_err(refused)
}

pub fn install_url(state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", state)
        .finish();
    format!("{INSTALL_URL}?{query}")
}

#[derive(Deserialize)]
pub struct Installed {
    pub access_token: String,
    pub installation_id: String,
    /// The team it was installed on; none for the account's own projects.
    pub team_id: Option<String>,
}

pub async fn exchange(code: &str) -> VercelResult<Installed> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", CLIENT_ID),
            ("client_secret", secret()?),
            ("code", code),
            ("redirect_uri", &redirect_uri(CALLBACK_PATH)),
        ])
        .finish();
    let response = client::http()?
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    let status = response.status().as_u16();
    let bytes = client::read_body(response).await?;
    if (200..300).contains(&status) {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let refused: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let reason = ["/error_description", "/error/message", "/error"]
        .iter()
        .find_map(|path| refused.pointer(path).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| format!("http {status}"));
    Err(VercelError::Auth(reason))
}

/// Signing out removes the integration from the team too; it is gone here either way.
pub async fn uninstall(token: &str, team: Option<&str>, configuration: &str) {
    let query: Vec<(&str, String)> = team.map(client::team_param).into_iter().collect();
    if let Err(error) = client::request(
        token,
        reqwest::Method::DELETE,
        &format!("/v1/integrations/configuration/{configuration}"),
        &query,
        None,
    )
    .await
    {
        eprintln!("vercel: removing the integration: {error}");
    }
}
