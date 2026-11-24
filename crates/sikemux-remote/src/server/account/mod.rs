//! Signing this host in to a Sikemux account, so devices on the same account
//! find it. Sign-in happens in the person's browser (OAuth with PKCE), which
//! hands back to a one-time listener on 127.0.0.1. The core passes the code
//! it got to the account server, which trades it at Clerk itself, and signs
//! the challenge that comes back with its key. The core never holds a sign-in
//! token; every later request is signed with the key, so taking the host off
//! the account cuts it off. The keychain keeps who signed in, the data folder their name and
//! picture, and the core's remote access which account owns the host. A host
//! with no browser signs in by a code approved elsewhere, and keeps no record.

mod allowance;
pub mod api;
mod code;
pub mod devices;
mod flags;
mod kept;
mod orgs;
mod profile;
pub mod shares;

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
pub use allowance::refresh as refresh_allowance;
pub use code::sign_in_by_code;
pub use orgs::{policy as org_policy, refresh as refresh_orgs, OrgPolicy};
pub use sikemux_calls::account::AccountPictureParams;
pub use sikemux_calls::account::{SignInCode, SignInPage};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sikemux_loopback::{challenge, Callback, Finish, LoopbackError};
use sikemux_process::sync::Locked;
use tokio::sync::{oneshot, watch};

use crate::accounts::protocol::{
    AccountProfile, Device, MacSignIn, MacSignInFinish, MacSignInState,
};
use sikemux_view::device::AccountInfo;
use sikemux_wire::protocol::HostRegistration;

use self::api::as_host;
use super::access::Peer;
use super::changed::Topic;
use super::join::NAME_LIMIT;
use super::remote_image::PictureData;
use super::{remote, Core, CoreError, CoreResult};
use crate::accounts::feature_name;

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
/// How often the core looks whether the person renamed this computer.
const RENAME_CHECK: Duration = Duration::from_secs(30);
const FOLDER: &str = "account";

#[cfg(not(any(test, feature = "testing")))]
const KEY_SERVICE: &str = "sikemux-account";
#[cfg(any(test, feature = "testing"))]
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

/// A sign-in under way.
struct Flight {
    cancel: oneshot::Sender<()>,
    done: watch::Receiver<Option<Result<AccountInfo, String>>>,
}

#[derive(Default)]
pub struct Account {
    /// Who signed in, as the keychain keeps it, once read; the core is the
    /// only one that writes it.
    saved: Mutex<Option<Option<Saved>>>,
    /// What clients are sent.
    shown: Mutex<Option<AccountInfo>>,
    flight: Mutex<Option<Flight>>,
    profile_tried: Mutex<Option<Instant>>,
}

impl Account {
    pub fn shown(&self) -> Option<AccountInfo> {
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
    if let Some(saved) = core.account().saved.locked().clone() {
        return saved;
    }
    let read = tokio::task::spawn_blocking(|| sikemux_keychain::read(KEY_SERVICE, key_account()))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .and_then(|text| decode_saved(&text));
    *core.account().saved.locked() = Some(read.clone());
    read
}

async fn keep_saved(core: &Core, saved: Option<Saved>) -> CoreResult<()> {
    if saved.is_none() {
        eprintln!("sikemux core: deleting the saved sign-in from the keychain");
    }
    let text = saved.as_ref().map(encode_saved).transpose()?;
    tokio::task::spawn_blocking(move || match text {
        Some(text) => sikemux_keychain::write(KEY_SERVICE, key_account(), &text),
        None => sikemux_keychain::delete(KEY_SERVICE, key_account()),
    })
    .await
    .map_err(|error| CoreError::from(error.to_string()))?
    .map_err(|error| CoreError::from(error.to_string()))?;
    *core.account().saved.locked() = Some(saved);
    Ok(())
}

/// The account let this host go while it was not looking: the core has a key
/// but no longer this account as its owner.
fn released(owner: Option<&str>, has_key: bool, user_id: &str) -> bool {
    has_key && owner != Some(user_id)
}

fn folder(core: &Core) -> Option<PathBuf> {
    Some(core.data_dir().as_ref()?.join(FOLDER))
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
pub async fn show(core: &Core) {
    let owner = core.remote().owner();
    let mut saved = saved(core).await;
    let has_key = core.remote().host_key().is_some();
    if saved
        .as_ref()
        .is_some_and(|saved| released(owner.as_deref(), has_key, &saved.user_id))
    {
        eprintln!(
            "sikemux core: the account no longer owns this host, so its saved sign-in is forgotten"
        );
        if let Err(error) = forget(core).await {
            eprintln!("sikemux core: could not forget the account: {error}");
        }
        saved = None;
    }
    let shown = match (owner, folder(core)) {
        (None, _) => None,
        (Some(owner), Some(folder)) => {
            let profile = profile::read(&folder)
                .await
                .filter(|profile| profile.user_id == owner);
            let picture_hash = profile
                .as_ref()
                .and_then(|profile| profile.picture.as_deref())
                .filter(|url| profile::picture_path(&folder, url).is_file())
                .map(profile::cache_key);
            let saved_email = saved
                .filter(|saved| saved.user_id == owner)
                .and_then(|saved| saved.email);
            let (name, email, staff) = match profile {
                Some(profile) => (profile.name, profile.email.or(saved_email), profile.staff),
                None => (None, saved_email, false),
            };
            let allowance = allowance::read(&folder, &owner).await;
            let orgs = orgs::read(&folder, &owner).await;
            Some(AccountInfo {
                name,
                email,
                picture_hash,
                staff,
                flags: flags::resolve(staff, &core.remote().flags_off()),
                pro_given: allowance.as_ref().is_some_and(allowance::pro_given),
                features: allowance
                    .map(|allowance| allowance.features.into_iter().map(feature_name).collect()),
                orgs,
            })
        }
        (Some(_), None) => Some(AccountInfo {
            name: None,
            email: None,
            picture_hash: None,
            staff: false,
            flags: Vec::new(),
            features: None,
            pro_given: false,
            orgs: Vec::new(),
        }),
    };
    let changed = {
        let mut kept = core.account().shown.locked();
        let changed = *kept != shown;
        *kept = shown;
        changed
    };
    if changed {
        core.changed(What::Read(Topic::Account));
        (core.remote_above().check_updates_soon)(core);
    }
}

/// Shows the account once the core is up, then asks the account service for
/// the name and picture again whenever the cached ones are a few hours old.
pub fn start(core: &Arc<Core>) {
    let core = Arc::downgrade(core);
    follow_renames(core.clone());
    tokio::spawn(async move {
        loop {
            let Some(alive) = core.upgrade() else {
                return;
            };
            refresh_profile(&alive).await;
            devices::keep_name_current(&alive).await;
            drop(alive);
            tokio::time::sleep(PROFILE_RETRY).await;
        }
    });
}

/// Passes the computer's new name on soon after the person renames it: to the account, which
/// tells the other devices and hosts, and to the devices paired here.
fn follow_renames(core: std::sync::Weak<Core>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RENAME_CHECK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(alive) = core.upgrade() else {
                return;
            };
            if let Ok(Some(_)) = tokio::task::spawn_blocking(super::host::renamed).await {
                devices::keep_name_current(&alive).await;
                remote::announce(&alive);
            }
        }
    });
}

async fn refresh_profile(core: &Core) {
    let (Some(owner), Some(folder)) = (core.remote().owner(), folder(core)) else {
        show(core).await;
        return;
    };
    let stale = profile::read(&folder)
        .await
        .filter(|profile| profile.user_id == owner)
        .is_none_or(|profile| profile.is_stale(profile::now()));
    let tried_lately = core
        .account()
        .profile_tried
        .locked()
        .is_some_and(|at| at.elapsed() < PROFILE_RETRY);
    if stale && !tried_lately {
        *core.account().profile_tried.locked() = Some(Instant::now());
        if let Ok(account) =
            as_host::<AccountProfile>(core, Method::GET, "/v1/account/profile", None).await
        {
            let fresh = profile::from_account(account, owner, profile::now());
            let _ = profile::save(&folder, &fresh).await;
        }
    }
    show(core).await;
}

/// The account's picture, while `hash` names the one cached.
pub async fn picture(core: &Core, hash: &str) -> Option<PictureData> {
    let (mime, bytes) = profile::picture(&folder(core)?, hash).await?;
    Some(PictureData {
        mime: mime.to_string(),
        data: STANDARD.encode(bytes),
    })
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
pub async fn sign_in(core: &Arc<Core>, peer: Peer) -> CoreResult<SignInPage> {
    sikemux_keychain::ready().map_err(|error| CoreError::from(error.to_string()))?;
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
    let signing = core.clone();
    take_off(core, async move {
        finish(&signing, &peer, &callback, &state, &verifier).await
    });
    Ok(SignInPage { url })
}

/// Runs `signing` as the sign-in under way, in place of any other, until it ends, is
/// cancelled or runs out of time.
fn take_off(
    core: &Arc<Core>,
    signing: impl Future<Output = Result<AccountInfo, String>> + Send + 'static,
) {
    let (cancel, cancelled) = oneshot::channel();
    let (done, finished) = watch::channel(None);
    if let Some(previous) = core.account().flight.locked().replace(Flight {
        cancel,
        done: finished,
    }) {
        let _ = previous.cancel.send(());
    }
    tokio::spawn(async move {
        let result = tokio::select! {
            result = signing => result,
            _ = cancelled => Err("sign-in was cancelled".to_owned()),
            () = tokio::time::sleep(SIGN_IN_TIMEOUT) => Err("sign-in took too long; try again".to_owned()),
        };
        let _ = done.send(Some(result));
    });
}

/// Waits for the sign-in under way, and answers with the account it signed in to.
pub async fn sign_in_finished(core: &Core) -> CoreResult<AccountInfo> {
    let mut done = core
        .account()
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

pub fn cancel_sign_in(core: &Core) {
    if let Some(flight) = core.account().flight.locked().take() {
        let _ = flight.cancel.send(());
    }
}

/// Takes this host off the account and turns remote access off, then forgets
/// the account here. The core tells the account, now or once it is back
/// online. Paired devices stay: they are the host's own list, approved one by
/// one.
pub async fn sign_out(core: &Arc<Core>, peer: &Peer) -> CoreResult<()> {
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
    let landed = callback
        .code(state)
        .await
        .map_err(|error| format!("sign-in was not finished: {}", error.0))?;
    let code = landed.code().to_owned();
    let joined = join(core, peer, code, callback.redirect_uri(), verifier).await;
    landed
        .answer(joined.as_ref().map(drop).map_err(String::clone))
        .await;
    joined
}

/// Joins this host to the account whose sign-in gave `code`.
async fn join(
    core: &Arc<Core>,
    peer: &Peer,
    code: String,
    redirect_uri: &str,
    verifier: &str,
) -> Result<AccountInfo, String> {
    let user_id = register_host(core, peer, code, redirect_uri, verifier).await?;
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
    if let Some(email) = fetch_profile(core, &user_id).await {
        keep_saved(core, Some(Saved { user_id, email }))
            .await
            .map_err(|error| error.to_string())?;
    }
    signed_in(core).await
}

/// Reads the name and picture of the account this host just signed in to, and answers
/// with its email once the account answers.
async fn fetch_profile(core: &Core, user_id: &str) -> Option<Option<String>> {
    let folder = folder(core);
    if let Some(folder) = &folder {
        let _ = profile::forget(folder).await;
    }
    let account = as_host::<AccountProfile>(core, Method::GET, "/v1/account/profile", None)
        .await
        .ok()?;
    let email = account.email.clone();
    if let Some(folder) = &folder {
        let fresh = profile::from_account(account, user_id.to_owned(), profile::now());
        let _ = profile::save(folder, &fresh).await;
    }
    *core.account().profile_tried.locked() = Some(Instant::now());
    Some(email)
}

async fn signed_in(core: &Core) -> Result<AccountInfo, String> {
    show(core).await;
    core.account()
        .shown()
        .ok_or_else(|| "the account let this host go as it signed in".to_owned())
}

/// Registers the core's key with the account the browser signed in to. The account trades
/// the browser's code at Clerk itself, so this host never holds a sign-in token. Answers with
/// the account.
async fn register_host(
    core: &Core,
    peer: &Peer,
    code: String,
    redirect_uri: &str,
    verifier: &str,
) -> Result<String, String> {
    let sign_in = MacSignIn {
        code,
        code_verifier: verifier.to_owned(),
        redirect_uri: redirect_uri.to_owned(),
    };
    let state: MacSignInState = code::post("/v1/mac-sign-ins", &sign_in).await?;
    sikemux_client::accounts::check_user_id(&state.account)?;
    let host = host_registration(core, &state.nonce, &state.account, peer)
        .await
        .map_err(|error| error.to_string())?;
    let finish = MacSignInFinish {
        key: host.key,
        name: host.name.chars().take(NAME_LIMIT).collect(),
        platform: super::host::PLATFORM,
        channel: host.channel.into(),
        nonce: state.nonce,
        signature: host.signature,
    };
    let _: Device = code::post("/v1/mac-sign-ins/finish", &finish).await?;
    Ok(state.account)
}

/// This host as `user_id`'s account is to register it, signed for the challenge `nonce`, when
/// the account may have it.
pub async fn host_registration(
    core: &Core,
    nonce: &str,
    user_id: &str,
    peer: &Peer,
) -> CoreResult<HostRegistration> {
    core.remote().may_join(user_id, peer)?;
    let (key, signature) = core.remote().sign_registration(nonce, user_id)?;
    let (host, name) =
        super::connection::blocking(|| Ok((super::host::info(), super::host::name()))).await?;
    Ok(HostRegistration {
        key,
        name,
        channel: host.channel,
        signature,
    })
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
}
