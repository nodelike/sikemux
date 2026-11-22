//! Which projects the account's teammates share with its orgs, and from which host, kept in the
//! account folder. The account says shares.changed or orgs.changed when it changes, and the host
//! reads it again then and each time its live connection comes back.

use reqwest::{Method, StatusCode};

use super::api::{as_host, host_request, host_send_any, Upload};
use super::{folder, kept};
use crate::accounts::protocol::{
    PeerTicket, PeerTicketRequest, ShareMove, ShareRegistration, ShareRegistry, ShareRemoval,
    ShareRename,
};
use crate::server::remote_state::RemoteParts;
use crate::server::{Core, CoreError, CoreResult};

const FILE: &str = "shares.json";
const PATH: &str = "/v1/account/shares";

/// The registry as the account last answered; none while signed out or not answered yet.
pub async fn read(core: &Core) -> Option<ShareRegistry> {
    let (owner, dir) = (core.remote().owner()?, folder(core)?);
    kept::read(&dir, FILE, &owner).await
}

/// Reads the registry again from the account, answering it only if the account answered.
pub async fn refresh(core: &Core) -> Option<ShareRegistry> {
    kept::refresh(core, PATH, FILE).await
}

/// Shares `project` with `org` from `host`, or moves it there. A share another of the person's
/// hosts made at the same moment is refused once, so it is asked again.
pub async fn register(core: &Core, registration: &ShareRegistration) -> CoreResult<()> {
    let mut tries = 0;
    loop {
        tries += 1;
        let response = send(core, Method::PUT, PATH, registration).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        if status != StatusCode::CONFLICT || tries == 2 {
            return Err(refused(response).await);
        }
    }
}

/// Moves a share to the project's home. The account refuses when the share is gone.
pub async fn move_share(core: &Core, share: &ShareMove) -> CoreResult<()> {
    let body = serde_json::to_value(share)?;
    host_request(core, Method::POST, &format!("{PATH}/move"), Some(body)).await?;
    Ok(())
}

/// What the account did with a renamed project's share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renamed {
    Moved,
    /// The account shares the old name with no org.
    Nothing,
    /// Another of the person's hosts holds the share; it moves it once it sees the rename.
    HeldElsewhere,
}

/// Moves the account's share of a renamed project to its new name.
pub async fn rename_share(core: &Core, rename: &ShareRename) -> CoreResult<Renamed> {
    let response = send(core, Method::POST, &format!("{PATH}/rename"), rename).await?;
    match response.status() {
        status if status.is_success() => Ok(Renamed::Moved),
        StatusCode::CONFLICT => Ok(Renamed::Nothing),
        StatusCode::FORBIDDEN => Ok(Renamed::HeldElsewhere),
        _ => Err(refused(response).await),
    }
}

async fn send(
    core: &Core,
    method: Method,
    path: &str,
    body: &impl serde::Serialize,
) -> CoreResult<reqwest::Response> {
    let body = Upload {
        bytes: serde_json::to_vec(body)?,
        content_type: "application/json".into(),
        headers: Vec::new(),
    };
    host_send_any(core, method, path, Some(body)).await
}

async fn refused(response: reqwest::Response) -> CoreError {
    let status = response.status();
    let said = response.text().await.unwrap_or_default();
    format!("the accounts server answered {status}: {said}").into()
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
