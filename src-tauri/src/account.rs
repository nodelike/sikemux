//! Signing this host in to a Sikemux account, so devices on the same account
//! find it. Sign-in happens in the person's browser (OAuth with PKCE), which
//! hands back to a one-time listener on 127.0.0.1. The refresh token stays in
//! the Keychain; the core keeps which account owns it, and holds the live
//! connection that tells it when the account lets this host go.

mod profile;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_core::accounts::protocol::{
    ApiError, Challenge, Channel, Device, DeviceList, DeviceRegistration, DeviceRole, Platform,
};
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::{AccountLinkState, BuildChannel, DeviceInfo, RemoteStatus};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

/// The Clerk instance accounts live in, and this app's OAuth client there. Dev builds use
/// Clerk's development instance, which the accounts server on this computer trusts.
#[cfg(not(debug_assertions))]
const CLERK: &str = "https://clerk.sikemux.com";
#[cfg(not(debug_assertions))]
const CLIENT_ID: &str = "I4QXaIf3c7zntZR8";
#[cfg(debug_assertions)]
const CLERK: &str = "https://immense-llama-6668.clerk.accounts.dev";
#[cfg(debug_assertions)]
const CLIENT_ID: &str = "IfRz79s1n2WGOt3J";
const SCOPES: &str = "email profile offline_access";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PROFILE_RETRY: Duration = Duration::from_secs(15 * 60);

#[cfg(not(test))]
const KEY_SERVICE: &str = "sikemux-account";
#[cfg(test)]
const KEY_SERVICE: &str = "sikemux-account-test";

/// Dev builds keep their own account entry, as they run their own core.
fn key_account() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "default"
    }
}

fn api(path: &str) -> String {
    format!(
        "{}{path}",
        sikemux_core::accounts::api_base().trim_end_matches('/')
    )
}

/// Emitted with the new [`AccountStatus`] when the account changes without
/// the app asking, such as this host being removed from it elsewhere.
pub const ACCOUNT_CHANGED_EVENT: &str = "account_changed";

fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

/// What the Keychain entry holds. Only the refresh token is secret, but
/// keeping it all in one entry means there is never half an account.
#[derive(Serialize, Deserialize)]
struct Saved {
    user_id: String,
    email: Option<String>,
    refresh_token: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    signed_in: bool,
    user_id: Option<String>,
    email: Option<String>,
    name: Option<String>,
    /// The account's picture as a `data:` URL.
    picture: Option<String>,
}

impl AccountStatus {
    fn signed_out() -> Self {
        Self {
            signed_in: false,
            user_id: None,
            email: None,
            name: None,
            picture: None,
        }
    }
}

fn profile_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("account data directory unavailable: {error}")))?
        .join("account"))
}

async fn saved_profile(dir: &std::path::Path, user_id: &str) -> Option<profile::Profile> {
    profile::read(dir)
        .await
        .filter(|profile| profile.user_id == user_id)
}

async fn status_of(dir: &std::path::Path, saved: Saved) -> AccountStatus {
    let profile = saved_profile(dir, &saved.user_id).await;
    let picture = match &profile {
        Some(profile) => profile::picture(dir, profile).await,
        None => None,
    };
    let (name, email) = match profile {
        Some(profile) => (profile.name, profile.email.or(saved.email)),
        None => (None, saved.email),
    };
    AccountStatus {
        signed_in: true,
        user_id: Some(saved.user_id),
        email,
        name,
        picture,
    }
}

/// The sign-in waiting on the browser, so a second one or Cancel can end it.
#[derive(Default)]
pub struct PendingSignIn(Mutex<Option<oneshot::Sender<()>>>);

impl PendingSignIn {
    fn replace(&self, next: Option<oneshot::Sender<()>>) {
        let mut pending = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = std::mem::replace(&mut *pending, next) {
            let _ = previous.send(());
        }
    }
}

async fn read_saved() -> AppResult<Option<Saved>> {
    let text = tokio::task::spawn_blocking(|| sikemux_keychain::read(KEY_SERVICE, key_account()))
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(text.and_then(|text| decode_saved(&text)))
}

/// The keychain takes only token-like text, so the saved sign-in goes in as base64 JSON.
fn encode_saved(saved: &Saved) -> AppResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(saved)?))
}

fn decode_saved(text: &str) -> Option<Saved> {
    let bytes = URL_SAFE_NO_PAD.decode(text.trim()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_saved(saved: &Saved) -> AppResult<()> {
    let text = encode_saved(saved)?;
    tokio::task::spawn_blocking(move || sikemux_keychain::write(KEY_SERVICE, key_account(), &text))
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .map_err(|error| AppError::Other(error.to_string()))
}

async fn delete_saved() -> AppResult<()> {
    tokio::task::spawn_blocking(|| sikemux_keychain::delete(KEY_SERVICE, key_account()))
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .map_err(|error| AppError::Other(error.to_string()))
}

/// The account let this host go while the app was not looking: the core
/// has a key but no longer this account as its owner.
fn released(remote: &RemoteStatus, user_id: &str) -> bool {
    !remote.core_id.is_empty() && remote.owner.as_deref() != Some(user_id)
}

/// Drops the saved sign-in, and asks the account service to drop its
/// refresh token too, best effort.
async fn forget(app: &AppHandle, saved: &Saved) -> AppResult<()> {
    delete_saved().await?;
    let _ = http()
        .post(format!("{CLERK}/oauth/token/revoke"))
        .form(&[
            ("token", saved.refresh_token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await;
    profile::forget(&profile_dir(app)?).await?;
    Ok(())
}

/// What this host knows about the account without asking the network. It is
/// signed in only while the core still has the account as its owner.
#[tauri::command]
pub async fn account_status(
    app: AppHandle,
    manager: State<'_, PtyManager>,
) -> AppResult<AccountStatus> {
    let dir = profile_dir(&app)?;
    let Some(saved) = read_saved().await? else {
        return Ok(AccountStatus::signed_out());
    };
    let remote = match manager.client().await {
        Ok(core) => core.remote_status().await.ok(),
        Err(_) => None,
    };
    if remote.is_some_and(|remote| released(&remote, &saved.user_id)) {
        forget(&app, &saved).await?;
        return Ok(AccountStatus::signed_out());
    }
    Ok(status_of(&dir, saved).await)
}

/// The core reported that the account let this host go, so the sign-in
/// saved here goes too, and the app hears it is signed out.
pub fn notice_remote(app: &AppHandle, remote: &RemoteStatus) {
    let removed = remote.owner.is_none()
        && remote
            .account
            .as_ref()
            .is_some_and(|link| link.state == AccountLinkState::Removed);
    if !removed {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(Some(saved)) = read_saved().await else {
            return;
        };
        match forget(&app, &saved).await {
            Ok(()) => {
                let _ = app.emit(ACCOUNT_CHANGED_EVENT, AccountStatus::signed_out());
            }
            Err(error) => eprintln!("sikemux: could not forget the account: {error}"),
        }
    });
}

/// Asks the account service for the name and picture again once the cached
/// ones are a few hours old. Offline, the cached ones stay.
#[tauri::command]
pub async fn account_refresh_profile(app: AppHandle) -> AppResult<AccountStatus> {
    static LAST_TRY: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::const_new(None);
    let dir = profile_dir(&app)?;
    let mut last_try = LAST_TRY.lock().await;
    let Some(saved) = read_saved().await? else {
        return Ok(AccountStatus::signed_out());
    };
    let stale = saved_profile(&dir, &saved.user_id)
        .await
        .is_none_or(|profile| profile.is_stale(profile::now()));
    let tried_lately = last_try.is_some_and(|at| at.elapsed() < PROFILE_RETRY);
    if stale && !tried_lately {
        *last_try = Some(Instant::now());
        if let Ok(access_token) = refreshed_access_token(&saved).await {
            if let Some(info) = user_info(&access_token).await {
                let fresh = info.into_profile(saved.user_id.clone(), profile::now());
                let _ = profile::save(&dir, &fresh).await;
            }
        }
    }
    drop(last_try);
    let saved = read_saved().await?.unwrap_or(saved);
    Ok(status_of(&dir, saved).await)
}

/// A new access token for the saved account. When the account service hands
/// back a new refresh token too, it replaces the saved one.
async fn refreshed_access_token(saved: &Saved) -> AppResult<String> {
    let response = http()
        .post(format!("{CLERK}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", saved.refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::Other(format!(
            "the account service did not renew the sign-in ({})",
            response.status()
        )));
    }
    let tokens: Tokens = response.json().await?;
    if let Some(next) = tokens
        .refresh_token
        .filter(|next| *next != saved.refresh_token)
    {
        write_saved(&Saved {
            user_id: saved.user_id.clone(),
            email: saved.email.clone(),
            refresh_token: next,
        })
        .await?;
    }
    Ok(tokens.access_token)
}

/// Opens sign-in in the browser, waits for it, then registers this host's
/// core with the account and records the account as its owner, which turns
/// remote access on.
#[tauri::command]
pub async fn account_sign_in(
    app: AppHandle,
    manager: State<'_, PtyManager>,
    pending: State<'_, PendingSignIn>,
) -> AppResult<AccountStatus> {
    let (cancel, cancelled) = oneshot::channel();
    pending.replace(Some(cancel));
    let signed_in = tokio::select! {
        result = sign_in_with_browser() => result,
        _ = cancelled => Err(AppError::Other("sign-in was cancelled".into())),
        _ = tokio::time::sleep(SIGN_IN_TIMEOUT) => {
            Err(AppError::Other("sign-in took too long; try again".into()))
        }
    };
    pending.replace(None);
    let tokens = signed_in?;

    let user_id = subject(&tokens.access_token)?;
    let info = user_info(&tokens.access_token).await.unwrap_or_default();
    let email = info.email.clone();
    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        AppError::Other("the sign-in did not give this host a way to stay signed in".into())
    })?;
    let core = manager.client().await?;
    register_host(&core, &tokens.access_token, &user_id).await?;
    core.set_owner(Some(user_id.clone()))
        .await
        .map_err(core_error)?;
    let saved = Saved {
        user_id: user_id.clone(),
        email: email.clone(),
        refresh_token,
    };
    if let Err(error) = write_saved(&saved).await {
        let _ = core.set_owner(None).await;
        return Err(error);
    }
    let dir = profile_dir(&app)?;
    let _ = profile::forget(&dir).await;
    let _ = profile::save(&dir, &info.into_profile(user_id, profile::now())).await;
    Ok(status_of(&dir, saved).await)
}

/// The phones on the account this host has not paired, newest first, so the
/// person can let them in right after signing in.
#[tauri::command]
pub async fn account_phones(manager: State<'_, PtyManager>) -> AppResult<Vec<Device>> {
    let Some(saved) = read_saved().await? else {
        return Ok(Vec::new());
    };
    let access_token = refreshed_access_token(&saved).await?;
    let response = http()
        .get(api("/v1/devices"))
        .query(&[("role", "client")])
        .bearer_auth(access_token)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(api_failure(response).await);
    }
    let list: DeviceList = response.json().await?;
    let paired = manager
        .client()
        .await?
        .remote_status()
        .await
        .map_err(core_error)?
        .devices;
    Ok(unpaired_phones(list.devices, &paired))
}

fn unpaired_phones(devices: Vec<Device>, paired: &[DeviceInfo]) -> Vec<Device> {
    devices
        .into_iter()
        .filter(|device| device.role == DeviceRole::Client)
        .filter(|device| !paired.iter().any(|known| known.id == device.key))
        .collect()
}

#[tauri::command]
pub fn account_cancel_sign_in(pending: State<'_, PendingSignIn>) {
    pending.replace(None);
}

/// Takes this host off the account and turns remote access off, then forgets
/// the account here. The core tells the account, now or once it is back
/// online. Paired devices stay: they are the host's own list, approved one by
/// one.
#[tauri::command]
pub async fn account_sign_out(
    app: AppHandle,
    manager: State<'_, PtyManager>,
) -> AppResult<AccountStatus> {
    let core = manager.client().await?;
    core.set_owner(None).await.map_err(core_error)?;
    if let Some(saved) = read_saved().await? {
        forget(&app, &saved).await?;
    }
    Ok(AccountStatus::signed_out())
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
}

struct Pkce {
    verifier: String,
    challenge: String,
    state: String,
}

fn random_text(bytes: usize) -> AppResult<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|error| AppError::Other(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

impl Pkce {
    fn new() -> AppResult<Self> {
        let verifier = random_text(32)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
            state: random_text(16)?,
        })
    }
}

fn authorize_url(pkce: &Pkce, redirect: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect)
        .append_pair("scope", SCOPES)
        .append_pair("state", &pkce.state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{CLERK}/oauth/authorize?{query}")
}

async fn sign_in_with_browser() -> AppResult<Tokens> {
    let pkce = Pkce::new()?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    open::that(authorize_url(&pkce, &redirect))?;
    let code = wait_for_callback(&listener, &pkce.state).await?;
    let response = http()
        .post(format!("{CLERK}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::Other(format!(
            "the account service did not finish the sign-in ({})",
            response.status()
        )));
    }
    Ok(response.json().await?)
}

/// What the browser lands on, read from the request line of each connection
/// until the one carrying this sign-in's `state` arrives.
async fn wait_for_callback(listener: &TcpListener, state: &str) -> AppResult<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Ok(Ok(target)) =
            tokio::time::timeout(Duration::from_secs(10), request_target(&mut stream)).await
        else {
            continue;
        };
        match callback(&target, state) {
            Callback::Code(code) => {
                answer(
                    &mut stream,
                    200,
                    "Signed in. You can close this tab and go back to Sikemux.",
                )
                .await;
                return Ok(code);
            }
            Callback::Refused(reason) => {
                answer(
                    &mut stream,
                    200,
                    "Sign-in was not finished. You can close this tab.",
                )
                .await;
                return Err(AppError::Other(reason));
            }
            Callback::Other => answer(&mut stream, 404, "Nothing here.").await,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Refused(String),
    Other,
}

fn callback(target: &str, state: &str) -> Callback {
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Other;
    };
    if url.path() != "/callback" {
        return Callback::Other;
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(state) {
        return Callback::Other;
    }
    if let Some(error) = param("error") {
        let description = param("error_description").unwrap_or(error);
        return Callback::Refused(format!("sign-in was not finished: {description}"));
    }
    match param("code") {
        Some(code) if !code.is_empty() => Callback::Code(code),
        _ => Callback::Other,
    }
}

async fn request_target(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") && buffer.len() < 16 * 1024 {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
    let head = String::from_utf8_lossy(&buffer);
    let line = head.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Ok(target.to_owned()),
        _ => Ok(String::new()),
    }
}

async fn answer(stream: &mut TcpStream, status: u16, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Sikemux</title>\
         <body style=\"font:15px system-ui;background:#0b0b0d;color:#e7e7ea;display:grid;place-items:center;height:100vh;margin:0\">\
         <p>{message}</p>"
    );
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The signed-in user's id, read from the access token Clerk just issued to
/// this app. The server checks the token's signature on every request.
fn subject(access_token: &str) -> AppResult<String> {
    #[derive(Deserialize)]
    struct Claims {
        sub: String,
    }
    let payload = access_token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Claims>(&bytes).ok())
        .ok_or_else(|| AppError::Other("the sign-in token could not be read".into()))?;
    sikemux_core::accounts::check_user_id(&payload.sub).map_err(AppError::from)?;
    Ok(payload.sub)
}

/// The account's email, name and picture. Missing them does not stop sign-in.
async fn user_info(access_token: &str) -> Option<profile::UserInfo> {
    let response = http()
        .get(format!("{CLERK}/oauth/userinfo"))
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

async fn api_failure(response: Response) -> AppError {
    let status = response.status();
    match response.json::<ApiError>().await {
        Ok(body) => AppError::Other(format!(
            "the account server refused: {}",
            body.error.message
        )),
        Err(_) => AppError::Other(format!("the account server answered {status}")),
    }
}

fn channel(build: BuildChannel) -> Channel {
    match build {
        BuildChannel::Dev => Channel::Dev,
        BuildChannel::Nightly => Channel::Nightly,
        BuildChannel::Stable => Channel::Stable,
    }
}

async fn register_host(core: &CoreClient, access_token: &str, user_id: &str) -> AppResult<Device> {
    let response = http()
        .post(api("/v1/devices/challenge"))
        .bearer_auth(access_token)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(api_failure(response).await);
    }
    let challenge: Challenge = response.json().await?;
    let signed = core
        .sign_registration(challenge.nonce.clone(), user_id.to_owned())
        .await
        .map_err(core_error)?;
    let registration = DeviceRegistration {
        key: signed.key,
        role: DeviceRole::Host,
        name: signed.name.chars().take(64).collect(),
        platform: Platform::Macos,
        channel: Some(channel(signed.channel)),
        nonce: challenge.nonce,
        signature: signed.signature,
    };
    let response = http()
        .post(api("/v1/devices"))
        .bearer_auth(access_token)
        .json(&registration)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(api_failure(response).await);
    }
    Ok(response.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_sign_in_reaches_the_keychain_as_token_like_text_and_comes_back() {
        let saved = Saved {
            user_id: "user_1".into(),
            email: Some("someone@example.com".into()),
            refresh_token: "rt.abc-def_ghi".into(),
        };
        let text = encode_saved(&saved).expect("encode");
        assert!(text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')));
        let back = decode_saved(&text).expect("decode");
        assert_eq!(back.user_id, "user_1");
        assert_eq!(back.email.as_deref(), Some("someone@example.com"));
        assert_eq!(back.refresh_token, "rt.abc-def_ghi");
    }

    #[test]
    fn the_callback_must_carry_this_sign_in_state() {
        assert_eq!(
            callback("/callback?code=abc&state=s1", "s1"),
            Callback::Code("abc".into())
        );
        assert_eq!(
            callback("/callback?code=abc&state=other", "s1"),
            Callback::Other
        );
        assert_eq!(callback("/callback?code=abc", "s1"), Callback::Other);
        assert_eq!(callback("/favicon.ico", "s1"), Callback::Other);
        assert_eq!(
            callback(
                "/callback?error=access_denied&error_description=No&state=s1",
                "s1"
            ),
            Callback::Refused("sign-in was not finished: No".into())
        );
    }

    #[test]
    fn the_authorize_url_asks_for_a_pkce_code_back_on_this_mac() {
        let pkce = Pkce::new().unwrap();
        let url = url::Url::parse(&authorize_url(&pkce, "http://127.0.0.1:5555/callback")).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], CLIENT_ID);
        assert_eq!(params["redirect_uri"], "http://127.0.0.1:5555/callback");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(
            params["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
        assert_eq!(params["state"], pkce.state);
        assert!(params["scope"].contains("offline_access"));
    }

    #[test]
    fn the_host_is_signed_in_only_while_the_core_keeps_the_account() {
        let remote = |owner: Option<&str>, core_id: &str| -> RemoteStatus {
            serde_json::from_value(serde_json::json!({
                "enabled": false, "coreId": core_id, "addresses": [], "devices": [],
                "connected": [], "pending": [], "owner": owner,
            }))
            .unwrap()
        };
        assert!(!released(&remote(Some("user_1"), "key"), "user_1"));
        assert!(released(&remote(None, "key"), "user_1"));
        assert!(released(&remote(Some("user_2"), "key"), "user_1"));
        assert!(!released(&remote(None, ""), "user_1"));
    }

    #[test]
    fn only_phones_this_host_has_not_paired_are_offered() {
        let device = |key: &str, role: DeviceRole| Device {
            key: key.into(),
            role,
            name: key.into(),
            platform: Platform::Ios,
            channel: None,
            created_at: "2026-10-08T00:00:00Z".into(),
            last_seen_at: None,
        };
        let paired = DeviceInfo {
            id: "paired".into(),
            name: "Paired".into(),
            platform: "ios".into(),
            access: sikemux_core::protocol::DeviceAccess::Full,
            paired_at: 0,
            last_seen: None,
        };
        let offered = unpaired_phones(
            vec![
                device("new", DeviceRole::Client),
                device("paired", DeviceRole::Client),
                device("host", DeviceRole::Host),
            ],
            &[paired],
        );
        let keys: Vec<_> = offered.iter().map(|device| device.key.as_str()).collect();
        assert_eq!(keys, ["new"]);
    }

    #[test]
    fn the_user_comes_from_the_token_subject() {
        let claims = URL_SAFE_NO_PAD.encode(br#"{"sub":"user_2abc","client_id":"x"}"#);
        assert_eq!(subject(&format!("h.{claims}.s")).unwrap(), "user_2abc");
        let machine = URL_SAFE_NO_PAD.encode(br#"{"sub":"mch_1"}"#);
        assert!(subject(&format!("h.{machine}.s")).is_err());
        assert!(subject("not-a-token").is_err());
    }

    #[tokio::test]
    async fn the_listener_answers_strays_and_returns_the_code() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            for path in [
                "/favicon.ico",
                "/callback?code=c1&state=wrong",
                "/callback?code=c2&state=s1",
            ] {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                stream
                    .write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut reply = String::new();
                stream.read_to_string(&mut reply).await.unwrap();
                assert!(reply.starts_with("HTTP/1.1 "));
            }
        });
        assert_eq!(wait_for_callback(&listener, "s1").await.unwrap(), "c2");
        browser.await.unwrap();
    }
}
