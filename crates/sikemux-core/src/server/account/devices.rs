//! The person's devices and hosts on the account, as this host sees and changes them.

use std::sync::Arc;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::accounts::host::ApprovedHost;
use crate::accounts::protocol::{
    Challenge, Device, DeviceList, DeviceRegistration, DeviceRole, Platform,
};
use crate::server::changed::What;
use sikemux_wire::accounts::protocol::{DeviceKey, JoinTicket};
use sikemux_wire::protocol::{DeviceInfo, Event};

use super::super::hosts::account::forget_hosts_off_account;
use super::super::join::NAME_LIMIT;
use super::super::{Core, CoreError, CoreResult};
use super::api::{as_host, host_request};

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
        name: params.name.chars().take(NAME_LIMIT).collect(),
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

#[derive(serde::Deserialize, serde::Serialize)]
pub struct DeviceParams {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// The key this host's app goes on the network with, offered by the app the first time.
#[derive(Deserialize)]
pub(crate) struct AppKeyParams {
    offered: String,
}

/// Keeps the app's network key for an app that may keep no secrets of its own, as on Linux,
/// where only the core reaches the guarded store. Answers with the key kept before, if any.
pub(crate) async fn app_device_key(params: AppKeyParams) -> CoreResult<String> {
    const SERVICE: &str = "sikemux-device";
    const ACCOUNT: &str = "app";
    let failed = |error: sikemux_keychain::KeychainError| CoreError::from(error.to_string());
    super::super::connection::blocking(move || {
        if let Some(kept) = sikemux_keychain::read(SERVICE, ACCOUNT).map_err(failed)? {
            return Ok(kept);
        }
        sikemux_keychain::write(SERVICE, ACCOUNT, &params.offered).map_err(failed)?;
        Ok(params.offered)
    })
    .await
}

/// The name this host last gave itself on the account, kept beside the core's data.
const NAME_SENT_FILE: &str = "account-host-name";

fn name_sent_path(core: &Core) -> Option<std::path::PathBuf> {
    Some(core.config.data_dir.as_ref()?.join(NAME_SENT_FILE))
}

pub(crate) async fn remember_name_sent(core: &Core, name: &str) {
    let Some(path) = name_sent_path(core) else {
        return;
    };
    let bytes = name.as_bytes().to_vec();
    let _ = tokio::task::spawn_blocking(move || {
        sikemux_process::files::write_atomic(&path, &bytes, true)
    })
    .await;
}

/// Gives the account this computer's new name after the person renames it, unless the
/// account's name for this host is no longer the one it last sent, since then someone
/// chose that name and it stays.
pub(crate) async fn keep_name_current(core: &Core) {
    let (Some(path), Some(_)) = (name_sent_path(core), core.remote.owner()) else {
        return;
    };
    let Ok(current) = tokio::task::spawn_blocking(super::super::host::name).await else {
        return;
    };
    let current: String = current.chars().take(NAME_LIMIT).collect();
    let sent = tokio::fs::read_to_string(&path).await.ok();
    if sent.as_deref() == Some(current.as_str()) {
        return;
    }
    let key = core.remote.status().core_id;
    let Ok(hosts) = as_host::<DeviceList>(core, Method::GET, "/v1/devices?role=host", None).await
    else {
        return;
    };
    let Some(on_account) = hosts.devices.into_iter().find(|known| known.key == key) else {
        return;
    };
    if on_account.name != current && sent.as_deref() == Some(on_account.name.as_str()) {
        let renamed = rename_device(
            core,
            DeviceParams {
                key: key.to_string(),
                name: Some(current.clone()),
            },
        )
        .await;
        if renamed.is_err() {
            return;
        }
    }
    remember_name_sent(core, &current).await;
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
    crate::server::remote::rename_paired(core, &params.key, &name)
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
        core.changed(What::Event(&Event::AccountDevice {
            key,
            name: device.name,
            platform: serde_json::to_value(device.platform)
                .ok()
                .and_then(|platform| platform.as_str().map(str::to_owned))
                .unwrap_or_default(),
        }));
    });
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
            access: sikemux_wire::protocol::DeviceAccess::Full,
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
