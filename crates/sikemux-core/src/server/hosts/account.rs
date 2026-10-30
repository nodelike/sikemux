//! Every host on an account links with every other (ADR 0018). This host asks
//! the account which hosts it has, links with each it is not linked with,
//! with a ticket it asks for itself, and calls each what the account calls it.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sikemux_process::clock::{epoch_ms, epoch_secs};
use sikemux_process::sync::Locked;

use crate::accounts::host::ApprovedHost;
use crate::accounts::protocol::{
    Challenge, Device, DeviceKey, DeviceList, DeviceRegistration, DeviceRole, JoinTicket, Platform,
};
use crate::accounts::HostRequest;
use crate::protocol::DeviceInfo;
use crate::protocol::Event;

use super::super::{Core, CoreError, CoreResult};
use super::join::{link_with, LinkParams};

/// How soon a pass that found one already running looks again.
const AGAIN: Duration = Duration::from_secs(5);
/// How long the accounts server remembers a signature, refusing it if it comes again.
const SIGNATURE_MEMORY_SECS: u64 = 60;

/// What the account asks one of the person's devices to sign before this host adds it:
/// the account's nonce, who owns the account, and what the account calls this host.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceChallenge {
    nonce: String,
    user_id: String,
    host_name: String,
}

/// Starts adding one of the person's own devices, such as an app, to the account as this host.
pub(crate) async fn device_challenge(core: &Core) -> CoreResult<DeviceChallenge> {
    let status = core.remote.status();
    let user_id = status
        .owner
        .filter(|_| !status.core_id.is_empty())
        .ok_or_else(|| CoreError::from("sign in to reach your other hosts"))?;
    let hosts: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    let host_name = hosts
        .devices
        .iter()
        .find(|known| known.key == status.core_id)
        .map_or_else(|| "this host".to_owned(), |known| known.name.clone());
    let challenge: Challenge = as_host(core, Method::POST, "/v1/devices/challenge", None).await?;
    Ok(DeviceChallenge {
        nonce: challenge.nonce,
        user_id,
        host_name,
    })
}

/// A device of the person's, signed over a challenge from [`device_challenge`], and the
/// host it is to join.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JoinTicketParams {
    key: DeviceKey,
    name: String,
    platform: Platform,
    nonce: String,
    signature: String,
    host: DeviceKey,
}

/// Adds the device to the account as one this host added, and asks the account for the
/// ticket that lets it join `host`.
pub(crate) async fn device_join_ticket(
    core: &Core,
    params: JoinTicketParams,
) -> CoreResult<JoinTicket> {
    let registration = DeviceRegistration {
        key: params.key.clone(),
        role: DeviceRole::Client,
        name: params.name.chars().take(64).collect(),
        platform: params.platform,
        channel: None,
        nonce: params.nonce,
        signature: params.signature,
    };
    let _: Device = as_host(
        core,
        Method::POST,
        "/v1/devices",
        Some(serde_json::to_value(&registration)?),
    )
    .await?;
    as_host(
        core,
        Method::POST,
        &format!("/v1/devices/{}/join", params.key),
        Some(json!({ "host": params.host })),
    )
    .await
}

/// The code another host showed while signing in, as `ABCD-EFGH`.
#[derive(Deserialize)]
pub(crate) struct SignInCodeParams {
    code: String,
}

impl SignInCodeParams {
    pub(crate) fn new(code: String) -> Self {
        Self { code }
    }
}

/// Approves the sign-in code another host showed, as this host, for the person installing
/// Sikemux there, and answers with that host.
pub(crate) async fn approve_host_sign_in(
    core: &Core,
    params: SignInCodeParams,
) -> CoreResult<ApprovedHost> {
    let code = params.code.trim();
    if code.is_empty()
        || code.len() > 64
        || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("that is not a sign-in code".into());
    }
    as_host(
        core,
        Method::POST,
        &format!("/v1/host-sign-ins/{code}/approve-from-host"),
        None,
    )
    .await
}

/// Calls the accounts API as this host, signed with its key.
pub(crate) async fn as_host<T: DeserializeOwned>(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<T> {
    host_request(core, method, path, body)
        .await?
        .json()
        .await
        .map_err(|error| CoreError::from(error.to_string()))
}

/// The second to sign a request with. The same request signed twice in one second would carry
/// the same signature, which the accounts server refuses as a replay, so a repeat takes the
/// next second instead.
fn unused_second(request: String) -> u64 {
    static SIGNED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let now = epoch_secs();
    let mut signed = SIGNED.get_or_init(Default::default).locked();
    signed.retain(|_, second| *second + SIGNATURE_MEMORY_SECS > now);
    let second = signed.get(&request).map_or(now, |last| now.max(last + 1));
    signed.insert(request, second);
    second
}

/// Calls the accounts API as this host, signed with its key, for an answer
/// with nothing to read.
pub(crate) async fn host_request(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<reqwest::Response> {
    let body = body
        .map(|body| serde_json::to_vec(&body))
        .transpose()?
        .map(|bytes| Upload {
            bytes,
            content_type: "application/json".into(),
            headers: Vec::new(),
        });
    host_send(core, method, path, body).await
}

/// A body sent as it is, such as a file's bytes, with what the server reads about it.
pub(crate) struct Upload {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub headers: Vec<(&'static str, String)>,
}

/// Calls the accounts API as this host, signed with its key over `body`, whose hash goes along
/// so the server can check it before reading the body.
pub(crate) async fn host_send(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Upload>,
) -> CoreResult<reqwest::Response> {
    let base = core
        .remote
        .signed_in_api()
        .ok_or_else(|| CoreError::from("this host is not signed in"))?;
    let body_sha256 = hex::encode(Sha256::digest(
        body.as_ref()
            .map(|body| body.bytes.as_slice())
            .unwrap_or_default(),
    ));
    let time = unused_second(format!("{method} {path} {body_sha256}"));
    let request = HostRequest {
        method: method.as_str().to_owned(),
        path: path.to_owned(),
        time,
        body_sha256,
    };
    let (key, signature) = core.remote.sign_host_request(&request)?;
    let mut sent = crate::accounts::client()
        .map_err(CoreError::from)?
        .request(method, format!("{}{path}", base.trim_end_matches('/')))
        .header(
            "authorization",
            format!("SikemuxHost {key}.{}.{signature}", request.time),
        );
    if let Some(body) = body {
        sent = sent
            .header("content-type", body.content_type)
            .header("x-body-sha256", &request.body_sha256);
        for (name, value) in body.headers {
            sent = sent.header(name, value);
        }
        sent = sent.body(body.bytes);
    }
    let response = sent
        .send()
        .await
        .map_err(|error| CoreError::from(error.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let said = response.text().await.unwrap_or_default();
        return Err(format!("the accounts server answered {status}: {said}").into());
    }
    Ok(response)
}

/// A device or host on the account, by its key.
fn device_path(key: &str) -> CoreResult<String> {
    let usable = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_'));
    if !usable {
        return Err(CoreError::from("that is not a device on the account"));
    }
    Ok(format!("/v1/devices/{key}"))
}

#[derive(serde::Deserialize)]
pub(crate) struct DeviceParams {
    key: String,
    #[serde(default)]
    name: Option<String>,
}

/// Renames a device or host on the account, which every device and host
/// shows from then on.
pub(crate) async fn rename_device(core: &Core, params: DeviceParams) -> CoreResult<()> {
    let name = params
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CoreError::from("a device needs a name"))?;
    let path = format!("{}/name", device_path(&params.key)?);
    host_request(core, Method::POST, &path, Some(json!({ "name": name }))).await?;
    Ok(())
}

/// Takes a device or host off the account, which revokes it on every host at once.
/// A host the account already lost is only forgotten here.
pub(crate) async fn remove_device(core: &Core, params: DeviceParams) -> CoreResult<()> {
    let Err(error) = host_request(core, Method::DELETE, &device_path(&params.key)?, None).await
    else {
        return Ok(());
    };
    let hosts: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    forget_hosts_off_account(core, &hosts)?;
    if hosts.devices.iter().any(|host| host.key == params.key) {
        return Err(error);
    }
    Ok(())
}

/// Forgets the linked hosts that `hosts`, the account's, no longer holds.
fn forget_hosts_off_account(core: &Core, hosts: &DeviceList) -> CoreResult<()> {
    let keys: Vec<String> = hosts.devices.iter().map(|host| host.key.clone()).collect();
    let gone = core.remote.forget_links_off(&keys)?;
    for host in &gone {
        super::unlinked(core, host, "it was removed from your account");
    }
    if !gone.is_empty() {
        super::super::remote::announce(core);
    }
    Ok(())
}

/// The phones on the account this host has not paired, newest first, so the
/// person can let them in right after signing in.
pub(crate) async fn phones(core: &Core) -> CoreResult<Vec<Device>> {
    let status = core.remote.status();
    if status.owner.is_none() || status.core_id.is_empty() {
        return Ok(Vec::new());
    }
    let list: DeviceList = as_host(core, Method::GET, "/v1/devices?role=client", None).await?;
    Ok(unpaired_phones(list.devices, &status.devices))
}

fn unpaired_phones(devices: Vec<Device>, paired: &[DeviceInfo]) -> Vec<Device> {
    devices
        .into_iter()
        .filter(|device| device.role == DeviceRole::Client)
        .filter(|device| !paired.iter().any(|known| known.id == device.key))
        .collect()
}

/// Links this host with every other host on its account it is not linked
/// with yet. One pass at a time; a pass asked for meanwhile runs after.
pub(crate) fn relink(core: &Arc<Core>) {
    if core.remote.signed_in_api().is_none() {
        return;
    }
    if core.hosts.linking.swap(true, Ordering::AcqRel) {
        core.hosts.relink.at(epoch_ms() + AGAIN.as_millis() as u64);
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        if let Err(error) = link_all(&core).await {
            eprintln!("sikemux core: could not link with the account's other hosts: {error}");
        }
        core.hosts.linking.store(false, Ordering::Release);
    });
}

/// Tells the app that a device or host joined the account, by the name the
/// account gives it.
pub(crate) fn tell_joined(core: &Arc<Core>, key: String) {
    let core = core.clone();
    tokio::spawn(async move {
        let devices: CoreResult<DeviceList> =
            as_host(&core, Method::GET, "/v1/devices", None).await;
        let Some(device) = devices
            .ok()
            .and_then(|list| list.devices.into_iter().find(|device| device.key == key))
        else {
            return;
        };
        core.broadcast_local(&Event::AccountDevice {
            key,
            name: device.name,
            platform: serde_json::to_value(device.platform)
                .ok()
                .and_then(|platform| platform.as_str().map(str::to_owned))
                .unwrap_or_default(),
        });
    });
}

async fn link_all(core: &Arc<Core>) -> CoreResult<()> {
    let own = core
        .remote
        .core_id()
        .ok_or_else(|| CoreError::from("this host's key has not loaded"))?;
    let hosts: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    forget_hosts_off_account(core, &hosts)?;
    let names: Vec<(String, String)> = hosts
        .devices
        .iter()
        .map(|host| (host.key.clone(), host.name.clone()))
        .collect();
    if core.remote.name_links(&names)? {
        super::super::remote::announce(core);
    }
    let linked: Vec<String> = core
        .remote
        .links()
        .into_iter()
        .map(|link| link.id)
        .collect();
    for host in hosts.devices {
        if host.role != DeviceRole::Host || host.key == own || linked.contains(&host.key) {
            continue;
        }
        let ticket: JoinTicket = as_host(
            core,
            Method::POST,
            &format!("/v1/devices/{own}/join"),
            Some(json!({ "host": host.key })),
        )
        .await?;
        let params = LinkParams {
            ticket,
            name: host.name.clone(),
            platform: serde_json::to_value(host.platform)?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            addresses: Vec::new(),
        };
        if let Err(error) = link_with(core, params).await {
            eprintln!(
                "sikemux core: could not link with {}: {error}",
                if host.name.is_empty() {
                    &host.key
                } else {
                    &host.name
                }
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::protocol::Platform;

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
            access: crate::protocol::DeviceAccess::Full,
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
}
