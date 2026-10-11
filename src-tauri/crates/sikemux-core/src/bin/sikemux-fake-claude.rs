//! A stand-in for the `claude` binary in tests: it speaks just enough of
//! Claude Code's stream-json protocol for the core to start it, prompt it,
//! stop it, resume its sessions and take them back to an earlier message.
//! Each session's transcript is written where Claude Code keeps its own, under
//! `CLAUDE_CONFIG_DIR`, so a new process can resume what an earlier one said.
//!
//! A prompt's first word picks what the turn does:
//! - `stream N` sends N pieces of text;
//! - `ask` asks permission to run a command and says which answer it got;
//! - `hold MS` says `holding`, waits, then says `held`, or stops early on an
//!   interrupt;
//! - `count` says how many messages the person sent before this one;
//! - `limit` fails the way Claude Code does when the account is out of usage;
//! - anything else is echoed back.
//!
//! A model change is answered the way Claude Code does, with a message saying
//! so. `FAKE_CLAUDE_SETTINGS_MODEL` names the model its settings choose.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Claude {
    out: Mutex<std::io::Stdout>,
    waiting: Mutex<HashMap<String, Sender<Value>>>,
    interrupted: AtomicBool,
    session_id: String,
    transcript: PathBuf,
    /// The newest record of the conversation, which the next one follows.
    leaf: Mutex<Option<String>>,
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .find_map(|arg| arg.strip_prefix(&format!("--{name}=")).map(str::to_owned))
}

impl Claude {
    fn send(&self, message: Value) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{message}");
            let _ = out.flush();
        }
    }

    fn record(&self, mut record: Value) {
        let uuid = record["uuid"].as_str().unwrap_or_default().to_owned();
        if let Ok(mut leaf) = self.leaf.lock() {
            record["parentUuid"] = json!(leaf.clone());
            *leaf = Some(uuid);
        }
        record["sessionId"] = json!(self.session_id);
        record["isSidechain"] = json!(false);
        if let Some(parent) = self.transcript.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.transcript)
        {
            let _ = writeln!(file, "{record}");
        }
    }

    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.transcript)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The conversation as it stands: the chain back from the newest record.
    fn chain(&self) -> Vec<Value> {
        let records = self.records();
        let by_uuid: HashMap<&str, &Value> = records
            .iter()
            .filter_map(|record| Some((record["uuid"].as_str()?, record)))
            .collect();
        let mut chain = Vec::new();
        let mut next = self.leaf.lock().ok().and_then(|leaf| leaf.clone());
        while let Some(uuid) = next {
            let Some(record) = by_uuid.get(uuid.as_str()) else {
                break;
            };
            chain.push((*record).clone());
            next = record["parentUuid"].as_str().map(str::to_owned);
        }
        chain.reverse();
        chain
    }

    fn ask(&self, request: Value) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        let (answer, answered) = channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id.clone(), answer);
        }
        self.send(json!({ "type": "control_request", "request_id": id, "request": request }));
        answered
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or(Value::Null)
    }

    fn say(&self, message_id: &str, text: &str) {
        self.send(json!({
            "type": "stream_event",
            "parent_tool_use_id": null,
            "event": { "type": "message_start", "message": { "id": message_id, "model": "fake", "usage": { "input_tokens": 10, "output_tokens": 1 } } },
        }));
        self.send(json!({
            "type": "stream_event",
            "parent_tool_use_id": null,
            "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } },
        }));
        let assistant = json!({
            "type": "assistant",
            "uuid": uuid::Uuid::new_v4().to_string(),
            "parent_tool_use_id": null,
            "message": { "id": message_id, "model": "fake", "role": "assistant", "content": [{ "type": "text", "text": text }], "usage": { "input_tokens": 10, "output_tokens": 5 } },
        });
        self.record(assistant.clone());
        self.send(assistant);
    }

    fn finish(&self, prompt: &str, error: Option<&str>) {
        let mut result = json!({
            "type": "result",
            "subtype": if error.is_some() { "error_during_execution" } else { "success" },
            "is_error": error.is_some(),
            "stop_reason": if error.is_some() { Value::Null } else { json!("end_turn") },
            "user_message_uuid": prompt,
            "total_cost_usd": 0.01,
            "modelUsage": { "fake": { "contextWindow": 200000 } },
        });
        if let Some(error) = error {
            result["result"] = json!(error);
        }
        self.send(result);
        self.send(json!({ "type": "system", "subtype": "session_state_changed", "state": "idle" }));
    }

    fn turn(&self, uuid: &str, text: &str) {
        self.interrupted.store(false, Ordering::Release);
        let message_id = format!("msg-{uuid}");
        let mut words = text.split_whitespace();
        match words.next().unwrap_or_default() {
            "stream" => {
                let count: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                self.send(json!({
                    "type": "stream_event",
                    "parent_tool_use_id": null,
                    "event": { "type": "message_start", "message": { "id": message_id, "model": "fake" } },
                }));
                let mut said = String::new();
                for index in 0..count {
                    let piece = format!("w{index} ");
                    said.push_str(&piece);
                    self.send(json!({
                        "type": "stream_event",
                        "parent_tool_use_id": null,
                        "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": piece } },
                    }));
                }
                let assistant = json!({
                    "type": "assistant",
                    "uuid": uuid::Uuid::new_v4().to_string(),
                    "parent_tool_use_id": null,
                    "message": { "id": message_id, "model": "fake", "role": "assistant", "content": [{ "type": "text", "text": said }] },
                });
                self.record(assistant.clone());
                self.send(assistant);
                self.finish(uuid, None);
            }
            "ask" => {
                let tool_id = format!("toolu-{uuid}");
                let input = json!({ "command": "echo hi", "description": "Say hi" });
                let answer = self.ask(json!({
                    "subtype": "can_use_tool",
                    "tool_name": "Bash",
                    "input": input,
                    "tool_use_id": tool_id,
                    "permission_suggestions": [],
                }));
                let allowed = answer["behavior"] == "allow";
                self.say(&message_id, if allowed { "allowed" } else { "denied" });
                self.finish(uuid, None);
            }
            "hold" => {
                let ms: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(1000);
                self.say(&format!("{message_id}-a"), "holding");
                let until = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < until {
                    if self.interrupted.load(Ordering::Acquire) {
                        self.finish(uuid, Some("[Request interrupted by user]"));
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.say(&format!("{message_id}-b"), "held");
                self.finish(uuid, None);
            }
            "count" => {
                let earlier = self
                    .chain()
                    .iter()
                    .filter(|record| record["type"] == "user" && record["uuid"] != uuid)
                    .count();
                self.say(&message_id, &format!("{earlier} earlier"));
                self.finish(uuid, None);
            }
            "limit" => {
                self.send(json!({
                    "type": "assistant",
                    "parent_tool_use_id": null,
                    "error": "rate_limit",
                    "message": { "id": message_id, "model": "<synthetic>", "role": "assistant", "content": [{ "type": "text", "text": "You've hit your limit · resets 3pm" }] },
                }));
                self.finish(uuid, Some("You've hit your limit · resets 3pm"));
            }
            _ => {
                self.say(&message_id, &format!("echo: {text}"));
                self.finish(uuid, None);
            }
        }
    }

    fn control(&self, request_id: &str, request: &Value) {
        let response = match request["subtype"].as_str() {
            Some("initialize") => json!({
                "models": [
                    { "value": "default", "resolvedModel": "fake", "displayName": "Default", "description": "Fake", "supportsEffort": true, "supportedEffortLevels": ["low", "high"] },
                    { "value": "small", "resolvedModel": "fake-small", "displayName": "Small", "description": "Smaller" },
                ],
                "commands": [{ "name": "review", "description": "Review the change", "argumentHint": "" }],
            }),
            Some("interrupt") => {
                self.interrupted.store(true, Ordering::Release);
                json!({ "still_queued": [] })
            }
            Some("rewind_files") => json!({ "canRewind": true, "filesChanged": [] }),
            Some("set_model") => {
                let model = request["model"].as_str().unwrap_or("default");
                self.send(json!({
                    "type": "user",
                    "uuid": uuid::Uuid::new_v4().to_string(),
                    "isReplay": true,
                    "parent_tool_use_id": null,
                    "message": { "role": "user", "content": format!("<local-command-stdout>Set model to {model}</local-command-stdout>") },
                }));
                json!({})
            }
            Some("get_settings") => json!({
                "effective": { "model": std::env::var("FAKE_CLAUDE_SETTINGS_MODEL").ok() },
            }),
            _ => json!({}),
        };
        self.send(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        }));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let resume = flag(&args, "resume");
    let session_id = resume
        .clone()
        .or_else(|| flag(&args, "session-id"))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let transcript = config
        .join("projects")
        .join("fake")
        .join(format!("{session_id}.jsonl"));
    let claude = Arc::new(Claude {
        out: Mutex::new(std::io::stdout()),
        waiting: Mutex::new(HashMap::new()),
        interrupted: AtomicBool::new(false),
        session_id,
        transcript,
        leaf: Mutex::new(None),
    });
    if resume.is_some() {
        let records = claude.records();
        if records.is_empty() {
            eprintln!("No conversation found with session ID");
            std::process::exit(1);
        }
        let leaf = flag(&args, "resume-session-at").or_else(|| {
            records
                .iter()
                .rev()
                .find_map(|record| record["uuid"].as_str().map(str::to_owned))
        });
        if let Ok(mut current) = claude.leaf.lock() {
            *current = leaf;
        }
    }

    let (turns, queued) = channel::<(String, String)>();
    let working = claude.clone();
    std::thread::spawn(move || {
        while let Ok((uuid, text)) = queued.recv() {
            working.turn(&uuid, &text);
        }
    });

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match message["type"].as_str() {
            Some("control_request") => {
                let id = message["request_id"].as_str().unwrap_or_default();
                claude.control(id, &message["request"]);
            }
            Some("control_response") => {
                let id = message["response"]["request_id"]
                    .as_str()
                    .unwrap_or_default();
                let answer = claude
                    .waiting
                    .lock()
                    .ok()
                    .and_then(|mut waiting| waiting.remove(id));
                if let Some(answer) = answer {
                    let _ = answer.send(message["response"]["response"].clone());
                }
            }
            Some("user") => {
                let uuid = message["uuid"].as_str().unwrap_or_default().to_owned();
                let text: String = message["message"]["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|block| block["text"].as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let record = json!({
                    "type": "user",
                    "uuid": uuid,
                    "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
                });
                claude.record(record);
                let mut echo = message.clone();
                echo["isReplay"] = json!(true);
                claude.send(echo);
                if message.get("priority").and_then(Value::as_str) == Some("now") {
                    claude.interrupted.store(true, Ordering::Release);
                }
                let _ = turns.send((uuid, text));
            }
            _ => {}
        }
    }
}
