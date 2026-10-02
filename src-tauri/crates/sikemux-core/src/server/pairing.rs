//! The core's half of pairing; [`crate::pairing`] describes the exchange.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, SendStream};
use tokio::sync::Semaphore;

use crate::pairing::{self, PairMessage, APPROVAL_TIMEOUT};
use crate::protocol::PendingDevice;

use super::remote;
use super::Core;

const STEP: Duration = Duration::from_secs(15);
/// Time for the device to read the last answer before the connection closes.
const LINGER: Duration = Duration::from_secs(2);
const NAME_LIMIT: usize = 64;
/// Pairing connections answered at once. One person pairs one device at a
/// time; anything past this is turned away before it costs anything.
static IN_PROGRESS: Semaphore = Semaphore::const_new(4);
const PLATFORM_LIMIT: usize = 16;
const NO_CODE: &str =
    "No pairing code is open on this Mac. Open Settings, then Devices, and choose Pair a device.";
const UNREADABLE: &str = "The Mac could not read this device's pairing message.";
const DECLINED: &str = "The Mac did not approve this device.";

fn clean(text: &str, limit: usize) -> String {
    let kept: String = text
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect();
    kept.trim().to_owned()
}

async fn finish(writer: &mut SendStream, connection: &Connection, last: &PairMessage) {
    let _ = pairing::send(writer, last).await;
    let _ = writer.finish();
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

async fn refuse(writer: &mut SendStream, connection: &Connection, message: &str) {
    let refusal = PairMessage::Refused {
        message: message.into(),
    };
    finish(writer, connection, &refusal).await;
}

pub(super) async fn serve(core: Arc<Core>, connection: Connection) {
    let Ok(_slot) = IN_PROGRESS.try_acquire() else {
        connection.close(0u32.into(), b"busy");
        return;
    };
    let device_id = connection.remote_id().to_string();
    let Ok(Ok((mut writer, mut reader))) = tokio::time::timeout(STEP, connection.accept_bi()).await
    else {
        return;
    };
    let Ok(PairMessage::Hello {
        name,
        platform,
        spake,
    }) = pairing::receive(&mut reader, STEP).await
    else {
        refuse(&mut writer, &connection, UNREADABLE).await;
        return;
    };
    let (Some(core_id), Some(code)) = (core.remote.core_id(), core.remote.attempt()) else {
        refuse(&mut writer, &connection, NO_CODE).await;
        return;
    };
    remote::announce(&core);
    let (exchange, outbound) = pairing::core_start(&code, &device_id, &core_id);
    let Some(key) = hex::decode(&spake)
        .ok()
        .and_then(|inbound| exchange.finish(&inbound).ok())
    else {
        refuse(&mut writer, &connection, UNREADABLE).await;
        return;
    };
    let challenge = PairMessage::Challenge {
        spake: hex::encode(outbound),
        confirm: pairing::core_confirmation(&key),
    };
    if pairing::send(&mut writer, &challenge).await.is_err() {
        return;
    }
    // A device that typed another code stops here without confirming, having
    // already seen that the core's confirmation does not match.
    let confirmed = matches!(
        pairing::receive(&mut reader, STEP).await,
        Ok(PairMessage::Confirm { confirm }) if pairing::device_confirms(&key, &confirm)
    );
    if !confirmed {
        connection.close(0u32.into(), b"wrong code");
        return;
    }
    core.remote.spend_offer(&code);
    let request = PendingDevice {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.clone(),
        name: clean(&name, NAME_LIMIT),
        platform: clean(&platform, PLATFORM_LIMIT),
    };
    let answered = core.remote.ask(request.clone());
    remote::announce(&core);
    let _ = pairing::send(&mut writer, &PairMessage::Waiting).await;
    let access = tokio::select! {
        answer = tokio::time::timeout(APPROVAL_TIMEOUT, answered) => {
            answer.ok().and_then(Result::ok).flatten()
        }
        _ = connection.closed() => None,
    };
    core.remote.forget_pending(&request.id);
    let outcome = match access {
        Some(access) => PairMessage::Approved { access },
        None => PairMessage::Refused {
            message: DECLINED.into(),
        },
    };
    remote::announce(&core);
    finish(&mut writer, &connection, &outcome).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_loses_control_characters_and_length() {
        assert_eq!(
            clean("  Kishore's\u{7}\niPhone  ", NAME_LIMIT),
            "Kishore'siPhone"
        );
        assert_eq!(clean(&"x".repeat(200), NAME_LIMIT).len(), NAME_LIMIT);
    }
}
