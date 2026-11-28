#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

//! Adding and removing a custom ACP agent through the running core: its own socket and data
//! folder, and a `custom-agents.json` in a folder of its own, so nothing of the person's is read
//! or written.

use sikemux_process::command::{Child, Stdio};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_core::client::probe;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const WAIT: Duration = Duration::from_secs(60);

struct Sandbox {
    root: tempfile::TempDir,
    socket: PathBuf,
    child: Option<Child>,
}

impl Sandbox {
    fn start() -> Self {
        let root = tempfile::Builder::new()
            .prefix("custom-agents")
            .tempdir_in("/tmp")
            .expect("temp dir");
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let socket = root.path().join("core.sock");
        let endpoint = root.path().join("cli.json");
        let log = std::fs::File::create(root.path().join("core.log")).expect("log");
        let child = sikemux_process::user_environment::command(CORE)
            .arg("core")
            .arg("--socket")
            .arg(&socket)
            .arg("--cli-endpoint")
            .arg(&endpoint)
            .arg("--data-dir")
            .arg(root.path().join("data"))
            .arg("--idle-exit-secs")
            .arg("600")
            .env("HOME", &home)
            .env("SHELL", "/bin/sh")
            .env("PATH", "/usr/bin:/bin")
            .env(
                sikemux_process::vars::CUSTOM_AGENTS_PATH,
                root.path().join("config").join("custom-agents.json"),
            )
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("log"))
            .stderr(log)
            .spawn()
            .expect("core");
        let deadline = Instant::now() + WAIT;
        while probe(&socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
            assert!(Instant::now() < deadline, "the core never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
        Self {
            root,
            socket,
            child: Some(child),
        }
    }

    fn file(&self) -> PathBuf {
        self.root.path().join("config").join("custom-agents.json")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn kinds(listed: &Value) -> Vec<&str> {
    listed
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|agent| agent["type"].as_str())
        .collect()
}

async fn catalog(client: &CoreClient) -> Value {
    client
        .call("agentCatalog", json!({}))
        .await
        .expect("agentCatalog")
}

fn entry<'a>(listed: &'a Value, kind: &str) -> &'a Value {
    listed
        .as_array()
        .and_then(|agents| agents.iter().find(|agent| agent["type"] == kind))
        .unwrap_or_else(|| panic!("{kind} is not listed in {listed}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_custom_agent_is_added_listed_even_when_broken_and_removed() {
    let sandbox = Sandbox::start();
    let (client, _events) = CoreClient::connect(Path::new(&sandbox.socket))
        .await
        .expect("connect");

    let before = catalog(&client).await;
    assert!(!kinds(&before).contains(&"dima"));
    assert!(kinds(&before).contains(&"claude"));

    // Its command is not on this machine, which the setup page must still list.
    let added = client
        .call(
            "customAgentAdd",
            json!({
                "type": "dima",
                "label": "DimCode",
                "command": "/nonexistent/dima",
                "args": ["acp"],
                "env": { "DIMA_MODE": "test" }
            }),
        )
        .await
        .expect("customAgentAdd");
    assert_eq!(added["type"], "dima");
    assert_eq!(added["label"], "DimCode");

    let listed = catalog(&client).await;
    let dima = entry(&listed, "dima");
    assert_eq!(dima["custom"], true);
    assert_eq!(dima["label"], "DimCode");
    assert_eq!(dima["command"], "/nonexistent/dima");
    assert_eq!(dima["args"], json!(["acp"]));
    assert_eq!(dima["chat"], "acp");
    assert_ne!(entry(&listed, "claude")["custom"], true);

    let setup = client
        .call("agentSetup", json!({}))
        .await
        .expect("agentSetup");
    let row = entry(&setup, "dima");
    assert_eq!(row["available"], false, "{row}");
    let installed = client
        .call("installedAgents", json!({}))
        .await
        .expect("installedAgents");
    assert!(kinds(&installed).contains(&"dima"), "{installed}");

    let text = std::fs::read_to_string(sandbox.file()).expect("the file");
    assert!(text.contains("\"DIMA_MODE\""), "{text}");

    let again = client
        .call(
            "customAgentAdd",
            json!({ "type": "dima", "command": "/other" }),
        )
        .await
        .expect_err("a second dima");
    assert!(again.to_string().contains("listed twice"), "{again}");
    let built_in = client
        .call(
            "customAgentAdd",
            json!({ "type": "claude", "command": "claude" }),
        )
        .await
        .expect_err("a built-in's id");
    assert!(built_in.to_string().contains("built-in"), "{built_in}");
    let refused = client
        .call("customAgentRemove", json!({ "kind": "claude" }))
        .await
        .expect_err("removing a built-in");
    assert!(refused.to_string().contains("built-in"), "{refused}");

    let removed = client
        .call("customAgentRemove", json!({ "kind": "dima" }))
        .await
        .expect("customAgentRemove");
    assert_eq!(removed, json!(true));
    let after = catalog(&client).await;
    assert!(!kinds(&after).contains(&"dima"), "{after}");
    assert!(kinds(&after).contains(&"claude"));
    let setup = client
        .call("agentSetup", json!({}))
        .await
        .expect("agentSetup");
    assert!(!kinds(&setup).contains(&"dima"), "{setup}");
    let removed_again = client
        .call("customAgentRemove", json!({ "kind": "dima" }))
        .await
        .expect("customAgentRemove");
    assert_eq!(removed_again, json!(false));
}
