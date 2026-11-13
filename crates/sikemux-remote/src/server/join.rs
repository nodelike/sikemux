//! The host's half of joining; [`crate::join`] describes the exchange.

use crate::server::remote_state::RemoteParts;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, SendStream};
use tokio::sync::Semaphore;

use crate::join::{joiner_name, Refusal};
use sikemux_client::join::{self, JoinHello, JoinReply, APPROVAL_TIMEOUT};
use sikemux_wire::protocol::{DeviceAccess, PendingDevice};

use super::notify::Signal;
use super::remote;
use super::Core;

/// A phone sends its ticket as soon as it connects, so reading one gets only
/// a moment.
const HELLO: Duration = Duration::from_secs(5);
/// Time for the phone to read the answer before the connection closes.
const LINGER: Duration = Duration::from_secs(2);
pub const NAME_LIMIT: usize = 64;
pub const PLATFORM_LIMIT: usize = 16;
pub struct Joins {
    /// Connections whose ticket is still being read. Past this, more are turned
    /// away before they cost anything.
    reading: Semaphore,
    /// Phones with a good ticket waiting for the person, who adds one at a time.
    /// Only a ticket the account issued for this host takes one of these.
    waiting: Semaphore,
}

impl Default for Joins {
    fn default() -> Self {
        Self {
            reading: Semaphore::const_new(32),
            waiting: Semaphore::const_new(4),
        }
    }
}

pub fn clean(text: &str, limit: usize) -> String {
    let kept: String = text
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect();
    kept.trim().to_owned()
}

async fn finish(writer: &mut SendStream, connection: &Connection, reply: &JoinReply) {
    let _ = join::send(writer, reply).await;
    let _ = writer.finish();
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

async fn refuse(writer: &mut SendStream, connection: &Connection, phone: &str, refusal: Refusal) {
    eprintln!(
        "sikemux core: refused a join ticket from {}: {}",
        phone.get(..8).unwrap_or(phone),
        refusal.reason()
    );
    let reply = JoinReply::Refused {
        reason: refusal.reason().into(),
    };
    finish(writer, connection, &reply).await;
}

pub async fn serve(core: Arc<Core>, connection: Connection) {
    let phone = connection.remote_id().to_string();
    let (mut writer, hello) = {
        let Ok(_reading) = core.joins().reading.try_acquire() else {
            connection.close(0u32.into(), b"busy");
            return;
        };
        let read = async {
            let (writer, mut reader) = connection.accept_bi().await.ok()?;
            Some((writer, join::receive::<JoinHello>(&mut reader, HELLO).await))
        };
        let Ok(Some(read)) = tokio::time::timeout(HELLO, read).await else {
            return;
        };
        read
    };
    let Ok(hello) = hello else {
        refuse(&mut writer, &connection, &phone, Refusal::Unreadable).await;
        return;
    };
    match core.remote().check_join(&hello.ticket, &phone) {
        Err(refusal) => {
            refuse(&mut writer, &connection, &phone, refusal).await;
            return;
        }
        Ok(Some(access)) => {
            finish(&mut writer, &connection, &JoinReply::Allowed { access }).await;
            return;
        }
        Ok(None) => {}
    }
    let request = PendingDevice {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: phone,
        name: clean(&joiner_name(&hello), NAME_LIMIT),
        platform: clean(&hello.platform, PLATFORM_LIMIT),
        from_account: true,
        expires_at: sikemux_process::clock::epoch_ms() + APPROVAL_TIMEOUT.as_millis() as u64,
    };
    if !core.remote().requires_approval_of(&request.device_id) {
        let reply = match core.remote().admit(
            &request,
            hello.ticket.account.clone(),
            hello.ticket.via.clone(),
        ) {
            Ok(()) => JoinReply::Allowed {
                access: DeviceAccess::Full,
            },
            Err(error) => JoinReply::Refused {
                reason: error.to_string(),
            },
        };
        remote::announce(&core);
        finish(&mut writer, &connection, &reply).await;
        return;
    }
    let Ok(_waiting) = core.joins().waiting.try_acquire() else {
        connection.close(0u32.into(), b"busy");
        return;
    };
    core.remote().forget_device_requests(&request.device_id);
    let answered = core.remote().ask(
        request.clone(),
        hello.ticket.account.clone(),
        None,
        hello.ticket.via.clone(),
    );
    remote::announce(&core);
    core.notify().send(Signal::Pairing(request.clone()));
    // The person may answer after the device gave up waiting; it is let in
    // the next time it comes.
    let access = tokio::time::timeout(APPROVAL_TIMEOUT, answered)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    core.remote().forget_pending(&request.id);
    remote::announce(&core);
    core.notify()
        .send(Signal::PairingSettled { id: request.id });
    let reply = match access {
        Some(access) => JoinReply::Allowed { access },
        None => JoinReply::Denied,
    };
    finish(&mut writer, &connection, &reply).await;
}
