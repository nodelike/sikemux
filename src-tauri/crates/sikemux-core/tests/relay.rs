//! A phone reaching a core only through a relay, the way it does from outside
//! the host's network. Needs an `iroh-relay` binary, named by
//! `SIKEMUX_IROH_RELAY`; without it the test passes without running.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use serde_json::json;
use sikemux_core::accounts::network;
use sikemux_core::accounts::protocol::Relay;
use sikemux_core::client::{probe, ClientEvent, CoreClient};
use sikemux_core::protocol::{
    BuildIdentity, DeviceAccess, LaunchIdentity, SpawnTarget, TerminalSpawn,
};
use sikemux_core::remote::{self, SecretKey};
use sikemux_core::server::{self, ServerConfig};

const WAIT: Duration = Duration::from_secs(60);
/// A phone that dials before the core has joined the relay waits out QUIC's
/// retries, so it tries again on a new endpoint, the way the app does.
const ATTEMPT: Duration = Duration::from_secs(5);

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("a free port")
}

fn start_relay(binary: &Path, dir: &Path) -> (Running, String) {
    let port = free_port();
    let config = dir.join("relay.toml");
    std::fs::write(
        &config,
        format!(
            "http_bind_addr = \"127.0.0.1:{port}\"\nenable_quic_addr_discovery = false\nenable_metrics = false\naccess = \"everyone\"\n"
        ),
    )
    .expect("relay config");
    let child = sikemux_process::user_environment::command(binary)
        .arg("--dev")
        .arg("--config-path")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("iroh-relay starts");
    let running = Running(child);
    let deadline = Instant::now() + WAIT;
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the relay never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    (running, format!("http://127.0.0.1:{port}/"))
}

fn remote_file(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_reaches_a_core_through_the_relay_alone() {
    let Some(binary) = std::env::var_os("SIKEMUX_IROH_RELAY") else {
        eprintln!("SIKEMUX_IROH_RELAY is not set; skipping the relay test");
        return;
    };
    std::env::set_var("SHELL", "/bin/sh");
    let dir = tempfile::tempdir().expect("temp dir");
    let (_relay, url) = start_relay(Path::new(&binary), dir.path());
    let relays = vec![Relay {
        url: url.clone(),
        region: "test".into(),
        quic_port: None,
    }];

    let core_key = SecretKey::generate();
    let phone_key = SecretKey::generate();
    let socket = dir.path().join("core.sock");
    let any = json!({ "nightly": "0.0.0", "stable": "0.0.0" });
    let stored = json!({
        "secretKey": hex::encode(core_key.to_bytes()),
        "enabled": true,
        "devices": [{
            "id": phone_key.public().to_string(),
            "name": "Phone",
            "platform": "ios",
            "access": DeviceAccess::Full,
            "pairedAt": 1,
            "lastSeen": null,
        }],
        "network": {
            "relays": relays,
            "minimumVersions": { "macos": any, "linux": any, "ios": any, "android": any },
        },
    });
    std::fs::write(remote_file(&socket), stored.to_string()).expect("remote file");
    let config = ServerConfig {
        idle_exit: Duration::from_secs(600),
        build: BuildIdentity {
            version: "0.0.0-test".into(),
            ..BuildIdentity::default()
        },
        ..ServerConfig::new(socket.clone())
    };
    let core = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    let (app, _app_events) = CoreClient::connect(&socket).await.expect("app");

    let addr = EndpointAddr::new(core_key.public()).with_relay_url(url.parse().expect("relay"));
    let deadline = Instant::now() + WAIT;
    let (_phone, client, mut events) = loop {
        let phone = Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .relay_mode(RelayMode::Custom(network::relay_map(&relays)))
            .secret_key(phone_key.clone())
            .bind()
            .await
            .expect("phone endpoint");
        let attempt = tokio::time::timeout(ATTEMPT, remote::connect(&phone, addr.clone())).await;
        match attempt {
            Ok(Ok((client, events))) => break (phone, client, events),
            Ok(Err(error)) => assert!(
                Instant::now() < deadline,
                "the phone never reached the core through the relay: {error}"
            ),
            Err(_) => assert!(
                Instant::now() < deadline,
                "the phone never reached the core through the relay"
            ),
        }
        phone.close().await;
    };
    let id = client
        .spawn(
            LaunchIdentity::default(),
            SpawnTarget::Terminal(TerminalSpawn {
                cols: 80,
                rows: 24,
                startup: Some("echo through-the-relay".into()),
                ..TerminalSpawn::default()
            }),
        )
        .await
        .expect("the phone starts a terminal");
    client.attach(id).await.expect("the phone attaches");
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("through-the-relay") {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("output in time")
            .expect("the connection stayed open");
        if let ClientEvent::Output { id: from, bytes } = event {
            client.ack(from, bytes.len());
            seen.extend_from_slice(&bytes);
        }
    }
    client.kill(id).await.expect("the phone ends the terminal");

    let _ = app.shutdown(true).await;
    let _ = core.join();
}
