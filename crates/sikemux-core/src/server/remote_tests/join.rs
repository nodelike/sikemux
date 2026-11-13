use crate::server::remote;
use crate::server::remote_state::RemoteParts;
use crate::server::Core;
use sikemux_client::join::{self, JoinHello, JoinReply};
use sikemux_wire::protocol::PendingDevice;
use std::sync::Arc;

use crate::server::ServerCore;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, SecretKey};

use crate::join::vector::{vector, Vector};
use crate::server::join::*;
use sikemux_client::join::JOIN_ALPN;
use sikemux_wire::accounts::protocol::JoinTicket;
use sikemux_wire::protocol::ask;
use sikemux_wire::protocol::{AllowedDevice, BuildIdentity, DeviceAccess, RemoteStatus};

const ACCOUNT: &str = "user_2vectorTest";
const WAIT: Duration = Duration::from_secs(30);

struct Host {
    core: Arc<Core>,
}

async fn host(vector: &Vector) -> Host {
    let core = Core::new(BuildIdentity::default(), None).unwrap();
    core.remote()
        .stand_in(SecretKey::generate(), Some(ACCOUNT), Vec::new());
    core.remote().listen_on_loopback();
    core.remote()
        .trust_join_key(&vector.ticket.key_id, vector.public);
    remote::set_enabled(&core, true).await.unwrap();
    Host { core }
}

/// A host whose person asked to approve each new device.
async fn locked_host(vector: &Vector) -> Host {
    let host = host(vector).await;
    let setting = serde_json::from_value(serde_json::json!({ "requireApproval": true })).unwrap();
    remote::set_require_approval(&host.core, setting).unwrap();
    host
}

impl Host {
    fn status(&self) -> RemoteStatus {
        self.core.remote().status()
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
        .remote()
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

    let (client, mut events) = sikemux_client::remote::connect(&endpoint, host.addr())
        .await
        .expect("the joined phone connects like any paired device");
    let first = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
    assert!(
        matches!(
            first,
            Some(sikemux_client::client::ClientEvent::DeviceView(_))
        ),
        "the joined phone hears the host's view first, as a paired one does"
    );
    client
        .ask(ask::List)
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
        .remote()
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
    assert!(core.remote().allow(vec![device]).is_err());
    assert!(core.remote().status().devices.is_empty());
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
    host.core.remote().answer(&request.id, None).unwrap();
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
