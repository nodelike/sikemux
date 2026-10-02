//! The phone's client against a real core on this machine, over loopback.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::protocol::{BuildIdentity, ChatLauncher, ProjectInfo};
use sikemux_core::server::{self, ServerConfig};

use super::*;

const WAIT: Duration = Duration::from_secs(10);

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
                if let Ok((client, _events)) = CoreClient::connect(&socket).await {
                    let _ = client.shutdown(true).await;
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
        "secretKey": hex::encode(core_key.to_bytes()),
        "enabled": true,
        "devices": devices,
    });
    std::fs::write(remote_file(&socket), serde_json::to_vec(&stored).unwrap()).unwrap();
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
        key,
        online: Mutex::new(Online {
            endpoint,
            generation: 0,
            reached: HashSet::new(),
        }),
        renewing: tokio::sync::Mutex::new(()),
    }
}

/// Keeps the views it hears, and throws back every batch the way a broken
/// screen would.
#[derive(Default)]
struct Views(Mutex<Vec<DeviceView>>);

impl CoreListener for Views {
    fn output(&self, _session: u64, _bytes: Vec<u8>) -> Result<(), ListenerError> {
        Ok(())
    }

    fn events(&self, events: Vec<CoreEvent>) -> Result<(), ListenerError> {
        for event in events {
            if let CoreEvent::View { view } = event {
                self.0.lock().unwrap().push(view);
            }
        }
        Err(ListenerError::Failed {
            message: "the app threw".into(),
        })
    }

    fn closed(&self) -> Result<(), ListenerError> {
        Ok(())
    }
}

fn block_on<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(work)
}

fn launcher() -> ChatLauncher {
    ChatLauncher {
        id: "opencode".into(),
        provider: "opencode".into(),
        label: "OpenCode".into(),
        program: "/bin/true".into(),
        args: Vec::new(),
        env: Default::default(),
        permission_mode: "default".into(),
    }
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
            .connect_to(core_key.public().to_string(), addr, views.clone())
            .await
            .expect("the phone connects");

        let project = ProjectInfo {
            id: "p".into(),
            name: "Project".into(),
            path: std::env::temp_dir(),
        };
        app.publish_workspace(vec![project], vec![launcher()])
            .await
            .expect("publish");
        let deadline = Instant::now() + WAIT;
        while !views.0.lock().unwrap().iter().any(|view| {
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
        assert!(connection.host().await.is_ok());
        assert!(matches!(
            connection.save_backdrop("/tmp".into(), "none".into()).await,
            Ok(None)
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
        let refused = device
            .connect_to(
                core_key.public().to_string(),
                addr,
                Arc::new(Views::default()),
            )
            .await;
        assert!(matches!(refused, Err(MobileError::Unpaired)));
    });
}
