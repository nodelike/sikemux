//! Muse Code, spoken to over its session protocol: JSON-RPC, one message per
//! line, on `muse serve`'s standard input and output. Every view item comes
//! out as the session updates the other agents send for the same work.

mod approvals;
mod commands;
mod config;
mod history;
mod items;
mod notifications;
mod state;

use sikemux_fold::UpdateKind;
use std::collections::HashMap;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use uuid::Uuid;

use crate::launch::PermissionMode;
use sikemux_wire::protocol::ChatLaunch;

use super::super::Respond;
use super::file_link;
use super::json::text;
use super::models::Models;
use super::parts::{parts, picture, Part};
use super::pending::{Answered, Purpose};
use super::process::{Lines, Process, Reading, Released, Writer};
use super::rpc::{error_text, Rpc, ANSWER_WAIT};
use super::rpc_driver::{self, RpcAgent};
use super::servers;
use super::session::{self, Backend, Opened, Prompt, Sink, Steered};
use crate::sessions::for_agent::context_text;
use approvals::Question;
use commands::Command;
use items::Follow;
use state::Session;

/// The largest picture handed to Muse, which takes at most ten megabytes in
/// one message once it is base64.
const MAX_IMAGE_BYTES: u64 = 7 * 1024 * 1024;

/// A command's id. Muse wants every command to carry a fresh UUIDv7, and names
/// a fresh turn by the id of the command that started it.
fn command_id() -> String {
    Uuid::now_v7().to_string()
}

type Driver = rpc_driver::Driver<Session>;

impl Driver {
    /// Asks Muse for what an item needs, read once the answer comes.
    fn follow(&self, follow: Follow) {
        let (method, params, purpose) = match follow {
            Follow::Subagent(child) => (
                "view/subscribe",
                json!({ "sessionId": child }),
                Purpose::Subscribe(child),
            ),
            Follow::ReadSubagent { child, result } => (
                "session/read",
                json!({ "sessionId": child, "excludeItems": false }),
                Purpose::ReadSubagent { child, result },
            ),
            Follow::Patch {
                session,
                call,
                output_ref,
            } => (
                "item/readOutput",
                json!({ "sessionId": session, "itemId": call, "outputRef": output_ref }),
                Purpose::Patch { session, call },
            ),
        };
        if let Err(error) = self.rpc.send_for(method, params, purpose) {
            eprintln!("sikemux core: Muse was not asked {method}: {error}");
        }
    }

    fn show_patch(&self, session: &str, call: &str, read: &Value) {
        let content = match read["encoding"].as_str() {
            Some("base64") => read["content"]
                .as_str()
                .and_then(|content| {
                    base64::engine::general_purpose::STANDARD
                        .decode(content)
                        .ok()
                })
                .and_then(|bytes| String::from_utf8(bytes).ok()),
            _ => read["content"].as_str().map(str::to_owned),
        };
        let Some(content) = content else {
            return;
        };
        let cwd = self.session.locked().cwd.clone();
        let blocks =
            items::patch_blocks(&content, &cwd, &|path| std::fs::read_to_string(path).ok());
        if !blocks.is_empty() {
            self.sink.update(
                        session,
                        json!({ "sessionUpdate": UpdateKind::ToolCallUpdate, "toolCallId": call, "content": blocks }),
                    );
        }
    }

    /// Puts one stage of an approval to the person, once.
    fn ask(&self, params: &Value) {
        let session = self.session.locked().session_for(params);
        let Some(question) = approvals::question(&session, params) else {
            return;
        };
        if !self.session.locked().ask(question.key()) {
            return;
        }
        let reply = Asked {
            session: params["sessionId"].as_str().unwrap_or_default().to_owned(),
            question,
        };
        let request = reply.question.request.clone();
        self.sink.ask(request, reply, |reply| self.responder(reply));
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Asked {
    session: String,
    question: Question,
}

impl RpcAgent for Session {
    type Reply = Asked;

    const TURN_ENDED: &'static [&'static str] = &["expected", "not running", "no running", "idle"];

    fn addressed(id: &str) -> Value {
        json!({ "sessionId": id, "commandId": command_id() })
    }

    fn turn_id_in(answer: &Value) -> Option<&str> {
        text(answer, "turnId")
    }

    fn carry_out(driver: &Driver, follow: Follow) {
        driver.follow(follow);
    }

    fn notification(driver: &Driver, method: &str, params: Value) {
        if matches!(method, "approval/requested" | "approval/updated") {
            driver.ask(&params);
            return;
        }
        let Some(notification) = notifications::read(method, params) else {
            return;
        };
        let effects = driver.session.locked().handle(notification);
        driver.apply(effects);
    }

    fn request(driver: &Driver, id: Value, method: &str, params: &Value) {
        match method {
            "approval/request" => {
                driver.rpc.respond(id, json!({}));
                driver.ask(params);
            }
            "userInput/request" => {
                driver.rpc.respond(id, json!({}));
                let told = driver.sink.question(params);
                let rpc = driver.rpc.clone();
                let params = json!({
                    "sessionId": params["sessionId"],
                    "commandId": command_id(),
                    "userInputId": params["userInputId"],
                    "clarification": { "format": "text", "content": told },
                });
                tokio::spawn(async move {
                    if let Err(error) = rpc.request("userInput/clarify", params).await {
                        eprintln!(
                            "sikemux core: Muse did not take the note on its question: {error}"
                        );
                    }
                });
            }
            _ => driver
                .rpc
                .refuse(id, &format!("Sikemux does not handle {method}")),
        }
    }

    fn purpose_answered(driver: &Driver, purpose: Purpose, result: Answered) {
        match (purpose, result) {
            (Purpose::Subscribe(child), Err(error)) => eprintln!(
                "sikemux core: Muse subagent {child} could not be followed: {}",
                error_text(&error)
            ),
            (Purpose::ReadSubagent { child, result }, read) => {
                let said: Vec<Value> = read
                    .ok()
                    .and_then(|read| read["history"]["items"].as_array().cloned())
                    .unwrap_or_default();
                let effects = if said.iter().any(|item| item["kind"] != "userMessage") {
                    driver.session.locked().told_subagent(&child, &said)
                } else {
                    result
                        .map(|result| driver.session.locked().subagent_said(&child, &result))
                        .unwrap_or_default()
                };
                driver.apply(effects);
            }
            (Purpose::Patch { session, call }, Ok(read)) => {
                driver.show_patch(&session, &call, &read)
            }
            _ => {}
        }
    }

    fn decide(driver: &Driver, reply: Asked, chosen: Option<String>) {
        let Asked { session, question } = reply;
        let Some(decision) = question.decision(&session, command_id(), chosen.as_deref()) else {
            return;
        };
        let sent = driver.rpc.start("approval/decide", decision);
        tokio::spawn(async move {
            let answered = match sent {
                Ok(sent) => sent.answer(Some(ANSWER_WAIT)).await,
                Err(error) => Err(error),
            };
            if let Err(error) = answered {
                eprintln!(
                    "sikemux core: Muse did not take an approval: {}",
                    error_text(&error)
                );
            }
        });
    }
}

/// What a Muse chat holds beside its connection and session, for a core that
/// takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MuseKept {
    session_tools: bool,
    #[serde(flatten)]
    kept: Kept,
}

/// What a Muse chat knows of its session, as it hands it to a core that
/// takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    cwd: String,
    models: Models,
    model: String,
    effort: Option<String>,
    mode: PermissionMode,
    /// What `skill/list` answered, for the commands the chat offers.
    skills: Value,
}

/// A running `muse serve` and the connection to it.
struct Host {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    /// Whether sessions can be given tool servers of their own.
    session_tools: bool,
}

impl Host {
    async fn start(
        launch: &ChatLaunch,
        mode: PermissionMode,
        driver: impl FnOnce(Rpc) -> Driver,
    ) -> Result<Self, String> {
        // `serve` has to come first, so the launch's own arguments follow it.
        let mut bare = launch.clone();
        let mut args = config::serve_args(mode);
        args.splice(1..1, std::mem::take(&mut bare.args));
        let (process, writer, lines) = Process::spawn(&bare, args, Vec::<(String, String)>::new())?;
        let rpc = Rpc::new(writer, true);
        let driver = driver(rpc);
        let (reading, _) = Reading::start(driver.clone(), lines);
        let initialized = driver
            .rpc
            .request(
                "initialize",
                json!({
                    "clientInfo": crate::sessions::client_info(),
                    "capabilities": {
                        "requestedCapabilities": ["sessionMcp"],
                        "userInputDialogs": true,
                    },
                }),
            )
            .await
            .map_err(|error| process.start_error(error))?;
        driver
            .rpc
            .notify("initialized", Value::Null)
            .map_err(|error| process.start_error(error))?;
        let session_tools = initialized["grantedCapabilities"]
            .as_array()
            .is_some_and(|granted| granted.iter().any(|name| name == "sessionMcp"));
        Ok(Self {
            process,
            driver,
            reading,
            session_tools,
        })
    }

    /// The per-session settings Muse takes when it opens or loads a session.
    fn session_config(&self, launch: &ChatLaunch) -> Value {
        let servers = config::tool_servers(&servers::tool_servers(launch));
        if self.session_tools && !servers.is_empty() {
            json!({ "mcpServers": servers })
        } else {
            json!({})
        }
    }
}

pub(crate) struct Muse {
    host: Host,
    launch: ChatLaunch,
    kept: Kept,
}

async fn list_models(rpc: &Rpc) -> Models {
    let listed = rpc.request("model/list", json!({})).await;
    config::models(
        listed
            .ok()
            .and_then(|listed| listed["models"].as_array().cloned())
            .unwrap_or_default(),
    )
}

/// The conversations of every subagent `items` started, and theirs.
async fn subagent_histories(rpc: &Rpc, items: &[Value]) -> HashMap<String, Vec<Value>> {
    let read = |child: String| async move {
        let read = rpc
            .request(
                "session/read",
                json!({ "sessionId": child, "excludeItems": false }),
            )
            .await
            .unwrap_or_else(|error| {
                eprintln!("sikemux core: Muse subagent history {child} could not be read: {error}");
                Value::Null
            });
        read["history"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    session::subagent_histories(history::subagents(items), read, history::subagents).await
}

/// A file the person attached, as Muse's input for it: the picture itself
/// when it is one Muse can look at, otherwise a link to it.
fn attachment(name: &str, uri: &str) -> Value {
    let image = picture(uri).and_then(|(path, media_type)| {
        if std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        Some(json!({
            "type": "image",
            "mediaType": media_type,
            "base64Data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }))
    });
    image.unwrap_or_else(|| json!({ "type": "text", "text": file_link(Some(name), uri) }))
}

fn input(parts: Vec<Part>) -> Vec<Value> {
    let said = |text: String| json!({ "type": "text", "text": text });
    parts
        .into_iter()
        .map(|part| match part {
            Part::Text(text) => said(text),
            Part::File { name, uri } => attachment(&name, &uri),
            Part::Context { uri, text } => said(format!(
                "{}\n{}",
                file_link(None, &uri),
                context_text(&uri, &text)
            )),
        })
        .collect()
}

impl Muse {
    fn driver(&self) -> &Driver {
        &self.host.driver
    }

    fn rpc(&self) -> &Rpc {
        &self.host.driver.rpc
    }

    fn session_id(&self) -> String {
        self.driver().session_id()
    }

    fn prompt_input(&self, prompt: &Prompt) -> Result<Vec<Value>, String> {
        Ok(input(parts(prompt)?))
    }

    async fn send_turn(&self, turn: u64, prompt: &Prompt, input: Vec<Value>) -> Result<(), String> {
        let id = command_id();
        let mut params = json!({
            "sessionId": self.session_id(),
            "commandId": id,
            "input": input,
            "displayText": prompt.text,
            "ifBusy": "queue",
        });
        if let Some(effort) = &self.kept.effort {
            params["reasoningEffort"] = json!(effort);
        }
        self.driver()
            .send_turn(turn, prompt, Some(&id), "turn/start", params)
            .await
    }

    async fn command(&self, turn: u64, prompt: &Prompt, command: Command) -> Result<(), String> {
        let session_id = self.session_id();
        match command {
            Command::Skill {
                selector,
                arguments,
            } => {
                let mut part = json!({ "type": "skill", "selector": selector });
                if !arguments.is_empty() {
                    part["arguments"] = json!(arguments);
                }
                return self.send_turn(turn, prompt, vec![part]).await;
            }
            Command::Compact => {
                self.rpc()
                    .request(
                        "session/compact",
                        json!({ "sessionId": session_id, "commandId": command_id() }),
                    )
                    .await?;
            }
            Command::Rename(name) => {
                self.rpc()
                    .request(
                        "session/rename",
                        json!({ "sessionId": session_id, "commandId": command_id(), "name": name }),
                    )
                    .await?;
            }
            Command::Status => {
                let (totals, context) = {
                    let session = self.driver().session.locked();
                    (session.totals.clone(), session.context.clone())
                };
                self.driver().say(&commands::status_text(&commands::Status {
                    model: &self.kept.model,
                    effort: self.kept.effort.as_deref(),
                    cwd: &self.kept.cwd,
                    approval: config::approval_mode(self.kept.mode),
                    sandbox: config::sandbox_name(self.kept.mode),
                    session: &session_id,
                    totals: totals.as_ref(),
                    context: context.as_ref(),
                }));
            }
            Command::Usage => {
                let read = self
                    .rpc()
                    .request("usage/read", json!({}))
                    .await
                    .unwrap_or(Value::Null);
                self.driver().say(&commands::usage_text(&read));
            }
            Command::Hint(said) => self.driver().say(&said),
        }
        self.driver().answered(turn);
        Ok(())
    }

    /// Opens the chat's session in `host`: the one it resumes, or a new one.
    /// Answers with Muse's account of the session and its history.
    async fn open(
        host: &Host,
        launch: &ChatLaunch,
        mode: PermissionMode,
        history: bool,
    ) -> Result<(Value, Vec<Value>), String> {
        let config = host.session_config(launch);
        let rpc = &host.driver.rpc;
        let failed = |error: String| host.process.start_error(error);
        let Some(resume_id) = &launch.resume_id else {
            let opened = rpc
                .request(
                    "session/start",
                    json!({
                        "commandId": command_id(),
                        "workspaceRoot": launch.cwd,
                        "approvalMode": config::approval_mode(mode),
                        "config": config,
                    }),
                )
                .await
                .map_err(failed)?;
            return Ok((opened["session"].clone(), Vec::new()));
        };
        let mut params = json!({
            "commandId": command_id(),
            "sessionId": resume_id,
            "config": config,
        });
        if history {
            params["history"] = json!("inline");
        } else {
            params["excludeItems"] = json!(true);
        }
        let resumed = rpc
            .request("session/resume", params)
            .await
            .map_err(failed)?;
        rpc.request(
            "session/setApprovalMode",
            json!({ "sessionId": resume_id, "commandId": command_id(), "mode": config::approval_mode(mode) }),
        )
        .await
        .map_err(failed)?;
        let mut items = resumed["history"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if history && items.is_empty() && resumed["history"]["mode"] != "inline" {
            items = page_history(rpc, resume_id).await;
        }
        Ok((resumed["session"].clone(), items))
    }

    async fn replay(&self, session: &Value, items: &[Value]) {
        let driver = self.driver();
        let session_id = self.session_id();
        if let Some(title) = history::session_title(session) {
            driver
                .sink
                .update(&session_id, sikemux_fold::update::session_title(&title));
            driver.session.locked().turns.titled = true;
        }
        let children = subagent_histories(&driver.rpc, items).await;
        let mut out = Vec::new();
        history::replay(&session_id, items, &children, &self.kept.cwd, &mut out);
        for (session, update) in out {
            driver.sink.update(&session, update);
        }
        driver.session.locked().turns.restore(history::turns(items));
        driver
            .sink
            .update(&session_id, sikemux_fold::update::history_loaded());
    }

    async fn offer_commands(&mut self) {
        self.kept.skills = self
            .rpc()
            .request("skill/list", json!({ "sessionId": self.session_id() }))
            .await
            .unwrap_or(Value::Null);
        self.driver().sink.update(
            &self.session_id(),
            json!({
                "sessionUpdate": UpdateKind::AvailableCommandsUpdate,
                "availableCommands": commands::available(&self.kept.skills),
            }),
        );
    }

    /// Starts `muse serve` again under `mode`, whose sandbox the running one
    /// cannot change, and takes the session up in it.
    async fn restart(&mut self, mode: PermissionMode) -> Result<(), String> {
        let old = self.host.driver.clone();
        let mut launch = self.launch.clone();
        launch.resume_id = Some(self.session_id());
        let host = Host::start(&launch, mode, |rpc| Driver { rpc, ..old }).await?;
        let old = std::mem::replace(&mut self.host, host);
        // The old host lets go of the session only once it has died.
        old.process.stop().await;
        Self::open(&self.host, &launch, mode, false).await.map(drop)
    }
}

/// Every item of a session's view, paged in when Muse would not hand its
/// history over whole.
async fn page_history(rpc: &Rpc, session_id: &str) -> Vec<Value> {
    let pages = rpc
        .pages("view/page", |cursor| {
            let mut params = json!({ "sessionId": session_id, "limit": 1000 });
            if !cursor.is_null() {
                params["cursor"] = cursor.clone();
            }
            params
        })
        .await
        .unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, Value> = HashMap::new();
    for page in &pages {
        for event in page["events"].as_array().into_iter().flatten() {
            let item = &event["params"]["item"];
            let Some(id) = item["itemId"].as_str() else {
                continue;
            };
            if !latest.contains_key(id) {
                order.push(id.to_owned());
            }
            latest.insert(id.to_owned(), item.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

impl Backend for Muse {
    type Reply = Asked;

    async fn start(
        launch: &ChatLaunch,
        _brief: Option<&str>,
        sink: Sink,
    ) -> Result<(Self, Opened), String> {
        let mode = launch.permission_mode;
        let cwd = launch.cwd.to_string_lossy().into_owned();
        // Muse asks again about approvals left waiting as soon as a session
        // loads, so the session is known by its id before then.
        let session = Session::new(launch.resume_id.clone().unwrap_or_default(), cwd.clone());
        let host = Host::start(launch, mode, |rpc| Driver::new(rpc, sink, session)).await?;
        let models = list_models(&host.driver.rpc).await;
        let (opened, items) = Self::open(&host, launch, mode, true).await?;
        let session_id = opened["sessionId"]
            .as_str()
            .ok_or("Muse opened a session without an id")?
            .to_owned();
        *host.driver.session.locked() = Session::new(session_id.clone(), cwd.clone());

        let model = launch
            .model
            .clone()
            .filter(|model| models.offers(model))
            .or_else(|| text(&opened, "modelId").map(str::to_owned))
            .or_else(|| models.default_model())
            .unwrap_or_default();
        if !model.is_empty() && opened["modelId"] != model.as_str() {
            host.driver
                .rpc
                .request(
                    "session/setModel",
                    json!({ "sessionId": session_id, "commandId": command_id(), "model": { "modelId": model } }),
                )
                .await
                .map_err(|error| host.process.start_error(error))?;
        }
        let effort = models.effort_for(&model, launch.effort.as_deref());

        let mut muse = Self {
            host,
            launch: launch.clone(),
            kept: Kept {
                cwd,
                models,
                model,
                effort,
                mode,
                skills: Value::Null,
            },
        };
        if launch.resume_id.is_some() {
            muse.replay(&opened, &items).await;
        }
        muse.offer_commands().await;
        let opened = Opened {
            session_id,
            capabilities: Opened::native_capabilities(),
            setup: json!({
                "configOptions": muse.kept.models.options("Muse", &muse.kept.model, muse.kept.effort.as_deref()),
            }),
        };
        Ok((muse, opened))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        if let Some(command) = commands::parse(&prompt.text, &self.kept.skills) {
            return self.command(turn, &prompt, command).await;
        }
        let input = self.prompt_input(&prompt)?;
        self.send_turn(turn, &prompt, input).await
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static {
        Driver::steered(
            self.driver()
                .send_steer(&prompt, || self.prompt_input(&prompt)),
        )
    }

    async fn cancel(&mut self) -> Result<(), String> {
        self.driver().cancel();
        Ok(())
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        if config::serve_args(mode) != config::serve_args(self.kept.mode) {
            self.restart(mode).await?;
        } else {
            self.rpc()
                .request(
                    "session/setApprovalMode",
                    json!({ "sessionId": self.session_id(), "commandId": command_id(), "mode": config::approval_mode(mode) }),
                )
                .await?;
        }
        self.kept.mode = mode;
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        let session_id = self.session_id();
        match config_id {
            "model" => {
                if !self.kept.models.offers(value) {
                    return Err(format!("Muse does not offer the model {value}"));
                }
                self.rpc()
                    .request(
                        "session/setModel",
                        json!({ "sessionId": session_id, "commandId": command_id(), "model": { "modelId": value } }),
                    )
                    .await?;
                self.kept.model = value.to_owned();
                self.kept.effort = self
                    .kept
                    .models
                    .effort_for(value, self.kept.effort.as_deref());
            }
            "reasoning_effort" => {
                if !self.kept.models.supports_effort(&self.kept.model, value) {
                    return Err(format!(
                        "{} does not offer the effort {value}",
                        self.kept.model
                    ));
                }
                self.rpc()
                    .request(
                        "session/setReasoningEffort",
                        json!({ "sessionId": session_id, "commandId": command_id(), "reasoningEffort": value }),
                    )
                    .await?;
                self.kept.effort = Some(value.to_owned());
            }
            _ => return Err(format!("Muse has no option {config_id}")),
        }
        Ok(self
            .kept
            .models
            .options("Muse", &self.kept.model, self.kept.effort.as_deref()))
    }

    async fn stop_task(&mut self, task_id: &str) -> Result<(), String> {
        self.rpc()
            .request(
                "task/stop",
                json!({ "sessionId": self.session_id(), "commandId": command_id(), "taskId": task_id }),
            )
            .await
            .map(|_| ())
    }

    /// Muse goes back by copying the session up to the turn before, so the
    /// chat goes on in the copy. Going back to before the first message starts
    /// a new session.
    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let (session_id, turn_id, before) = {
            let session = self.driver().session.locked();
            let turn_id = session
                .turns
                .message_turns
                .get(message_id)
                .cloned()
                .unwrap_or_else(|| message_id.to_owned());
            let before = session
                .turns
                .before(&turn_id)
                .map_err(|()| "That message is no longer part of this chat")?;
            (session.session_id.clone(), turn_id, before)
        };
        let config = self.host.session_config(&self.launch);
        let moved = match before {
            Some(before) => {
                self.rpc()
                    .request(
                        "session/fork",
                        json!({
                            "commandId": command_id(),
                            "sessionId": session_id,
                            "cutPoint": { "lastTurnId": before },
                            "excludeItems": true,
                        }),
                    )
                    .await?
            }
            None => {
                self.rpc()
                    .request(
                        "session/start",
                        json!({
                            "commandId": command_id(),
                            "workspaceRoot": self.launch.cwd,
                            "approvalMode": config::approval_mode(self.kept.mode),
                            "config": config,
                        }),
                    )
                    .await?
            }
        };
        let moved = moved["session"]["sessionId"]
            .as_str()
            .ok_or("Muse went back without naming the session it made")?
            .to_owned();
        self.rpc()
            .request(
                "session/setApprovalMode",
                json!({ "sessionId": moved, "commandId": command_id(), "mode": config::approval_mode(self.kept.mode) }),
            )
            .await?;
        if !self.kept.model.is_empty() {
            self.rpc()
                .request(
                    "session/setModel",
                    json!({ "sessionId": moved, "commandId": command_id(), "model": { "modelId": self.kept.model } }),
                )
                .await?;
        }
        self.driver()
            .session
            .locked()
            .moved(moved.clone(), &turn_id);
        Ok(Some(moved))
    }

    async fn closed(&self) -> String {
        self.host.process.ended().await
    }

    fn pid(&self) -> u32 {
        self.host.process.pid()
    }

    async fn release(self) -> Result<(Value, Released), String> {
        let Self {
            host,
            launch: _,
            kept,
        } = self;
        let Host {
            process,
            driver: _,
            reading,
            session_tools,
        } = host;
        Driver::release(
            process,
            reading,
            MuseKept {
                session_tools,
                kept,
            },
        )
        .await
    }

    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let (driver, reading, kept): (_, _, MuseKept) = Driver::adopt(sink, state, writer, lines)?;
        Ok(Self {
            host: Host {
                process,
                driver,
                reading,
                session_tools: kept.session_tools,
            },
            launch: launch.clone(),
            kept: kept.kept,
        })
    }

    fn responder(&self, reply: Asked) -> Respond {
        self.host.driver.responder(reply)
    }
}
