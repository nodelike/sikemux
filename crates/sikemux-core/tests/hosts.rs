#![cfg(unix)]

//! Two cores on this machine standing in for two hosts on one account: they
//! link with a join ticket and the second host's approval, an agent on the
//! first starts a worker on the second from its own checkout, the two message
//! each other, and the worker's commits come back to a branch in the lead's
//! checkout.

mod folded;

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_core::accounts::protocol::JoinTicket;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::protocol::RemoteStatus;
use sikemux_core::remote::SecretKey;
use sikemux_core::server::{self, ServerConfig, ServerError};

const WAIT: Duration = Duration::from_secs(90);
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
const ACCOUNT: &str = "user_2test";
const ORIGIN: &str = "https://example.com/acme/widget.git";

/// The shared vector's throwaway signing key, which a debug core trusts as
/// the local accounts server's.
fn signing_key() -> ring::signature::Ed25519KeyPair {
    use base64::Engine;
    let vector: Value =
        serde_json::from_str(include_str!("../../../server/protocol/vectors/join.json"))
            .expect("vector");
    let pem = vector["privateKeyPem"].as_str().expect("pem");
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .expect("der");
    ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).expect("key")
}

fn init_env(home: &Path) {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        let vector: Value =
            serde_json::from_str(include_str!("../../../server/protocol/vectors/join.json"))
                .expect("vector");
        let dev = home.join(".config/sikemux/dev");
        std::fs::create_dir_all(&dev).expect("dev folder");
        std::fs::write(
            dev.join("join-signing-key.pem"),
            vector["privateKeyPem"].as_str().expect("pem"),
        )
        .expect("dev key");
        std::env::set_var("HOME", home);
        std::env::set_var("SHELL", "/bin/sh");
        std::env::remove_var("SIKEMUX_SHELL");
    });
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = sikemux_process::user_environment::command("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct TestCore {
    socket: PathBuf,
    thread: Option<JoinHandle<Result<(), ServerError>>>,
}

impl Drop for TestCore {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
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
            let _ = thread.join();
        }
    }
}

/// A core signed in to the account, remote access on, on loopback only.
fn start_core(dir: &Path) -> TestCore {
    std::fs::create_dir_all(dir).expect("core folder");
    let socket = dir.join("core.sock");
    let mut remote = socket.as_os_str().to_owned();
    remote.push(".remote.json");
    let stored = json!({
        "enabled": true,
        "owner": ACCOUNT,
        "devices": [],
    });
    std::fs::write(remote, stored.to_string()).expect("remote file");
    server::trust_remote_file(&socket, &SecretKey::generate()).expect("host key");
    let config = ServerConfig {
        idle_exit: Duration::from_secs(600),
        cli_endpoint: Some(dir.join("cli.json")),
        // A data folder puts the core's git guard on agents' PATH, which in a
        // test is this test binary.
        data_dir: None,
        remote_direct_only: true,
        ..ServerConfig::new(socket.clone())
    };
    let endpoint = dir.join("cli.json");
    let thread = std::thread::spawn(move || server::run(config));
    let deadline = Instant::now() + WAIT;
    while probe(&socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    TestCore {
        socket,
        thread: Some(thread),
    }
}

async fn publish(app: &CoreClient, project: &Path) {
    let accounts = json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": FAKE_AGENT,
        }],
    });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("the accounts");
    let project = json!({ "id": "widget", "name": "widget", "path": project });
    app.call("addProject", project).await.expect("the project");
    app.call("setAgentDefaults", json!({ "permissionMode": "bypass" }))
        .await
        .expect("the defaults");
}

async fn listening(app: &CoreClient) -> RemoteStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = app.remote_status().await.expect("remote status");
        if !status.addresses.is_empty() {
            return status;
        }
        assert!(Instant::now() < deadline, "remote access never listened");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn ticket(host: &str, joiner: &str) -> JoinTicket {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let mut ticket: JoinTicket = serde_json::from_value(json!({
        "v": 1,
        "keyId": "dev-1",
        "account": ACCOUNT,
        "host": host,
        "phone": joiner,
        "issuedAt": now - 5,
        "expiresAt": now + 595,
        "signature": "0".repeat(128),
    }))
    .expect("ticket");
    let message = sikemux_core::join::ticket_message(&ticket);
    ticket.signature = hex::encode(signing_key().sign(message.as_bytes()).as_ref());
    ticket
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_starts_a_worker_on_a_linked_host_and_gets_its_commits_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    init_env(&dir.path().join("home"));

    let lead_repo = dir.path().join("lead/widget");
    std::fs::create_dir_all(&lead_repo).expect("lead repo");
    git(&lead_repo, &["init", "--quiet", "-b", "main"]);
    git(&lead_repo, &["remote", "add", "origin", ORIGIN]);
    std::fs::write(lead_repo.join("a.txt"), "one\n").expect("a.txt");
    git(&lead_repo, &["add", "a.txt"]);
    git(&lead_repo, &["commit", "--quiet", "-m", "one"]);
    let studio_repo = dir.path().join("studio/widget");
    std::fs::create_dir_all(studio_repo.parent().expect("parent")).expect("studio folder");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            &lead_repo.to_string_lossy(),
            &studio_repo.to_string_lossy(),
        ],
    );
    git(&studio_repo, &["remote", "set-url", "origin", ORIGIN]);
    std::fs::write(lead_repo.join("a.txt"), "one\ntwo\n").expect("a.txt");
    git(&lead_repo, &["commit", "--quiet", "-am", "two"]);
    std::fs::write(lead_repo.join("b.txt"), "not committed\n").expect("b.txt");

    let lead_core = start_core(&dir.path().join("lead/core"));
    let studio_core = start_core(&dir.path().join("studio/core"));
    let (lead_app, lead_events) = CoreClient::connect(&lead_core.socket)
        .await
        .expect("lead app");
    let (studio_app, _studio_events) = CoreClient::connect(&studio_core.socket)
        .await
        .expect("studio app");
    publish(&lead_app, &lead_repo).await;
    publish(&studio_app, &studio_repo).await;
    let lead_status = listening(&lead_app).await;
    let studio_status = listening(&studio_app).await;

    let linking = tokio::spawn({
        let ticket = ticket(&studio_status.core_id, &lead_status.core_id);
        let addresses = studio_status.addresses.clone();
        let socket = lead_core.socket.clone();
        async move {
            let (app, _events) = CoreClient::connect(&socket).await.expect("lead app");
            app.call(
                "linkHost",
                json!({ "ticket": ticket, "name": "studio", "platform": "macos", "addresses": addresses }),
            )
            .await
        }
    });
    let linked = linking
        .await
        .expect("linking")
        .expect("the lead's host links");
    assert_eq!(linked["links"][0]["name"], "studio");
    let studio_links = studio_app
        .call("pairing", Value::Null)
        .await
        .expect("pairing");
    assert_eq!(studio_links["links"].as_array().map(Vec::len), Some(1));

    let (lead, _) = lead_app
        .start_chat("opencode:fake".into(), "widget".into(), None)
        .await
        .expect("the lead starts");
    let mut said = Chat::new(lead_events, &lead);
    said.wait = WAIT;
    said.attached(&lead_app.acp_attach(lead.clone()).await.expect("attach"));
    let spawn = json!({
        "host": "studio",
        "cli": "opencode",
        "prompt": "run cat b.txt && printf done > c.txt && git add c.txt && git -c user.name=W -c user.email=w@example.com commit -qm worker",
    });
    lead_app
        .acp_prompt(
            lead.clone(),
            None,
            format!("tool agent.spawn {spawn}"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt the lead");
    let answered = said.until_said("tool ").await;
    assert!(answered.contains("on studio"), "{answered}");
    let reported = said.until_said("Its commits came back to the branch").await;
    assert!(
        reported.contains("not committed"),
        "the worker saw the lead's uncommitted file: {reported}"
    );
    let branch = git(
        &lead_repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/sikemux/studio",
        ],
    );
    assert!(!branch.is_empty(), "no branch came back");
    assert_eq!(
        git(&lead_repo, &["show", &format!("{branch}:c.txt")]),
        "done"
    );
    assert_eq!(
        git(&lead_repo, &["show", &format!("{branch}:b.txt")]),
        "not committed"
    );
    assert_eq!(
        git(&lead_repo, &["log", "-1", "--format=%s", &branch]),
        "worker"
    );

    let listed = json!({});
    lead_app
        .acp_prompt(
            lead.clone(),
            None,
            format!("tool agent.list {listed}"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt the lead");
    let list = said.until_said("tool answered").await;
    assert!(list.contains("studio"), "{list}");

    let worker_name = branch.rsplit('/').next().expect("worker name").to_owned();
    let read = json!({ "agent": format!("{worker_name} on studio") });
    lead_app
        .acp_prompt(
            lead.clone(),
            None,
            format!("tool agent.read {read}"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt the lead");
    let read = said.until_said("tool answered").await;
    assert!(read.contains("not committed"), "{read}");

    let send = json!({ "to": format!("{worker_name} on studio"), "text": "run echo second-task" });
    lead_app
        .acp_prompt(
            lead.clone(),
            None,
            format!("tool agent.send {send}"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("prompt the lead");
    said.until_said("second-task").await;
}
