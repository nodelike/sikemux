//! The accounts server, as a host reaches it. A host with no browser signs in
//! by a code its person approves on a device already signed in.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_core::accounts::host::{
    HostSignIn, HostSignInFinish, HostSignInPoll, HostSignInStart, HostSignInState,
    HostSignInStatus,
};
use sikemux_core::accounts::protocol::Device;
use sikemux_core::client::CoreClient;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub fn api(path: &str) -> String {
    format!(
        "{}{path}",
        sikemux_core::accounts::api_base().trim_end_matches('/')
    )
}

pub fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("build the account server's HTTP client")
    })
}

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
    let host = sikemux_core::server::host_info();
    let start = HostSignInStart {
        key,
        name: host.name.chars().take(64).collect(),
        platform: sikemux_core::server::HOST_PLATFORM,
        channel: host.channel.into(),
    };
    let signing: HostSignIn = post("/v1/host-sign-ins", &start).await?;
    show(&signing);
    let poll = HostSignInPoll {
        code: signing.code.clone(),
    };
    let interval = Duration::from_secs(signing.interval.max(1).unsigned_abs());
    let (account, nonce) = loop {
        tokio::time::sleep(interval).await;
        let state: HostSignInState = post("/v1/host-sign-ins/poll", &poll).await?;
        if let (HostSignInStatus::Approved, Some(account), Some(nonce)) =
            (state.status, state.account, state.nonce)
        {
            break (account, nonce);
        }
    };
    sikemux_core::accounts::check_user_id(&account).map_err(|error| error.to_string())?;
    let signed = core
        .sign_registration(nonce.clone(), account.clone())
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

async fn post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, String> {
    let response = http()
        .post(api(path))
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(sikemux_core::accounts::refusal(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}
