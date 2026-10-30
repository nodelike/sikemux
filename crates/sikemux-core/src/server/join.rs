//! The host's half of joining; [`crate::join`] describes the exchange.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, SendStream};
use tokio::sync::Semaphore;

use crate::join::{self, JoinHello, JoinReply, Refusal, APPROVAL_TIMEOUT};
use crate::protocol::{DeviceAccess, PendingDevice};

use super::notify::Signal;
use super::remote;
use super::Core;

/// A phone sends its ticket as soon as it connects, so reading one gets only
/// a moment.
const HELLO: Duration = Duration::from_secs(5);
/// Time for the phone to read the answer before the connection closes.
const LINGER: Duration = Duration::from_secs(2);
pub(super) const NAME_LIMIT: usize = 64;
pub(super) const PLATFORM_LIMIT: usize = 16;
/// Connections whose ticket is still being read. Past this, more are turned
/// away before they cost anything.
static READING: Semaphore = Semaphore::const_new(32);
/// Phones with a good ticket waiting for the person, who adds one at a time.
/// Only a ticket the account issued for this host takes one of these.
static WAITING: Semaphore = Semaphore::const_new(4);

pub(super) fn clean(text: &str, limit: usize) -> String {
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

pub(super) async fn serve(core: Arc<Core>, connection: Connection) {
    let phone = connection.remote_id().to_string();
    let (mut writer, hello) = {
        let Ok(_reading) = READING.try_acquire() else {
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
    match core.remote.check_join(&hello.ticket, &phone) {
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
        name: clean(&join::joiner_name(&hello), NAME_LIMIT),
        platform: clean(&hello.platform, PLATFORM_LIMIT),
        from_account: true,
        expires_at: sikemux_process::clock::epoch_ms() + APPROVAL_TIMEOUT.as_millis() as u64,
    };
    if !core.remote.requires_approval() {
        let reply = match core.remote.admit(&request, hello.ticket.account.clone()) {
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
    let Ok(_waiting) = WAITING.try_acquire() else {
        connection.close(0u32.into(), b"busy");
        return;
    };
    core.remote.forget_device_requests(&request.device_id);
    let answered = core
        .remote
        .ask(request.clone(), hello.ticket.account.clone(), None);
    remote::announce(&core);
    core.notify.send(Signal::Pairing(request.clone()));
    // The person may answer after the device gave up waiting; it is let in
    // the next time it comes.
    let access = tokio::time::timeout(APPROVAL_TIMEOUT, answered)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    core.remote.forget_pending(&request.id);
    remote::announce(&core);
    core.notify.send(Signal::PairingSettled { id: request.id });
    let reply = match access {
        Some(access) => JoinReply::Allowed { access },
        None => JoinReply::Denied,
    };
    finish(&mut writer, &connection, &reply).await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::endpoint::presets;
    use iroh::{Endpoint, EndpointAddr, SecretKey};

    use super::*;
    use crate::accounts::protocol::JoinTicket;
    use crate::join::vector::{vector, Vector};
    use crate::join::JOIN_ALPN;
    use crate::protocol::{AllowedDevice, BuildIdentity, DeviceAccess, RemoteStatus};

    const ACCOUNT: &str = "user_2vectorTest";
    const WAIT: Duration = Duration::from_secs(30);

    /// The core takes only a few joins at once, which these tests would exceed side by side.
    static ONE_HOST_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct Host {
        core: Arc<Core>,
        _turn: tokio::sync::MutexGuard<'static, ()>,
    }

    async fn host(vector: &Vector) -> Host {
        let turn = ONE_HOST_AT_A_TIME.lock().await;
        let core = Core::new(BuildIdentity::default(), None).unwrap();
        core.remote
            .stand_in(SecretKey::generate(), Some(ACCOUNT), Vec::new());
        core.remote.listen_on_loopback();
        core.remote
            .trust_join_key(&vector.ticket.key_id, vector.public);
        remote::set_enabled(&core, true).await.unwrap();
        Host { core, _turn: turn }
    }

    /// A host whose person asked to approve each new device.
    async fn locked_host(vector: &Vector) -> Host {
        let host = host(vector).await;
        let setting =
            serde_json::from_value(serde_json::json!({ "requireApproval": true })).unwrap();
        remote::set_require_approval(&host.core, setting).unwrap();
        host
    }

    impl Host {
        fn status(&self) -> RemoteStatus {
            self.core.remote.status()
        }

        fn addr(&self) -> EndpointAddr {
            let status = self.status();
            status.addresses.iter().fold(
                EndpointAddr::new(status.core_id.parse().unwrap()),
                |addr, address| addr.with_ip_addr(address.parse().unwrap()),
            )
        }

        async fn until_asked(&self) -> PendingDevice {
            tokio::time::timeout(WAIT, async {
                loop {
                    if let Some(request) = self.status().pending.first() {
                        return request.clone();
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the host never asked")
        }
    }

    async fn phone(key: &SecretKey) -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    fn now() -> i64 {
        sikemux_process::clock::epoch_secs() as i64
    }

    fn ticket(vector: &Vector, host: &Host, phone: &SecretKey) -> JoinTicket {
        vector.signed(|ticket| {
            ticket.account = ACCOUNT.into();
            ticket.host = host.status().core_id;
            ticket.phone = phone.public().to_string();
            ticket.issued_at = now() - 5;
            ticket.expires_at = ticket.issued_at + 600;
        })
    }

    fn hello(ticket: JoinTicket) -> JoinHello {
        JoinHello {
            ticket,
            name: "Pixel 8\u{7}".into(),
            platform: "android".into(),
        }
    }

    #[test]
    fn a_device_name_loses_control_characters_and_length() {
        assert_eq!(
            clean("  Kishore's\u{7}\niPhone  ", NAME_LIMIT),
            "Kishore'siPhone"
        );
        assert_eq!(clean(&"x".repeat(200), NAME_LIMIT).len(), NAME_LIMIT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_phone_with_a_ticket_joins_once_the_person_allows_it() {
        let vector = vector();
        let host = locked_host(&vector).await;
        let key = SecretKey::generate();
        let endpoint = phone(&key).await;
        let hello = hello(ticket(&vector, &host, &key));

        let joining = tokio::spawn({
            let (endpoint, addr, hello) = (endpoint.clone(), host.addr(), hello.clone());
            async move { join::join(&endpoint, addr, &hello).await }
        });
        let request = host.until_asked().await;
        assert!(request.from_account);
        assert_eq!(request.name, "Pixel 8");
        assert_eq!(request.platform, "android");
        assert_eq!(request.device_id, key.public().to_string());
        let waits = request
            .expires_at
            .saturating_sub(sikemux_process::clock::epoch_ms());
        assert!(waits > 580_000 && waits <= 600_000, "{waits}");
        host.core
            .remote
            .answer(&request.id, Some(DeviceAccess::Watch))
            .unwrap();

        let reply = joining.await.unwrap().unwrap();
        assert_eq!(
            reply,
            JoinReply::Allowed {
                access: DeviceAccess::Watch
            }
        );
        let status = host.status();
        assert!(status.pending.is_empty());
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].id, key.public().to_string());
        assert_eq!(status.devices[0].name, "Pixel 8");
        assert_eq!(status.devices[0].access, DeviceAccess::Watch);

        let (client, mut events) = crate::remote::connect(&endpoint, host.addr())
            .await
            .expect("the joined phone connects like any paired device");
        let first = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
        assert!(
            matches!(
                first,
                Some(crate::client::ClientEvent::Event(
                    crate::protocol::Event::DeviceView { .. }
                ))
            ),
            "the joined phone hears the host's view first, as a paired one does"
        );
        client
            .list()
            .await
            .expect("the joined phone lists sessions");

        let again = join::join(&endpoint, host.addr(), &hello).await.unwrap();
        assert_eq!(
            again,
            JoinReply::Allowed {
                access: DeviceAccess::Watch
            }
        );
        assert!(host.status().pending.is_empty());
        remote::stop(&host.core).await;
    }

    fn allowed(key: &SecretKey, access: DeviceAccess) -> AllowedDevice {
        AllowedDevice {
            id: key.public().to_string(),
            name: "Pixel 8".into(),
            platform: "android".into(),
            access,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_phone_with_a_ticket_from_the_account_joins_at_full_access_without_asking() {
        let vector = vector();
        let host = host(&vector).await;
        let key = SecretKey::generate();
        let endpoint = phone(&key).await;

        let hello = hello(ticket(&vector, &host, &key));
        let reply = join::join(&endpoint, host.addr(), &hello).await.unwrap();
        assert_eq!(
            reply,
            JoinReply::Allowed {
                access: DeviceAccess::Full
            }
        );
        let status = host.status();
        assert!(status.pending.is_empty());
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].access, DeviceAccess::Full);
        remote::stop(&host.core).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn allowing_a_phone_at_sign_in_answers_it_if_it_is_already_waiting() {
        let vector = vector();
        let host = locked_host(&vector).await;
        let key = SecretKey::generate();
        let endpoint = phone(&key).await;
        let hello = hello(ticket(&vector, &host, &key));
        let joining = tokio::spawn({
            let addr = host.addr();
            async move { join::join(&endpoint, addr, &hello).await }
        });
        host.until_asked().await;
        host.core
            .remote
            .allow(vec![allowed(&key, DeviceAccess::Watch)])
            .unwrap();

        assert_eq!(
            joining.await.unwrap().unwrap(),
            JoinReply::Allowed {
                access: DeviceAccess::Watch
            }
        );
        let status = host.status();
        assert!(status.pending.is_empty());
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].access, DeviceAccess::Watch);
        remote::stop(&host.core).await;
    }

    #[test]
    fn only_device_keys_can_be_allowed() {
        let core = Core::new(BuildIdentity::default(), None).unwrap();
        let mut device = allowed(&SecretKey::generate(), DeviceAccess::Full);
        device.id = "not-a-key".into();
        assert!(core.remote.allow(vec![device]).is_err());
        assert!(core.remote.status().devices.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_phone_the_person_denies_is_not_paired() {
        let vector = vector();
        let host = locked_host(&vector).await;
        let key = SecretKey::generate();
        let endpoint = phone(&key).await;
        let hello = hello(ticket(&vector, &host, &key));
        let joining = tokio::spawn({
            let addr = host.addr();
            async move { join::join(&endpoint, addr, &hello).await }
        });
        let request = host.until_asked().await;
        host.core.remote.answer(&request.id, None).unwrap();
        assert_eq!(joining.await.unwrap().unwrap(), JoinReply::Denied);
        assert!(host.status().devices.is_empty());
        remote::stop(&host.core).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_bad_ticket_is_refused_without_asking_anyone() {
        let vector = vector();
        let host = host(&vector).await;
        let key = SecretKey::generate();
        let endpoint = phone(&key).await;
        let someone_else = SecretKey::generate();
        let theirs = hello(ticket(&vector, &host, &someone_else));
        let reply = join::join(&endpoint, host.addr(), &theirs).await.unwrap();
        assert_eq!(
            reply,
            JoinReply::Refused {
                reason: "wrong_phone".into()
            }
        );

        let connection = endpoint.connect(host.addr(), JOIN_ALPN).await.unwrap();
        let (mut writer, mut reader) = connection.open_bi().await.unwrap();
        join::send(&mut writer, &serde_json::json!({ "hello": "there" }))
            .await
            .unwrap();
        let reply: JoinReply = join::receive(&mut reader, WAIT).await.unwrap();
        assert_eq!(
            reply,
            JoinReply::Refused {
                reason: "unreadable".into()
            }
        );
        let status = host.status();
        assert!(status.pending.is_empty());
        assert!(status.devices.is_empty());
        remote::stop(&host.core).await;
    }
}
