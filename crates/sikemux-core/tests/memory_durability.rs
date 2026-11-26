#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    reason = "a failing test helper is meant to panic"
)]

//! Cross-harness memory durability: verifies that terse person corrections, compaction
//! re-injection, pinned decisions, supersession, FTS5/BM25 recall past 50+ newer decisions,
//! and Claude -> Antigravity -> Codex harness switches all preserve memory intact.

mod folded;

use sikemux_process::command::{Child, Stdio};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folded::Chat;
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_core::client::probe;
use sikemux_view::chat::{ChatMessage, ChatRole};
use sikemux_wire::protocol::ask;

const CORE: &str = env!("CARGO_BIN_EXE_sikemux-core");
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_sikemux-fake-acp-agent");
const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_sikemux-fake-claude");
const FAKE_CODEX: &str = env!("CARGO_BIN_EXE_sikemux-fake-codex");
const WAIT: Duration = Duration::from_secs(90);

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_core(dir: &Path, socket: &Path) -> KilledOnDrop {
    let log = std::fs::File::create(dir.join("core.log")).expect("log");
    let endpoint = dir.join("cli.json");
    let child = sikemux_process::user_environment::command(CORE)
        .arg("core")
        .arg("--socket")
        .arg(socket)
        .arg("--cli-endpoint")
        .arg(&endpoint)
        .arg("--data-dir")
        .arg(dir.join("data"))
        .arg("--idle-exit-secs")
        .arg("600")
        .env("HOME", dir.join("home"))
        .env("SHELL", "/bin/sh")
        .env("PATH", "/usr/bin:/bin")
        .env("FAKE_ACP_DIR", dir.join("acp"))
        .env(sikemux_process::vars::AGENT_SEARCH, "path")
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("log"))
        .stderr(log)
        .spawn()
        .expect("start the core");
    let core = KilledOnDrop(child);
    let deadline = Instant::now() + WAIT;
    while probe(socket, Duration::from_secs(1)).is_err() || !endpoint.exists() {
        assert!(Instant::now() < deadline, "the core never answered");
        std::thread::sleep(Duration::from_millis(10));
    }
    core
}

fn write_agy(root: &Path) -> PathBuf {
    let agy = root.join("agy");
    let models = json!([{ "value": "gemini-4-neon", "name": "Gemini 4 Neon" }]).to_string();
    std::fs::write(
        &agy,
        format!(
            "#!/bin/sh\n[ \"$1\" = acp ] || exit 0\nexport FAKE_ACP_MODELS='{models}'\nexec '{FAKE_AGENT}'\n"
        ),
    )
    .expect("agy wrapper");
    std::fs::set_permissions(&agy, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod agy");
    agy
}

fn answers<'a>(rows: &'a [ChatMessage], prompt: &'a str) -> impl Iterator<Item = String> + 'a {
    rows.chunk_by(|_, next| next.role != ChatRole::User)
        .filter(move |turn| folded::prompts(turn) == [prompt])
        .map(folded::said)
}

struct Agent {
    client: CoreClient,
    chat: Chat,
    id: String,
}

impl Agent {
    async fn open(socket: &Path, id: &str) -> Self {
        let (client, events) = CoreClient::connect(socket).await.expect("connect");
        let mut chat = Chat::new(events, id);
        chat.wait = WAIT;
        chat.attached(&client.acp_attach(id.to_owned()).await.expect("attach"));
        Self {
            client,
            chat,
            id: id.to_owned(),
        }
    }

    async fn says(&mut self, text: &str, expect: &str) -> String {
        let before = answers(&self.chat.view.rows, text).count();
        self.client
            .ask(ask::AcpPrompt {
                agent_id: self.id.clone(),
                message_id: Some(uuid::Uuid::new_v4().to_string()),
                text: text.to_owned(),
                paths: Vec::new(),
                context: Vec::new(),
            })
            .await
            .expect("prompt");
        let mut said = String::new();
        self.chat
            .until(|chat| {
                said = answers(&chat.view.rows, text)
                    .nth(before)
                    .unwrap_or_default();
                !chat.head().running && said.contains(expect) && !said.is_empty()
            })
            .await;
        said
    }

    async fn tool(&mut self, method: &str, params: Value) -> Value {
        let prompt = format!("tool {method} {params}");
        let answer = self.says(&prompt, "tool answered: ").await;
        let json_str = answer
            .split_once("tool answered: ")
            .map(|(_, rest)| rest.trim())
            .expect("tool answered payload");
        serde_json::from_str(json_str).expect("valid JSON tool response")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_survives_terse_correction_compaction_harness_switches_and_55_decisions() {
    let root = tempfile::Builder::new()
        .prefix("mem-v2")
        .tempdir_in("/tmp")
        .expect("temp dir");
    let root = sikemux_process::files::real_path(root.path()).expect("canonical");
    let (project, claude, codex) = (
        root.join("project"),
        root.join("claude"),
        root.join("codex"),
    );
    for dir in [
        root.join("home"),
        claude.clone(),
        codex.clone(),
        project.clone(),
    ] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    std::fs::write(
        claude.join("fake-account.json"),
        json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "email": "qa@example.com",
            "subscriptionType": "max"
        })
        .to_string(),
    )
    .expect("claude account");
    let agy = write_agy(&root);
    let socket = root.join("core.sock");
    let _core = start_core(&root, &socket);
    let (app, _events) = CoreClient::connect(&socket).await.expect("connect");
    let accounts = json!({ "profiles": [
        { "id": "builtin-claude", "name": "Claude", "provider": "claude", "accent": "#d97757",
          "executablePath": FAKE_CLAUDE, "configPath": claude },
        { "id": "agy", "name": "Antigravity", "provider": "antigravity", "accent": "#4285f4",
          "executablePath": agy },
        { "id": "builtin-codex", "name": "Codex", "provider": "codex", "accent": "#10a37f",
          "executablePath": FAKE_CODEX, "configPath": codex },
    ] });
    app.call("setAgentAccounts", accounts)
        .await
        .expect("accounts");
    app.call(
        "addProject",
        json!({ "id": "sikemux", "name": "sikemux", "path": project }),
    )
    .await
    .expect("project");

    // Start on Claude, then hand off to Antigravity to verify Claude -> Antigravity -> Codex.
    let (id, _) = app
        .start_chat("claude:builtin-claude".into(), "sikemux".into(), None)
        .await
        .expect("claude chat starts");
    let mut claude_chat = Agent::open(&socket, &id).await;
    let _ = claude_chat
        .says("Starting Memory v2 durability run on Claude.", "echo:")
        .await;

    // (c.1) Switch Claude -> Antigravity
    app.call(
        "handOff",
        json!({ "agentId": id, "provider": "antigravity", "profileId": "agy" }),
    )
    .await
    .expect("hand off claude -> antigravity");
    let mut agent = Agent::open(&socket, &id).await;
    agent
        .chat
        .until(|chat| {
            chat.head()
                .handoff
                .as_ref()
                .is_some_and(|h| h.from == "claude")
        })
        .await;

    // (a) Terse person correction ("Bruv neon bruv, not fable") stays a first-class person rule
    // instead of being demoted to an agent note.
    let _ = agent.says("Bruv neon bruv, not fable", "echo:").await;
    let added = agent
        .tool(
            "memory.add",
            json!({
                "kind": "rule",
                "person": true,
                "pinned": true,
                "text": "Always use Gemini 4 Neon via Antigravity for design consults, never Fable."
            }),
        )
        .await;
    let note = added["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("as the person's rule"),
        "terse person correction was demoted: {added}"
    );

    // Durable pinned knowledge entry with path & topic tags
    let _ = agent
        .tool(
            "memory.add",
            json!({
                "kind": "knowledge",
                "pinned": true,
                "about": ["crates/sikemux-core/src/server/memory/", "mcp"],
                "text": "Sikemux Memory v2 indexes rules, knowledge, and decisions with SQLite FTS5/BM25."
            }),
        )
        .await;

    // Decision #1 superseded by pinned Decision #2
    let old_dec = agent
        .tool(
            "decision.keep",
            json!({
                "text": "Use Fable 5.2 for design consults.",
                "why": "Initial routing choice."
            }),
        )
        .await;
    assert!(old_dec["note"]
        .as_str()
        .unwrap_or_default()
        .contains("Decision #1"));

    let new_dec = agent
        .tool(
            "decision.keep",
            json!({
                "text": "Consult Gemini 4 Neon via Antigravity for architecture shape, never Fable.",
                "why": "Person banned Fable and picked Neon.",
                "pinned": true,
                "supersedes": 1
            }),
        )
        .await;
    assert!(new_dec["note"]
        .as_str()
        .unwrap_or_default()
        .contains("Decision #2"));

    // (d) Add 55 newer routine decisions to verify pinned + FTS5 recall past LIMIT 5
    for i in 1..=55 {
        let _ = agent
            .tool(
                "decision.keep",
                json!({
                    "text": format!("Routine UI polish decision #{i} adjusting row padding."),
                    "why": "Visual cleanup."
                }),
            )
            .await;
    }

    let listed = agent.tool("memory.list", json!({})).await;
    let decisions = listed["decisions"].as_array().expect("decisions array");
    assert_eq!(
        decisions.len(),
        20,
        "default decisions list must be bounded"
    );
    assert!(
        decisions
            .iter()
            .any(|d| d["id"] == 2 && d["pinned"] == true),
        "pinned decision #2 fell off default list: {decisions:?}"
    );
    assert!(
        !decisions.iter().any(|d| d["id"] == 1),
        "superseded decision #1 leaked into standing decisions"
    );
    assert_eq!(listed["total"]["decisions"], 56);

    let searched = agent
        .tool(
            "memory.list",
            json!({ "kind": "decisions", "query": "neon fable consult" }),
        )
        .await;
    let hits = searched["decisions"].as_array().expect("search hits");
    assert!(!hits.is_empty());
    assert_eq!(
        hits[0]["id"], 2,
        "FTS5/BM25 failed to rank decision #2 first: {hits:?}"
    );

    let knowledge = agent
        .tool(
            "memory.list",
            json!({ "kind": "knowledge", "query": "mcp fts5" }),
        )
        .await;
    let k_hits = knowledge["knowledge"].as_array().expect("knowledge hits");
    assert_eq!(k_hits.len(), 1);
    assert!(k_hits[0]["text"].as_str().unwrap().contains("FTS5/BM25"));

    // (b) Compaction: `/compact` triggers universal post-compaction memory re-injection on the
    // very next turn, carrying the terse-corrected rule + pinned decision #2 while omitting
    // superseded decision #1.
    let _ = agent.says("/compact", "echo:").await;
    let after_compact_ctx = agent
        .says("context", "Project rules after compaction")
        .await;
    assert!(
        after_compact_ctx
            .contains("Always use Gemini 4 Neon via Antigravity for design consults, never Fable."),
        "terse person rule missing after compaction: {after_compact_ctx}"
    );
    assert!(
        after_compact_ctx
            .contains("Consult Gemini 4 Neon via Antigravity for architecture shape, never Fable."),
        "pinned decision #2 missing after compaction: {after_compact_ctx}"
    );
    assert!(
        !after_compact_ctx.contains("Use Fable 5.2 for design consults."),
        "superseded decision #1 leaked after compaction: {after_compact_ctx}"
    );

    // (c.2) Switch Antigravity -> Codex and verify Codex runs with the handoff cleanly
    app.call(
        "handOff",
        json!({ "agentId": id, "provider": "codex", "profileId": "builtin-codex" }),
    )
    .await
    .expect("hand off antigravity -> codex");
    let mut codex_chat = Agent::open(&socket, &id).await;
    codex_chat
        .chat
        .until(|chat| {
            chat.head()
                .handoff
                .as_ref()
                .is_some_and(|h| h.from == "antigravity")
        })
        .await;
    let _ = codex_chat
        .says("Continue on Codex after Antigravity.", "echo:")
        .await;

    // (c.3) Switch Codex -> Antigravity and verify the warm handoff packet and durable store
    // still carry active rules, pinned knowledge, and pinned decision #2.
    app.call(
        "handOff",
        json!({ "agentId": id, "provider": "antigravity", "profileId": "agy" }),
    )
    .await
    .expect("hand off codex -> antigravity");
    let mut back = Agent::open(&socket, &id).await;
    back.chat
        .until(|chat| {
            chat.head()
                .handoff
                .as_ref()
                .is_some_and(|h| h.from == "codex")
        })
        .await;
    let handoff_ctx = back
        .says("context", "## Active rules and standing policy")
        .await;
    assert!(
        handoff_ctx
            .contains("Always use Gemini 4 Neon via Antigravity for design consults, never Fable."),
        "active rules missing in warm handoff packet: {handoff_ctx}"
    );
    assert!(
        handoff_ctx
            .contains("Consult Gemini 4 Neon via Antigravity for architecture shape, never Fable."),
        "pinned decision #2 missing in warm handoff packet: {handoff_ctx}"
    );

    let verified = back.tool("memory.list", json!({ "pinned": true })).await;
    assert!(
        verified["rules"]
            .as_array()
            .is_some_and(|r| r.iter().any(|item| item["text"]
                .as_str()
                .unwrap_or("")
                .contains("Gemini 4 Neon"))),
        "pinned person rule missing after Claude -> Antigravity -> Codex cycle: {verified}"
    );
    assert!(
        verified["knowledge"].as_array().is_some_and(|k| k
            .iter()
            .any(|item| item["text"].as_str().unwrap_or("").contains("FTS5/BM25"))),
        "pinned knowledge missing after Claude -> Antigravity -> Codex cycle: {verified}"
    );
    assert!(
        verified["decisions"]
            .as_array()
            .is_some_and(|d| d.iter().any(|item| item["id"] == 2)),
        "pinned decision missing after Claude -> Antigravity -> Codex cycle: {verified}"
    );
}
