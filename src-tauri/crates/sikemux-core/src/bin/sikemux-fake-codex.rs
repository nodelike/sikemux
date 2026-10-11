//! A stand-in for `codex app-server` in tests: it speaks just enough of the
//! app server's JSON-RPC for the core to start a thread, prompt it, stop it,
//! resume it and take it back to before a turn. Threads are kept in
//! `CODEX_HOME/fake-threads`, so a new process can resume what an earlier one
//! said.
//!
//! A prompt's first word picks what the turn does:
//! - `stream N` sends N pieces of text;
//! - `ask` asks to run a command and says which answer it got;
//! - `hold MS` says `holding`, waits, then says `held`, or stops early on an
//!   interrupt;
//! - `count` says how many turns came before this one;
//! - `limit` fails the way Codex does when the account is out of usage;
//! - anything else is echoed back.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Codex {
    out: Mutex<std::io::Stdout>,
    waiting: Mutex<HashMap<u64, Sender<Value>>>,
    next_request: AtomicU64,
    interrupted: AtomicBool,
    home: PathBuf,
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Codex {
    fn send(&self, message: Value) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{message}");
            let _ = out.flush();
        }
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(json!({ "id": id, "result": result }));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "method": method, "params": params }));
    }

    fn path(&self, thread_id: &str) -> PathBuf {
        self.home
            .join("fake-threads")
            .join(format!("{thread_id}.json"))
    }

    fn turns(&self, thread_id: &str) -> Option<Vec<Value>> {
        let text = std::fs::read_to_string(self.path(thread_id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, thread_id: &str, turns: &[Value]) {
        let path = self.path(thread_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, Value::Array(turns.to_vec()).to_string());
    }

    fn thread(&self, thread_id: &str, turns: Vec<Value>) -> Value {
        json!({
            "thread": { "id": thread_id, "status": { "type": "idle" }, "turns": turns, "name": null, "historyMode": "paginated" },
            "model": "fake",
            "reasoningEffort": "medium",
        })
    }

    fn ask(&self, method: &str, params: Value) -> Value {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed) + 1_000;
        let (answer, answered) = channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(id, answer);
        }
        self.send(json!({ "id": id, "method": method, "params": params }));
        answered
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or(Value::Null)
    }

    fn say(&self, thread_id: &str, turn_id: &str, items: &mut Vec<Value>, text: &str) {
        let id = format!("msg-{}", uuid());
        self.notify(
            "item/started",
            json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "agentMessage", "id": id, "text": "" } }),
        );
        self.notify(
            "item/agentMessage/delta",
            json!({ "threadId": thread_id, "turnId": turn_id, "itemId": id, "delta": text }),
        );
        let item =
            json!({ "type": "agentMessage", "id": id, "text": text, "phase": "final_answer" });
        self.notify(
            "item/completed",
            json!({ "threadId": thread_id, "turnId": turn_id, "item": item }),
        );
        items.push(item);
    }

    fn run(&self, thread_id: String, turn_id: String, input: Vec<Value>) {
        self.interrupted.store(false, Ordering::Release);
        let text: String = input
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let user = json!({ "type": "userMessage", "id": uuid(), "content": input });
        let mut items = vec![user.clone()];
        self.notify(
            "thread/status/changed",
            json!({ "threadId": thread_id, "status": { "type": "active", "activeFlags": [] } }),
        );
        self.notify("turn/started", json!({ "threadId": thread_id, "turn": { "id": turn_id, "items": [], "status": "inProgress" } }));
        self.notify(
            "item/started",
            json!({ "threadId": thread_id, "turnId": turn_id, "item": user }),
        );
        self.notify(
            "item/completed",
            json!({ "threadId": thread_id, "turnId": turn_id, "item": user }),
        );
        let earlier = self.turns(&thread_id).map_or(0, |turns| turns.len());
        let mut words = text.split_whitespace();
        let mut status = "completed";
        let mut error = Value::Null;
        match words.next().unwrap_or_default() {
            "stream" => {
                let count: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                let id = format!("msg-{}", uuid());
                self.notify(
                    "item/started",
                    json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "agentMessage", "id": id, "text": "" } }),
                );
                let mut said = String::new();
                for index in 0..count {
                    let piece = format!("w{index} ");
                    said.push_str(&piece);
                    self.notify(
                        "item/agentMessage/delta",
                        json!({ "threadId": thread_id, "turnId": turn_id, "itemId": id, "delta": piece }),
                    );
                }
                let item = json!({ "type": "agentMessage", "id": id, "text": said, "phase": "final_answer" });
                self.notify(
                    "item/completed",
                    json!({ "threadId": thread_id, "turnId": turn_id, "item": item }),
                );
                items.push(item);
            }
            "ask" => {
                let id = format!("exec-{}", uuid());
                let command = json!({ "type": "commandExecution", "id": id, "command": "echo hi", "cwd": "/tmp", "status": "inProgress", "commandActions": [], "aggregatedOutput": null, "exitCode": null });
                self.notify(
                    "item/started",
                    json!({ "threadId": thread_id, "turnId": turn_id, "item": command }),
                );
                let answer = self.ask(
                    "item/commandExecution/requestApproval",
                    json!({ "threadId": thread_id, "turnId": turn_id, "itemId": id, "command": "echo hi", "cwd": "/tmp", "reason": null }),
                );
                let allowed = matches!(
                    answer["decision"].as_str(),
                    Some("accept" | "acceptForSession")
                );
                let mut done = command.clone();
                done["status"] = json!(if allowed { "completed" } else { "declined" });
                if allowed {
                    done["aggregatedOutput"] = json!("hi\n");
                    done["exitCode"] = json!(0);
                }
                self.notify(
                    "item/completed",
                    json!({ "threadId": thread_id, "turnId": turn_id, "item": done }),
                );
                items.push(done);
                self.say(
                    &thread_id,
                    &turn_id,
                    &mut items,
                    if allowed { "allowed" } else { "denied" },
                );
            }
            "hold" => {
                let ms: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(1000);
                self.say(&thread_id, &turn_id, &mut items, "holding");
                let until = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < until {
                    if self.interrupted.load(Ordering::Acquire) {
                        status = "interrupted";
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                if status == "completed" {
                    self.say(&thread_id, &turn_id, &mut items, "held");
                }
            }
            "count" => self.say(
                &thread_id,
                &turn_id,
                &mut items,
                &format!("{earlier} earlier"),
            ),
            "limit" => {
                status = "failed";
                error = json!({ "message": "You've hit your usage limit. Try again at 3pm.", "codexErrorInfo": "usageLimitExceeded", "additionalDetails": null });
                self.notify(
                    "thread/status/changed",
                    json!({ "threadId": thread_id, "status": { "type": "systemError" } }),
                );
                self.notify("error", json!({ "error": error, "willRetry": false, "threadId": thread_id, "turnId": turn_id }));
            }
            _ => self.say(&thread_id, &turn_id, &mut items, &format!("echo: {text}")),
        }
        self.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": thread_id, "turnId": turn_id, "tokenUsage": { "total": { "totalTokens": 1200 }, "last": { "totalTokens": 1200 }, "modelContextWindow": 200000 } }),
        );
        if status != "failed" {
            self.notify(
                "thread/status/changed",
                json!({ "threadId": thread_id, "status": { "type": "idle" } }),
            );
        }
        let mut turns = self.turns(&thread_id).unwrap_or_default();
        turns.push(json!({ "id": turn_id, "items": items, "status": status, "error": error }));
        self.save(&thread_id, &turns);
        self.notify(
            "turn/completed",
            json!({ "threadId": thread_id, "turn": { "id": turn_id, "items": [], "status": status, "error": error } }),
        );
    }

    fn request(self: &Arc<Self>, id: &Value, method: &str, params: &Value) {
        let thread_id = params["threadId"].as_str().unwrap_or_default().to_owned();
        match method {
            "initialize" => self.reply(id, json!({ "userAgent": "fake-codex/0.0.0", "codexHome": self.home })),
            "account/read" => self.reply(id, json!({ "account": { "type": "apiKey" }, "requiresOpenaiAuth": true })),
            "model/list" => self.reply(
                id,
                json!({
                    "data": [
                        { "id": "fake", "model": "fake", "displayName": "Fake", "description": "A stand-in", "hidden": false, "isDefault": true,
                          "supportedReasoningEfforts": [{ "reasoningEffort": "low", "description": "Low" }, { "reasoningEffort": "medium", "description": "Medium" }, { "reasoningEffort": "high", "description": "High" }],
                          "defaultReasoningEffort": "medium", "inputModalities": ["text", "image"] },
                    ],
                    "nextCursor": null,
                }),
            ),
            "thread/start" => {
                let thread_id = uuid();
                self.save(&thread_id, &[]);
                let thread = self.thread(&thread_id, Vec::new());
                self.reply(id, thread.clone());
                self.notify("thread/started", json!({ "thread": thread["thread"] }));
            }
            "thread/resume" | "thread/read" => match self.turns(&thread_id) {
                Some(turns) => {
                    let shown = if params["excludeTurns"] == true { Vec::new() } else { turns };
                    self.reply(id, self.thread(&thread_id, shown));
                }
                None => self.send(json!({ "id": id, "error": { "code": -32600, "message": format!("thread not found: {thread_id}") } })),
            },
            "thread/turns/list" => {
                let turns = self.turns(&thread_id).unwrap_or_default();
                self.reply(id, json!({ "data": turns, "nextCursor": null }));
            }
            "turn/start" => {
                let turn_id = uuid();
                self.reply(id, json!({ "turn": { "id": turn_id, "items": [], "status": "inProgress" } }));
                let input = params["input"].as_array().cloned().unwrap_or_default();
                let codex = self.clone();
                std::thread::spawn(move || codex.run(thread_id, turn_id, input));
            }
            "turn/interrupt" => {
                self.interrupted.store(true, Ordering::Release);
                self.reply(id, json!({}));
            }
            "turn/steer" => self.reply(id, json!({ "turnId": params["expectedTurnId"] })),
            "thread/revert" => {
                let before = params["beforeTurnId"].as_str().unwrap_or_default();
                let mut turns = self.turns(&thread_id).unwrap_or_default();
                match turns.iter().position(|turn| turn["id"] == before) {
                    Some(position) => {
                        turns.truncate(position);
                        self.save(&thread_id, &turns);
                        self.reply(id, self.thread(&thread_id, Vec::new()));
                        self.notify("thread/reverted", json!({ "threadId": thread_id }));
                    }
                    None => self.send(json!({ "id": id, "error": { "code": -32600, "message": format!("turn not found: {before}") } })),
                }
            }
            _ => self.reply(id, json!({})),
        }
    }
}

fn main() {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let codex = Arc::new(Codex {
        out: Mutex::new(std::io::stdout()),
        waiting: Mutex::new(HashMap::new()),
        next_request: AtomicU64::new(0),
        interrupted: AtomicBool::new(false),
        home,
    });
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message["method"].as_str().map(str::to_owned);
        match (message.get("id"), method) {
            (Some(id), Some(method)) => codex.request(id, &method, &message["params"]),
            (Some(id), None) => {
                let answer = id
                    .as_u64()
                    .and_then(|id| codex.waiting.lock().ok()?.remove(&id));
                if let Some(answer) = answer {
                    let _ = answer.send(message["result"].clone());
                }
            }
            _ => {}
        }
    }
}
