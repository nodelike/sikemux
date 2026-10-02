//! Pairing a device with a core. The person reads a short code off the Mac
//! and types it on the device. Both sides run SPAKE2 on the code, bound to
//! both keys, so the code never crosses the wire, a guess costs one live
//! attempt, and a device that dialled an impostor finds out before it says
//! anything. Only then does the core ask the person to approve the device.

use std::time::Duration;

use hmac::{KeyInit, Mac};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::protocol::{encode_control, read_frame_within, DeviceAccess, FrameKind};

pub const PAIR_ALPN: &[u8] = b"sikemux/pair/1";
pub const CODE_DIGITS: usize = 6;
/// How long the core waits for the person at the Mac to answer.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const STEP_TIMEOUT: Duration = Duration::from_secs(15);
/// Every pairing message is a few hundred bytes. Either side reads them
/// before it knows who sent them, so nothing larger is accepted.
const MAX_MESSAGE_BYTES: usize = 4096;
const CORE_LABEL: &[u8] = b"sikemux pairing: the core knows the code";
const DEVICE_LABEL: &[u8] = b"sikemux pairing: the device knows the code";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PairMessage {
    /// Device to core.
    Hello {
        name: String,
        platform: String,
        spake: String,
    },
    /// Core to device.
    Challenge {
        spake: String,
        confirm: String,
    },
    /// Device to core.
    Confirm {
        confirm: String,
    },
    /// Core to device: the person at the Mac is being asked.
    Waiting,
    Approved {
        access: DeviceAccess,
    },
    Refused {
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("{0}")]
    Refused(String),
    #[error("the code does not match the one on the Mac")]
    WrongCode,
    #[error("could not reach the Mac: {0}")]
    Connection(String),
}

pub struct PairingRequest<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub platform: &'a str,
}

/// The digits of a code, however the person spaced or dashed it.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

/// What the Mac's pairing QR code holds: the core's key and the open code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingLink {
    pub core: iroh::PublicKey,
    pub code: String,
}

impl PairingLink {
    pub fn to_url(&self) -> String {
        format!("sikemux://pair?core={}&code={}", self.core, self.code)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let url = url::Url::parse(text.trim()).ok()?;
        if url.scheme() != "sikemux" || url.host_str() != Some("pair") {
            return None;
        }
        let value = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let code = normalize_code(&value("code")?);
        if code.len() != CODE_DIGITS {
            return None;
        }
        Some(Self {
            core: value("core")?.parse().ok()?,
            code,
        })
    }
}

pub(crate) type Exchange = Spake2<Ed25519Group>;

fn identities(device_id: &str, core_id: &str) -> (Identity, Identity) {
    (
        Identity::new(device_id.as_bytes()),
        Identity::new(core_id.as_bytes()),
    )
}

pub(crate) fn device_start(code: &str, device_id: &str, core_id: &str) -> (Exchange, Vec<u8>) {
    let (device, core) = identities(device_id, core_id);
    Spake2::start_a(&Password::new(normalize_code(code)), &device, &core)
}

pub(crate) fn core_start(code: &str, device_id: &str, core_id: &str) -> (Exchange, Vec<u8>) {
    let (device, core) = identities(device_id, core_id);
    Spake2::start_b(&Password::new(normalize_code(code)), &device, &core)
}

fn mac(key: &[u8], label: &[u8]) -> hmac::Hmac<Sha256> {
    let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .expect("HMAC takes a key of any length");
    mac.update(label);
    mac
}

pub(crate) fn core_confirmation(key: &[u8]) -> String {
    hex::encode(mac(key, CORE_LABEL).finalize().into_bytes())
}

pub(crate) fn device_confirmation(key: &[u8]) -> String {
    hex::encode(mac(key, DEVICE_LABEL).finalize().into_bytes())
}

fn confirms(key: &[u8], label: &[u8], confirmation: &str) -> bool {
    hex::decode(confirmation).is_ok_and(|bytes| mac(key, label).verify_slice(&bytes).is_ok())
}

pub(crate) fn core_confirms(key: &[u8], confirmation: &str) -> bool {
    confirms(key, CORE_LABEL, confirmation)
}

pub(crate) fn device_confirms(key: &[u8], confirmation: &str) -> bool {
    confirms(key, DEVICE_LABEL, confirmation)
}

pub(crate) async fn send(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &PairMessage,
) -> std::io::Result<()> {
    writer.write_all(&encode_control(message)?).await
}

pub(crate) async fn receive(
    reader: &mut (impl AsyncRead + Unpin),
    limit: Duration,
) -> std::io::Result<PairMessage> {
    let frame = tokio::time::timeout(limit, read_frame_within(reader, MAX_MESSAGE_BYTES))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer in time"))??
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    if frame.kind != FrameKind::Control {
        return Err(std::io::Error::other("unexpected frame"));
    }
    Ok(serde_json::from_slice(&frame.payload)?)
}

fn connection_error(error: impl std::fmt::Display) -> PairError {
    PairError::Connection(error.to_string())
}

/// Pairs this device with the core at `core`, waiting while the person at the
/// Mac decides. Answers with what the device was approved to do.
pub async fn pair(
    endpoint: &Endpoint,
    core: EndpointAddr,
    request: PairingRequest<'_>,
) -> Result<DeviceAccess, PairError> {
    let core_id = core.id.to_string();
    let device_id = endpoint.id().to_string();
    let connection = endpoint
        .connect(core, PAIR_ALPN)
        .await
        .map_err(connection_error)?;
    let (mut writer, mut reader) = connection.open_bi().await.map_err(connection_error)?;
    let (exchange, outbound) = device_start(request.code, &device_id, &core_id);
    send(
        &mut writer,
        &PairMessage::Hello {
            name: request.name.into(),
            platform: request.platform.into(),
            spake: hex::encode(outbound),
        },
    )
    .await
    .map_err(connection_error)?;
    let (spake, confirm) = match receive(&mut reader, STEP_TIMEOUT)
        .await
        .map_err(connection_error)?
    {
        PairMessage::Challenge { spake, confirm } => (spake, confirm),
        PairMessage::Refused { message } => return Err(PairError::Refused(message)),
        _ => return Err(PairError::Connection("the Mac answered out of turn".into())),
    };
    let inbound = hex::decode(spake).map_err(connection_error)?;
    let key = exchange
        .finish(&inbound)
        .map_err(|_| PairError::WrongCode)?;
    if !core_confirms(&key, &confirm) {
        connection.close(0u32.into(), b"wrong code");
        return Err(PairError::WrongCode);
    }
    send(
        &mut writer,
        &PairMessage::Confirm {
            confirm: device_confirmation(&key),
        },
    )
    .await
    .map_err(connection_error)?;
    let answer_within = APPROVAL_TIMEOUT + STEP_TIMEOUT;
    loop {
        match receive(&mut reader, answer_within)
            .await
            .map_err(connection_error)?
        {
            PairMessage::Waiting => continue,
            PairMessage::Approved { access } => {
                connection.close(0u32.into(), b"paired");
                return Ok(access);
            }
            PairMessage::Refused { message } => return Err(PairError::Refused(message)),
            _ => return Err(PairError::Connection("the Mac answered out of turn".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(device_code: &str, core_code: &str) -> (bool, bool) {
        let (device, to_core) = device_start(device_code, "device-key", "core-key");
        let (core, to_device) = core_start(core_code, "device-key", "core-key");
        let core_key = core.finish(&to_core).unwrap();
        let device_key = device.finish(&to_device).unwrap();
        (
            core_confirms(&device_key, &core_confirmation(&core_key)),
            device_confirms(&core_key, &device_confirmation(&device_key)),
        )
    }

    #[test]
    fn the_same_code_agrees_on_both_sides() {
        assert_eq!(run("482 913", "482-913"), (true, true));
    }

    #[test]
    fn a_different_code_is_caught_by_both_sides() {
        assert_eq!(run("482913", "482914"), (false, false));
    }

    #[test]
    fn a_device_that_dialled_another_key_does_not_agree() {
        let (device, to_core) = device_start("482913", "device-key", "impostor-key");
        let (core, to_device) = core_start("482913", "device-key", "core-key");
        let core_key = core.finish(&to_core).unwrap();
        let device_key = device.finish(&to_device).unwrap();
        assert!(!core_confirms(&device_key, &core_confirmation(&core_key)));
    }

    #[test]
    fn a_pairing_link_holds_the_core_and_the_code() {
        let core = iroh::SecretKey::generate().public();
        let link = PairingLink {
            core,
            code: "482913".into(),
        };
        let url = link.to_url();
        assert!(url.starts_with("sikemux://pair?core="));
        assert_eq!(PairingLink::parse(&url), Some(link));
        assert_eq!(PairingLink::parse("sikemux://pair?code=482913"), None);
        assert_eq!(
            PairingLink::parse(&format!("sikemux://pair?core={core}&code=12")),
            None
        );
        assert_eq!(
            PairingLink::parse(&format!("https://pair?core={core}&code=482913")),
            None
        );
    }

    #[test]
    fn a_core_confirmation_is_not_a_device_one() {
        let key = b"shared";
        assert!(!device_confirms(key, &core_confirmation(key)));
        assert!(!core_confirms(key, "not hex"));
    }
}
