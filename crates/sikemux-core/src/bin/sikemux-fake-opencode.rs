//! A stand-in for `opencode serve --stdio` in tests: it serves just enough of OpenCode's HTTP
//! API and event stream for the core to open a session, prompt it, stop it, answer what it
//! asks and pick its model. It prints where it listens, as OpenCode does, and leaves once its
//! input closes.
//!
//! A prompt's first word picks what the turn does:
//! - `stream N` sends N pieces of text;
//! - `ask` asks to run `echo hi` and says which answer it got, or `allowed` at once when the
//!   rules allow it;
//! - `push` does the same for `git push`, which the person's own config denies;
//! - `count` says how many messages came before it;
//! - `hold MS` says `holding`, waits, then says `held`, or stops early on an interrupt;
//!   `hold PATH` waits while that file is there;
//! - `fail` fails the turn as a provider that will not answer;
//! - `todo` writes a todo list of two;
//! - `subagent` starts an `explore` subagent, which says `found it` once the turn has ended;
//! - `question` asks with a form and says what it was told in place of an answer;
//! - anything else is echoed back.
//!
//! Compacting runs on its own, as OpenCode's `/compact` does.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

#[derive(Default)]
struct Fake {
    password: String,
    listeners: Mutex<Vec<TcpStream>>,
    sessions: Mutex<HashMap<String, Value>>,
    answers: Mutex<HashMap<String, Sender<String>>>,
    seq: AtomicU64,
    log: Mutex<Vec<Value>>,
    interrupted: AtomicBool,
}

/// The build agent's permission rules: OpenCode's catch-all, then the person's own config,
/// which denies `git push`.
fn build_rules() -> Value {
    json!([
        { "action": "*", "resource": "*", "effect": "allow" },
        { "action": "shell", "resource": "git push *", "effect": "deny" },
    ])
}

fn uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn models() -> Value {
    json!([
        {
            "id": "fake-model", "providerID": "fake", "name": "Fake", "enabled": true,
            "variants": [{ "id": "low" }, { "id": "high" }],
            "limit": { "context": 100000 },
        },
        {
            "id": "plain", "providerID": "fake", "name": "Plain", "enabled": true,
            "limit": { "context": 1000 },
        },
    ])
}

struct Request {
    method: String,
    path: String,
    authorized: bool,
    body: Value,
}

fn read_request(stream: &TcpStream, password: &str) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let method = words.next()?.to_owned();
    let path = words.next()?.to_owned();
    let mut length = 0;
    let mut authorized = false;
    let expected = format!("opencode:{password}");
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "authorization" => {
                authorized = value
                    .strip_prefix("Basic ")
                    .and_then(decode_base64)
                    .is_some_and(|said| said == expected)
            }
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        authorized,
        body: serde_json::from_slice(&body).unwrap_or_default(),
    })
}

fn decode_base64(text: &str) -> Option<String> {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut held = 0;
    let mut out = Vec::new();
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        bits = bits << 6 | LETTERS.iter().position(|letter| *letter == byte)? as u32;
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
        }
    }
    String::from_utf8(out).ok()
}

fn answer(mut stream: &TcpStream, status: u16, body: Value) {
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

impl Fake {
    fn emit(&self, kind: &str, data: Value) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let event = json!({ "type": kind, "data": data, "durable": { "seq": seq } });
        let frame = format!("data: {event}\n\n");
        self.log.locked().push(event);
        self.listeners
            .locked()
            .retain_mut(|stream| stream.write_all(frame.as_bytes()).is_ok());
    }

    fn serve(self: &Arc<Self>, stream: TcpStream) {
        let Some(request) = read_request(&stream, &self.password) else {
            return;
        };
        if !request.authorized {
            return answer(&stream, 401, json!({ "message": "unauthorized" }));
        }
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or_default()
            .to_owned();
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (request.method.as_str(), parts.as_slice()) {
            ("GET", ["api", "event"]) => {
                let mut stream = stream;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";
                let connected = json!({ "type": "server.connected", "data": {} });
                if write!(stream, "{head}data: {connected}\n\n").is_ok() {
                    self.listeners.locked().push(stream);
                }
            }
            ("GET", ["api", "model"]) => answer(&stream, 200, json!({ "data": models() })),
            ("GET", ["api", "model", "default"]) => {
                answer(&stream, 200, json!({ "data": models()[0] }))
            }
            ("GET", ["api", "agent"]) => answer(
                &stream,
                200,
                json!({ "data": [
                    { "id": "build", "name": "Build", "mode": "primary", "permissions": build_rules() },
                    { "id": "plan", "name": "Plan", "mode": "primary" },
                    { "id": "general", "name": "General", "mode": "subagent" },
                ] }),
            ),
            ("PUT", ["api", "experimental", "mcp", _]) => answer(&stream, 200, json!({})),
            ("GET", ["api", "command"]) => answer(
                &stream,
                200,
                json!({ "data": [{ "name": "review", "description": "Review changes" }] }),
            ),
            ("POST", ["api", "session"]) => {
                let id = format!("ses_{}", uuid());
                let session = json!({
                    "id": id,
                    "title": "",
                    "model": request.body["model"],
                    "agent": request.body["agent"],
                    "permissions": request.body["permissions"],
                    "messages": [],
                });
                self.sessions.locked().insert(id, session.clone());
                answer(&stream, 200, json!({ "data": session }));
            }
            ("GET", ["api", "session", id]) => match self.sessions.locked().get(*id) {
                Some(session) => answer(&stream, 200, json!({ "data": session })),
                None => answer(&stream, 404, json!({ "message": "no such session" })),
            },
            ("PATCH", ["api", "session", id]) => {
                if let Some(session) = self.sessions.locked().get_mut(*id) {
                    if let Some(permissions) = request.body.get("permissions") {
                        session["permissions"] = permissions.clone();
                    }
                }
                answer(&stream, 200, json!({}));
            }
            ("POST", ["api", "session", id, "revert", "stage"]) => {
                if let Some(session) = self.sessions.locked().get_mut(*id) {
                    session["revert"] = request.body["messageID"].clone();
                }
                answer(
                    &stream,
                    200,
                    json!({ "data": { "messageID": request.body["messageID"] } }),
                );
            }
            ("POST", ["api", "session", id, "revert", "commit"]) => {
                if let Some(session) = self.sessions.locked().get_mut(*id) {
                    let revert = session["revert"].take();
                    if let Some(messages) = session["messages"].as_array_mut() {
                        if let Some(at) = messages.iter().position(|message| *message == revert) {
                            messages.truncate(at);
                        }
                    }
                }
                answer(&stream, 204, json!({}));
            }
            ("POST", ["api", "session", id, "model"]) => {
                if let Some(session) = self.sessions.locked().get_mut(*id) {
                    session["model"] = request.body["model"].clone();
                }
                answer(&stream, 200, json!({}));
                self.emit("session.model.selected", json!({ "sessionID": id }));
            }
            ("POST", ["api", "session", id, "agent"]) => {
                answer(&stream, 200, json!({}));
                self.emit("session.agent.selected", json!({ "sessionID": id }));
            }
            ("POST", ["api", "session", _, "interrupt"]) => {
                self.interrupted.store(true, Ordering::SeqCst);
                answer(&stream, 200, json!({}));
            }
            ("POST", ["api", "session", _, "permission", id, "reply"]) => {
                answer(&stream, 200, json!({}));
                let decision = request.body["decision"].as_str().unwrap_or_default();
                if let Some(waiting) = self.answers.locked().remove(*id) {
                    let _ = waiting.send(decision.to_owned());
                }
            }
            ("POST", ["api", "session", id, "prompt"]) => {
                let message = format!("msg_{}", uuid());
                if let Some(session) = self.sessions.locked().get_mut(*id) {
                    if let Some(messages) = session["messages"].as_array_mut() {
                        messages.push(json!(message));
                    }
                }
                answer(
                    &stream,
                    200,
                    json!({ "data": { "id": message, "sessionID": id, "type": "user" } }),
                );
                let fake = self.clone();
                let session = (*id).to_owned();
                let text = request.body["text"].as_str().unwrap_or_default().to_owned();
                std::thread::spawn(move || fake.turn(&session, &text));
            }
            ("POST", ["api", "session", id, "compact"]) => {
                answer(&stream, 200, json!({}));
                let fake = self.clone();
                let session = (*id).to_owned();
                std::thread::spawn(move || fake.compact(&session));
            }
            ("DELETE", ["api", "session", _, "form", id]) => {
                answer(&stream, 200, json!({}));
                let message = url::Url::parse(&format!("http://fake{}", request.path))
                    .ok()
                    .and_then(|url| {
                        url.query_pairs()
                            .find(|(key, _)| key == "message")
                            .map(|(_, message)| message.into_owned())
                    })
                    .unwrap_or_default();
                if let Some(waiting) = self.answers.locked().remove(*id) {
                    let _ = waiting.send(message);
                }
            }
            ("POST", ["api", "session", id, "command"]) => {
                answer(&stream, 200, json!({}));
                let fake = self.clone();
                let session = (*id).to_owned();
                let text = format!("ran {}", request.body["name"].as_str().unwrap_or_default());
                std::thread::spawn(move || fake.turn(&session, &text));
            }
            ("GET", ["api", "experimental", "session", id, "log"]) => {
                let after = request
                    .path
                    .split_once("after=")
                    .and_then(|(_, after)| after.parse::<u64>().ok())
                    .unwrap_or(0);
                let missed: Vec<Value> = self
                    .log
                    .locked()
                    .iter()
                    .filter(|event| event["durable"]["seq"].as_u64() > Some(after))
                    .filter(|event| event["data"]["sessionID"] == *id)
                    .cloned()
                    .collect();
                answer(&stream, 200, json!({ "data": missed }))
            }
            _ => answer(
                &stream,
                404,
                json!({ "message": format!("no route {path}") }),
            ),
        }
    }

    fn compact(&self, session: &str) {
        self.emit("session.execution.started", json!({ "sessionID": session }));
        let reason = json!({ "sessionID": session, "reason": "manual" });
        self.emit("session.compaction.started", reason.clone());
        let mut ended = reason;
        ended["text"] = json!("summary");
        self.emit("session.compaction.ended", ended);
        self.emit(
            "session.execution.succeeded",
            json!({ "sessionID": session }),
        );
    }

    /// A subagent's own run, which says `found it`.
    fn subagent(&self, child: &str) {
        self.emit("session.execution.started", json!({ "sessionID": child }));
        let data = json!({
            "sessionID": child, "assistantMessageID": format!("msg_{}", uuid()), "ordinal": 0,
        });
        let mut delta = data.clone();
        delta["delta"] = json!("found it");
        self.emit("session.text.delta", delta);
        let mut ended = data;
        ended["text"] = json!("found it");
        self.emit("session.text.ended", ended);
        self.emit(
            "session.step.ended",
            json!({
                "sessionID": child, "finish": "stop", "cost": 0.01,
                "tokens": { "input": 300, "output": 20, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
            }),
        );
        self.emit("session.execution.succeeded", json!({ "sessionID": child }));
    }

    /// What OpenCode's rules say to running `command`: the build agent's, then the session's,
    /// the last match winning.
    fn effect(&self, session: &str, command: &str) -> String {
        let sessions = self.sessions.locked();
        let session_rules = sessions[session]["permissions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let matches = |pattern: &str| {
            pattern == "*"
                || pattern == command
                || pattern
                    .strip_suffix(" *")
                    .is_some_and(|stem| command == stem || command.starts_with(&format!("{stem} ")))
        };
        let agent_rules = build_rules().as_array().cloned().unwrap_or_default();
        agent_rules
            .into_iter()
            .chain(session_rules)
            .filter(|rule| rule["action"] == "*" || rule["action"] == "shell")
            .rfind(|rule| matches(rule["resource"].as_str().unwrap_or_default()))
            .and_then(|rule| rule["effect"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn ask(&self, session: &str, message: &str, call: &str, command: &str) -> String {
        let asked = format!("per_{}", uuid());
        let (answered, heard) = channel();
        self.answers.locked().insert(asked.clone(), answered);
        self.emit(
            "permission.asked",
            json!({
                "sessionID": session, "id": asked, "action": "shell",
                "resources": [command], "source": { "messageID": message, "id": call },
            }),
        );
        let decision = heard.recv().unwrap_or_default();
        self.emit(
            "permission.replied",
            json!({ "sessionID": session, "requestID": asked, "reply": decision }),
        );
        decision
    }

    fn turn(&self, session: &str, text: &str) {
        self.interrupted.store(false, Ordering::SeqCst);
        let message = format!("msg_{}", uuid());
        let base = json!({ "sessionID": session, "assistantMessageID": message, "ordinal": 0 });
        let with = |extra: Value| {
            let mut data = base.clone();
            for (key, value) in extra.as_object().into_iter().flatten() {
                data[key] = value.clone();
            }
            data
        };
        self.emit("session.execution.started", json!({ "sessionID": session }));
        let mut said = String::new();
        let mut say = |piece: &str| {
            said.push_str(piece);
            self.emit("session.text.delta", with(json!({ "delta": piece })));
        };
        let (verb, rest) = text.split_once(' ').unwrap_or((text, ""));
        let mut ending = "session.execution.succeeded";
        let mut error = Value::Null;
        let mut subagent = None;
        match verb {
            "stream" => (0..rest.parse().unwrap_or(1)).for_each(|n| say(&format!("w{n} "))),
            "count" => {
                let sessions = self.sessions.locked();
                let sent = sessions[session]["messages"].as_array().map_or(0, Vec::len);
                drop(sessions);
                say(&format!("{} earlier", sent.saturating_sub(1)));
            }
            "ask" | "push" => {
                let command = if verb == "push" {
                    "git push"
                } else {
                    "echo hi"
                };
                let call = format!("call_{}", uuid());
                self.emit(
                    "session.tool.input.started",
                    json!({ "sessionID": session, "id": call, "name": "bash" }),
                );
                let input = json!({ "command": command });
                self.emit(
                    "session.tool.called",
                    json!({ "sessionID": session, "id": call, "input": input }),
                );
                let decision = match self.effect(session, command).as_str() {
                    "allow" => "once".to_owned(),
                    "deny" => "deny".to_owned(),
                    _ => self.ask(session, &message, &call, command),
                };
                self.emit(
                    "session.tool.success",
                    json!({
                        "sessionID": session, "id": call,
                        "content": [{ "type": "text", "text": "hi" }], "metadata": {},
                    }),
                );
                say(match decision.as_str() {
                    "reject" => "rejected",
                    "deny" => "denied",
                    _ => "allowed",
                });
            }
            "hold" => {
                say("holding");
                let until = rest
                    .parse()
                    .ok()
                    .map(|ms| Instant::now() + Duration::from_millis(ms));
                let waiting = || match until {
                    Some(until) => Instant::now() < until,
                    None => std::path::Path::new(rest).exists(),
                };
                while waiting() && !self.interrupted.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                match self.interrupted.load(Ordering::SeqCst) {
                    true => ending = "session.execution.interrupted",
                    false => say("held"),
                }
            }
            "todo" => {
                let call = format!("call_{}", uuid());
                self.emit(
                    "session.tool.input.started",
                    json!({ "sessionID": session, "id": call, "name": "todowrite" }),
                );
                let input = json!({ "todos": [
                    { "content": "look", "status": "completed" },
                    { "content": "fix", "status": "in_progress" },
                ] });
                self.emit(
                    "session.tool.called",
                    json!({ "sessionID": session, "id": call, "input": input }),
                );
                self.emit(
                    "session.tool.success",
                    json!({
                        "sessionID": session, "id": call,
                        "content": [{ "type": "text", "text": "Todos updated" }], "metadata": {},
                    }),
                );
                say("planned");
            }
            "subagent" => {
                let child = format!("ses_{}", uuid());
                self.emit(
                    "session.created",
                    json!({
                        "sessionID": child, "parentID": session, "agent": "explore",
                        "title": "Look around", "slug": "look", "version": "2",
                    }),
                );
                say("started");
                subagent = Some(child);
            }
            "question" => {
                let form = format!("frm_{}", uuid());
                let (answered, heard) = channel();
                self.answers.locked().insert(form.clone(), answered);
                self.emit(
                    "form.created",
                    json!({ "form": {
                        "id": form, "sessionID": session, "title": "Questions",
                        "metadata": { "kind": "question" },
                        "fields": [{
                            "key": "q0", "title": "Colour", "description": "Which colour?",
                            "type": "string", "custom": true,
                            "options": [{ "value": "Red", "label": "Red" }, { "value": "Blue", "label": "Blue" }],
                        }],
                    } }),
                );
                let told = heard.recv().unwrap_or_default();
                say(&format!("told: {told}"));
            }
            "fail" => {
                ending = "session.execution.failed";
                error = json!({ "type": "provider.auth", "message": "Not signed in" });
            }
            _ => say(text),
        }
        self.emit("session.text.ended", with(json!({ "text": said })));
        self.emit(
            "session.step.ended",
            json!({
                "sessionID": session, "finish": "stop", "cost": 0.01,
                "tokens": { "input": 100, "output": 10, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
            }),
        );
        self.emit(ending, json!({ "sessionID": session, "error": error }));
        if let Some(child) = subagent {
            self.subagent(&child);
        }
    }
}

fn main() {
    let password = std::env::var("OPENCODE_PASSWORD").unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("serve") {
        eprintln!("the fake OpenCode only serves");
        std::process::exit(2);
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let port = listener.local_addr().expect("an address").port();
    println!("{}", json!({ "url": format!("http://127.0.0.1:{port}") }));
    let _ = std::io::stdout().flush();
    let fake = Arc::new(Fake {
        password,
        ..Fake::default()
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let fake = fake.clone();
            std::thread::spawn(move || fake.serve(stream));
        }
    });
    let mut input = String::new();
    while std::io::stdin()
        .read_line(&mut input)
        .is_ok_and(|read| read > 0)
    {
        input.clear();
    }
}
