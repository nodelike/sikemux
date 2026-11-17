//! Which projects the account's teammates share with its orgs, and from which host, kept in the
//! account folder. The account says shares.changed or orgs.changed when it changes, and the host
//! reads it again then and each time its live connection comes back.

use reqwest::Method;

use super::api::{as_host, host_request};
use super::{folder, kept};
use crate::accounts::protocol::{
    PeerTicket, PeerTicketRequest, ShareRegistration, ShareRegistry, ShareRemoval,
};
use crate::server::remote_state::RemoteParts;
use crate::server::{Core, CoreResult};

const FILE: &str = "shares.json";
const PATH: &str = "/v1/account/shares";

/// The registry as the account last answered; none while signed out or not answered yet.
pub async fn read(core: &Core) -> Option<ShareRegistry> {
    let (owner, dir) = (core.remote().owner()?, folder(core)?);
    kept::read(&dir, FILE, &owner).await
}

pub async fn refresh(core: &Core) {
    kept::refresh::<ShareRegistry>(core, PATH, FILE).await;
}

/// Shares `project` with `org` from `host`, or moves it there.
pub async fn register(core: &Core, registration: &ShareRegistration) -> CoreResult<()> {
    let body = serde_json::to_value(registration)?;
    host_request(core, Method::PUT, PATH, Some(body)).await?;
    Ok(())
}

/// Stops sharing `project` with `org`, whether or not it was shared.
pub async fn remove(core: &Core, removal: &ShareRemoval) -> CoreResult<()> {
    let body = serde_json::to_value(removal)?;
    host_request(core, Method::POST, &format!("{PATH}/remove"), Some(body)).await?;
    Ok(())
}

/// The server's word, good for a few minutes, that this host belongs to a member of the org, for
/// the teammate's host that shares a project with it.
pub async fn ticket(core: &Core, request: &PeerTicketRequest) -> CoreResult<PeerTicket> {
    let body = serde_json::to_value(request)?;
    as_host(core, Method::POST, "/v1/account/peer-tickets", Some(body)).await
}
