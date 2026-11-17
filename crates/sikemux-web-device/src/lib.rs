//! The web app as a device on the person's account, for one thing: reading a host's look over
//! iroh, end to end, the way a phone reads it. The browser keeps its own key, registers it as a
//! `web` client, and hands the host the look-only ticket the accounts server signs for it.

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey};
use sikemux_wire::look::{LOOK_ALPN, MAX_LOOK_BYTES};
use sikemux_wire::protocol::{encode_control, read_frame_within, FrameKind};
use wasm_bindgen::prelude::*;

fn failed(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

fn secret(bytes: &[u8]) -> Result<SecretKey, JsError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| JsError::new("a device key is 32 bytes"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

#[wasm_bindgen(js_name = newKey)]
pub fn new_key() -> Vec<u8> {
    SecretKey::generate().to_bytes().to_vec()
}

#[wasm_bindgen(js_name = publicKey)]
pub fn public_key(key: &[u8]) -> Result<String, JsError> {
    Ok(secret(key)?.public().to_string())
}

/// The key's signature, in hex, that registers it with the account `user_id` for the accounts
/// server's challenge `nonce`.
#[wasm_bindgen(js_name = signRegistration)]
pub fn sign_registration(key: &[u8], nonce: &str, user_id: &str) -> Result<String, JsError> {
    let key = secret(key)?;
    let message = format!("sikemux-register|{nonce}|{user_id}|{}", key.public());
    Ok(key
        .sign(message.as_bytes())
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Asks `host` for its look through the relays at `relays`, with the look-only `ticket` (JSON),
/// and answers with the host's `LookReply` as JSON.
#[wasm_bindgen(js_name = readLook)]
pub async fn read_look(
    key: &[u8],
    relays: Vec<String>,
    host: &str,
    ticket: &str,
) -> Result<String, JsError> {
    let relays: Vec<RelayUrl> = relays
        .iter()
        .filter_map(|relay| relay.parse().ok())
        .collect();
    if relays.is_empty() {
        return Err(JsError::new("the network names no relay"));
    }
    let host: PublicKey = host.parse().map_err(failed)?;
    let ticket: serde_json::Value = serde_json::from_str(ticket).map_err(failed)?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(
            relays.iter().cloned().collect::<RelayMap>(),
        ))
        .secret_key(secret(key)?)
        .bind()
        .await
        .map_err(failed)?;
    let addr = relays
        .into_iter()
        .fold(EndpointAddr::new(host), EndpointAddr::with_relay_url);
    let answer = ask(&endpoint, addr, &ticket).await;
    endpoint.close().await;
    answer
}

async fn ask(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    ticket: &serde_json::Value,
) -> Result<String, JsError> {
    let connection = endpoint.connect(addr, LOOK_ALPN).await.map_err(failed)?;
    let (mut writer, mut reader) = connection.open_bi().await.map_err(failed)?;
    writer
        .write_all(&encode_control(ticket).map_err(failed)?)
        .await
        .map_err(failed)?;
    writer.finish().map_err(failed)?;
    let frame = read_frame_within(&mut reader, MAX_LOOK_BYTES)
        .await
        .map_err(failed)?
        .ok_or_else(|| JsError::new("the host closed without answering"))?;
    if frame.kind != FrameKind::Control {
        return Err(JsError::new("the host answered with an unexpected frame"));
    }
    connection.close(0u32.into(), b"done");
    String::from_utf8(frame.payload).map_err(failed)
}
