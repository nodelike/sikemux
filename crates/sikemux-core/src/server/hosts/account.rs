//! Every host on an account links with every other (ADR 0018). This host asks
//! the account which hosts it has, links with each it is not linked with,
//! with a ticket it asks for itself, and calls each what the account calls it.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde_json::json;
use sikemux_process::clock::epoch_ms;

use crate::accounts::protocol::{DeviceList, DeviceRole};
use sikemux_wire::accounts::protocol::JoinTicket;

use super::super::account::api::as_host;
use super::super::{Core, CoreError, CoreResult};
use super::join::{link_with, LinkParams};

/// How soon a pass that found one already running looks again.
const AGAIN: Duration = Duration::from_secs(5);

/// Forgets the linked hosts that `hosts`, the account's, no longer holds.
pub(crate) fn forget_hosts_off_account(core: &Core, hosts: &DeviceList) -> CoreResult<()> {
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
