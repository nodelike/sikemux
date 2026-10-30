//! This host's webhooks: addresses on the accounts server that reach it wherever it is. The
//! server holds what senders post until the host takes it over `/v1/hooks/live`, a connection
//! that runs while the host has a webhook and comes back whenever it drops.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Method;
use ring::rand::{SecureRandom, SystemRandom};
use sikemux_process::sync::Locked;

use super::hosts::account::{as_host, host_request};
use super::triggers::{self, HookDelivery};
use super::{Core, CoreError, CoreResult};
use crate::accounts::hooks::{
    Hook, HookAdd, HookClientMessage, HookCloseCode, HookDelivery as Received, HookDone, HookHello,
    HookMethod, HookPong, HookRole, HookServerMessage,
};
use crate::accounts::live::{parse_rfc3339, socket_url};
use crate::accounts::socket::{self, Closed, Failures, Frame, Socket};

/// A webhook just added, and where senders post, its secret at the end of that address.
pub(crate) struct AddedHook {
    pub hook_id: String,
    pub url: String,
}

#[derive(Default)]
pub(crate) struct Hooks {
    link: Mutex<LinkState>,
}

#[derive(Default)]
struct LinkState {
    running: bool,
    /// Asked for while running, so a connection the server just closed as idle is made again.
    again: bool,
}

/// Gives this host a new webhook.
pub(crate) async fn register_hook(core: &Arc<Core>) -> CoreResult<AddedHook> {
    let mut secret = [0u8; 32];
    SystemRandom::new()
        .fill(&mut secret)
        .map_err(|_| CoreError::from("could not make a webhook's secret"))?;
    let token = hex::encode(secret);
    let add = HookAdd { token };
    let hook: Hook = as_host(
        core,
        Method::POST,
        "/v1/hooks",
        Some(serde_json::to_value(add)?),
    )
    .await?;
    connect(core);
    Ok(AddedHook {
        hook_id: hook.id,
        url: hook.url,
    })
}

/// Takes one of this host's webhooks away, with the requests the server still holds for it.
pub(crate) async fn remove_hook(core: &Core, hook_id: &str) -> CoreResult<()> {
    let is_id = hook_id.len() == 32
        && hook_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !is_id {
        return Err(CoreError::from("that is not one of this host's webhooks"));
    }
    host_request(core, Method::DELETE, &format!("/v1/hooks/{hook_id}"), None).await?;
    Ok(())
}

/// Makes sure the connection runs while this host is signed in. The server closes it at once
/// when the host has no webhooks.
pub(crate) fn connect(core: &Arc<Core>) {
    let Some(base) = core.remote.signed_in_api() else {
        return;
    };
    let mut link = core.hooks.link.locked();
    if link.running {
        link.again = true;
        return;
    }
    link.running = true;
    link.again = false;
    tokio::spawn(run(
        Arc::downgrade(core),
        socket_url(&base, "/v1/hooks/live"),
    ));
}

enum Ended {
    /// The host has no webhooks, or is no longer on an account.
    Done,
    Retry {
        hint: Option<Duration>,
        was_open: bool,
    },
}

async fn run(core: Weak<Core>, url: String) {
    let mut failures = Failures::default();
    loop {
        let ended = match core.upgrade() {
            Some(core) => session(&core, &url).await,
            None => return,
        };
        match ended {
            Ended::Done => {
                let Some(core) = core.upgrade() else {
                    return;
                };
                let mut link = core.hooks.link.locked();
                if !link.again {
                    link.running = false;
                    return;
                }
                link.again = false;
            }
            Ended::Retry { hint, was_open } => {
                tokio::time::sleep(failures.wait(hint, was_open)).await;
            }
        }
    }
}

async fn send(socket: &mut Socket, message: &HookClientMessage) -> bool {
    socket::send(socket, message).await
}

/// The request as the triggers take it.
fn delivery(received: Received) -> Result<HookDelivery, String> {
    let body = STANDARD
        .decode(&received.body)
        .map_err(|_| "its body is not base64")?;
    Ok(HookDelivery {
        hook_id: received.hook,
        delivery_id: received.id,
        received_at: parse_rfc3339(&received.received_at).ok_or("its time is not readable")?,
        method: match received.method {
            HookMethod::Post => "POST",
            HookMethod::Put => "PUT",
            HookMethod::Patch => "PATCH",
            HookMethod::Unknown => return Err("its method is not one this build knows".into()),
        }
        .to_owned(),
        headers: received
            .headers
            .into_iter()
            .map(|header| (header.name, header.value))
            .collect(),
        query: received.query,
        body,
    })
}

/// Whether the request is done with: taken by the triggers, or one that never could be.
fn handle(core: &Arc<Core>, received: Received) -> bool {
    let id = received.id.clone();
    let delivery = match delivery(received) {
        Ok(delivery) => delivery,
        Err(error) => {
            eprintln!("sikemux core: dropped a webhook's request {id}, as {error}");
            return true;
        }
    };
    match triggers::delivered(core, delivery) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("sikemux core: a webhook's request {id} waits to be tried again: {error}");
            false
        }
    }
}

async fn session(core: &Arc<Core>, url: &str) -> Ended {
    let retry = |hint, was_open| Ended::Retry { hint, was_open };
    let Some(mut socket) = socket::connect(url).await else {
        return retry(None, false);
    };
    let nonce = socket::challenge(&mut socket, |message| match message {
        HookServerMessage::Challenge(challenge) => Some(challenge.nonce),
        _ => None,
    });
    let Some(nonce) = nonce.await else {
        return retry(None, false);
    };
    let Some((key, signature)) = core.remote.sign_hooks(&nonce) else {
        return Ended::Done;
    };
    let hello = HookClientMessage::Hello(HookHello {
        role: HookRole::Host,
        key,
        signature,
    });
    if !send(&mut socket, &hello).await {
        return retry(None, false);
    }

    let mut open = false;
    let mut hint = None;
    let mut silence = socket::HEARTBEAT * 2;
    loop {
        let message = match socket::next_frame(&mut socket, silence).await {
            Frame::Server(message) => message,
            Frame::Other => continue,
            Frame::Closed(code) => {
                return match code.and_then(|code| HookCloseCode::from_code(code.into())) {
                    Some(HookCloseCode::Idle | HookCloseCode::Unauthenticated) => Ended::Done,
                    Some(HookCloseCode::Replaced) => {
                        retry(socket::wait_after(Closed::Replaced, hint), open)
                    }
                    Some(HookCloseCode::OverLimit) => {
                        retry(socket::wait_after(Closed::OverLimit, hint), open)
                    }
                    _ => retry(hint, open),
                };
            }
        };
        let sent = match message {
            HookServerMessage::Ready(ready) => {
                open = true;
                silence = socket::silence(ready.heartbeat_ms);
                true
            }
            HookServerMessage::Delivery(delivered) => {
                let id = delivered.delivery.id.clone();
                let handled = handle(core, delivered.delivery);
                send(
                    &mut socket,
                    &HookClientMessage::Done(HookDone { id, handled }),
                )
                .await
            }
            HookServerMessage::Ping(_) => {
                send(&mut socket, &HookClientMessage::Pong(HookPong {})).await
            }
            HookServerMessage::Bye(bye) => {
                hint = Some(socket::asked_wait(bye.reconnect_after_ms));
                true
            }
            HookServerMessage::Challenge(_) | HookServerMessage::Unknown => true,
        };
        if !sent {
            return retry(hint, open);
        }
    }
}
