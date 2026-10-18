// Signing in through the browser with a Linear account. Linear sends the browser
// back to this machine with a one-time code, traded for an access token and a
// refresh token. The app proves it asked for the code with PKCE, so no client
// secret is compiled in.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_loopback::{redirect_uri, Callback, LoopbackError};

use crate::client;
use crate::config::{self, Workspace};
use crate::error::{LinearError, LinearResult};

const CLIENT_ID: &str = "a597de542170c3976bde9c670f879ee7";
/// Registered as the app's callback, so it cannot move without changing it there too.
const CALLBACK_PATH: &str = "/linear/callback";
const AUTHORIZE_URL: &str = "https://linear.app/oauth/authorize";
const TOKEN_URL: &str = "https://api.linear.app/oauth/token";
const REVOKE_URL: &str = "https://api.linear.app/oauth/revoke";
const SCOPES: &str = "read,write";
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

fn refused(error: LoopbackError) -> LinearError {
    LinearError::Auth(error.0)
}

pub fn new_state() -> LinearResult<String> {
    sikemux_loopback::new_state().map_err(refused)
}

/// The secret the browser never sees; Linear hands out tokens only to whoever holds it.
pub fn new_verifier() -> LinearResult<String> {
    Ok(format!("{}{}", new_state()?, new_state()?))
}

fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub async fn listen() -> LinearResult<Callback> {
    Callback::bind(CALLBACK_PATH, "Linear")
        .await
        .map_err(refused)
}

pub async fn code(callback: &Callback, state: &str) -> LinearResult<String> {
    callback.code(state).await.map_err(refused)
}

pub fn authorize_url(state: &str, verifier: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri(CALLBACK_PATH))
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge(verifier))
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

pub fn bearer(access_token: &str) -> String {
    format!("Bearer {access_token}")
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_lifetime")]
    pub expires_in: u64,
}

fn default_lifetime() -> u64 {
    24 * 3600
}

async fn post_form(url: &str, fields: &[(&str, &str)]) -> LinearResult<(u16, Vec<u8>)> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let response = client::http()?
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    let status = response.status().as_u16();
    Ok((status, client::read_body(response).await?))
}

async fn token_request(fields: &[(&str, &str)]) -> LinearResult<Tokens> {
    let mut fields = fields.to_vec();
    fields.push(("client_id", CLIENT_ID));
    let (status, bytes) = post_form(TOKEN_URL, &fields).await?;
    if (200..300).contains(&status) {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let refused: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let reason = ["error_description", "error"]
        .iter()
        .find_map(|key| refused.get(key).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| format!("http {status}"));
    Err(LinearError::Auth(reason))
}

pub async fn exchange(code: &str, verifier: &str) -> LinearResult<Tokens> {
    token_request(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &redirect_uri(CALLBACK_PATH)),
        ("code_verifier", verifier),
    ])
    .await
}

/// Linear hands back a new refresh token every time, and the old one stops working.
async fn refresh(refresh_token: &str) -> LinearResult<Tokens> {
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ])
    .await
}

/// Signing out tells Linear to forget the sign-in too; it is gone here either way.
pub async fn revoke(refresh_token: &str) {
    if let Err(error) = post_form(
        REVOKE_URL,
        &[
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
        ],
    )
    .await
    {
        eprintln!("linear: revoking the sign-in: {error}");
    }
}

struct Access {
    token: String,
    expires: Instant,
}

static ACCESS: tokio::sync::Mutex<BTreeMap<String, Access>> =
    tokio::sync::Mutex::const_new(BTreeMap::new());

pub async fn remember(workspace_id: &str, tokens: &Tokens) {
    ACCESS
        .lock()
        .await
        .insert(workspace_id.to_string(), access_of(tokens));
}

pub async fn forget(workspace_id: &str) {
    ACCESS.lock().await.remove(workspace_id);
}

fn access_of(tokens: &Tokens) -> Access {
    Access {
        token: tokens.access_token.clone(),
        expires: Instant::now() + Duration::from_secs(tokens.expires_in),
    }
}

/// One refresh at a time: the lock is held across it, because spending a refresh
/// token twice would sign the workspace out.
pub async fn access_token(workspace: &Workspace) -> LinearResult<String> {
    let mut held = ACCESS.lock().await;
    if let Some(access) = held
        .get(&workspace.id)
        .filter(|access| access.expires > Instant::now() + EXPIRY_MARGIN)
    {
        return Ok(access.token.clone());
    }
    let reading = workspace.clone();
    let refresh_token = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(LinearError::Unconfigured)?;
    let tokens = refresh(&refresh_token).await?;
    let writing = workspace.clone();
    let replacement = tokens.refresh_token.clone();
    config::blocking(Box::new(move || {
        config::keychain_write(&writing, &replacement)
    }))
    .await?;
    let access = access_of(&tokens);
    let token = access.token.clone();
    held.insert(workspace.id.clone(), access);
    Ok(token)
}

/// Linear refused `rejected`, so it is dropped and a fresh one fetched. A request that
/// already renewed it in the meantime is answered with that one rather than refreshing again.
pub async fn renew(workspace: &Workspace, rejected: &str) -> LinearResult<String> {
    {
        let mut held = ACCESS.lock().await;
        if held
            .get(&workspace.id)
            .is_some_and(|access| access.token == rejected)
        {
            held.remove(&workspace.id);
        }
    }
    access_token(workspace).await
}
