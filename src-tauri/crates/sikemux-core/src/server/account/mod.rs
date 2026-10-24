//! Signing this host in to a Sikemux account, so devices on the same account
//! find it. Sign-in happens in the person's browser (OAuth with PKCE), which
//! hands back to a one-time listener on 127.0.0.1. That sign-in's token only
//! registers the core's key with the account and is then dropped; every later
//! request is signed with the key, so taking the host off the account cuts it
//! off. The keychain keeps who signed in, the data folder their name and
//! picture, and the core's remote access which account owns the host. A host
//! with no browser signs in by a code approved elsewhere, and keeps no record.

mod profile;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_loopback::{challenge, Callback, Finish, LoopbackError};
use sikemux_process::sync::Locked;
use tokio::sync::{oneshot, watch};

use crate::accounts::protocol::{AccountProfile, Challenge, DeviceRegistration, DeviceRole};
use crate::protocol::AccountInfo;

use super::access::Peer;
use super::changed::Topic;
use super::hosts::account::{as_host, client};
use super::{remote, Core, CoreError, CoreResult};

/// The Clerk instance accounts live in, and this host's OAuth client there.
/// Dev builds use Clerk's development instance, which the accounts server on
/// this computer trusts.
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
/// How often the name and picture are looked at, and how soon a failed
/// fetch of them is tried again.
const PROFILE_RETRY: Duration = Duration::from_secs(15 * 60);
const FOLDER: &str = "account";

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

/// Who signed this host in from the browser.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    user_id: String,
    email: Option<String>,
}

/// A browser sign-in under way.
struct Flight {
    cancel: oneshot::Sender<()>,
    done: watch::Receiver<Option<Result<AccountInfo, String>>>,
}

#[derive(Default)]
pub(crate) struct Account {
    /// Who signed in, as the keychain keeps it, once read; the core is the
    /// only one that writes it.
    saved: Mutex<Option<Option<Saved>>>,
    /// What clients are sent.
    shown: Mutex<Option<AccountInfo>>,
    flight: Mutex<Option<Flight>>,
    profile_tried: Mutex<Option<Instant>>,
}

impl Account {
    pub(crate) fn shown(&self) -> Option<AccountInfo> {
        self.shown.locked().clone()
    }
}

/// The keychain takes only token-like text, so the saved sign-in goes in as base64 JSON.
fn encode_saved(saved: &Saved) -> CoreResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(saved)?))
}

fn decode_saved(text: &str) -> Option<Saved> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(text.trim()).ok()?).ok()
}

async fn saved(core: &Core) -> Option<Saved> {
    if let Some(saved) = core.account.saved.locked().clone() {
        return saved;
    }
    let read = tokio::task::spawn_blocking(|| sikemux_keychain::read(KEY_SERVICE, key_account()))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .and_then(|text| decode_saved(&text));
    *core.account.saved.locked() = Some(read.clone());
    read
}

async fn keep_saved(core: &Core, saved: Option<Saved>) -> CoreResult<()> {
    let text = saved.as_ref().map(encode_saved).transpose()?;
    tokio::task::spawn_blocking(move || match text {
        Some(text) => sikemux_keychain::write(KEY_SERVICE, key_account(), &text),
        None => sikemux_keychain::delete(KEY_SERVICE, key_account()),
    })
    .await
    .map_err(|error| CoreError::from(error.to_string()))?
    .map_err(|error| CoreError::from(error.to_string()))?;
    *core.account.saved.locked() = Some(saved);
    Ok(())
}

/// The account let this host go while it was not looking: the core has a key
/// but no longer this account as its owner.
fn released(owner: Option<&str>, has_key: bool, user_id: &str) -> bool {
    has_key && owner != Some(user_id)
}

fn folder(core: &Core) -> Option<PathBuf> {
    Some(core.workspaces.data_dir()?.join(FOLDER))
}

async fn forget(core: &Core) -> CoreResult<()> {
    keep_saved(core, None).await?;
    if let Some(folder) = folder(core) {
        profile::forget(&folder).await?;
    }
    Ok(())
}

/// Works out what clients show of the account again, and tells them when it
/// changed. A sign-in the account let go of while the host was not looking
/// is forgotten here.
pub(crate) async fn show(core: &Core) {
    let owner = core.remote.owner();
    let mut saved = saved(core).await;
    let has_key = core.remote.host_key().is_some();
    if saved
        .as_ref()
        .is_some_and(|saved| released(owner.as_deref(), has_key, &saved.user_id))
    {
        if let Err(error) = forget(core).await {
            eprintln!("sikemux core: could not forget the account: {error}");
        }
        saved = None;
    }
    let shown = match (owner, saved, folder(core)) {
        (None, ..) => None,
        (Some(_), Some(saved), Some(folder)) => {
            let profile = profile::read(&folder)
                .await
                .filter(|profile| profile.user_id == saved.user_id);
            let picture_hash = profile
                .as_ref()
                .and_then(|profile| profile.picture.as_deref())
                .filter(|url| profile::picture_path(&folder, url).is_file())
                .map(profile::cache_key);
            let (name, email) = match profile {
                Some(profile) => (profile.name, profile.email.or(saved.email)),
                None => (None, saved.email),
            };
            Some(AccountInfo {
                name,
                email,
                picture_hash,
            })
        }
        (Some(_), ..) => Some(AccountInfo {
            name: None,
            email: None,
            picture_hash: None,
        }),
    };
    let changed = {
        let mut kept = core.account.shown.locked();
        let changed = *kept != shown;
        *kept = shown;
        changed
    };
    if changed {
        core.view_changed();
        core.changes.changed(Topic::Account, None, None);
    }
}

/// Shows the account once the core is up, then asks the account service for
/// the name and picture again whenever the cached ones are a few hours old.
pub(crate) fn start(core: &Arc<Core>) {
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        loop {
            let Some(alive) = core.upgrade() else {
                return;
            };
            refresh_profile(&alive).await;
            drop(alive);
            tokio::time::sleep(PROFILE_RETRY).await;
        }
    });
}

async fn refresh_profile(core: &Core) {
    let (Some(saved), Some(folder)) = (saved(core).await, folder(core)) else {
        show(core).await;
        return;
    };
    let stale = profile::read(&folder)
        .await
        .filter(|profile| profile.user_id == saved.user_id)
        .is_none_or(|profile| profile.is_stale(profile::now()));
    let tried_lately = core
        .account
        .profile_tried
        .locked()
        .is_some_and(|at| at.elapsed() < PROFILE_RETRY);
    if stale && !tried_lately {
        *core.account.profile_tried.locked() = Some(Instant::now());
        if let Ok(account) =
            as_host::<AccountProfile>(core, Method::GET, "/v1/account/profile", None).await
        {
            let fresh = profile::from_account(account, saved.user_id, profile::now());
            let _ = profile::save(&folder, &fresh).await;
        }
    }
    show(core).await;
}

/// The account's picture, while `hash` names the one cached.
pub(crate) async fn picture(core: &Core, hash: &str) -> Option<Value> {
    let (mime, bytes) = profile::picture(&folder(core)?, hash).await?;
    Some(json!({ "mime": mime, "data": STANDARD.encode(bytes) }))
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

/// Starts a browser sign-in, in place of any under way, and answers with the
/// address the person signs in at, which the client opens in their browser.
/// The browser must run on this host, since it hands back to a listener here.
pub(crate) async fn sign_in(core: &Arc<Core>, peer: Peer) -> CoreResult<Value> {
    let refused = |error: LoopbackError| CoreError::from(error.0);
    let signed_in = format!("{}/signed-in", crate::accounts::app_base());
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
    let url = authorize_url(callback.redirect_uri(), &state, &challenge(&verifier));
    let (cancel, cancelled) = oneshot::channel();
    let (done, finished) = watch::channel(None);
    if let Some(previous) = core.account.flight.locked().replace(Flight {
        cancel,
        done: finished,
    }) {
        let _ = previous.cancel.send(());
    }
    let core = core.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            result = finish(&core, &peer, &callback, &state, &verifier) => result,
            _ = cancelled => Err("sign-in was cancelled".to_owned()),
            () = tokio::time::sleep(SIGN_IN_TIMEOUT) => Err("sign-in took too long; try again".to_owned()),
        };
        let _ = done.send(Some(result));
    });
    Ok(json!({ "url": url }))
}

/// Waits for the browser sign-in under way, and answers with the account it
/// signed in to.
pub(crate) async fn sign_in_finished(core: &Core) -> CoreResult<AccountInfo> {
    let mut done = core
        .account
        .flight
        .locked()
        .as_ref()
        .map(|flight| flight.done.clone())
        .ok_or("no sign-in is under way")?;
    let finished = done
        .wait_for(Option::is_some)
        .await
        .map_err(|_| CoreError::from("sign-in was cancelled"))?
        .clone();
    match finished {
        Some(result) => result.map_err(CoreError::from),
        None => Err("sign-in was cancelled".into()),
    }
}

pub(crate) fn cancel_sign_in(core: &Core) {
    if let Some(flight) = core.account.flight.locked().take() {
        let _ = flight.cancel.send(());
    }
}

/// Takes this host off the account and turns remote access off, then forgets
/// the account here. The core tells the account, now or once it is back
/// online. Paired devices stay: they are the host's own list, approved one by
/// one.
pub(crate) async fn sign_out(core: &Arc<Core>, peer: &Peer) -> CoreResult<()> {
    remote::set_owner(core, None, peer).await?;
    forget(core).await?;
    show(core).await;
    Ok(())
}

async fn finish(
    core: &Arc<Core>,
    peer: &Peer,
    callback: &Callback,
    state: &str,
    verifier: &str,
) -> Result<AccountInfo, String> {
    let code = callback
        .code(state)
        .await
        .map_err(|error| format!("sign-in was not finished: {}", error.0))?;
    let response = client()
        .map_err(|error| error.to_string())?
        .post(format!("{CLERK}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", callback.redirect_uri()),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "the account service did not finish the sign-in ({})",
            response.status()
        ));
    }
    let tokens: Tokens = response.json().await.map_err(|error| error.to_string())?;
    let registered = register_host(core, peer, &tokens.access_token).await;
    revoke(tokens.refresh_token).await;
    let user_id = registered?;
    keep_saved(
        core,
        Some(Saved {
            user_id: user_id.clone(),
            email: None,
        }),
    )
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = remote::set_owner(core, Some(user_id.clone()), peer).await {
        let _ = keep_saved(core, None).await;
        return Err(error.to_string());
    }
    let folder = folder(core);
    if let Some(folder) = &folder {
        let _ = profile::forget(folder).await;
    }
    if let Ok(account) =
        as_host::<AccountProfile>(core, Method::GET, "/v1/account/profile", None).await
    {
        let email = account.email.clone();
        keep_saved(
            core,
            Some(Saved {
                user_id: user_id.clone(),
                email,
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
        if let Some(folder) = &folder {
            let fresh = profile::from_account(account, user_id, profile::now());
            let _ = profile::save(folder, &fresh).await;
        }
        *core.account.profile_tried.locked() = Some(Instant::now());
    }
    show(core).await;
    core.account
        .shown()
        .ok_or_else(|| "the account let this host go as it signed in".to_owned())
}

/// Clerk hands out a refresh token only for offline access, which Sikemux
/// does not ask for; should one come anyway, it is revoked at once.
async fn revoke(refresh_token: Option<String>) {
    let (Some(token), Ok(client)) = (refresh_token, client()) else {
        return;
    };
    let _ = client
        .post(format!("{CLERK}/oauth/token/revoke"))
        .form(&[
            ("token", token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await;
}

/// The signed-in user's id, read from the access token Clerk just issued to
/// this host. The server checks the token's signature on every request.
fn subject(access_token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Claims {
        sub: String,
    }
    let payload = access_token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Claims>(&bytes).ok())
        .ok_or("the sign-in token could not be read")?;
    crate::accounts::check_user_id(&payload.sub)?;
    Ok(payload.sub)
}

/// Registers the core's key with the account the browser signed in to, the
/// one thing the sign-in's token is used for. Answers with the account.
async fn register_host(core: &Core, peer: &Peer, access_token: &str) -> Result<String, String> {
    let user_id = subject(access_token)?;
    let api = |path: &str| {
        format!(
            "{}{path}",
            crate::accounts::api_base().trim_end_matches('/')
        )
    };
    let client = client().map_err(|error| error.to_string())?;
    let response = client
        .post(api("/v1/devices/challenge"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(crate::accounts::refusal(response).await);
    }
    let challenge: Challenge = response.json().await.map_err(|error| error.to_string())?;
    let signed = core
        .remote
        .may_join(&user_id, peer)
        .and_then(|()| core.remote.sign_registration(&challenge.nonce, &user_id));
    let (key, signature) = signed.map_err(|error| error.to_string())?;
    let host = tokio::task::spawn_blocking(super::host::info)
        .await
        .map_err(|error| error.to_string())?;
    let registration = DeviceRegistration {
        key,
        role: DeviceRole::Host,
        name: host.name.chars().take(64).collect(),
        platform: super::HOST_PLATFORM,
        channel: Some(host.channel.into()),
        nonce: challenge.nonce,
        signature,
    };
    let response = client
        .post(api("/v1/devices"))
        .bearer_auth(access_token)
        .json(&registration)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(crate::accounts::refusal(response).await);
    }
    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!released(Some("user_1"), true, "user_1"));
        assert!(released(None, true, "user_1"));
        assert!(released(Some("user_2"), true, "user_1"));
        assert!(!released(None, false, "user_1"));
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
