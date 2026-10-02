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
//! - `exit CODE` ends the process at once;
//! - anything else is echoed back.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Agent {
    out: Mutex<std::io::Stdout>,
    waiting: Mutex<HashMap<String, Sender<Value>>>,
    cancelled: AtomicBool,
    next_request: AtomicU64,
    model: Mutex<String>,
}

impl Agent {
    fn send(&self, message: Value) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{message}");
            let _ = out.flush();
        }
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
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id.clone(), answer);
        }
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/request_permission",
            "params": params,
        }));
        answered.recv().unwrap_or(Value::Null)
    }

    fn config(&self) -> Value {
        let current = self
            .model
            .lock()
            .map(|model| model.clone())
            .unwrap_or_default();
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

fn run_turn(agent: &Agent, session_id: &str, text: &str) -> &'static str {
    let mut words = text.split_whitespace();
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
            let calls = [
                ("read-1", "read", "Read src/app.ts", json!([])),
                (
                    "run-1",
                    "execute",
                    "pnpm test src/app",
                    json!([{ "type": "content", "content": { "type": "text", "text": "3 passed" } }]),
                ),
                (
                    "edit-1",
                    "edit",
                    "Edit src/app.ts",
                    json!([{ "type": "diff", "path": "/tmp/src/app.ts", "oldText": "a\nb\n", "newText": "a\nc\nd\n" }]),
                ),
            ];
            for (id, kind, title, content) in calls {
                agent.update(
                    session_id,
                    json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": kind, "status": "in_progress" }),
                );
                std::thread::sleep(Duration::from_millis(argument.unwrap_or(300)));
                agent.update(
                    session_id,
                    json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed", "content": content }),
                );
            }
            say("Done. The test reads the file once now.");
            "end_turn"
        }
        "exit" => std::process::exit(argument.unwrap_or(1) as i32),
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
            let waiter = agent
                .waiting
                .lock()
                .ok()
                .and_then(|mut waiting| waiting.remove(key));
            if let Some(waiter) = waiter {
                let _ = waiter.send(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        return;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        ("initialize", Some(id)) => agent.reply(
            &id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "embeddedContext": true },
                },
            }),
        ),
        ("session/new", Some(id)) => {
            let session_id = format!(
                "fake-{}-{}",
                std::process::id(),
                sessions.fetch_add(1, Ordering::Relaxed)
            );
            agent.reply(
                &id,
                json!({ "sessionId": session_id, "configOptions": agent.config() }),
            );
        }
        ("session/load", Some(id)) => {
            let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
            replay_history(agent, &session_id);
            agent.reply(&id, json!({ "configOptions": agent.config() }));
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
                if let (Some(value), Ok(mut model)) = (params["value"].as_str(), agent.model.lock())
                {
                    *model = value.to_owned();
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

fn main() {
    let agent = Arc::new(Agent {
        out: Mutex::new(std::io::stdout()),
        waiting: Mutex::new(HashMap::new()),
        cancelled: AtomicBool::new(false),
        next_request: AtomicU64::new(1),
        model: Mutex::new("fast".into()),
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
