use crate::accounts::live::{self};
use crate::accounts::protocol::AccountEvent;
use crate::join::Refusal;
use crate::server::access::Peer;
use crate::server::remote::account::*;
use crate::server::remote::listening::*;
use crate::server::remote_state::RemoteParts;
use crate::server::Core;
use iroh::SecretKey;
use sikemux_corekit::usage::remote_file as file_path;
use sikemux_process::clock::{epoch_ms, epoch_secs};
use sikemux_process::sync::Locked;
use sikemux_serve::server::host_key::{secret_from_hex, HostKey, Keeper};
use sikemux_wire::accounts::protocol::{JoinTicket, Network, RevokeReason};
use sikemux_wire::protocol::{
    AccountLinkState, DeviceAccess, DeviceInfo, PendingDevice, UpdateRequired,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::server::remote::*;

use crate::server::ServerCore;

/// Hosts that update keep the digest their last build took, over that
/// build's shape of the file; they come back with their account, devices
/// and links, and keep the canonical digest from then on.
#[test]
fn a_remote_file_from_an_earlier_build_stays_trusted() {
    for (file, digest) in [
        (
            include_str!("../fixtures/remote/nightly3.remote.json"),
            "5c7327e3e80c6089ca305449ee08a79bb12f01bed332c26cdd637d7b1f52e862",
        ),
        (
            include_str!("../fixtures/remote/nightly4.remote.json"),
            "42b087817bc6ada3d6e42d4295c06f7aef8c22abcf481f849a7f4609ebf9f7d2",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("core.sock");
        let path = file_path(&socket);
        std::fs::write(&path, file).unwrap();
        let keeper = Keeper::new(&socket, false).unwrap();
        keeper
            .save(&HostKey {
                secret: SecretKey::generate(),
                trusted: digest.into(),
                account: Some("user_2nightly".into()),
                retired: None,
                written: None,
            })
            .unwrap();
        for _ in 0..2 {
            let (stored, _, trusted) = load(&keeper, &path).unwrap();
            assert!(stored.enabled);
            assert_eq!(stored.owner.as_deref(), Some("user_2nightly"));
            assert_eq!(stored.devices.len(), 1);
            assert_eq!(stored.links.len(), 1);
            assert_eq!(stored.joined_through.len(), 1);
            assert_eq!(keeper.load().unwrap().unwrap().trusted, trusted);
        }
        std::fs::write(&path, file.replace("\"full\"", "\"watch\"")).unwrap();
        keeper
            .save(&HostKey {
                secret: SecretKey::generate(),
                trusted: digest.into(),
                account: Some("user_2nightly".into()),
                retired: None,
                written: None,
            })
            .unwrap();
        let (edited, _, _) = load(&keeper, &path).unwrap();
        assert!(edited.devices.is_empty() && !edited.enabled);
        assert_eq!(edited.owner.as_deref(), Some("user_2nightly"));
    }
}

/// The secret store moves ahead of the file before the file is written.
/// A host whose write failed, or that stopped in between, comes back with
/// its file trusted; once the file is written, its state before is not.
#[test]
fn a_file_behind_the_secret_store_stays_trusted_until_it_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("core.sock");
    let path = file_path(&socket);
    std::fs::write(
        &path,
        include_str!("../fixtures/remote/nightly4.remote.json"),
    )
    .unwrap();
    let keeper = Keeper::new(&socket, false).unwrap();
    keeper
        .save(&HostKey {
            secret: SecretKey::generate(),
            trusted: "42b087817bc6ada3d6e42d4295c06f7aef8c22abcf481f849a7f4609ebf9f7d2".into(),
            account: Some("user_2nightly".into()),
            retired: None,
            written: None,
        })
        .unwrap();
    let (mut stored, key, written) = load(&keeper, &path).unwrap();
    stored.links.clear();
    keeper
        .save(&HostKey {
            trusted: stored.trusted().unwrap(),
            written: Some(written),
            ..key
        })
        .unwrap();
    let (stored, key, written) = load(&keeper, &path).unwrap();
    assert_eq!(stored.links.len(), 1);
    assert_eq!(stored.owner.as_deref(), Some("user_2nightly"));

    let before = std::fs::read(&path).unwrap();
    let core = Core::new(sikemux_wire::protocol::BuildIdentity::default(), None).unwrap();
    {
        let mut inner = core.remote().inner.locked();
        inner.path = Some(path.clone());
        inner.keeper = Some(Keeper::new(&socket, false).unwrap());
        inner.secret = Some(key.secret.clone());
        inner.bound = key.account.clone();
        inner.kept = (written.clone(), key.account);
        inner.written = written;
        inner.stored = stored;
    }
    core.remote()
        .change(|stored| {
            stored.links.clear();
            Ok(())
        })
        .unwrap();
    assert_eq!(keeper.load().unwrap().unwrap().written, None);
    std::fs::write(&path, before).unwrap();
    let (put_back, _, _) = load(&keeper, &path).unwrap();
    assert!(put_back.links.is_empty() && !put_back.enabled);
}

#[test]
fn the_devices_survive_a_write_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/core.sock.remote.json");
    let stored = Stored {
        secret_key: None,
        enabled: true,
        owner: Some("user_2abc".into()),
        account_event_id: 7,
        pending_leave: Some("user_2old".into()),
        removed: None,
        network: None,
        notifications: BTreeMap::new(),
        revoked: BTreeMap::new(),
        shares: Vec::new(),
        joined_through: BTreeMap::new(),
        links: Vec::new(),
        require_approval: false,
        removed_here: BTreeMap::new(),
        declined_links: BTreeSet::new(),
        devices: vec![DeviceInfo {
            id: SecretKey::generate().public().to_string(),
            name: "Phone".into(),
            platform: "ios".into(),
            access: DeviceAccess::Watch,
            paired_at: 1,
            last_seen: None,
            via: None,
        }],
    };
    write_stored(&path, &stored).unwrap();
    let read = read_stored(&path).unwrap();
    assert!(read.enabled);
    assert_eq!(read.devices, stored.devices);
    assert_eq!(read.owner.as_deref(), Some("user_2abc"));
    assert_eq!(read.account_event_id, 7);
    assert_eq!(read.pending_leave.as_deref(), Some("user_2old"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn host_requests_sign_the_text_the_server_checks() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../server/protocol/vectors/host-request.json"
    ))
    .unwrap();
    let text = |name: &str| vector[name].as_str().unwrap().to_owned();
    let request: crate::accounts::HostRequest = serde_json::from_value(vector.clone()).unwrap();
    assert_eq!(
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
            text("body").as_bytes()
        )),
        request.body_sha256
    );
    assert_eq!(request.message(), text("message"));
    let remote = Remote::default();
    remote.inner.locked().secret = secret_from_hex(&text("secretKey"));
    assert!(remote.sign_host_request(&request).is_err());
    let (key, signature) = remote.sign_with_key(|_| request.message()).unwrap();
    assert_eq!(key, text("key"));
    assert_eq!(signature, text("signature"));
    assert_eq!(
        format!("SikemuxHost {key}.{}.{signature}", request.time),
        text("authorization")
    );
}

/// The same vector the server's tests verify, so both sides sign and
/// check exactly the same text.
#[test]
fn registrations_sign_the_text_the_server_checks() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../server/protocol/vectors/registration.json"
    ))
    .unwrap();
    let text = |name: &str| vector[name].as_str().unwrap().to_owned();
    let remote = Remote::default();
    remote.inner.locked().secret = secret_from_hex(&text("secretKey"));
    let (key, signature) = remote
        .sign_registration(&text("nonce"), &text("userId"))
        .unwrap();
    assert_eq!(key, text("key"));
    assert_eq!(
        sikemux_client::accounts::registration_message(&text("nonce"), &text("userId"), &key),
        text("message")
    );
    assert_eq!(signature, text("signature"));
}

fn phone(name: &str) -> DeviceInfo {
    DeviceInfo {
        id: SecretKey::generate().public().to_string(),
        name: name.into(),
        platform: "ios".into(),
        access: DeviceAccess::Full,
        paired_at: 1,
        last_seen: None,
        via: None,
    }
}

fn event(id: i64, kind: &str, key: Option<&str>, role: &str, reason: &str) -> AccountEvent {
    let mut event = serde_json::json!({ "id": id, "type": kind, "at": "2026-10-03T00:00:00Z" });
    if let Some(key) = key {
        event["key"] = key.into();
        event["role"] = role.into();
        event["reason"] = reason.into();
    }
    serde_json::from_value(event).unwrap()
}

/// A core signed in to an account with `phones` paired, saving to `dir`
/// and listening only on loopback.
fn signed_in(dir: &Path, phones: &[&DeviceInfo]) -> Arc<Core> {
    let core = Core::new(sikemux_wire::protocol::BuildIdentity::default(), None).unwrap();
    {
        let mut inner = core.remote().inner.locked();
        inner.path = Some(dir.join("core.sock.remote.json"));
        inner.direct_only = true;
        inner.secret = Some(SecretKey::generate());
        inner.stored.owner = Some("user_2abc".into());
        inner.stored.devices = phones.iter().map(|phone| (*phone).clone()).collect();
    }
    core
}

fn devices(core: &Core) -> Vec<String> {
    core.remote()
        .status()
        .devices
        .into_iter()
        .map(|device| device.name)
        .collect()
}

#[tokio::test]
async fn a_phone_the_account_revoked_is_forgotten_and_its_request_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (kept, gone) = (phone("Kept"), phone("Gone"));
    let core = signed_in(dir.path(), &[&kept, &gone]);
    let _waiting = core.remote().ask(
        PendingDevice {
            id: "request-1".into(),
            device_id: gone.id.clone(),
            name: "Gone".into(),
            platform: "ios".into(),
            from_account: true,
            expires_at: epoch_ms() + 120_000,
        },
        "user_2abc".into(),
        None,
        None,
    );
    let events = [
        event(4, "device.revoked", Some(&gone.id), "client", "removed"),
        event(5, "device.added", Some("someone"), "client", "removed"),
        event(6, "added.later", None, "", ""),
    ];
    apply_events(&core, &events);
    apply_events(&core, &events);

    assert_eq!(devices(&core), vec!["Kept"]);
    assert!(core.remote().status().pending.is_empty());
    assert_eq!(core.remote().status().owner.as_deref(), Some("user_2abc"));
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert_eq!(saved.account_event_id, 6);
    assert_eq!(saved.devices, vec![kept]);
}

fn ticket_for(core: &Core, phone: &str, issued_at: u64) -> JoinTicket {
    let vector = crate::join::vector::vector();
    core.remote()
        .trust_join_key(&vector.ticket.key_id, vector.public);
    vector.signed(|ticket| {
        ticket.account = "user_2abc".into();
        ticket.host = core.remote().core_id().unwrap();
        ticket.phone = phone.into();
        ticket.issued_at = issued_at as i64;
        ticket.expires_at = issued_at as i64 + 600;
    })
}

fn revoked_at(id: i64, key: &str, at_ms: u64) -> AccountEvent {
    let mut event = event(id, "device.revoked", Some(key), "client", "signed_out");
    event.at = live::rfc3339(at_ms);
    event
}

#[tokio::test]
async fn a_join_ticket_issued_before_the_account_revoked_the_phone_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    let phone = SecretKey::generate().public().to_string();
    let issued = epoch_secs() - 30;
    let ticket = ticket_for(&core, &phone, issued);
    assert_eq!(core.remote().check_join(&ticket, &phone), Ok(None));

    apply_events(&core, &[revoked_at(1, &phone, issued * 1000 + 10_000)]);
    assert_eq!(
        core.remote().check_join(&ticket, &phone),
        Err(Refusal::Revoked)
    );
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert_eq!(saved.revoked.get(&phone), Some(&(issued * 1000 + 10_000)));

    let reissued = ticket_for(&core, &phone, issued + 20);
    assert_eq!(core.remote().check_join(&reissued, &phone), Ok(None));
}

#[tokio::test]
async fn revocations_too_old_to_matter_are_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    let (old, recent) = (
        SecretKey::generate().public().to_string(),
        SecretKey::generate().public().to_string(),
    );
    let now = epoch_ms();
    apply_events(
        &core,
        &[
            revoked_at(1, &old, now - crate::join::REVOCATION_MEMORY_MS - 1_000),
            revoked_at(2, &recent, now - 1_000),
        ],
    );
    let kept: Vec<String> = core
        .remote()
        .inner
        .locked()
        .stored
        .revoked
        .keys()
        .cloned()
        .collect();
    assert_eq!(kept, vec![recent]);
}

#[tokio::test]
async fn a_signed_out_host_refuses_every_join_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    let phone = SecretKey::generate().public().to_string();
    let ticket = ticket_for(&core, &phone, epoch_secs());
    core.remote().inner.locked().stored.owner = None;
    assert_eq!(
        core.remote().check_join(&ticket, &phone),
        Err(Refusal::SignedOut)
    );
}

#[tokio::test]
async fn a_paired_phone_with_a_ticket_keeps_the_access_it_has() {
    let dir = tempfile::tempdir().unwrap();
    let mut paired = phone("Paired");
    paired.access = DeviceAccess::Watch;
    let core = signed_in(dir.path(), &[&paired]);
    let ticket = ticket_for(&core, &paired.id, epoch_secs());
    assert_eq!(
        core.remote().check_join(&ticket, &paired.id),
        Ok(Some(DeviceAccess::Watch))
    );
}

#[tokio::test]
async fn the_account_letting_go_of_this_host_signs_it_out_and_keeps_its_phones() {
    let dir = tempfile::tempdir().unwrap();
    let kept = phone("Kept");
    let core = signed_in(dir.path(), &[&kept]);
    let own = core.remote().core_id().unwrap();
    apply_events(
        &core,
        &[event(9, "device.revoked", Some(&own), "host", "removed")],
    );

    let status = core.remote().status();
    assert_eq!(status.owner, None);
    assert_eq!(devices(&core), vec!["Kept"]);
    let account = status.account.unwrap();
    assert_eq!(account.state, AccountLinkState::Removed);
    assert_eq!(account.reason, Some(RevokeReason::Removed));

    apply_events(&core, &[event(10, "account.deleted", None, "", "")]);
    let account = core.remote().status().account.unwrap();
    assert_eq!(account.reason, Some(RevokeReason::Removed));
}

#[tokio::test]
async fn a_deleted_account_signs_the_host_out() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    apply_events(&core, &[event(3, "account.deleted", None, "", "")]);
    let status = core.remote().status();
    assert_eq!(status.owner, None);
    assert_eq!(
        status.account.unwrap().reason,
        Some(RevokeReason::AccountDeleted)
    );
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert!(saved.removed.is_some());
}

/// Against a stand-in for the accounts server: a phone removed elsewhere
/// is forgotten while connected, and signing out leaves the account.
#[tokio::test]
async fn the_live_connection_revokes_phones_and_signing_out_leaves() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_websockets::{Message, ServerBuilder};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (kept, gone) = (phone("Kept"), phone("Gone"));
    let core = signed_in(dir.path(), &[&kept, &gone]);
    core.remote().inner.locked().accounts_api = Some(base);
    ensure_live(&core);

    let (stream, _) = listener.accept().await.unwrap();
    let (_, mut socket) = ServerBuilder::new().accept(stream).await.unwrap();
    let send = |value: serde_json::Value| Message::text(value.to_string());
    let nonce = "ab".repeat(32);
    socket
        .send(send(serde_json::json!({ "type": "challenge", "nonce": nonce, "expiresAt": "2026-10-03T00:00:30Z" })))
        .await
        .unwrap();
    let hello = socket.next().await.unwrap().unwrap();
    let hello: serde_json::Value = serde_json::from_str(hello.as_text().unwrap()).unwrap();
    assert_eq!(hello["key"], core.remote().core_id().unwrap());
    socket
        .send(send(
            serde_json::json!({ "type": "ready", "latest": 1, "heartbeatMs": 25000 }),
        ))
        .await
        .unwrap();
    socket
        .send(send(serde_json::json!({ "type": "events", "events": [
            { "id": 1, "type": "device.revoked", "at": "2026-10-03T00:00:00Z", "key": gone.id, "role": "client", "reason": "signed_out" }
        ] })))
        .await
        .unwrap();
    let ack = socket.next().await.unwrap().unwrap();
    assert_eq!(ack.as_text(), Some(r#"{"type":"ack","id":1}"#));
    assert_eq!(devices(&core), vec!["Kept"]);
    assert_eq!(
        core.remote().status().account.unwrap().state,
        AccountLinkState::Live
    );

    let signing_out = tokio::spawn({
        let core = core.clone();
        async move { set_owner(&core, None, &Peer::App).await }
    });
    let leave = socket.next().await.unwrap().unwrap();
    assert_eq!(leave.as_text(), Some(r#"{"type":"leave"}"#));
    socket
        .send(send(
            serde_json::json!({ "type": "revoked", "reason": "signed_out" }),
        ))
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(30), signing_out)
        .await
        .expect("signing out waits only for the server")
        .unwrap()
        .unwrap();
    assert_eq!(status.owner, None);
    tokio::time::timeout(Duration::from_secs(30), async {
        while core.remote().inner.locked().stored.pending_leave.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the account lets the host go");
    assert_eq!(core.remote().status().account, None);
    assert_eq!(devices(&core), vec!["Kept"]);
}

/// A host whose key was in the file, on an account, takes the old host off
/// the account with that key, even when the first start is offline.
#[tokio::test]
async fn a_key_moved_out_of_the_file_leaves_the_account() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_websockets::{Message, ServerBuilder};

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("core.sock");
    let old = SecretKey::generate();
    std::fs::write(
        file_path(&socket_path),
        serde_json::json!({
            "secretKey": hex::encode(old.to_bytes()),
            "enabled": true,
            "devices": [],
            "owner": "user_2abc",
        })
        .to_string(),
    )
    .unwrap();
    let retired = || {
        Keeper::new(&socket_path, false)
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .retired
            .map(|key| key.public())
    };

    let offline = Core::new(sikemux_wire::protocol::BuildIdentity::default(), None).unwrap();
    start(
        &offline,
        &socket_path,
        true,
        false,
        Some("http://127.0.0.1:9".into()),
    )
    .await;
    assert_eq!(retired(), Some(old.public()));
    let file = std::fs::read_to_string(file_path(&socket_path)).unwrap();
    assert!(!file.contains("secretKey"));
    assert_ne!(
        offline.remote().core_id().unwrap(),
        old.public().to_string()
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let core = Core::new(sikemux_wire::protocol::BuildIdentity::default(), None).unwrap();
    start(&core, &socket_path, true, false, Some(base)).await;
    let (stream, _) = listener.accept().await.unwrap();
    let (_, mut socket) = ServerBuilder::new().accept(stream).await.unwrap();
    let send = |value: serde_json::Value| Message::text(value.to_string());
    socket
        .send(send(serde_json::json!({ "type": "challenge", "nonce": "ab".repeat(32), "expiresAt": "2026-10-03T00:00:30Z" })))
        .await
        .unwrap();
    let hello = socket.next().await.unwrap().unwrap();
    let hello: serde_json::Value = serde_json::from_str(hello.as_text().unwrap()).unwrap();
    assert_eq!(hello["key"], old.public().to_string());
    socket
        .send(send(
            serde_json::json!({ "type": "ready", "latest": 1, "heartbeatMs": 25000 }),
        ))
        .await
        .unwrap();
    let leave = socket.next().await.unwrap().unwrap();
    assert_eq!(leave.as_text(), Some(r#"{"type":"leave"}"#));
    socket
        .send(send(
            serde_json::json!({ "type": "revoked", "reason": "signed_out" }),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while retired().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the retired key is forgotten once the account lets it go");
    assert!(core.remote().inner.locked().retired.is_none());
}

#[tokio::test]
async fn signing_out_offline_keeps_the_leave_for_the_next_connection() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    core.remote().inner.locked().accounts_api = Some("http://127.0.0.1:9".into());
    let status = set_owner(&core, None, &Peer::App).await.unwrap();
    assert_eq!(status.owner, None);
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert_eq!(saved.pending_leave.as_deref(), Some("user_2abc"));

    set_owner(&core, Some("user_2new".into()), &Peer::App)
        .await
        .unwrap();
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert_eq!(saved.pending_leave, None);
    stop_live(&core);
}

fn network_allowing(version: &str) -> Network {
    let any = serde_json::json!({ "nightly": "0.0.0", "stable": "0.0.0" });
    serde_json::from_value(serde_json::json!({
        "relays": [{ "url": "https://relay.example/", "region": "test", "quicPort": null }],
        "minimumVersions": {
            "macos": { "nightly": version, "stable": version },
            "linux": { "nightly": version, "stable": version },
            "ios": any,
            "android": any,
        },
    }))
    .unwrap()
}

#[tokio::test]
async fn a_build_older_than_the_server_allows_stays_off_until_it_is_allowed_again() {
    let dir = tempfile::tempdir().unwrap();
    let build = sikemux_wire::protocol::BuildIdentity {
        version: "0.5.0-nightly.1".into(),
        ..Default::default()
    };
    let core = Core::new(build, None).unwrap();
    {
        let mut inner = core.remote().inner.locked();
        inner.path = Some(dir.path().join("core.sock.remote.json"));
        inner.secret = Some(SecretKey::generate());
        inner.direct_only = true;
        inner.accounts_api = Some("http://127.0.0.1:9".into());
        inner.stored.owner = Some("user_2abc".into());
    }
    set_enabled(&core, true).await.unwrap();
    ensure_live(&core);
    assert!(core.remote().inner.locked().live.is_some());

    apply_network(&core, network_allowing("0.5.0-nightly.2")).await;
    let status = core.remote().status();
    assert_eq!(
        status.update_required,
        Some(UpdateRequired {
            current: "0.5.0-nightly.1".into(),
            minimum: "0.5.0-nightly.2".into(),
        })
    );
    assert!(status.enabled, "the switch stays on for after the update");
    assert!(core.remote().inner.locked().running.is_none());
    assert!(core.remote().inner.locked().live.is_none());
    assert!(set_enabled(&core, true).await.is_err());
    ensure_live(&core);
    assert!(core.remote().inner.locked().live.is_none());
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert_eq!(
        saved.network.unwrap().relays[0].url,
        "https://relay.example/"
    );
    assert_eq!(
        core.remote().inner.locked().relays[0].url,
        "https://relay.example/"
    );

    apply_network(&core, network_allowing("0.4.0")).await;
    assert_eq!(core.remote().status().update_required, None);
    assert!(core.remote().inner.locked().running.is_some());
    assert!(core.remote().inner.locked().live.is_some());
    stop(&core).await;
    stop_live(&core);
}

#[tokio::test]
async fn remote_access_follows_the_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    core.remote().inner.locked().stored.owner = None;
    let refused = set_enabled(&core, true).await.unwrap_err();
    assert_eq!(refused.to_string(), SIGNED_OUT);
    assert!(!core.remote().is_enabled());

    let status = set_owner(&core, Some("user_2abc".into()), &Peer::App)
        .await
        .unwrap();
    assert!(status.enabled, "signing in turns remote access on");
    assert!(core.remote().inner.locked().running.is_some());

    let status = set_owner(&core, None, &Peer::App).await.unwrap();
    assert!(!status.enabled, "signing out turns remote access off");
    assert!(core.remote().inner.locked().running.is_none());
    let saved = read_stored(&dir.path().join("core.sock.remote.json")).unwrap();
    assert!(!saved.enabled);
}

#[tokio::test]
async fn the_account_letting_go_of_this_host_turns_remote_access_off() {
    let dir = tempfile::tempdir().unwrap();
    let core = signed_in(dir.path(), &[]);
    set_enabled(&core, true).await.unwrap();
    let own = core.remote().core_id().unwrap();
    apply_events(
        &core,
        &[event(1, "device.revoked", Some(&own), "host", "removed")],
    );
    assert!(!core.remote().status().enabled);
    assert!(core.remote().inner.locked().running.is_none());
}
