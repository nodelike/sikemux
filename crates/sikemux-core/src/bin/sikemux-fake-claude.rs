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
//! `FAKE_CLAUDE_SCRIPT` names a JSON file of replies for recorded demos, each
//! `{ "when": "words in the prompt", "steps": [...] }`, a step being
//! `{ "think": "..." }`, `{ "say": "..." }` or `{ "say_file": "reply.md" }`
//! (beside the script), streamed a word at a time at `perSecond` (30), or
//! `{ "tool": "Read", "input": {...}, "result": "...", "ms": 600 }`. While
//! `FAKE_CLAUDE_CLOCK` names a file holding a time in milliseconds, those
//! pauses follow that time rather than the real one, so a recording that
//! draws slower than real time still sees the reply arrive at a steady pace.
//!
//! With `FAKE_CLAUDE_OUT_OF_USAGE=1` every prompt fails as `limit` does. A
//! message under an id the transcript already has is dropped, as Claude Code
//! drops it.
//!
//! A model change is answered the way Claude Code does, with a message saying
//! so. `FAKE_CLAUDE_SETTINGS_MODEL` names the model its settings choose, and
//! `FAKE_CLAUDE_MODELS`, a JSON list, the models it lists in place of its own.
//!
//! An account is its `CLAUDE_CONFIG_DIR`: `auth status --json` prints its
//! `fake-account.json`, or a signed-out status when there is none, and a usage
//! request answers with its `fake-usage.json`, or no limits.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

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
        let mut out = self.out.locked();
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn record(&self, mut record: Value) {
        let uuid = record["uuid"].as_str().unwrap_or_default().to_owned();
        record["parentUuid"] = json!(self.leaf.locked().replace(uuid));
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
        let mut next = self.leaf.locked().clone();
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
        self.waiting.locked().insert(id.clone(), answer);
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

    /// Goes back to before the message `target`, as Claude Code does in place.
    fn rewind(&self, target: Option<&str>) -> Value {
        let found = self.records().into_iter().find(|record| {
            record["uuid"]
                .as_str()
                .is_some_and(|uuid| Some(uuid) == target)
        });
        let Some(found) = found else {
            return json!({ "rewound": false, "error": "target not found", "reason": "target_not_found" });
        };
        let parent = found["parentUuid"].as_str().map(str::to_owned);
        *self.leaf.locked() = parent.clone();
        json!({
            "rewound": true,
            "targetMessageUuid": found["uuid"],
            "prefillText": found["message"]["content"][0]["text"],
            "precedingAssistantUuid": parent,
        })
    }

    fn turn(&self, uuid: &str, text: &str) {
        self.interrupted.store(false, Ordering::Release);
        self.send(json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "started" }));
        let message_id = format!("msg-{uuid}");
        if let Some(steps) = scripted_reply(text) {
            self.play(uuid, &message_id, &steps);
            return;
        }
        let mut words = text.split_whitespace();
        let out_of_usage = std::env::var("FAKE_CLAUDE_OUT_OF_USAGE").as_deref() == Ok("1");
        let verb = words.next().unwrap_or_default();
        match if out_of_usage { "limit" } else { verb } {
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
            // Fails the first time for want of the network, and answers when sent again.
            "offline" => {
                static FAILED_ONCE: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !FAILED_ONCE.swap(true, Ordering::AcqRel) {
                    self.send(json!({
                        "type": "system", "subtype": "api_retry", "attempt": 1, "max_retries": 10,
                        "retry_delay_ms": 500, "error_status": null, "error": "unknown",
                    }));
                    self.finish(uuid, Some("API Error: Connection error."));
                    return;
                }
                let assistant = json!({
                    "type": "assistant",
                    "uuid": uuid::Uuid::new_v4().to_string(),
                    "parent_tool_use_id": null,
                    "message": { "id": message_id, "model": "fake", "role": "assistant", "content": [{ "type": "text", "text": "back online" }] },
                });
                self.record(assistant.clone());
                self.send(assistant);
                self.finish(uuid, None);
            }
            "retry" => {
                let count: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                for attempt in 1..=count {
                    self.send(json!({
                        "type": "system", "subtype": "api_retry", "attempt": attempt, "max_retries": 10,
                        "retry_delay_ms": 500, "error_status": null, "error": "unknown",
                    }));
                }
                // Claude waits before it retries, so the chat has time to say so.
                std::thread::sleep(std::time::Duration::from_millis(100));
                let assistant = json!({
                    "type": "assistant",
                    "uuid": uuid::Uuid::new_v4().to_string(),
                    "parent_tool_use_id": null,
                    "message": { "id": message_id, "model": "fake", "role": "assistant", "content": [{ "type": "text", "text": "back" }] },
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
            "question" => {
                let input = json!({ "questions": [
                    { "question": "Which database should the cache use?", "header": "Cache store", "options": [
                        { "label": "Redis (recommended)", "description": "Fast, and already running here" },
                        { "label": "SQLite", "description": "No server to keep up" }
                    ] },
                    { "question": "Which checks should run before landing?", "header": "Checks", "multiSelect": true, "options": [
                        { "label": "Unit tests" }, { "label": "Clippy" }, { "label": "Format" }
                    ] },
                    { "question": "What is the database password?", "header": "Password", "isSecret": true }
                ] });
                self.ask(json!({ "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input, "tool_use_id": format!("toolu-{uuid}"), "permission_suggestions": [] }));
                self.say(&message_id, "asked");
                self.finish(uuid, None);
            }
            "delegate" => {
                let (spawner, task, read) = (
                    format!("toolu-task-{uuid}"),
                    format!("task-{uuid}"),
                    format!("toolu-read-{uuid}"),
                );
                let assistant = |id: String, parent: Option<&str>, content: Value| {
                    json!({ "type": "assistant", "uuid": uuid::Uuid::new_v4().to_string(), "parent_tool_use_id": parent,
                        "message": { "id": id, "model": "fake", "role": "assistant", "content": content } })
                };
                let result = |parent: Option<&str>, tool: &str, said: &str| {
                    json!({ "type": "user", "uuid": uuid::Uuid::new_v4().to_string(), "parent_tool_use_id": parent,
                        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": tool, "content": said }] } })
                };
                let input = json!({ "description": "Find the adapter", "prompt": "Find where the chat adapter starts\nThen report back", "subagent_type": "Explore" });
                self.send(assistant(
                    format!("{message_id}-task"),
                    None,
                    json!([{ "type": "tool_use", "id": spawner, "name": "Task", "input": input }]),
                ));
                self.send(json!({ "type": "system", "subtype": "task_started", "task_id": task, "tool_use_id": spawner, "description": "Find the adapter", "subagent_type": "Explore", "task_type": "local_agent" }));
                let looking = json!([{ "type": "text", "text": "Looking at the adapter." }, { "type": "tool_use", "id": read, "name": "Read", "input": { "file_path": "/tmp/src/adapter.ts" } }]);
                self.send(assistant(
                    format!("{message_id}-sub"),
                    Some(&spawner),
                    looking,
                ));
                self.send(result(Some(&spawner), &read, "export function start() {}"));
                let ms: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                std::thread::sleep(Duration::from_millis(ms));
                self.send(json!({ "type": "system", "subtype": "task_notification", "task_id": task, "status": "completed", "summary": "Found it" }));
                self.send(result(
                    None,
                    &spawner,
                    "The adapter starts in src/adapter.ts",
                ));
                self.say(&message_id, "It starts in src/adapter.ts.");
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
                let resets_at = sikemux_process::clock::epoch_secs() + 3600;
                self.send(json!({
                    "type": "assistant",
                    "parent_tool_use_id": null,
                    "error": "rate_limit",
                    "api_error": "usage_limit_reached",
                    "api_error_params": { "rate_limit_info": { "status": "rejected", "resetsAt": resets_at, "rateLimitType": "five_hour" } },
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

    /// Plays a reply from `FAKE_CLAUDE_SCRIPT`, each step its own message.
    fn play(&self, uuid: &str, message_id: &str, steps: &[Value]) {
        let mut clock = Clock::start();
        for (at, step) in steps.iter().enumerate() {
            let id = format!("{message_id}-{at}");
            let per_second = step["perSecond"].as_f64().unwrap_or(30.).max(1.);
            if let Some(name) = step["tool"].as_str() {
                let tool_id = format!("toolu-{uuid}-{at}");
                self.send(json!({
                    "type": "assistant", "uuid": uuid::Uuid::new_v4().to_string(), "parent_tool_use_id": null,
                    "message": { "id": id, "model": "fake", "role": "assistant",
                        "content": [{ "type": "tool_use", "id": tool_id, "name": name, "input": step["input"] }] },
                }));
                clock.wait(step["ms"].as_f64().unwrap_or(600.));
                self.send(json!({
                    "type": "user", "uuid": uuid::Uuid::new_v4().to_string(), "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": tool_id,
                        "content": step["result"].as_str().unwrap_or_default() }] },
                }));
                continue;
            }
            let (kind, said) = match (step["think"].as_str(), step["say"].as_str()) {
                (Some(thought), _) => ("thinking", thought.to_owned()),
                (_, Some(said)) => ("text", said.to_owned()),
                _ => (
                    "text",
                    script_file(step["say_file"].as_str().unwrap_or_default()),
                ),
            };
            self.send(json!({
                "type": "stream_event", "parent_tool_use_id": null,
                "event": { "type": "message_start", "message": { "id": id, "model": "fake" } },
            }));
            for piece in said.split_inclusive(char::is_whitespace) {
                let delta = if kind == "text" {
                    json!({ "type": "text_delta", "text": piece })
                } else {
                    json!({ "type": "thinking_delta", "thinking": piece })
                };
                self.send(json!({
                    "type": "stream_event", "parent_tool_use_id": null,
                    "event": { "type": "content_block_delta", "index": 0, "delta": delta },
                }));
                if !piece.trim().is_empty() {
                    clock.wait(1000. / per_second);
                }
            }
            let block = if kind == "text" {
                json!({ "type": "text", "text": said })
            } else {
                json!({ "type": "thinking", "thinking": said })
            };
            let assistant = json!({
                "type": "assistant", "uuid": uuid::Uuid::new_v4().to_string(), "parent_tool_use_id": null,
                "message": { "id": id, "model": "fake", "role": "assistant", "content": [block] },
            });
            self.record(assistant.clone());
            self.send(assistant);
            clock.wait(step["after"].as_f64().unwrap_or(150.));
        }
        self.finish(uuid, None);
    }

    fn control(&self, request_id: &str, request: &Value) {
        let response = match request["subtype"].as_str() {
            Some("initialize") => json!({
                "models": std::env::var("FAKE_CLAUDE_MODELS")
                    .ok()
                    .and_then(|models| serde_json::from_str::<Value>(&models).ok())
                    .unwrap_or_else(|| json!([
                        { "value": "default", "resolvedModel": "fake", "displayName": "Default", "description": "Fake", "supportsEffort": true, "supportedEffortLevels": ["low", "high"] },
                        { "value": "small", "resolvedModel": "fake-small", "displayName": "Small", "description": "Smaller" },
                    ])),
                "commands": [{ "name": "review", "description": "Review the change", "argumentHint": "" }],
            }),
            Some("interrupt") => {
                self.interrupted.store(true, Ordering::Release);
                json!({ "still_queued": [] })
            }
            Some("rewind_conversation") => self.rewind(request["target_message_uuid"].as_str()),
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
            Some("get_usage") => account_file("fake-usage.json")
                .unwrap_or_else(|| json!({ "rate_limits_available": false })),
            _ => json!({}),
        };
        self.send(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        }));
    }
}

/// The steps of the first reply in `FAKE_CLAUDE_SCRIPT` whose words `prompt` holds.
fn scripted_reply(prompt: &str) -> Option<Vec<Value>> {
    let script: Value =
        serde_json::from_slice(&std::fs::read(std::env::var_os("FAKE_CLAUDE_SCRIPT")?).ok()?)
            .ok()?;
    let prompt = prompt.to_lowercase();
    script.as_array()?.iter().find_map(|reply| {
        let when = reply["when"].as_str()?.to_lowercase();
        prompt
            .contains(&when)
            .then(|| reply["steps"].as_array().cloned())
            .flatten()
    })
}

/// A file named beside `FAKE_CLAUDE_SCRIPT`.
fn script_file(name: &str) -> String {
    std::env::var_os("FAKE_CLAUDE_SCRIPT")
        .map(PathBuf::from)
        .and_then(|script| std::fs::read_to_string(script.with_file_name(name)).ok())
        .unwrap_or_default()
}

/// Paces a scripted reply by `FAKE_CLAUDE_CLOCK`'s time when it is set, else by the real one.
struct Clock {
    file: Option<PathBuf>,
    at: f64,
}

impl Clock {
    fn start() -> Self {
        let file = std::env::var_os("FAKE_CLAUDE_CLOCK").map(PathBuf::from);
        let at = file.as_deref().map_or(0., Self::read);
        Self { file, at }
    }

    fn read(file: &std::path::Path) -> f64 {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|said| said.trim().parse().ok())
            .unwrap_or(0.)
    }

    fn wait(&mut self, ms: f64) {
        self.at += ms;
        match &self.file {
            Some(file) => {
                while Self::read(file) < self.at {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            None => std::thread::sleep(Duration::from_secs_f64(ms / 1000.)),
        }
    }
}

/// A file of the account the process runs as, from its config folder.
fn account_file(name: &str) -> Option<Value> {
    let folder = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR")?);
    serde_json::from_slice(&std::fs::read(folder.join(name)).ok()?).ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1..] == ["auth", "status", "--json"] {
        let status =
            account_file("fake-account.json").unwrap_or_else(|| json!({ "loggedIn": false }));
        println!("{status}");
        return;
    }
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
        let leaf = records
            .iter()
            .rev()
            .find_map(|record| record["uuid"].as_str().map(str::to_owned));
        *claude.leaf.locked() = leaf;
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
                let answer = claude.waiting.locked().remove(id);
                if let Some(answer) = answer {
                    let _ = answer.send(message["response"]["response"].clone());
                }
            }
            Some("user") => {
                let uuid = message["uuid"].as_str().unwrap_or_default().to_owned();
                if claude.records().iter().any(|record| record["uuid"] == uuid) {
                    continue;
                }
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
                let _ = turns.send((uuid, text));
            }
            _ => {}
        }
    }
}
