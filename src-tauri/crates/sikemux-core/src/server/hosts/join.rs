//! Linking two hosts. The person on one asks the account for a join ticket
//! naming the other host and this one, as a phone does, and this host hands it
//! over in a `join` ask. The other host checks the ticket itself and asks its
//! owner, who allows the link once. From then on both keep the other as
//! linked.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use crate::accounts::protocol::JoinTicket;
use crate::join::APPROVAL_TIMEOUT;
use crate::protocol::PendingDevice;

use super::super::join::{clean, NAME_LIMIT, PLATFORM_LIMIT};
use super::super::notify::Signal;
use super::super::remote::{self, unix_ms, LinkedHost, Pairing};
use super::super::{Core, CoreError, CoreResult};
use super::{addresses_of, adopt, dial, introduction, link};

const STEP: Duration = Duration::from_secs(15);
/// Time for the other host to read the answer before the connection closes.
const LINGER: Duration = Duration::from_secs(2);
/// Links asked for at once. Anything past this is turned away.
static IN_PROGRESS: Semaphore = Semaphore::const_new(4);

/// What the window passes to link this host with another: the ticket the
/// account gave it, and what the account calls the other host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkParams {
    ticket: JoinTicket,
    name: String,
    #[serde(default)]
    platform: String,
    /// Where the other host may be reached directly, beside the relays.
    #[serde(default)]
    addresses: Vec<String>,
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

/// Links this host with the one `params` names, once the person there
/// allows it, and answers with this host's devices and links.
pub(crate) async fn link_host(core: &Arc<Core>, params: LinkParams) -> CoreResult<Pairing> {
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
        linked_at: unix_ms(),
        last_seen: Some(unix_ms()),
        addresses: known,
    })?;
    adopt(core, &ticket.host, connection);
    Ok(remote::pairing(core))
}

async fn finish(incoming: link::Incoming, connection: &Connection, answer: Value) {
    incoming.reply(Ok(answer)).await;
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

/// A host not linked with this one asks to be. Only a `join` with a good
/// ticket gets as far as the person here, who allows it or not.
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
    core.remote.forget_device_requests(&host);
    let request = PendingDevice {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: host.clone(),
        name: clean(&hello.name, NAME_LIMIT),
        platform: clean(&hello.platform, PLATFORM_LIMIT),
        from_account: true,
        expires_at: unix_ms() + APPROVAL_TIMEOUT.as_millis() as u64,
    };
    let answered = core.remote.ask(
        request.clone(),
        hello.ticket.account.clone(),
        Some(addresses_of(&hello.addresses)),
    );
    remote::announce(&core);
    core.notify.send(Signal::Pairing(request.clone()));
    let allowed = tokio::select! {
        answer = tokio::time::timeout(APPROVAL_TIMEOUT, answered) => {
            answer.ok().and_then(Result::ok).flatten().is_some()
        }
        _ = connection.closed() => false,
    };
    core.remote.forget_pending(&request.id);
    remote::announce(&core);
    core.notify.send(Signal::PairingSettled { id: request.id });
    if !allowed {
        finish(incoming, &connection, json!({ "result": "denied" })).await;
        return;
    }
    let mut reply = introduction(&core);
    reply["result"] = "allowed".into();
    incoming.reply(Ok(reply)).await;
    adopt(&core, &host, connection);
}
