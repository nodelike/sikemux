use std::sync::Mutex;

use futures_util::StreamExt;
use iroh::SecretKey;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_websockets::{CloseCode, Message, ServerBuilder, WebSocketStream};

use super::*;
use crate::accounts::socket::OVER_LIMIT_WAIT;

const WAIT: Duration = Duration::from_secs(30);

struct Fake {
    secret: SecretKey,
    want: Mutex<Want>,
    cursor: Mutex<i64>,
    applied: Mutex<Vec<i64>>,
    links: Mutex<Vec<Link>>,
    removed: Mutex<Option<Option<RevokeReason>>>,
    pushed: Mutex<Vec<(Push, PushResult)>>,
}

impl Fake {
    fn new(want: Want) -> Arc<Self> {
        Arc::new(Self {
            secret: SecretKey::generate(),
            want: Mutex::new(want),
            cursor: Mutex::new(0),
            applied: Mutex::new(Vec::new()),
            links: Mutex::new(Vec::new()),
            removed: Mutex::new(None),
            pushed: Mutex::new(Vec::new()),
        })
    }

    fn key(&self) -> String {
        self.secret.public().to_string()
    }

    fn applied(&self) -> Vec<i64> {
        self.applied.lock().unwrap().clone()
    }
}

impl Account for Fake {
    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        crate::accounts::check_live(nonce).ok()?;
        let message = crate::accounts::live_message(nonce, &self.key());
        Some((
            self.key(),
            hex::encode(self.secret.sign(message.as_bytes()).to_bytes()),
        ))
    }

    fn want(&self) -> Want {
        *self.want.lock().unwrap()
    }

    fn cursor(&self) -> i64 {
        *self.cursor.lock().unwrap()
    }

    fn apply(&self, events: &[AccountEvent]) {
        let mut cursor = self.cursor.lock().unwrap();
        for event in events {
            assert!(event.id > *cursor, "event {} applied twice", event.id);
            *cursor = event.id;
            self.applied.lock().unwrap().push(event.id);
        }
    }

    fn rewind(&self, latest: i64) {
        let mut cursor = self.cursor.lock().unwrap();
        if *cursor > latest {
            *cursor = 0;
        }
    }

    fn link(&self, link: Link) {
        self.links.lock().unwrap().push(link);
    }

    fn removed(&self, reason: Option<RevokeReason>) {
        *self.removed.lock().unwrap() = Some(reason);
        *self.want.lock().unwrap() = Want::Stop;
    }

    fn pushed(&self, push: Push, result: PushResult) {
        self.pushed.lock().unwrap().push((push, result));
    }
}

struct Server {
    listener: TcpListener,
}

struct Peer {
    socket: WebSocketStream<TcpStream>,
}

impl Server {
    async fn new() -> Self {
        Self {
            listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        }
    }

    fn base(&self) -> String {
        format!("http://{}", self.listener.local_addr().unwrap())
    }

    /// Accepts the next connection and checks its hello the way the server
    /// does.
    async fn greet(&self, account: &Fake) -> Peer {
        let (stream, _) = tokio::time::timeout(WAIT, self.listener.accept())
            .await
            .expect("the host connects")
            .unwrap();
        let (_, socket) = ServerBuilder::new().accept(stream).await.unwrap();
        let mut peer = Peer { socket };
        let nonce = hex::encode(uuid::Uuid::new_v4().as_bytes()).repeat(2);
        peer.send(
            json!({ "type": "challenge", "nonce": nonce, "expiresAt": "2026-10-03T00:00:30Z" }),
        )
        .await;
        let hello = peer.receive().await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["role"], "host");
        assert_eq!(hello["key"], account.key());
        assert!(hello.get("token").is_none());
        assert_eq!(hello["app"]["platform"], "macos");
        let message = crate::accounts::live_message(&nonce, &account.key());
        assert_eq!(
            hello["signature"],
            hex::encode(account.secret.sign(message.as_bytes()).to_bytes())
        );
        peer
    }
}

impl Peer {
    async fn send(&mut self, value: Value) {
        self.socket
            .send(Message::text(value.to_string()))
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(WAIT, self.socket.next())
                .await
                .expect("the host answers")
                .expect("the connection stays open")
                .unwrap();
            if let Some(text) = message.as_text() {
                return serde_json::from_str(text).unwrap();
            }
        }
    }

    async fn ready(&mut self, latest: i64) {
        self.send(json!({ "type": "ready", "latest": latest, "heartbeatMs": 25000 }))
            .await;
    }

    async fn events(&mut self, ids: &[i64]) {
        let events: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "id": id, "type": "device.revoked", "at": "2026-10-03T00:00:00Z", "key": "k", "role": "client", "reason": "removed" }))
            .collect();
        self.send(json!({ "type": "events", "events": events }))
            .await;
    }

    async fn close(mut self, code: u16) {
        let _ = self
            .socket
            .send(Message::close(Some(CloseCode::try_from(code).unwrap()), ""))
            .await;
        let _ = tokio::time::timeout(WAIT, self.socket.next()).await;
    }
}

fn spawn(account: &Arc<Fake>, server: &Server) -> (JoinHandle<()>, watch::Sender<()>) {
    spawn_with(account, server, Arc::default())
}

fn spawn_with(
    account: &Arc<Fake>,
    server: &Server,
    outbox: Arc<Outbox>,
) -> (JoinHandle<()>, watch::Sender<()>) {
    let (changed, watching) = watch::channel(());
    let app = LiveApp {
        platform: crate::accounts::protocol::Platform::Macos,
        version: "0.0.0-test".into(),
    };
    let task = tokio::spawn(run(
        account.clone(),
        url(&server.base()),
        app,
        outbox,
        watching,
    ));
    (task, changed)
}

type JoinHandle<T> = tokio::task::JoinHandle<T>;

async fn finished(task: JoinHandle<()>) {
    tokio::time::timeout(WAIT, task)
        .await
        .expect("the connection ends")
        .unwrap();
}

#[test]
fn urls_follow_the_api_scheme() {
    assert_eq!(
        url("https://api.sikemux.com"),
        "wss://api.sikemux.com/v1/live"
    );
    assert_eq!(url("http://127.0.0.1:4000/"), "ws://127.0.0.1:4000/v1/live");
}

#[tokio::test]
async fn a_host_proves_its_key_applies_each_event_once_and_acknowledges_it() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, _changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(3).await;
    peer.send(json!({ "type": "ping" })).await;
    assert_eq!(peer.receive().await, json!({ "type": "pong" }));
    peer.events(&[2, 1]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 2 }));
    peer.send(json!({ "type": "added_later", "anything": true }))
        .await;
    peer.events(&[2, 3]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 3 }));
    assert_eq!(account.applied(), vec![1, 2, 3]);
    assert_eq!(account.links.lock().unwrap().last(), Some(&Link::Live));

    peer.send(json!({ "type": "revoked", "reason": "removed" }))
        .await;
    finished(task).await;
    assert_eq!(
        *account.removed.lock().unwrap(),
        Some(Some(RevokeReason::Removed))
    );
}

#[tokio::test]
async fn a_host_back_from_a_restart_catches_up_from_where_it_was() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(1).await;
    peer.events(&[1]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 1 }));
    peer.send(json!({ "type": "bye", "reconnectAfterMs": 10 }))
        .await;
    peer.close(1012).await;

    let mut peer = server.greet(&account).await;
    assert_eq!(account.links.lock().unwrap().last(), Some(&Link::Offline));
    peer.ready(2).await;
    peer.events(&[1, 2]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 2 }));
    assert_eq!(account.applied(), vec![1, 2]);

    *account.want.lock().unwrap() = Want::Stop;
    changed.send(()).unwrap();
    finished(task).await;
    assert_eq!(*account.removed.lock().unwrap(), None);
}

#[tokio::test]
async fn a_host_removed_while_away_gets_its_backlog_then_lets_go() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, _changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.events(&[4, 5]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 5 }));
    peer.send(json!({ "type": "revoked", "reason": "account_deleted" }))
        .await;
    finished(task).await;
    assert_eq!(account.applied(), vec![4, 5]);
    assert_eq!(
        *account.removed.lock().unwrap(),
        Some(Some(RevokeReason::AccountDeleted))
    );
}

#[tokio::test]
async fn a_key_the_server_has_forgotten_signs_the_host_out() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, _changed) = spawn(&account, &server);
    server.greet(&account).await.close(4401).await;
    finished(task).await;
    assert_eq!(*account.removed.lock().unwrap(), Some(None));
}

#[tokio::test]
async fn a_host_over_the_servers_limits_waits_out_the_minute() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (_changed, mut watching) = watch::channel(());
    let app = LiveApp {
        platform: crate::accounts::protocol::Platform::Macos,
        version: "0.0.0-test".into(),
    };
    let url = url(&server.base());
    let outbox = Outbox::default();
    let ended = session(&*account, &url, &app, &outbox, &mut watching);
    let refused = async {
        let mut peer = server.greet(&account).await;
        peer.ready(0).await;
        peer.close(4429).await;
    };
    let (ended, ()) = tokio::join!(ended, refused);
    let Ended::Retry { hint, .. } = ended else {
        panic!("the host gives up instead of retrying");
    };
    assert!(hint.is_some_and(|wait| wait >= OVER_LIMIT_WAIT));
}

#[tokio::test]
async fn the_network_coming_back_cuts_a_waiting_host_short() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    peer.close(4429).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    changed.send(()).unwrap();
    let back = tokio::time::timeout(Duration::from_secs(5), server.greet(&account)).await;
    assert!(
        back.is_ok(),
        "the host waits out its backoff instead of connecting now"
    );
    task.abort();
}

#[tokio::test]
async fn signing_out_sends_leave_once_the_server_is_ready() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let (task, changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    peer.send(json!({ "type": "ping" })).await;
    assert_eq!(peer.receive().await, json!({ "type": "pong" }));

    *account.want.lock().unwrap() = Want::Leave;
    changed.send(()).unwrap();
    assert_eq!(peer.receive().await, json!({ "type": "leave" }));
    peer.send(json!({ "type": "revoked", "reason": "signed_out" }))
        .await;
    finished(task).await;
    assert_eq!(
        *account.removed.lock().unwrap(),
        Some(Some(RevokeReason::SignedOut))
    );
}

#[tokio::test]
async fn a_leave_left_from_before_goes_out_on_the_next_connection() {
    let server = Server::new().await;
    let account = Fake::new(Want::Leave);
    let (task, _changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    assert_eq!(peer.receive().await, json!({ "type": "leave" }));
    peer.send(json!({ "type": "revoked", "reason": "signed_out" }))
        .await;
    finished(task).await;
}

#[tokio::test]
async fn a_cursor_beyond_the_servers_history_starts_over() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    *account.cursor.lock().unwrap() = 900;
    let (task, changed) = spawn(&account, &server);
    let mut peer = server.greet(&account).await;
    peer.ready(2).await;
    peer.events(&[1, 2]).await;
    assert_eq!(peer.receive().await, json!({ "type": "ack", "id": 2 }));
    assert_eq!(account.applied(), vec![1, 2]);
    *account.want.lock().unwrap() = Want::Stop;
    changed.send(()).unwrap();
    finished(task).await;
}

fn push(to: &str, card: &str, expires_at: u64) -> Push {
    Push {
        to: to.into(),
        kind: PushKind::Alert,
        collapse_id: card.repeat(32),
        blob: "AAAA".into(),
        expires_at,
    }
}

fn later() -> u64 {
    epoch_ms() + 60_000
}

#[test]
fn expiry_reads_as_an_rfc3339_time() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(1_791_003_600_123), "2026-10-03T05:00:00.123Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn server_times_read_back_to_the_millisecond() {
    for ms in [0, 1_791_003_600_123, 951_782_400_000, 4_102_444_799_999] {
        assert_eq!(parse_rfc3339(&rfc3339(ms)), Some(ms));
    }
    assert_eq!(
        parse_rfc3339("2026-10-03T05:00:00Z"),
        Some(1_791_003_600_000)
    );
    assert_eq!(
        parse_rfc3339("2026-10-03T05:00:00.5Z"),
        Some(1_791_003_600_500)
    );
    for bad in [
        "",
        "2026-10-03",
        "2026-10-03T05:00:00",
        "2026-13-03T05:00:00Z",
        "2026-10-03T05:00:00.x1Z",
        "2026-10-03T05:+0:00Z",
    ] {
        assert_eq!(parse_rfc3339(bad), None, "{bad}");
    }
}

#[test]
fn a_newer_alert_for_the_same_card_replaces_one_still_waiting() {
    let outbox = Outbox::default();
    outbox.post(push("phone", "a", later()));
    outbox.post(push("phone", "b", later()));
    outbox.post(Push {
        blob: "BBBB".into(),
        ..push("phone", "a", later())
    });
    outbox.post(push("phone", "c", epoch_ms() - 1));
    let waiting = outbox.waiting();
    assert_eq!(waiting.len(), 3);
    assert_eq!(waiting[0].collapse_id, "b".repeat(32));
    assert_eq!(waiting[1].blob, "BBBB");
    assert!(outbox.withdraw("phone", &"b".repeat(32)));
    assert!(!outbox.withdraw("phone", &"b".repeat(32)));
    let taken = outbox.take();
    assert_eq!(taken.len(), 1, "an expired push never goes out");
    assert_eq!(taken[0].r#ref, 1);
}

#[tokio::test]
async fn pushes_go_out_with_rising_refs_and_their_answers_come_back() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let outbox = Arc::new(Outbox::default());
    outbox.post(push("phone", "a", later()));
    let (task, changed) = spawn_with(&account, &server, outbox.clone());
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    let first = peer.receive().await;
    assert_eq!(first["type"], "push");
    assert_eq!(first["ref"], 1);
    assert_eq!(first["to"], "phone");
    assert_eq!(first["kind"], "alert");
    assert_eq!(first["collapseId"], "a".repeat(32));
    assert_eq!(first["blob"], "AAAA");
    assert!(first["expiresAt"].as_str().unwrap().ends_with('Z'));

    outbox.post(Push {
        kind: PushKind::Clear,
        ..push("phone", "b", later())
    });
    let second = peer.receive().await;
    assert_eq!(second["ref"], 2);
    assert_eq!(second["kind"], "clear");

    peer.send(json!({ "type": "pushed", "ref": 2, "result": "no_token" }))
        .await;
    peer.send(json!({ "type": "pushed", "ref": 99, "result": "sent" }))
        .await;
    peer.send(json!({ "type": "pushed", "ref": 1, "result": "sent" }))
        .await;
    peer.send(json!({ "type": "ping" })).await;
    assert_eq!(peer.receive().await, json!({ "type": "pong" }));
    let pushed = account.pushed.lock().unwrap().clone();
    assert_eq!(pushed.len(), 2);
    assert_eq!(pushed[0].0.collapse_id, "b".repeat(32));
    assert_eq!(pushed[0].1, PushResult::NoToken);
    assert_eq!(pushed[1].0.collapse_id, "a".repeat(32));
    assert_eq!(pushed[1].1, PushResult::Sent);

    *account.want.lock().unwrap() = Want::Stop;
    changed.send(()).unwrap();
    finished(task).await;
}

#[tokio::test]
async fn a_push_left_unanswered_goes_out_again_on_the_next_connection() {
    let server = Server::new().await;
    let account = Fake::new(Want::Stay);
    let outbox = Arc::new(Outbox::default());
    let (task, changed) = spawn_with(&account, &server, outbox.clone());
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    peer.send(json!({ "type": "ping" })).await;
    assert_eq!(peer.receive().await, json!({ "type": "pong" }));
    outbox.post(push("phone", "a", later()));
    assert_eq!(peer.receive().await["ref"], 1);
    peer.send(json!({ "type": "bye", "reconnectAfterMs": 10 }))
        .await;
    peer.close(1012).await;

    outbox.post(push("phone", "b", later()));
    let mut peer = server.greet(&account).await;
    peer.ready(0).await;
    let again = peer.receive().await;
    assert_eq!(again["ref"], 2);
    assert_eq!(again["collapseId"], "a".repeat(32));
    let newer = peer.receive().await;
    assert_eq!(newer["ref"], 3);
    assert_eq!(newer["collapseId"], "b".repeat(32));

    *account.want.lock().unwrap() = Want::Stop;
    changed.send(()).unwrap();
    finished(task).await;
}
