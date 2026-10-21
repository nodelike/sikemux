//! Every host on an account links with every other (ADR 0018). This host asks
//! the account which hosts it has, links with each it is not linked with,
//! with a ticket it asks for itself, and calls each what the account calls it.

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::{Client, Method};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sikemux_process::clock::{epoch_ms, epoch_secs};

use crate::accounts::protocol::{DeviceList, DeviceRole, JoinTicket};
use crate::accounts::HostRequest;
use crate::protocol::Event;

use super::super::{Core, CoreError, CoreResult};
use super::join::{link_with, LinkParams};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How soon a pass that found one already running looks again.
const AGAIN: Duration = Duration::from_secs(5);

fn client() -> CoreResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let tls = sikemux_client::accounts::tls_config()?;
            Client::builder()
                .tls_backend_preconfigured((*tls).clone())
                .timeout(REQUEST_TIMEOUT)
                .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| CoreError::from("this host cannot reach the accounts server"))
}

/// Calls the accounts API as this host, signed with its key.
pub(crate) async fn as_host<T: DeserializeOwned>(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<T> {
    let base = core
        .remote
        .signed_in_api()
        .ok_or_else(|| CoreError::from("this host is not signed in"))?;
    let body = body.map(|body| serde_json::to_vec(&body)).transpose()?;
    let request = HostRequest {
        method: method.as_str().to_owned(),
        path: path.to_owned(),
        time: epoch_secs(),
        body_sha256: hex::encode(Sha256::digest(body.as_deref().unwrap_or_default())),
    };
    let (key, signature) = core.remote.sign_host_request(&request)?;
    let mut sent = client()?
        .request(method, format!("{}{path}", base.trim_end_matches('/')))
        .header(
            "authorization",
            format!("SikemuxHost {key}.{}.{signature}", request.time),
        );
    if let Some(body) = body {
        sent = sent.header("content-type", "application/json").body(body);
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
    response
        .json()
        .await
        .map_err(|error| CoreError::from(error.to_string()))
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
