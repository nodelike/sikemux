//! The accounts server, as a host reaches it. A host with no browser signs in
//! by a code its person approves on a device already signed in.

use sikemux_wire::protocol::ask;
use std::time::Duration;

use reqwest::Response;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_client::client::CoreClient;
use sikemux_core::accounts::host::{
    HostSignIn, HostSignInFinish, HostSignInPoll, HostSignInStart, HostSignInState,
    HostSignInStatus,
};
use sikemux_core::accounts::protocol::Device;
use sikemux_process::clock::epoch_ms;

/// Signs this host in without a browser. `show` gets the code its person
/// approves on a device already signed in. Answers with the account.
pub async fn sign_in_by_code(
    core: &CoreClient,
    show: impl FnOnce(&HostSignIn),
) -> Result<String, String> {
    let key = core
        .remote_status()
        .await
        .map_err(|error| error.to_string())?
        .core_id;
    if key.is_empty() {
        return Err("the core's key has not loaded".into());
    }
    let host = sikemux_core::server::host::info();
    let start = HostSignInStart {
        key,
        name: host.name.chars().take(64).collect(),
        platform: sikemux_core::server::host::PLATFORM,
        channel: host.channel.into(),
    };
    let signing: HostSignIn = post("/v1/host-sign-ins", &start).await?;
    show(&signing);
    let poll = HostSignInPoll {
        code: signing.code.clone(),
    };
    let (account, nonce) = wait_for_approval(&signing, &poll).await?;
    sikemux_client::accounts::check_user_id(&account).map_err(|error| error.to_string())?;
    let signed = core
        .ask(ask::SignRegistration {
            nonce: nonce.clone(),
            user_id: account.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    let finish = HostSignInFinish {
        code: signing.code,
        nonce,
        signature: signed.signature,
    };
    let _: Device = post("/v1/host-sign-ins/finish", &finish).await?;
    core.set_owner(Some(account.clone()))
        .await
        .map_err(|error| error.to_string())?;
    Ok(account)
}

/// Asks whether the person approved the code until they do or it expires,
/// riding out a dropped network, a busy server and being asked to slow down.
/// Answers with the account and the challenge to sign.
async fn wait_for_approval(
    signing: &HostSignIn,
    poll: &HostSignInPoll,
) -> Result<(String, String), String> {
    let interval = Duration::from_secs(signing.interval.max(1).unsigned_abs());
    let expires = sikemux_core::accounts::live::parse_rfc3339(&signing.expires_at)
        .ok_or("the account server gave a code with no expiry")?;
    let mut wait = interval;
    loop {
        tokio::time::sleep(wait).await;
        if epoch_ms() >= expires {
            return Err(
                "the code expired before it was approved; run `sikemux login` again".into(),
            );
        }
        wait = interval;
        match send("/v1/host-sign-ins/poll", poll).await {
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
            Ok(response) => return Err(sikemux_core::accounts::refusal(response).await),
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
    let base = sikemux_core::accounts::api_base();
    sikemux_core::accounts::client()?
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())
}

async fn post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, String> {
    let response = send(path, body).await?;
    if !response.status().is_success() {
        return Err(sikemux_core::accounts::refusal(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}
