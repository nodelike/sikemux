//! Signing this host in to a Sikemux account, so devices on the same account
//! find it. Sign-in happens in the person's browser (OAuth with PKCE), which
//! hands back to a one-time listener on 127.0.0.1. That sign-in's token only
//! registers the core's key with the account and is then dropped; every later
//! request is signed with the key, so taking the host off the account cuts it
//! off. The Keychain keeps who signed in; the core keeps which account owns
//! it, and holds the live connection that tells it when the account lets this
//! host go. A host with no browser signs in by a code approved elsewhere.

mod profile;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_core::accounts::protocol::{
    AccountProfile, Challenge, Device, DeviceList, DeviceRegistration, DeviceRole, JoinTicket,
};
use sikemux_core::client::CoreClient;
use sikemux_core::protocol::{AccountLinkState, DeviceInfo, RemoteStatus};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use sikemux_host::account::{api, http};
use sikemux_loopback::{challenge, Callback, Finish, LoopbackError};
use sikemux_process::sync::Locked;

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
const SCOPES: &str = "email profile";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
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

/// Emitted with the new [`AccountStatus`] when the account changes without
/// the app asking, such as this host being removed from it elsewhere.
pub const ACCOUNT_CHANGED_EVENT: &str = "account_changed";

/// Who signed this host in from the browser, so the top bar can name them.
#[derive(Serialize, Deserialize)]
struct Saved {
    user_id: String,
    email: Option<String>,
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

    /// A host signed in by code keeps no sign-in here, so only the core
    /// knows its account, and only by id.
    fn owned_by(owner: Option<String>) -> Self {
        Self {
            signed_in: owner.is_some(),
            user_id: owner,
            ..Self::signed_out()
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
        let mut pending = self.0.locked();
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

async fn forget(app: &AppHandle) -> AppResult<()> {
    delete_saved().await?;
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
    let remote = match manager.client().await {
        Ok(core) => core.remote_status().await.ok(),
        Err(_) => None,
    };
    let owner = remote.as_ref().and_then(|remote| remote.owner.clone());
    let Some(saved) = read_saved().await? else {
        return Ok(AccountStatus::owned_by(owner));
    };
    if remote.is_some_and(|remote| released(&remote, &saved.user_id)) {
        forget(&app).await?;
        return Ok(AccountStatus::owned_by(owner));
    }
    Ok(status_of(&profile_dir(&app)?, saved).await)
}

/// Follows the core's owner when it changes without the app asking: the
/// account let this host go, or the host signed in or out by code.
pub fn notice_remote(app: &AppHandle, remote: &RemoteStatus) {
    static OWNER: Mutex<Option<Option<String>>> = Mutex::new(None);
    if remote.core_id.is_empty() {
        return;
    }
    let changed = {
        let mut last = OWNER.locked();
        let changed = last.as_ref().is_some_and(|last| *last != remote.owner);
        *last = Some(remote.owner.clone());
        changed
    };
    if !changed {
        return;
    }
    let removed = remote
        .account
        .as_ref()
        .is_some_and(|link| link.state == AccountLinkState::Removed);
    let (app, owner) = (app.clone(), remote.owner.clone());
    tauri::async_runtime::spawn(async move {
        let Ok(saved) = read_saved().await else {
            return;
        };
        match (saved, &owner) {
            (None, _) => {}
            (Some(saved), Some(owner)) if saved.user_id == *owner => return,
            (Some(_), None) if !removed => return,
            (Some(_), _) => {
                if let Err(error) = forget(&app).await {
                    eprintln!("sikemux: could not forget the account: {error}");
                    return;
                }
            }
        }
        let _ = app.emit(ACCOUNT_CHANGED_EVENT, AccountStatus::owned_by(owner));
    });
}

/// Asks the account service for the name and picture again once the cached
/// ones are a few hours old. Offline, the cached ones stay.
#[tauri::command]
pub async fn account_refresh_profile(
    app: AppHandle,
    manager: State<'_, PtyManager>,
) -> AppResult<AccountStatus> {
    static LAST_TRY: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::const_new(None);
    let dir = profile_dir(&app)?;
    let mut last_try = LAST_TRY.lock().await;
    let Some(saved) = read_saved().await? else {
        return account_status(app, manager).await;
    };
    let stale = saved_profile(&dir, &saved.user_id)
        .await
        .is_none_or(|profile| profile.is_stale(profile::now()));
    let tried_lately = last_try.is_some_and(|at| at.elapsed() < PROFILE_RETRY);
    if stale && !tried_lately {
        *last_try = Some(Instant::now());
        if let Ok(core) = manager.client().await {
            if let Ok(account) = account_profile(&core).await {
                let fresh = profile::from_account(account, saved.user_id.clone(), profile::now());
                let _ = profile::save(&dir, &fresh).await;
            }
        }
    }
    drop(last_try);
    Ok(status_of(&dir, saved).await)
}

/// The core's key and its signature, as the accounts server checks them.
#[derive(Serialize, Deserialize)]
pub(crate) struct HostSignature {
    pub key: String,
    pub signature: String,
}

/// Calls the accounts API as this host, signed with the core's key, which
/// the server takes until the host is taken off the account.
async fn host_request(
    core: &CoreClient,
    method: Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> AppResult<Response> {
    let body = match body {
        Some(body) => serde_json::to_vec(body)?,
        None => Vec::new(),
    };
    let time = profile::now();
    let signed = core
        .call(
            "signHostRequest",
            serde_json::json!({
                "method": method.as_str(),
                "path": path,
                "time": time,
                "bodySha256": hex::encode(Sha256::digest(&body)),
            }),
        )
        .await
        .map_err(core_error)?;
    let HostSignature { key, signature } = serde_json::from_value(signed)?;
    let mut request = http().request(method, api(path)).header(
        AUTHORIZATION,
        format!("SikemuxHost {key}.{time}.{signature}"),
    );
    if !body.is_empty() {
        request = request.header(CONTENT_TYPE, "application/json").body(body);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(api_failure(response).await);
    }
    Ok(response)
}

async fn as_host<T: DeserializeOwned>(
    core: &CoreClient,
    method: Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> AppResult<T> {
    Ok(host_request(core, method, path, body).await?.json().await?)
}

/// Approves the sign-in code another host showed, for "Install Sikemux here",
/// and answers with that host.
pub(crate) async fn approve_host_code(
    core: &CoreClient,
    code: &str,
) -> AppResult<sikemux_core::accounts::host::ApprovedHost> {
    as_host(
        core,
        Method::POST,
        &format!("/v1/host-sign-ins/{code}/approve-from-host"),
        None,
    )
    .await
}

/// Renames a device or host on the account, which every device and host
/// shows from then on.
#[tauri::command]
pub async fn account_rename_device(
    manager: State<'_, PtyManager>,
    key: String,
    name: String,
) -> AppResult<()> {
    let core = manager.client().await?;
    host_request(
        &core,
        Method::POST,
        &format!("/v1/devices/{key}/name"),
        Some(&serde_json::json!({ "name": name })),
    )
    .await?;
    Ok(())
}

/// Takes a device or host off the account, which revokes it on every host at
/// once.
#[tauri::command]
pub async fn account_remove_device(manager: State<'_, PtyManager>, key: String) -> AppResult<()> {
    let core = manager.client().await?;
    host_request(&core, Method::DELETE, &format!("/v1/devices/{key}"), None).await?;
    Ok(())
}

async fn account_profile(core: &CoreClient) -> AppResult<AccountProfile> {
    as_host(core, Method::GET, "/v1/account/profile", None).await
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
    let core = manager.client().await?;
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
    let registered = register_host(&core, &tokens.access_token).await;
    revoke(tokens.refresh_token).await;
    let user_id = registered?;

    let account = account_profile(&core).await.ok();
    let saved = Saved {
        user_id: user_id.clone(),
        email: account.as_ref().and_then(|account| account.email.clone()),
    };
    write_saved(&saved).await?;
    if let Err(error) = core.set_owner(Some(user_id.clone())).await {
        let _ = delete_saved().await;
        return Err(core_error(error));
    }
    let dir = profile_dir(&app)?;
    let _ = profile::forget(&dir).await;
    if let Some(account) = account {
        let _ = profile::save(
            &dir,
            &profile::from_account(account, user_id, profile::now()),
        )
        .await;
    }
    Ok(status_of(&dir, saved).await)
}

/// Clerk hands out a refresh token only for offline access, which this app
/// does not ask for; should one come anyway, it is revoked at once.
async fn revoke(refresh_token: Option<String>) {
    let Some(token) = refresh_token else {
        return;
    };
    let _ = http()
        .post(format!("{CLERK}/oauth/token/revoke"))
        .form(&[
            ("token", token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await;
}

/// The core's status, while an account owns this host.
async fn owned(core: &CoreClient) -> AppResult<Option<RemoteStatus>> {
    let remote = core.remote_status().await.map_err(core_error)?;
    Ok((remote.owner.is_some() && !remote.core_id.is_empty()).then_some(remote))
}

/// The phones on the account this host has not paired, newest first, so the
/// person can let them in right after signing in.
#[tauri::command]
pub async fn account_phones(manager: State<'_, PtyManager>) -> AppResult<Vec<Device>> {
    let core = manager.client().await?;
    let Some(remote) = owned(&core).await? else {
        return Ok(Vec::new());
    };
    let list: DeviceList = as_host(&core, Method::GET, "/v1/devices?role=client", None).await?;
    Ok(unpaired_phones(list.devices, &remote.devices))
}

/// Adds this app to the account as a device of its own, added by this host,
/// and asks the account for a ticket that lets it join `host`.
pub(crate) async fn app_join_ticket(
    core: &CoreClient,
    device: &sikemux_core::device::Device,
    host: &str,
) -> AppResult<JoinTicket> {
    let remote = owned(core)
        .await?
        .ok_or_else(|| AppError::Other("sign in to reach your other hosts".into()))?;
    let user_id = remote.owner.unwrap_or_default();
    let hosts: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    let own = hosts
        .devices
        .iter()
        .find(|known| known.key == remote.core_id)
        .map_or("this host", |known| known.name.as_str());
    let challenge: Challenge = as_host(core, Method::POST, "/v1/devices/challenge", None).await?;
    let signature = device
        .sign_registration(&challenge.nonce, &user_id)
        .map_err(|reason| AppError::Other(reason.into()))?;
    let registration = DeviceRegistration {
        key: device.id(),
        role: DeviceRole::Client,
        name: format!("Sikemux app on {own}").chars().take(64).collect(),
        platform: sikemux_core::accounts::protocol::Platform::Macos,
        channel: None,
        nonce: challenge.nonce,
        signature,
    };
    let _: Device = as_host(
        core,
        Method::POST,
        "/v1/devices",
        Some(&serde_json::to_value(&registration)?),
    )
    .await?;
    as_host(
        core,
        Method::POST,
        &format!("/v1/devices/{}/join", device.id()),
        Some(&serde_json::json!({ "host": host })),
    )
    .await
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
    if read_saved().await?.is_some() {
        forget(&app).await?;
    }
    Ok(AccountStatus::signed_out())
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
}

fn authorize_url(redirect: &str, state: &str, challenge: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect)
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{CLERK}/oauth/authorize?{query}")
}

/// The browser finishes on the web app's page, which looks like the rest of the sign-in.
async fn sign_in_with_browser() -> AppResult<Tokens> {
    let refused = |error: LoopbackError| AppError::Other(error.0);
    let signed_in = format!("{}/signed-in", sikemux_core::accounts::app_base());
    let callback = Callback::bind_any(
        "/callback",
        Finish::Redirect {
            refused: format!("{signed_in}?cancelled"),
            done: signed_in,
        },
    )
    .await
    .map_err(refused)?;
    let state = sikemux_loopback::new_state().map_err(refused)?;
    let verifier = sikemux_loopback::new_verifier().map_err(refused)?;
    open::that(authorize_url(
        callback.redirect_uri(),
        &state,
        &challenge(&verifier),
    ))?;
    let code = callback
        .code(&state)
        .await
        .map_err(|error| AppError::Other(format!("sign-in was not finished: {}", error.0)))?;
    let response = http()
        .post(format!("{CLERK}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", callback.redirect_uri()),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier.as_str()),
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

async fn api_failure(response: Response) -> AppError {
    AppError::Other(sikemux_host::account::refusal(response).await)
}

/// Registers the core's key with the account the browser signed in to, the
/// one thing the sign-in's token is used for. Answers with the account.
async fn register_host(core: &CoreClient, access_token: &str) -> AppResult<String> {
    let user_id = subject(access_token)?;
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
        .sign_registration(challenge.nonce.clone(), user_id.clone())
        .await
        .map_err(core_error)?;
    let registration = DeviceRegistration {
        key: signed.key,
        role: DeviceRole::Host,
        name: signed.name.chars().take(64).collect(),
        platform: sikemux_core::server::HOST_PLATFORM,
        channel: Some(signed.channel.into()),
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
    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_core::accounts::protocol::Platform;

    #[test]
    fn a_saved_sign_in_reaches_the_keychain_as_token_like_text_and_comes_back() {
        let saved = Saved {
            user_id: "user_1".into(),
            email: Some("someone@example.com".into()),
        };
        let text = encode_saved(&saved).expect("encode");
        assert!(text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')));
        let back = decode_saved(&text).expect("decode");
        assert_eq!(back.user_id, "user_1");
        assert_eq!(back.email.as_deref(), Some("someone@example.com"));
    }

    #[test]
    fn the_authorize_url_asks_for_a_pkce_code_back_on_this_mac() {
        let url = url::Url::parse(&authorize_url(
            "http://127.0.0.1:5555/callback",
            "s1",
            &challenge("v1"),
        ))
        .unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], CLIENT_ID);
        assert_eq!(params["redirect_uri"], "http://127.0.0.1:5555/callback");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], challenge("v1"));
        assert_eq!(params["state"], "s1");
        assert!(!params["scope"].contains("offline_access"));
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
}
