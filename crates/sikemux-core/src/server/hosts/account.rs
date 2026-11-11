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

use super::super::account::api::{as_host, host_send_any, retry_after, Upload};
use super::super::{Core, CoreError, CoreResult};
use super::join::{link_with, LinkParams, Linking};

/// How soon a pass that found one already running looks again.
const AGAIN: Duration = Duration::from_secs(5);
/// How long to wait when the account is too busy to say how long.
const BUSY: Duration = Duration::from_secs(60);

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
    let on_account: Vec<String> = hosts.devices.iter().map(|host| host.key.clone()).collect();
    let linked: Vec<String> = core
        .remote
        .links()
        .into_iter()
        .map(|link| link.id)
        .collect();
    let declined = core.remote.declined_links();
    let now = epoch_ms();
    let mut next = None;
    let mut links = Vec::new();
    for host in hosts.devices {
        if host.role != DeviceRole::Host
            || host.key == own
            || linked.contains(&host.key)
            || declined.contains(&host.key)
        {
            continue;
        }
        if let Some(due) = core.remote.relink_due(&host.key).filter(|due| *due > now) {
            next = sooner(next, due);
            continue;
        }
        let ticket = match ticket(core, &own, &host.key).await {
            Ok(Ticket::Issued(ticket)) => ticket,
            Ok(Ticket::Later(at)) => {
                next = sooner(next, at);
                break;
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: no ticket to link with {}: {error}",
                    host.key.get(..8).unwrap_or(&host.key)
                );
                next = sooner(next, core.remote.relink_failed(&host.key));
                continue;
            }
        };
        let params = LinkParams {
            ticket,
            name: host.name.clone(),
            platform: serde_json::to_value(host.platform)?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            addresses: Vec::new(),
        };
        let core = core.clone();
        let on_account = on_account.clone();
        links.push(async move {
            let named = if host.name.is_empty() {
                &host.key
            } else {
                &host.name
            };
            match link_with(&core, params).await {
                Ok(Linking::Linked) => {
                    core.remote.relink_succeeded(&host.key);
                    None
                }
                Ok(Linking::Declined) => {
                    core.remote.relink_succeeded(&host.key);
                    eprintln!("sikemux core: {named} turned down linking; it is not asked again");
                    if let Err(error) = core.remote.decline_link(&host.key, &on_account) {
                        eprintln!("sikemux core: could not save that {named} turned down linking: {error}");
                    }
                    None
                }
                Err(error) => {
                    eprintln!("sikemux core: could not link with {named}: {error}");
                    Some(core.remote.relink_failed(&host.key))
                }
            }
        });
    }
    for due in futures_util::future::join_all(links)
        .await
        .into_iter()
        .flatten()
    {
        next = sooner(next, due);
    }
    if let Some(next) = next {
        core.hosts.relink.at(next);
    }
    Ok(())
}

fn sooner(next: Option<u64>, at: u64) -> Option<u64> {
    Some(next.map_or(at, |next| next.min(at)))
}

enum Ticket {
    Issued(JoinTicket),
    /// The account asks for no more tickets until then.
    Later(u64),
}

/// A ticket for this host, `own`, to join `host`, or when the account will
/// give one if it asks to wait.
async fn ticket(core: &Core, own: &str, host: &str) -> CoreResult<Ticket> {
    let body = serde_json::to_vec(&json!({ "host": host }))?;
    let response = host_send_any(
        core,
        Method::POST,
        &format!("/v1/devices/{own}/join"),
        Some(Upload {
            bytes: body,
            content_type: "application/json".into(),
            headers: Vec::new(),
        }),
    )
    .await?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let wait = retry_after(&response).unwrap_or(BUSY);
        return Ok(Ticket::Later(epoch_ms() + wait.as_millis() as u64));
    }
    if !response.status().is_success() {
        let status = response.status();
        let said = response.text().await.unwrap_or_default();
        return Err(format!("the accounts server answered {status}: {said}").into());
    }
    let ticket = response
        .json()
        .await
        .map_err(|error| CoreError::from(error.to_string()))?;
    Ok(Ticket::Issued(ticket))
}
