//! A stand-in chat agent for tests: it speaks just enough of the Agent Client
//! Protocol over stdio for the core to start it, prompt it, stop it and load
//! its sessions. Each session's turns are written to `FAKE_ACP_DIR`, so a new
//! process can load a session an earlier one ran.
//!
//! A prompt's first word picks what the turn does:
//! - `stream N` sends N pieces of text;
//! - `ask` asks permission and says which answer it got;
//! - `hold MS` says `holding`, waits, then says `held`, or stops early on a
//!   cancel;
//! - `plan` sets out a plan of three steps, one done, one under way, one to do;
//! - `exit CODE` ends the process at once;
//! - `who` says which account the process runs as;
//! - `run COMMAND` runs a shell command in the session's folder, as a tool
//!   call, and says what it printed;
//! - `tool METHOD JSON` calls one of Sikemux's agent tools, as the agent, and
//!   says the answer;
//! - anything else is echoed back.
//!
//! A message wrapped as one from another agent is read from its first line
//! inside the wrapping.
//!
//! The account is `FAKE_ACP_ACCOUNT`. An account named in `FAKE_ACP_LIMITED`
//! has run out of usage, and every turn on it fails the way Claude's adapter
//! reports that, until the Unix time in milliseconds `FAKE_ACP_RESETS_AT`
//! names, which the failure gives when set. When `FAKE_ACP_SIGN_IN` names a
//! file, the process reads who is signed in from it once, as it starts, and
//! refuses every prompt while nobody was. With `FAKE_ACP_STRICT_LOAD` set, loading a session that never
//! saved a turn fails the way Claude's adapter does. While the file
//! `FAKE_ACP_START_GATE` names exists, the agent does not finish starting. With
//! `FAKE_ACP_ENV_DIR` set, it writes the environment it was started with to
//! `<pid>.env` there, one `NAME=value` a line.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

/// What a session was started with.
#[derive(Clone, Default)]
struct Session {
    cwd: Option<PathBuf>,
    /// The file naming the tool endpoint Sikemux handed the session.
    endpoint: Option<PathBuf>,
}

struct Agent {
    out: Mutex<std::io::Stdout>,
    sessions: Mutex<HashMap<String, Session>>,
    waiting: Mutex<HashMap<String, Sender<Value>>>,
    cancelled: AtomicBool,
    next_request: AtomicU64,
    model: Mutex<String>,
    account: String,
    signed_in: AtomicBool,
}

impl Agent {
    fn send(&self, message: Value) {
        let mut out = self.out.locked();
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn update(&self, session_id: &str, update: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session_id, "update": update },
        }));
    }

    fn say(&self, session_id: &str, kind: &str, text: &str) {
        self.update(
            session_id,
            json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } }),
        );
    }

    fn ask(&self, params: Value) -> Value {
        let id = format!("fake-{}", self.next_request.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = channel();
        self.waiting.locked().insert(id.clone(), answer);
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/request_permission",
            "params": params,
        }));
        answered.recv().unwrap_or(Value::Null)
    }

    fn config(&self) -> Value {
        let current = self.model.locked().clone();
        json!([{
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current,
            "options": [
                { "value": "fast", "name": "Fast" },
                { "value": "slow", "name": "Slow" },
            ],
        }])
    }
}

fn history_file(session_id: &str) -> Option<PathBuf> {
    let directory = std::env::var_os("FAKE_ACP_DIR")?;
    Some(PathBuf::from(directory).join(format!("{session_id}.jsonl")))
}

fn record(session_id: &str, role: &str, text: &str) {
    let Some(path) = history_file(session_id) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}", json!({ "role": role, "text": text }));
    }
}

fn replay_history(agent: &Agent, session_id: &str) {
    let Some(contents) =
        history_file(session_id).and_then(|path| std::fs::read_to_string(path).ok())
    else {
        return;
    };
    for line in contents.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = if entry["role"] == "user" {
            "user_message_chunk"
        } else {
            "agent_message_chunk"
        };
        agent.say(session_id, kind, entry["text"].as_str().unwrap_or_default());
    }
}

fn prompt_text(params: &Value) -> String {
    params["prompt"]
        .as_array()
        .and_then(|blocks| blocks.iter().find_map(|block| block["text"].as_str()))
        .unwrap_or_default()
        .to_owned()
}

/// Runs `command` in the session's folder, answering what it printed.
fn run_command(session: &Session, command: &str) -> String {
    let mut shell = sikemux_process::user_environment::command("/bin/sh");
    shell.args(["-c", command]);
    if let Some(cwd) = &session.cwd {
        shell.current_dir(cwd);
    }
    match shell.output() {
        Ok(output) => format!(
            "ran with status {}: {}{}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("did not run: {error}"),
    }
}

/// Calls one of Sikemux's agent tools over the endpoint the session names.
fn call_tool(session: &Session, method: &str, params: &str) -> String {
    use sikemux_core::cli::auth::new_nonce;
    use sikemux_core::cli::protocol::{
        CliClientCommand, CliClientHello, CliEndpointDescriptor, CliServerResponse, HarnessRequest,
        CLI_PROTOCOL_VERSION,
    };
    let call = || -> Result<String, String> {
        let endpoint = session.endpoint.as_ref().ok_or("no tool endpoint")?;
        let descriptor: CliEndpointDescriptor =
            serde_json::from_slice(&std::fs::read(endpoint).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let params: Value = serde_json::from_str(params).map_err(|error| error.to_string())?;
        let stream = std::net::TcpStream::connect(("127.0.0.1", descriptor.port))
            .map_err(|error| error.to_string())?;
        let mut reader = std::io::BufReader::new(stream);
        let mut send = |message: Value| -> Result<CliServerResponse, String> {
            let mut line = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
            line.push(b'\n');
            reader
                .get_mut()
                .write_all(&line)
                .map_err(|error| error.to_string())?;
            let mut answer = String::new();
            reader
                .read_line(&mut answer)
                .map_err(|error| error.to_string())?;
            serde_json::from_str(&answer).map_err(|error| error.to_string())
        };
        let hello = CliClientHello::Hello {
            protocol: CLI_PROTOCOL_VERSION,
            nonce: new_nonce(),
        };
        send(serde_json::to_value(hello).map_err(|error| error.to_string())?)?;
        let command = CliClientCommand::Harness {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token,
            request: HarnessRequest {
                id: format!("fake-{}", std::process::id()),
                project: session
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                agent_id: None,
                method: method.to_owned(),
                params,
            },
        };
        match send(serde_json::to_value(command).map_err(|error| error.to_string())?)? {
            CliServerResponse::Result { value } => Ok(value.to_string()),
            CliServerResponse::Error { message } => Err(message),
            other => Err(format!("{other:?}")),
        }
    };
    match call() {
        Ok(answer) => format!("tool answered: {answer}"),
        Err(error) => format!("tool failed: {error}"),
    }
}

/// The line of `text` that says what to do: its first, or the first inside
/// a message from another agent.
fn command_line(text: &str) -> &str {
    let mut lines = text.lines().map(str::trim);
    match lines.next() {
        Some(first) if first.starts_with('<') => lines.next().unwrap_or(text),
        _ => text,
    }
}

fn run_turn(agent: &Agent, session_id: &str, text: &str) -> &'static str {
    let session = agent
        .sessions
        .locked()
        .get(session_id)
        .cloned()
        .unwrap_or_default();
    let line = command_line(text);
    let mut words = line.split_whitespace();
    let command = words.next().unwrap_or_default();
    let argument = words.next().and_then(|word| word.parse::<u64>().ok());
    let mut said = String::new();
    let mut say = |piece: &str| {
        agent.say(session_id, "agent_message_chunk", piece);
        said.push_str(piece);
    };
    let stop_reason = match command {
        "stream" => {
            for index in 0..argument.unwrap_or(3) {
                say(&format!("w{index} "));
                std::thread::sleep(Duration::from_millis(2));
            }
            "end_turn"
        }
        "ask" => {
            let answer = agent.ask(json!({
                "sessionId": session_id,
                "toolCall": { "toolCallId": "call-1", "title": "Touch a file" },
                "options": [
                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                    { "optionId": "deny", "name": "Deny", "kind": "reject_once" },
                ],
            }));
            let outcome = answer["outcome"]["optionId"]
                .as_str()
                .unwrap_or("cancelled")
                .to_owned();
            say(&format!("answered {outcome}"));
            "end_turn"
        }
        "hold" => {
            say("holding ");
            let deadline = Instant::now() + Duration::from_millis(argument.unwrap_or(1_000));
            let mut reason = "end_turn";
            while Instant::now() < deadline {
                if agent.cancelled.swap(false, Ordering::AcqRel) {
                    reason = "cancelled";
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if reason == "end_turn" {
                say("held");
            }
            reason
        }
        // A turn shaped like real work: a thought, then a read, a command and an
        // edit, each started and finished, then an answer.
        "work" => {
            agent.say(
                session_id,
                "agent_thought_chunk",
                "Reading the test before changing anything.",
            );
            let printed: Vec<String> = (1..=14)
                .map(|n| format!("✓ src/app.test.ts > case {n}"))
                .chain(["3 passed".into()])
                .collect();
            let calls = [
                ("read-1", "read", "Read src/app.ts", json!([]), Value::Null),
                (
                    "run-1",
                    "execute",
                    "pnpm test src/app",
                    json!([{ "type": "content", "content": { "type": "text", "text": "3 passed" } }]),
                    json!({ "output": printed.join("\n") }),
                ),
                (
                    "edit-1",
                    "edit",
                    "Edit src/app.ts",
                    json!([{ "type": "diff", "path": "/tmp/src/app.ts", "oldText": "a\nb\n", "newText": "a\nc\nd\n" }]),
                    Value::Null,
                ),
            ];
            for (id, kind, title, content, raw_output) in calls {
                agent.update(
                    session_id,
                    json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": kind, "status": "in_progress" }),
                );
                std::thread::sleep(Duration::from_millis(argument.unwrap_or(300)));
                agent.update(
                    session_id,
                    json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed", "content": content, "rawOutput": raw_output }),
                );
            }
            say("Done. The test reads the file once now.");
            "end_turn"
        }
        "plan" => {
            agent.update(
                session_id,
                json!({ "sessionUpdate": "plan", "entries": [
                    { "content": "Read the code", "priority": "high", "status": "completed" },
                    { "content": "Fix the bug", "priority": "high", "status": "in_progress" },
                    { "content": "Run the tests", "priority": "medium", "status": "pending" },
                ] }),
            );
            say("planned");
            "end_turn"
        }
        "exit" => std::process::exit(argument.unwrap_or(1) as i32),
        "who" => {
            say(&format!("account {}", agent.account));
            "end_turn"
        }
        "run" => {
            let command = line.trim_start().trim_start_matches("run").trim();
            agent.update(
                session_id,
                json!({ "sessionUpdate": "tool_call", "toolCallId": "run-1", "title": command, "kind": "execute", "status": "in_progress" }),
            );
            let output = run_command(&session, command);
            agent.update(
                session_id,
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "run-1", "status": "completed" }),
            );
            say(&output);
            "end_turn"
        }
        "tool" => {
            let rest = line.trim_start().trim_start_matches("tool").trim();
            let (method, params) = rest.split_once(' ').unwrap_or((rest, "{}"));
            say(&call_tool(&session, method, params.trim()));
            "end_turn"
        }
        _ => {
            say(&format!("echo: {text}"));
            "end_turn"
        }
    };
    record(session_id, "user", text);
    record(session_id, "agent", &said);
    stop_reason
}

fn handle(agent: &Arc<Agent>, message: Value, sessions: &AtomicU64) {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // An answer to a request this agent sent.
        if let Some(key) = id.as_ref().and_then(Value::as_str) {
            let waiter = agent.waiting.locked().remove(key);
            if let Some(waiter) = waiter {
                let _ = waiter.send(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        return;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        ("initialize", Some(id)) => {
            if let Some(gate) = std::env::var_os("FAKE_ACP_START_GATE") {
                while std::path::Path::new(&gate).exists() {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            let mut answer = json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "embeddedContext": true },
                },
            });
            if params["clientCapabilities"]["auth"]["terminal"] == true {
                answer["authMethods"] = json!([
                    { "type": "terminal", "id": "fake-login", "name": "Log in", "args": ["login"] },
                    { "id": "fake-key", "name": "API key", "description": "Use a key from the environment" },
                    { "id": "elsewhere", "name": "Elsewhere", "_meta": { "terminal-auth": { "command": "/bin/elsewhere", "args": [] } } },
                ]);
            }
            agent.reply(&id, answer);
        }
        ("session/new", Some(id)) => {
            let session_id = format!(
                "fake-{}-{}",
                std::process::id(),
                sessions.fetch_add(1, Ordering::Relaxed)
            );
            let endpoint = params["mcpServers"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|server| server["env"].as_array().cloned().unwrap_or_default())
                .find(|variable| variable["name"] == "SIKEMUX_CLI_ENDPOINT")
                .and_then(|variable| variable["value"].as_str().map(PathBuf::from));
            agent.sessions.locked().insert(
                session_id.clone(),
                Session {
                    cwd: params["cwd"].as_str().map(PathBuf::from),
                    endpoint,
                },
            );
            agent.reply(
                &id,
                json!({ "sessionId": session_id, "configOptions": agent.config() }),
            );
        }
        ("session/load", Some(id))
            if std::env::var_os("FAKE_ACP_STRICT_LOAD").is_some()
                && !history_file(params["sessionId"].as_str().unwrap_or_default())
                    .is_some_and(|path| path.exists()) =>
        {
            agent.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32002, "message": "Resource not found", "data": { "uri": params["sessionId"] } },
            }));
        }
        ("session/load", Some(id)) => {
            let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
            replay_history(agent, &session_id);
            agent.reply(&id, json!({ "configOptions": agent.config() }));
        }
        ("authenticate", Some(id)) if params["methodId"] == "fake-key" => {
            agent.signed_in.store(true, Ordering::Relaxed);
            agent.reply(&id, json!({}));
        }
        ("session/prompt", Some(id)) if !agent.signed_in.load(Ordering::Relaxed) => {
            agent.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": "Authentication required" },
            }))
        }
        ("session/prompt", Some(id)) if limited(&agent.account) => {
            let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
            record(&session_id, "user", &prompt_text(&params));
            agent.reply(
                &id,
                json!({
                    "stopReason": "end_turn",
                    "_meta": { "jetbrains": { "air": { "version": 1, "sessionFailure": {
                        "id": "turn:error",
                        "revision": 1,
                        "category": "limit",
                        "severity": "error",
                        "title": format!("{} hit its usage limit", agent.account),
                        "actions": [],
                        "resetsAt": resets_at(),
                    } } } },
                }),
            );
        }
        ("session/prompt", Some(id)) => {
            let agent = agent.clone();
            std::thread::spawn(move || {
                let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
                agent.cancelled.store(false, Ordering::Release);
                let stop_reason = run_turn(&agent, &session_id, &prompt_text(&params));
                agent.reply(&id, json!({ "stopReason": stop_reason }));
            });
        }
        ("session/cancel", None) => agent.cancelled.store(true, Ordering::Release),
        ("session/set_config_option", Some(id)) => {
            if params["configId"] == "model" {
                if let Some(value) = params["value"].as_str() {
                    *agent.model.locked() = value.to_owned();
                }
            }
            agent.reply(&id, json!({ "configOptions": agent.config() }));
        }
        ("session/set_mode", Some(id)) => agent.reply(&id, json!({})),
        (_, Some(id)) => agent.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("{method} is not faked") },
        })),
        (_, None) => {}
    }
}

fn limited(account: &str) -> bool {
    let reset = resets_at().is_some_and(|at| sikemux_process::clock::epoch_ms() >= at);
    !reset
        && std::env::var("FAKE_ACP_LIMITED")
            .unwrap_or_default()
            .split(',')
            .any(|limited| limited == account)
}

fn resets_at() -> Option<u64> {
    std::env::var("FAKE_ACP_RESETS_AT").ok()?.parse().ok()
}

fn main() {
    if let Some(dir) = std::env::var_os("FAKE_ACP_ENV_DIR") {
        let lines: String = std::env::vars()
            .map(|(name, value)| format!("{name}={value}\n"))
            .collect();
        let _ = std::fs::write(
            PathBuf::from(dir).join(format!("{}.env", std::process::id())),
            lines,
        );
    }
    let signed_in = match std::env::var_os("FAKE_ACP_SIGN_IN") {
        Some(path) => std::fs::read_to_string(path).is_ok_and(|who| !who.trim().is_empty()),
        None => true,
    };
    let agent = Arc::new(Agent {
        out: Mutex::new(std::io::stdout()),
        sessions: Mutex::new(HashMap::new()),
        waiting: Mutex::new(HashMap::new()),
        cancelled: AtomicBool::new(false),
        next_request: AtomicU64::new(1),
        model: Mutex::new("fast".into()),
        account: std::env::var("FAKE_ACP_ACCOUNT").unwrap_or_else(|_| "main".into()),
        signed_in: AtomicBool::new(signed_in),
    });
    let sessions = AtomicU64::new(1);
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if let Ok(message) = serde_json::from_str::<Value>(&line) {
            handle(&agent, message, &sessions);
        }
    }
}
