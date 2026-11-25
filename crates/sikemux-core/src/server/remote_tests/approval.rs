//! A host whose person asked to approve each new device neither links with a host the accounts
//! server lists nor seals a vault's key for it until they let its key in.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::SecretKey;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::{BuildIdentity, DeviceAccess, PendingDevice};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::accounts::vault::Vault;
use crate::server::hosts::account::relink;
use crate::server::parts::Parts;
use crate::server::remote;
use crate::server::remote_state::RemoteParts;
use crate::server::vault::{keys, random_key};
use crate::server::{Core, ServerCore};

const ACCOUNT: &str = "user_2approval";
const WAIT: Duration = Duration::from_secs(30);

type Asked = Arc<Mutex<Vec<(String, Value)>>>;

/// An accounts server that lists `hosts` and takes everything else, keeping what it was asked.
async fn accounts_server(hosts: Vec<String>) -> (String, Asked) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let asked = Asked::default();
    let devices: Vec<Value> = hosts
        .iter()
        .map(|key| {
            json!({ "key": key, "role": "host", "name": "Studio", "platform": "macos",
                "createdAt": "2026-11-25T00:00:00Z", "lastSeenAt": null })
        })
        .collect();
    let listed = json!({ "devices": devices }).to_string();
    tokio::spawn({
        let asked = asked.clone();
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (asked, listed) = (asked.clone(), listed.clone());
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let mut first = String::new();
                    stream.read_line(&mut first).await.unwrap();
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        stream.read_line(&mut line).await.unwrap();
                        let line = line.trim().to_lowercase();
                        if line.is_empty() {
                            break;
                        }
                        if let Some(value) = line.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).await.unwrap();
                    let request = first.rsplit_once(' ').unwrap().0.to_owned();
                    let (status, answer) = match request.as_str() {
                        "GET /v1/devices?role=host" => ("200 OK", listed),
                        path if path.ends_with("/join") => ("503 Service Unavailable", "{}".into()),
                        _ => ("200 OK", "{}".into()),
                    };
                    let body = serde_json::from_slice(&body).unwrap_or_default();
                    asked.locked().push((request, body));
                    let answer = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    stream.write_all(answer.as_bytes()).await.unwrap();
                });
            }
        }
    });
    (base, asked)
}

struct Host {
    core: Arc<Core>,
    secret: SecretKey,
    asked: Asked,
}

impl Host {
    /// A host on an account the server says has `others` as well.
    async fn new(others: &[&str], lock: bool) -> Self {
        let secret = SecretKey::generate();
        let hosts = std::iter::once(secret.public().to_string())
            .chain(others.iter().map(|key| (*key).to_owned()))
            .collect();
        let (base, asked) = accounts_server(hosts).await;
        let core = Core::new(BuildIdentity::default(), None).unwrap();
        core.remote()
            .stand_in(secret.clone(), Some(ACCOUNT), Vec::new());
        core.remote().listen_on_loopback();
        core.remote().set_accounts_api(&base);
        if lock {
            let setting = serde_json::from_value(json!({ "requireApproval": true })).unwrap();
            remote::set_require_approval(&core, setting).unwrap();
        }
        Self {
            core,
            secret,
            asked,
        }
    }

    fn me(&self) -> String {
        self.secret.public().to_string()
    }

    /// The hosts it asked the server for a ticket to link with.
    fn tickets(&self) -> Vec<String> {
        let asked = self.asked.locked();
        asked
            .iter()
            .filter(|(request, _)| request.ends_with("/join"))
            .map(|(_, body)| body["host"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Looks for hosts to link with, and waits for the pass to end.
    async fn look(&self) {
        let passes = |asked: &Asked| {
            let asked = asked.locked();
            asked
                .iter()
                .filter(|(r, _)| r.contains("role=host"))
                .count()
        };
        let before = passes(&self.asked);
        relink(&self.core);
        until("the host looked", || passes(&self.asked) > before).await;
        until("the pass ended", || {
            !self
                .core
                .hosts()
                .linking
                .load(std::sync::atomic::Ordering::Acquire)
        })
        .await;
    }

    async fn asks_about(&self, key: &str) -> PendingDevice {
        until("the host asked the person", || {
            !self.core.remote().pending_links().is_empty()
        })
        .await;
        let mut pending = self.core.remote().pending_links();
        assert_eq!(pending.len(), 1);
        let request = pending.remove(0);
        assert_eq!(request.device_id, key);
        request
    }

    /// The hosts it sealed a key for, over every request since the last call.
    fn sealed_for(&self) -> Vec<String> {
        let mut sealed: Vec<String> = std::mem::take(&mut *self.asked.locked())
            .iter()
            .flat_map(|(_, body)| body["keys"].as_array().cloned().unwrap_or_default())
            .map(|key| key["hostKey"].as_str().unwrap().to_owned())
            .collect();
        sealed.sort();
        sealed
    }

    /// Everyone a vault's key is sealed for, when the server lists `waiting` as without it and
    /// `holders` as holding it: as it is handed out, then as it is replaced.
    async fn vault_keys(&self, waiting: &[&str], holders: &[&str]) -> [Vec<String>; 2] {
        let key = random_key().unwrap();
        let vault: Vault = serde_json::from_value(json!({
            "generation": 1, "recoveryKey": SecretKey::generate().public().to_string(),
            "recoveryWrap": "", "key": null, "holders": holders, "waiting": waiting,
            "rotate": true, "entries": [],
        }))
        .unwrap();
        keys::hand_out(&self.core, ACCOUNT, &vault, &key)
            .await
            .unwrap();
        let handed = self.sealed_for();
        assert!(keys::replace(&self.core, ACCOUNT, &self.me(), &vault, &key)
            .await
            .unwrap());
        [handed, self.sealed_for()]
    }
}

async fn until(what: &str, done: impl Fn() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never: {what}"));
}

fn made_up() -> String {
    SecretKey::generate().public().to_string()
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    keys.sort();
    keys
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_host_links_with_a_listed_host_only_once_the_person_allows_it() {
    let other = made_up();
    let host = Host::new(&[&other], true).await;
    host.look().await;
    let request = host.asks_about(&other).await;
    assert_eq!(request.name, "Studio");
    assert!(host.tickets().is_empty());
    assert!(host.core.remote().links().is_empty());

    host.look().await;
    assert_eq!(host.core.remote().pending_links().len(), 1);
    assert!(host.tickets().is_empty());

    host.core
        .remote()
        .answer(&request.id, Some(DeviceAccess::Full))
        .unwrap();
    host.look().await;
    assert_eq!(host.tickets(), vec![other]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_person_denies_is_neither_linked_nor_asked_about_again() {
    let other = made_up();
    let host = Host::new(&[&other], true).await;
    host.look().await;
    let request = host.asks_about(&other).await;
    host.core.remote().answer(&request.id, None).unwrap();
    host.look().await;
    assert!(host.tickets().is_empty());
    assert!(host.core.remote().pending_links().is_empty());
    assert!(host.vault_keys(&[&other], &[]).await[0].is_empty());
}

/// Its card would outlive the question, and a late no would undo nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_card_goes_once_the_host_is_let_in_another_way() {
    let other = made_up();
    let host = Host::new(&[&other], true).await;
    host.look().await;
    host.asks_about(&other).await;
    host.core.remote().approve_host(&other).unwrap();
    assert!(host.core.remote().pending_links().is_empty());

    let host = Host::new(&[&other], true).await;
    host.look().await;
    host.asks_about(&other).await;
    let setting = serde_json::from_value(json!({ "requireApproval": false })).unwrap();
    remote::set_require_approval(&host.core, setting).unwrap();
    assert!(host.core.remote().pending_links().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlocked_host_links_with_a_listed_host_without_asking() {
    let other = made_up();
    let host = Host::new(&[&other], false).await;
    host.look().await;
    assert_eq!(host.tickets(), vec![other]);
    assert!(host.core.remote().pending_links().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_host_seals_the_vault_key_only_for_hosts_the_person_allowed() {
    let (new, held) = (made_up(), made_up());
    let host = Host::new(&[&new, &held], true).await;
    let me = host.me();
    let [handed, replaced] = host.vault_keys(&[&new], &[&me, &held]).await;
    assert!(handed.is_empty());
    assert_eq!(replaced, sorted(&[&me]));

    host.core.remote().approve_host(&new).unwrap();
    let [handed, replaced] = host.vault_keys(&[&new], &[&me, &held]).await;
    assert_eq!(handed, sorted(&[&new]));
    assert_eq!(replaced, sorted(&[&me, &new]));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlocked_host_seals_the_vault_key_for_every_host_on_the_account() {
    let (new, held) = (made_up(), made_up());
    let host = Host::new(&[&new, &held], false).await;
    let me = host.me();
    let [handed, replaced] = host.vault_keys(&[&new], &[&me, &held]).await;
    assert_eq!(handed, sorted(&[&new]));
    assert_eq!(replaced, sorted(&[&me, &new, &held]));
}
