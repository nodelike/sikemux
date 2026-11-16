//! Signing in a host with no browser: the core shows a code its person approves
//! on a device already signed in, then registers its key with that account.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Response;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_calls::account::SignInCode;
use sikemux_process::clock::epoch_ms;

use super::{host_registration, remote, signed_in, take_off};
use crate::accounts::host::{
    HostSignIn, HostSignInFinish, HostSignInPoll, HostSignInStart, HostSignInState,
    HostSignInStatus,
};
use crate::accounts::protocol::Device;
use crate::server::access::Peer;
use crate::server::remote_state::RemoteParts;
use crate::server::{connection, host, Core, CoreError, CoreResult};

/// Starts a sign-in by code, in place of any under way, and answers with the code and
/// where the person enters it. `accountSignInFinish` waits for it to end.
pub async fn sign_in_by_code(core: &Arc<Core>, peer: Peer) -> CoreResult<SignInCode> {
    let key = core.remote().status().core_id;
    if key.is_empty() {
        return Err("the core's key has not loaded".into());
    }
    let info = connection::blocking(|| Ok(host::info())).await?;
    let start = HostSignInStart {
        key,
        name: info.name.chars().take(64).collect(),
        platform: host::PLATFORM,
        channel: info.channel.into(),
    };
    let signing: HostSignIn = post("/v1/host-sign-ins", &start)
        .await
        .map_err(CoreError::from)?;
    let shown = SignInCode {
        url: signing.verification_url.clone(),
        code: signing.user_code.clone(),
    };
    let core_signing = core.clone();
    take_off(core, async move {
        let account = finish(&core_signing, &peer, signing).await?;
        remote::set_owner(&core_signing, Some(account.clone()), &peer)
            .await
            .map_err(|error| error.to_string())?;
        super::fetch_profile(&core_signing, &account).await;
        signed_in(&core_signing).await
    });
    Ok(shown)
}

async fn finish(core: &Core, peer: &Peer, signing: HostSignIn) -> Result<String, String> {
    let (account, nonce) = wait_for_approval(&signing).await?;
    sikemux_client::accounts::check_user_id(&account)?;
    let registration = host_registration(core, &nonce, &account, peer)
        .await
        .map_err(|error| error.to_string())?;
    let finish = HostSignInFinish {
        code: signing.code,
        nonce,
        signature: registration.signature,
    };
    let _: Device = post("/v1/host-sign-ins/finish", &finish).await?;
    Ok(account)
}

/// Asks whether the person approved the code until they do or it expires,
/// riding out a dropped network, a busy server and being asked to slow down.
/// Answers with the account and the challenge to sign.
async fn wait_for_approval(signing: &HostSignIn) -> Result<(String, String), String> {
    let poll = HostSignInPoll {
        code: signing.code.clone(),
    };
    let interval = Duration::from_secs(signing.interval.max(1).unsigned_abs());
    let expires = crate::accounts::live::parse_rfc3339(&signing.expires_at)
        .ok_or("the account server gave a code with no expiry")?;
    let mut wait = interval;
    loop {
        tokio::time::sleep(wait).await;
        if epoch_ms() >= expires {
            return Err("the code expired before it was approved; try again".into());
        }
        wait = interval;
        match send("/v1/host-sign-ins/poll", &poll).await {
            Ok(response) if response.status().is_success() => {
                let state: HostSignInState =
                    response.json().await.map_err(|error| error.to_string())?;
                if let (HostSignInStatus::Approved, Some(account), Some(nonce)) =
                    (state.status, state.account, state.nonce)
                {
                    return Ok((account, nonce));
                }
            }
            Ok(response) if passing(response.status().as_u16()) => {
                wait = retry_after(&response).unwrap_or(interval).max(interval);
            }
            Ok(response) => return Err(crate::accounts::refusal(response).await),
            Err(_) => {}
        }
    }
}

/// A refusal that says nothing about the code: the server is busy or asks
/// for fewer requests.
fn passing(status: u16) -> bool {
    status >= 500 || status == 429
}

fn retry_after(response: &Response) -> Option<Duration> {
    let seconds = response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

async fn send(path: &str, body: &impl Serialize) -> Result<Response, String> {
    let base = crate::accounts::api_base();
    crate::accounts::client()?
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())
}

async fn post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, String> {
    let response = send(path, body).await?;
    if !response.status().is_success() {
        return Err(crate::accounts::refusal(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}
