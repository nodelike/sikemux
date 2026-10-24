//! What the app asks of the account, through its core: a ticket to join each of the
//! account's other hosts, and approving the code another host shows while signing in.
//! The core makes every account call itself, signed with its key.

use serde::{Deserialize, Serialize};
use sikemux_core::accounts::protocol::JoinTicket;
use sikemux_core::client::CoreClient;

use crate::error::{AppError, AppResult};
use crate::pty::core_error;

/// The core's key and its signature, as the accounts server checks them.
#[derive(Serialize, Deserialize)]
pub(crate) struct HostSignature {
    pub key: String,
    pub signature: String,
}

/// Adds this app to the account as a device of its own, added by this host, and asks the
/// account through the core for a ticket that lets it join `host`.
pub(crate) async fn app_join_ticket(
    core: &CoreClient,
    device: &sikemux_core::device::Device,
    host: &str,
) -> AppResult<JoinTicket> {
    let challenge = core
        .call("deviceChallenge", serde_json::json!({}))
        .await
        .map_err(core_error)?;
    let text = |key: &str| challenge[key].as_str().unwrap_or_default().to_owned();
    let signature = device
        .sign_registration(&text("nonce"), &text("userId"))
        .map_err(|reason| AppError::Other(reason.into()))?;
    let ticket = core
        .call(
            "deviceJoinTicket",
            serde_json::json!({
                "key": device.id(),
                "name": format!("Sikemux app on {}", text("hostName")),
                "platform": "macos",
                "nonce": text("nonce"),
                "signature": signature,
                "host": host,
            }),
        )
        .await
        .map_err(core_error)?;
    Ok(serde_json::from_value(ticket)?)
}
