//! The phone's client against a real core on this machine, over loopback.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::protocol::BuildIdentity;
use sikemux_core::server::{self, ServerConfig};

use iroh::endpoint::presets;
use iroh::Endpoint;

use super::*;

const WAIT: Duration = Duration::from_secs(30);

struct TestCore {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    thread: Option<std::thread::JoinHandle<Result<(), server::ServerError>>>,
}

impl Drop for TestCore {
    fn drop(&mut self) {
        let socket = self.socket.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(async {
                let deadline = Instant::now() + WAIT;
                while Instant::now() < deadline {
                    if let Ok((client, _events)) = CoreClient::connect(&socket).await {
                        if client.shutdown(true).await.is_ok() {
                            return;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
        })
        .join();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn remote_file(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

fn start_core(core_key: &SecretKey, phone: Option<&SecretKey>) -> TestCore {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("core.sock");
    let devices: Vec<_> = phone
        .map(|key| {
            json!({
                "id": key.public().to_string(),
                "name": "Phone",
                "platform": "ios",
                "access": "full",
                "pairedAt": 1,
                "lastSeen": null,
            })
        })
        .into_iter()
        .collect();
    let stored = json!({
        "enabled": true,
        "devices": devices,
    });
    std::fs::write(remote_file(&socket), serde_json::to_vec(&stored).unwrap()).unwrap();
    sikemux_core::server::trust_remote_file(&socket, core_key).unwrap();
    let config = ServerConfig {
        idle_exit: Duration::from_secs(600),
        build: BuildIdentity {
            version: "0.0.0-test".into(),
            ..BuildIdentity::default()
        },
        remote_direct_only: true,
        ..ServerConfig::new(socket.clone())
    };
    let thread = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    TestCore {
        _dir: dir,
        socket,
        thread: Some(thread),
    }
}

async fn core_addr(app: &CoreClient) -> EndpointAddr {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = app.remote_status().await.expect("remote status");
        if !status.addresses.is_empty() {
            let id = status.core_id.parse().expect("core id");
            return status
                .addresses
                .iter()
                .fold(EndpointAddr::new(id), |addr, address| {
                    addr.with_ip_addr(address.parse().expect("address"))
                });
        }
        assert!(Instant::now() < deadline, "remote access never listened");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn loopback_device(key: SecretKey) -> Device {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key.clone())
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .expect("loopback address")
        .bind()
        .await
        .expect("device endpoint");
    Device {
        online: Arc::new(sikemux_client::device::Device::on_endpoint(
            key,
            endpoint,
            Vec::new(),
        )),
        relays: Vec::new(),
    }
}

/// Keeps the views it hears, and throws back every batch the way a broken
/// screen would.
#[derive(Default)]
struct Views {
    views: Mutex<Vec<sikemux_wire::protocol::DeviceView>>,
    closed: AtomicBool,
}

impl Views {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl CoreListener for Views {
    fn events(&self, events: Vec<CoreEvent>) -> Result<(), ListenerError> {
        for event in events {
            if let CoreEvent::View { view_json } = event {
                let view = serde_json::from_str(&view_json).expect("the view is the core's JSON");
                self.views.lock().unwrap().push(view);
            }
        }
        Err(ListenerError::Failed {
            message: "the app threw".into(),
        })
    }

    fn closed(&self) -> Result<(), ListenerError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn block_on<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(work)
}

#[test]
fn a_phone_hears_the_mac_s_view_and_asks_it_typed_questions() {
    let core_key = SecretKey::generate();
    let phone_key = SecretKey::generate();
    let core = start_core(&core_key, Some(&phone_key));
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        let addr = core_addr(&app).await;
        let device = loopback_device(phone_key).await;
        let views = Arc::new(Views::default());
        let connection = device
            .connect_to(addr, views.clone())
            .await
            .expect("the phone connects");

        let project =
            serde_json::json!({ "id": "p", "name": "Project", "path": std::env::temp_dir() });
        app.call("addProject", project).await.expect("the project");
        let deadline = Instant::now() + WAIT;
        while !views.views.lock().unwrap().iter().any(|view| {
            view.workspace
                .projects
                .iter()
                .any(|project| project.name == "Project")
        }) {
            assert!(Instant::now() < deadline, "the phone never saw the project");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(connection.is_open(), "a throwing app keeps its connection");
        assert!(matches!(
            connection.attach_chat("nobody".into(), None).await,
            Ok(ChatAttachment::Missing)
        ));
        assert!(matches!(
            connection.chat_history("nobody".into(), "a run".into(), 5, 10).await,
            Err(MobileError::Refused { message }) if message.contains("not running")
        ));
        assert!(connection.host().await.is_ok());
        assert!(matches!(
            connection.save_backdrop("/tmp".into(), "none".into()).await,
            Ok(None)
        ));
        assert!(matches!(
            connection.attach_file("nobody".into(), "a.txt".into(), "text/plain".into(), b"a".to_vec()).await,
            Err(MobileError::Refused { message }) if message.contains("not running")
        ));
        assert!(matches!(
            connection.steer("nobody".into(), "and the tests".into(), Vec::new()).await,
            Err(MobileError::Refused { message }) if message.contains("not running")
        ));
        assert!(matches!(
            connection.stop_task("nobody".into(), "shell".into()).await,
            Err(MobileError::Refused { message }) if message.contains("not running")
        ));
        assert!(matches!(
            connection.set_permission_mode("nobody".into(), "bypass".into()).await,
            Err(MobileError::Refused { message }) if message.contains("not running")
        ));
        assert!(connection.stop_chat("nobody".into()).await.is_ok());
        assert!(matches!(
            connection.attach_file("nobody".into(), "big.bin".into(), String::new(), vec![0; 10 * 1024 * 1024 + 1]).await,
            Err(MobileError::Invalid { message }) if message.contains("10 MB")
        ));
    });
}

#[test]
fn a_phone_the_mac_does_not_know_is_told_it_is_unpaired() {
    let core_key = SecretKey::generate();
    let core = start_core(&core_key, None);
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        let addr = core_addr(&app).await;
        let device = loopback_device(SecretKey::generate()).await;
        let refused = device.connect_to(addr, Arc::new(Views::default())).await;
        assert!(matches!(refused, Err(MobileError::Unpaired)));
    });
}

fn join_vector() -> (String, SecretKey, SecretKey) {
    let vector: serde_json::Value =
        serde_json::from_str(include_str!("../../../server/protocol/vectors/join.json"))
            .expect("the vector is JSON");
    (
        vector["ticket"].to_string(),
        SecretKey::from_bytes(&[8; 32]),
        SecretKey::from_bytes(&[7; 32]),
    )
}

#[test]
fn a_join_ticket_reaches_the_host_and_its_refusal_comes_back() {
    let (ticket, core_key, phone_key) = join_vector();
    let core = start_core(&core_key, None);
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        let addr = core_addr(&app).await;
        let device = loopback_device(phone_key).await;
        let ticket = read_ticket(&ticket, &device.id(), &core_key.public().to_string())
            .expect("the vector names this phone and this host");
        let hello = JoinHello {
            ticket,
            name: "Pixel".into(),
            platform: "android".into(),
        };
        let answer = device.join_at(addr, hello).await;
        assert_eq!(
            answer.expect("the host answers"),
            JoinAnswer::Refused {
                reason: "unknown_key".into()
            }
        );
    });
}

async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// An agent that starts but never answers, so waking it waits.
fn slow_agent(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("slow-agent");
    std::fs::write(
        &path,
        "#!/bin/sh\n[ \"$1\" = acp ] && exec sleep 600\nexit 0\n",
    )
    .expect("the slow agent");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("runnable");
    path
}

#[test]
fn closing_a_connection_fails_what_waits_on_it_and_tells_the_app_nothing_more() {
    let core_key = SecretKey::generate();
    let phone_key = SecretKey::generate();
    let core = start_core(&core_key, Some(&phone_key));
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        app.register_window()
            .await
            .expect("the app opens its window");
        let addr = core_addr(&app).await;
        let project = serde_json::json!({ "id": "p", "path": std::env::temp_dir() });
        app.call("addProject", project).await.expect("the project");
        let accounts = serde_json::json!({
            "profiles": [{
                "id": "slow",
                "name": "Slow",
                "provider": "opencode",
                "accent": "#a277ff",
                "executablePath": slow_agent(core._dir.path()),
            }],
        });
        app.call("setAgentAccounts", accounts)
            .await
            .expect("the accounts");
        let deadline = Instant::now() + WAIT;
        while !app
            .workspace()
            .await
            .expect("the workspace")
            .launchers
            .iter()
            .any(|launcher| launcher.id == "opencode:slow")
        {
            assert!(Instant::now() < deadline, "the slow agent is never offered");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let agent = serde_json::json!({
            "id": "agent-sleepy",
            "provider": "opencode",
            "project": "p",
            "cwd": std::env::temp_dir(),
            "title": "opencode",
            "permissionMode": "bypass",
            "profileId": "slow",
            "asleep": true,
        });
        app.call("createAgent", serde_json::json!({ "agent": agent }))
            .await
            .expect("the agent");
        let device = loopback_device(phone_key).await;
        let views = Arc::new(Views::default());
        let connection = device
            .connect_to(addr, views.clone())
            .await
            .expect("the phone connects");

        let waiting = connection.clone();
        let wake = tokio::spawn(async move {
            let asked = Instant::now();
            (
                waiting.wake_chat("agent-sleepy".into()).await,
                asked.elapsed(),
            )
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        connection.close();
        let (woken, waited) = wake.await.expect("wake task");
        assert!(matches!(woken, Err(MobileError::Connection { .. })));
        assert!(
            waited < Duration::from_secs(5),
            "a closed connection does not wait out the request"
        );
        assert!(!connection.is_open());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!views.closed(), "the app closed it and hears nothing more");
    });
}

#[test]
fn a_renewed_endpoint_closes_the_connections_on_the_old_one_and_a_closed_phone_stays_off() {
    let core_key = SecretKey::generate();
    let phone_key = SecretKey::generate();
    let core = start_core(&core_key, Some(&phone_key));
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        let addr = core_addr(&app).await;
        let device = loopback_device(phone_key).await;
        let host = core_key.public();
        let views = Arc::new(Views::default());
        let _connection = device
            .connect_to(addr.clone(), views.clone())
            .await
            .expect("the phone connects");

        device.online.renew_after_failing(host, 0).await;
        until("the old endpoint's connection closing", || views.closed()).await;
        assert_eq!(device.online.generation(), 1);

        let _again = device
            .connect_to(addr.clone(), Arc::new(Views::default()))
            .await
            .expect("the phone connects from the fresh endpoint");
        device.close().await;
        device.online.renew_after_failing(host, 1).await;
        assert!(device.online.is_closed());
        assert_eq!(device.online.generation(), 1, "a closed phone never renews");
        assert!(matches!(
            device.connect_to(addr, Arc::new(Views::default())).await,
            Err(MobileError::Connection { .. })
        ));
    });
}

#[test]
fn a_host_reached_before_comes_back_in_one_try_when_the_endpoint_has_gone_stale() {
    let core_key = SecretKey::generate();
    let phone_key = SecretKey::generate();
    let core = start_core(&core_key, Some(&phone_key));
    block_on(async {
        let (app, _events) = CoreClient::connect(&core.socket).await.expect("app");
        let addr = core_addr(&app).await;
        let device = loopback_device(phone_key).await;
        let views = Arc::new(Views::default());
        let first = device
            .connect_to(addr.clone(), views.clone())
            .await
            .expect("the phone connects");
        let stale = device.online.endpoint().expect("online").0;
        stale.close().await;
        until("the stale endpoint's connection closing", || views.closed()).await;
        assert!(!first.is_open());

        let started = Instant::now();
        let again = device
            .connect_to(addr, Arc::new(Views::default()))
            .await
            .expect("the phone connects again from a fresh endpoint");
        assert!(again.is_open());
        assert_eq!(device.online.endpoint().expect("online").1, 1);
        assert!(
            started.elapsed() < Duration::from_secs(6 + 5),
            "took {:?}",
            started.elapsed()
        );
    });
}
