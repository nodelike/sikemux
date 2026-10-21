//! Linking two hosts. One asks the account for a join ticket naming the other
//! host and itself, as a phone does, and hands it over in a `join` ask. The
//! other host checks the ticket itself and links, or asks its person first
//! while they asked to approve each new host. From then on both keep the other
//! as linked.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use tokio::sync::Semaphore;

use crate::accounts::protocol::JoinTicket;
use crate::join::APPROVAL_TIMEOUT;
use crate::protocol::PendingDevice;

use super::super::join::{clean, NAME_LIMIT, PLATFORM_LIMIT};
use super::super::notify::Signal;
use super::super::remote::{self, LinkedHost, Pairing};
use super::super::{Core, CoreError, CoreResult};
use super::{addresses_of, adopt, dial, introduction, link};

const STEP: Duration = Duration::from_secs(15);
/// Time for the other host to read the answer before the connection closes.
const LINGER: Duration = Duration::from_secs(2);
/// Links asked for at once. Anything past this is turned away.
static IN_PROGRESS: Semaphore = Semaphore::const_new(4);

/// What linking with another host takes: the ticket the account gave this
/// host for it, and what the account calls it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkParams {
    pub ticket: JoinTicket,
    pub name: String,
    #[serde(default)]
    pub platform: String,
    /// Where the other host may be reached directly, beside the relays.
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// Links this host with the one `params` names, and answers with this host's
/// devices and links.
pub(crate) async fn link_host(core: &Arc<Core>, params: LinkParams) -> CoreResult<Pairing> {
    link_with(core, params).await?;
    Ok(remote::pairing(core))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hello {
    ticket: JoinTicket,
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    addresses: Value,
}

/// Links this host with the one `params` names.
pub(super) async fn link_with(core: &Arc<Core>, params: LinkParams) -> CoreResult<()> {
    let own = core
        .remote
        .core_id()
        .ok_or_else(|| CoreError::from("this host's key has not loaded"))?;
    let ticket = params.ticket;
    if ticket.phone != own {
        return Err("that ticket is for another host".into());
    }
    if ticket.host == own {
        return Err("a host cannot link with itself".into());
    }
    let name = clean(&params.name, NAME_LIMIT);
    let addresses = addresses_of(&json!(params.addresses));
    let connection = dial(core, &ticket.host, &addresses)
        .await
        .map_err(|error| CoreError::from(format!("{name} is out of reach: {error}")))?;
    let mut hello = introduction(core);
    hello["ticket"] = serde_json::to_value(&ticket)?;
    let reply = link::ask(&connection, "join", hello, None, APPROVAL_TIMEOUT + STEP)
        .await
        .map_err(|error| CoreError::from(format!("{name} did not link: {error}")))?;
    match reply["result"].as_str() {
        Some("allowed") => {}
        Some("denied") => return Err(format!("{name} did not allow the link").into()),
        _ => {
            return Err(format!(
                "{name} refused the link: {}",
                reply["reason"].as_str().unwrap_or("no reason given")
            )
            .into())
        }
    }
    let mut known = addresses_of(&reply["addresses"]);
    known.extend(addresses);
    known.dedup();
    let platform = match clean(&params.platform, PLATFORM_LIMIT) {
        platform if platform.is_empty() => clean(
            reply["platform"].as_str().unwrap_or_default(),
            PLATFORM_LIMIT,
        ),
        platform => platform,
    };
    core.remote.add_link(LinkedHost {
        id: ticket.host.clone(),
        name: if name.is_empty() {
            clean(reply["name"].as_str().unwrap_or("Host"), NAME_LIMIT)
        } else {
            name
        },
        platform,
        linked_at: epoch_ms(),
        last_seen: Some(epoch_ms()),
        addresses: known,
    })?;
    adopt(core, &ticket.host, connection);
    remote::announce(core);
    Ok(())
}

async fn finish(incoming: link::Incoming, connection: &Connection, answer: Value) {
    incoming.reply(Ok(answer)).await;
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

/// A host not linked with this one asks to be. A `join` with a good ticket
/// from this host's own account links them, or waits for the person while
/// they asked to approve each new host.
pub(super) async fn answer(core: Arc<Core>, connection: Connection) {
    let Ok(_slot) = IN_PROGRESS.try_acquire() else {
        connection.close(0u32.into(), b"busy");
        return;
    };
    let host = connection.remote_id().to_string();
    let Ok(Some(Ok(incoming))) = tokio::time::timeout(STEP, link::next(&connection)).await else {
        connection.close(link::NOT_LINKED.into(), b"not linked");
        return;
    };
    if incoming.method != "join" {
        incoming
            .reply(Err("this host is not linked with yours".into()))
            .await;
        connection.close(link::NOT_LINKED.into(), b"not linked");
        return;
    }
    let Ok(hello) = serde_json::from_value::<Hello>(incoming.params.clone()) else {
        let refused = json!({ "result": "refused", "reason": "unreadable" });
        finish(incoming, &connection, refused).await;
        return;
    };
    if let Err(refusal) = core.remote.check_join(&hello.ticket, &host) {
        eprintln!(
            "sikemux core: refused a link from {}: {}",
            host.get(..8).unwrap_or(&host),
            refusal.reason()
        );
        let refused = json!({ "result": "refused", "reason": refusal.reason() });
        finish(incoming, &connection, refused).await;
        return;
    }
    let name = clean(&hello.name, NAME_LIMIT);
    let platform = clean(&hello.platform, PLATFORM_LIMIT);
    let addresses = addresses_of(&hello.addresses);
    if core.remote.requires_approval() {
        core.remote.forget_device_requests(&host);
        let request = PendingDevice {
            id: uuid::Uuid::new_v4().to_string(),
            device_id: host.clone(),
            name,
            platform,
            from_account: true,
            expires_at: epoch_ms() + APPROVAL_TIMEOUT.as_millis() as u64,
        };
        let answered = core.remote.ask(
            request.clone(),
            hello.ticket.account.clone(),
            Some(addresses),
        );
        remote::announce(&core);
        core.notify.send(Signal::Pairing(request.clone()));
        // The person may answer after the other host gave up waiting; the
        // link is kept, and either host uses it the next time.
        let allowed = tokio::time::timeout(APPROVAL_TIMEOUT, answered)
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .is_some();
        core.remote.forget_pending(&request.id);
        remote::announce(&core);
        core.notify.send(Signal::PairingSettled { id: request.id });
        if !allowed {
            finish(incoming, &connection, json!({ "result": "denied" })).await;
            return;
        }
    } else {
        let linked = core.remote.add_link(LinkedHost {
            id: host.clone(),
            name: if name.is_empty() { "Host".into() } else { name },
            platform,
            linked_at: epoch_ms(),
            last_seen: Some(epoch_ms()),
            addresses,
        });
        if let Err(error) = linked {
            let refused = json!({ "result": "refused", "reason": error.to_string() });
            finish(incoming, &connection, refused).await;
            return;
        }
        remote::announce(&core);
    }
    let mut reply = introduction(&core);
    reply["result"] = "allowed".into();
    incoming.reply(Ok(reply)).await;
    adopt(&core, &host, connection);
    // The account's name for the new host replaces the one it gave itself.
    core.hosts.relink.now();
}
