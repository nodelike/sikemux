//! Codex, spoken to over its app server: JSON-RPC, one message per line, on
//! `codex app-server`'s standard input and output. Every thread item comes out
//! as the ACP session updates Codex's ACP adapter would send for it.

mod approvals;
mod commands;
mod config;
mod history;
mod items;
mod state;
mod subagents;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::acp::prompt_blocks;
use crate::launch::providers::ROUTED_ENV;
use crate::protocol::ChatLaunch;

use super::super::Respond;
use super::file_link;
use super::process::{Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, Incoming, Rpc, RpcState, Sent, ANSWER_WAIT};
use super::session::{Backend, Opened, Prompt, Sink, TurnEnd};
use approvals::Question;
use commands::Command;
use config::{Mode, Models};
use state::{user_input_told, Effect, Reply, Session};

/// How long a finished turn waits for the subagents it started.
const SUBAGENT_WAIT: Duration = Duration::from_secs(10 * 60);

/// How many subagent threads a resumed history reads.
const MAX_SUBAGENT_THREADS: usize = 64;

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// What the reader and the chat's loop share.
#[derive(Clone)]
struct Driver {
    rpc: Rpc,
    sink: Sink,
    session: Arc<Mutex<Session>>,
}

impl Driver {
    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Update(session, update) => self.sink.update(&session, update),
                Effect::End(turn, end) => self.sink.end_turn(turn, end),
                Effect::WorkStarted if !self.sink.running() => self.sink.work_started(),
                Effect::WorkStarted => {}
                Effect::WorkEnded(stop_reason) => self.sink.work_ended(stop_reason),
                Effect::Interrupt(turn_id) => self.interrupt(turn_id),
                Effect::AwaitSubagents(turn) => self.await_subagents(turn),
            }
        }
    }

    fn await_subagents(&self, turn: u64) {
        let driver = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SUBAGENT_WAIT).await;
            let effects = driver.lock().subagents_waited(turn);
            driver.apply(effects);
        });
    }

    /// Asks Codex to stop the turn with this id. Codex never answers an
    /// interrupt for a turn that is not running.
    fn interrupt(&self, turn_id: String) {
        let thread_id = self.lock().thread_id.clone();
        let _ = self.rpc.send_for(
            "turn/interrupt",
            json!({ "threadId": thread_id, "turnId": turn_id }),
            json!({ "interrupt": turn_id }),
        );
    }

    /// The answer to a request sent with `purpose`.
    fn purpose_answered(&self, purpose: Value, result: Result<Value, Value>) {
        if let Some(turn_id) = purpose["interrupt"].as_str() {
            if result.is_ok() {
                let effects = self.lock().interrupted(turn_id);
                self.apply(effects);
            }
        } else if let Some(steer) = purpose.get("steer") {
            if result.is_ok() {
                let paths: Vec<String> =
                    serde_json::from_value(steer["paths"].clone()).unwrap_or_default();
                self.sink
                    .prompted(steer["text"].as_str().unwrap_or_default(), &paths);
            }
        }
    }

    fn responder(&self, reply: Value) -> Respond {
        let driver = self.clone();
        Box::new(move |chosen| driver.answer_question(&reply, chosen))
    }

    /// Tells Codex what the person chose for the question `reply` holds.
    fn answer_question(&self, reply: &Value, chosen: Option<String>) {
        let Ok(question) = serde_json::from_value::<Question>(reply["question"].clone()) else {
            self.rpc
                .refuse(reply["id"].clone(), "The question could not be read");
            return;
        };
        if let Some(call) = question
            .resumes
            .as_ref()
            .filter(|_| question.allows(chosen.as_deref()))
        {
            let session = question.request["sessionId"].as_str().unwrap_or_default();
            self.sink.update(
                session,
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": call, "status": "in_progress" }),
            );
        }
        self.rpc
            .respond(reply["id"].clone(), question.answer(chosen.as_deref()));
    }

    fn notification(&self, method: &str, params: &Value) {
        let effects = self.lock().handle(method, params);
        self.apply(effects);
    }

    fn request(&self, id: Value, method: &str, params: &Value) {
        let reply = self.lock().question(method, params);
        match reply {
            Reply::Answer(answer) => self.rpc.respond(id, answer),
            Reply::PutQuestion => {
                let told = self.sink.question(params);
                self.rpc.respond(id, user_input_told(params, &told));
            }
            Reply::Refuse => self
                .rpc
                .refuse(id, &format!("Sikemux does not handle {method}")),
            Reply::Ask(question) => {
                let request = question.request.clone();
                let reply = match serde_json::to_value(&question) {
                    Ok(question) => json!({ "id": id, "question": question }),
                    Err(_) => {
                        self.rpc.respond(id, question.answer(None));
                        return;
                    }
                };
                self.sink.ask(request, reply.clone(), self.responder(reply));
            }
        }
    }
}

impl Reads for Driver {
    type Nudge = ();

    fn message(&mut self, message: Value) {
        match self.rpc.read(message) {
            Some(Incoming::Notification { method, params }) => self.notification(&method, &params),
            Some(Incoming::Request { id, method, params }) => self.request(id, &method, &params),
            Some(Incoming::Answer { purpose, result }) => self.purpose_answered(purpose, result),
            None => {}
        }
    }

    fn closed(&mut self) {
        self.lock().gone = true;
        self.rpc.close();
    }
}

/// What a Codex chat hands a core that takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexState {
    rpc: RpcState,
    session: Session,
    thread_id: String,
    cwd: String,
    models: Models,
    model: String,
    effort: Option<String>,
    mode: Mode,
    account: Value,
    servers: Vec<String>,
}

pub(crate) struct Codex {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    thread_id: String,
    cwd: String,
    models: Models,
    model: String,
    effort: Option<String>,
    mode: Mode,
    account: Value,
    servers: Vec<String>,
}

/// Why starting failed, with the last thing Codex printed when it stopped.
fn start_error(process: &Process, error: String) -> String {
    let stderr = process.stderr();
    match stderr.lines().rev().find(|line| !line.trim().is_empty()) {
        Some(line) => format!("{error}: {}", line.trim()),
        None => error,
    }
}

async fn list_models(rpc: &Rpc) -> Result<Models, String> {
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = rpc
            .request(
                "model/list",
                json!({ "cursor": cursor, "limit": null, "includeHidden": false }),
            )
            .await?;
        models.extend(page["data"].as_array().cloned().unwrap_or_default());
        match page["nextCursor"].as_str() {
            Some(next) if cursor != next => cursor = Value::from(next),
            _ => break,
        }
    }
    Ok(Models(models))
}

async fn list_turns(rpc: &Rpc, thread_id: &str) -> Result<Vec<Value>, String> {
    let mut turns = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = rpc
            .request(
                "thread/turns/list",
                json!({
                    "threadId": thread_id,
                    "cursor": cursor,
                    "itemsView": "full",
                    "sortDirection": "asc",
                }),
            )
            .await?;
        turns.extend(page["data"].as_array().cloned().unwrap_or_default());
        match page["nextCursor"].as_str() {
            Some(next) if cursor != next => cursor = Value::from(next),
            _ => break,
        }
    }
    Ok(turns)
}

/// The turns of every subagent thread a history started, and theirs.
async fn subagent_histories(rpc: &Rpc, turns: &[Value]) -> HashMap<String, Vec<Value>> {
    let mut histories = HashMap::new();
    let mut queue = history::subagent_threads(turns);
    while let Some(thread) = queue.pop() {
        if histories.contains_key(&thread) || histories.len() >= MAX_SUBAGENT_THREADS {
            continue;
        }
        let turns = list_turns(rpc, &thread).await.unwrap_or_else(|error| {
            eprintln!("sikemux core: Codex subagent history {thread} could not be read: {error}");
            Vec::new()
        });
        queue.extend(history::subagent_threads(&turns));
        histories.insert(thread, turns);
    }
    histories
}

/// Codex's input for the prompt's blocks, the way Codex's adapter writes them,
/// with attached images handed over as images when the model reads them.
fn input(blocks: &[Value], images: bool) -> Vec<Value> {
    let text = |text: String| json!({ "type": "text", "text": text, "text_elements": [] });
    blocks
        .iter()
        .filter_map(|block| match block["type"].as_str()? {
            "text" => Some(text(block["text"].as_str()?.to_owned())),
            "resource_link" => {
                let uri = block["uri"].as_str()?;
                let path = Url::parse(uri).ok().and_then(|url| url.to_file_path().ok());
                let image = path.as_deref().filter(|path| {
                    images
                        && path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| {
                                IMAGE_EXTENSIONS.contains(&extension.to_lowercase().as_str())
                            })
                });
                match image {
                    Some(path) => Some(json!({ "type": "localImage", "path": path })),
                    None => Some(text(file_link(block["name"].as_str(), uri))),
                }
            }
            "resource" => {
                let resource = &block["resource"];
                let uri = resource["uri"].as_str()?;
                let body = resource["text"].as_str()?;
                Some(text(format!(
                    "{}\n<context ref=\"{uri}\">\n{body}\n</context>",
                    file_link(None, uri)
                )))
            }
            _ => None,
        })
        .collect()
}

impl Codex {
    /// Writes a steer to Codex now, so a stop sent after it reaches Codex
    /// after it too. None when no turn is running.
    fn send_steer(&self, prompt: &Prompt) -> Result<Option<Sent>, String> {
        let Some(turn_id) = self.driver.lock().active_turn() else {
            return Ok(None);
        };
        let input = self.prompt_input(prompt)?;
        self.driver
            .rpc
            .start_for(
                "turn/steer",
                json!({ "threadId": self.thread_id, "expectedTurnId": turn_id, "input": input }),
                Some(json!({ "steer": { "text": prompt.text, "paths": prompt.paths } })),
            )
            .map(Some)
            .map_err(|error| error_text(&error))
    }

    fn prompt_input(&self, prompt: &Prompt) -> Result<Vec<Value>, String> {
        let blocks = prompt_blocks(
            prompt.text.clone(),
            prompt.paths.clone(),
            prompt.context.clone(),
            true,
        )?;
        let blocks = serde_json::to_value(blocks).map_err(|error| error.to_string())?;
        let blocks = blocks.as_array().cloned().unwrap_or_default();
        Ok(input(&blocks, self.models.takes_images(&self.model)))
    }

    fn say(&self, text: &str) {
        self.driver
            .sink
            .update(&self.thread_id, items::agent_text(text, None, None));
    }

    fn answered(&self, turn: u64) {
        self.driver.sink.end_turn(
            turn,
            TurnEnd::Completed(json!({ "stopReason": "end_turn" })),
        );
    }

    /// Sends a request that begins turn `turn` in Codex.
    async fn send_turn(
        &self,
        turn: u64,
        prompt: &Prompt,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        self.driver.lock().begin(
            turn,
            prompt.message_id.clone(),
            history::title(&prompt.text),
        );
        match self.driver.rpc.request(method, params).await {
            Ok(result) => {
                if let Some(id) = result["turn"]["id"].as_str() {
                    let effects = self.driver.lock().started(turn, id);
                    self.driver.apply(effects);
                }
                Ok(())
            }
            Err(error) => {
                self.driver.lock().abandon(turn);
                Err(error)
            }
        }
    }

    async fn command(&self, turn: u64, prompt: &Prompt, command: Command) -> Result<(), String> {
        let rpc = &self.driver.rpc;
        let thread = json!(self.thread_id);
        match command {
            Command::Compact => {
                return self
                    .send_turn(
                        turn,
                        prompt,
                        "thread/compact/start",
                        json!({ "threadId": thread }),
                    )
                    .await;
            }
            Command::Review(target) => {
                let params = json!({ "threadId": thread, "target": target, "delivery": "inline" });
                return self.send_turn(turn, prompt, "review/start", params).await;
            }
            Command::Rename(name) => {
                rpc.request(
                    "thread/name/set",
                    json!({ "threadId": thread, "name": name }),
                )
                .await?;
            }
            Command::Skills => {
                let skills = rpc
                    .request("skills/list", json!({ "cwds": [self.cwd] }))
                    .await?;
                self.say(&commands::skills_text(&skills));
            }
            Command::Mcp => {
                let statuses = rpc
                    .request("mcpServerStatus/list", json!({ "threadId": thread }))
                    .await?;
                let statuses = statuses["data"].as_array().cloned().unwrap_or_default();
                self.say(&commands::servers_text(&statuses, &self.servers));
            }
            Command::Status => {
                let token_usage = self.driver.lock().token_usage.clone();
                self.say(&commands::status_text(&commands::Status {
                    model: &self.model,
                    cwd: &self.cwd,
                    approval: self.mode.approval_policy(),
                    sandbox: self.mode.sandbox_name(),
                    account: &self.account,
                    session: &self.thread_id,
                    token_usage: token_usage.as_ref(),
                }));
            }
            Command::Usage(text) => self.say(&text),
        }
        self.answered(turn);
        Ok(())
    }

    async fn replay_history(&self, thread: &Value) -> Result<(), String> {
        let rpc = &self.driver.rpc;
        let sink = &self.driver.sink;
        let turns = list_turns(rpc, &self.thread_id).await?;
        let children = subagent_histories(rpc, &turns).await;
        if let Some(title) = history::thread_title(thread, &turns) {
            sink.update(
                &self.thread_id,
                json!({ "sessionUpdate": "session_info_update", "title": title }),
            );
            self.driver.lock().titled = true;
        }
        let mut out = Vec::new();
        history::replay(
            &self.thread_id,
            &turns,
            &children,
            &items::read_file,
            &mut out,
        );
        for (session, update) in out {
            sink.update(&session, update);
        }
        self.driver
            .lock()
            .message_turns
            .extend(history::message_turns(&turns));
        Ok(())
    }

    async fn offer_commands(&self) {
        let skills = self
            .driver
            .rpc
            .request("skills/list", json!({ "cwds": [self.cwd] }))
            .await
            .unwrap_or(Value::Null);
        self.driver.sink.update(
            &self.thread_id,
            json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": commands::available(&skills),
            }),
        );
    }
}

impl Backend for Codex {
    async fn start(launch: &ChatLaunch, sink: Sink) -> Result<(Self, Opened), String> {
        let mode = Mode::from_app(&launch.permission_mode)?;
        let (process, writer, lines) = Process::spawn(
            launch,
            ["app-server".to_owned()],
            Vec::<(String, String)>::new(),
        )?;
        let rpc = Rpc::new(writer);
        let driver = Driver {
            rpc: rpc.clone(),
            sink,
            session: Arc::new(Mutex::new(Session::new(String::new()))),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        let failed = |error: String| start_error(&process, error);

        let initialized = rpc
            .request(
                "initialize",
                json!({
                    "clientInfo": { "name": "sikemux", "title": "Sikemux", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": { "experimentalApi": true, "requestAttestation": false },
                }),
            )
            .await
            .map_err(failed)?;
        // Codex names itself as `<client>/<its version> (<system>) …`.
        let version = initialized["userAgent"]
            .as_str()
            .and_then(|agent| agent.split_once('/'))
            .and_then(|(_, rest)| rest.split_whitespace().next());
        if let Some(version) = version {
            driver.sink.cli_version(version);
        }
        rpc.notify("initialized", Value::Null).map_err(failed)?;
        if let Some(library) = launch.env.get(crate::skills::LIBRARY_ENV) {
            let added = rpc
                .request("skills/extraRoots/set", json!({ "extraRoots": [library] }))
                .await;
            if let Err(error) = added {
                eprintln!("Sikemux skills are unavailable to this Codex chat: {error}");
            }
        }
        let account = rpc
            .request("account/read", json!({ "refreshToken": false }))
            .await
            .map(|read| read["account"].clone())
            .unwrap_or(Value::Null);
        let models = if launch.env.contains_key(ROUTED_ENV) {
            Models::default()
        } else {
            list_models(&rpc).await.map_err(failed)?
        };

        let cwd = launch.cwd.to_string_lossy().into_owned();
        let configured = rpc
            .request("config/read", json!({ "includeLayers": true, "cwd": cwd }))
            .await
            .map(|read| config::configured_servers(&read))
            .unwrap_or_default();
        let session_config = config::session_config(&cwd, &launch.mcp_servers, &configured);
        let servers: Vec<String> = session_config["mcp_servers"]
            .as_object()
            .map(|servers| servers.keys().cloned().collect())
            .unwrap_or_default();
        let opened = match &launch.resume_id {
            Some(thread_id) => {
                rpc.request(
                    "thread/resume",
                    json!({ "threadId": thread_id, "cwd": cwd, "config": session_config, "excludeTurns": true }),
                )
                .await
            }
            None => {
                rpc.request("thread/start", json!({ "cwd": cwd, "config": session_config }))
                    .await
            }
        }
        .map_err(failed)?;
        let thread = &opened["thread"];
        let thread_id = thread["id"]
            .as_str()
            .ok_or("Codex opened a thread without an id")?
            .to_owned();
        {
            let mut session = driver.lock();
            *session = Session::new(thread_id.clone());
            session.titled = thread["name"].as_str().and_then(history::title).is_some();
            session.starting_servers = servers.iter().cloned().collect();
        }

        let model = launch
            .model
            .clone()
            .filter(|model| models.offers(model))
            .or_else(|| opened["model"].as_str().map(str::to_owned))
            .or_else(|| {
                models
                    .0
                    .iter()
                    .find(|model| model["isDefault"] == true)
                    .and_then(|model| model["id"].as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let wanted = launch
            .effort
            .clone()
            .filter(|effort| models.supports_effort(&model, effort))
            .or_else(|| opened["reasoningEffort"].as_str().map(str::to_owned));
        let effort = models.effort_for(&model, wanted.as_deref());

        let codex = Self {
            process,
            driver,
            reading,
            thread_id: thread_id.clone(),
            cwd,
            models,
            model,
            effort,
            mode,
            account,
            servers,
        };
        if launch.resume_id.is_some() {
            codex.replay_history(thread).await?;
        }
        codex.offer_commands().await;
        let opened = Opened {
            session_id: thread_id,
            capabilities: json!({
                "loadSession": true,
                "steering": true,
                "editing": true,
                "promptCapabilities": { "image": true, "embeddedContext": true },
            }),
            setup: json!({
                "modes": config::modes(mode),
                "configOptions": codex.models.options(&codex.model, codex.effort.as_deref()),
            }),
        };
        Ok((codex, opened))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let input = self.prompt_input(&prompt)?;
        if let Some(command) = commands::parse(&prompt.text) {
            return self.command(turn, &prompt, command).await;
        }
        let summary = if self.account["type"] == "apiKey" || !self.models.reasons(&self.model) {
            "none"
        } else {
            "auto"
        };
        let params = json!({
            "threadId": self.thread_id,
            "input": input,
            "approvalPolicy": self.mode.approval_policy(),
            "approvalsReviewer": "user",
            "sandboxPolicy": self.mode.sandbox_policy(),
            "model": self.model,
            "effort": self.effort,
            "summary": summary,
            "serviceTier": null,
            "clientUserMessageId": prompt.message_id,
        });
        self.send_turn(turn, &prompt, "turn/start", params).await
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send + 'static {
        let sent = self.send_steer(&prompt);
        async move {
            let Some(sent) = sent? else {
                return Ok("promptRequired".to_owned());
            };
            match sent
                .answer(Some(ANSWER_WAIT))
                .await
                .map_err(|error| error_text(&error))
            {
                Ok(_) => Ok("injected".to_owned()),
                Err(error)
                    if error.contains("no active turn")
                        || error.contains("expected active turn") =>
                {
                    Ok("promptRequired".to_owned())
                }
                Err(error) => Err(error),
            }
        }
    }

    async fn cancel(&mut self) -> Result<(), String> {
        let effects = self.driver.lock().cancel();
        self.driver.apply(effects);
        Ok(())
    }

    async fn set_permission_mode(&mut self, mode: &str) -> Result<(), String> {
        self.mode = Mode::from_app(mode)?;
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                if !self.models.offers(value) {
                    return Err(format!("Codex does not offer the model {value}"));
                }
                self.model = value.to_owned();
                self.effort = self.models.effort_for(value, self.effort.as_deref());
            }
            "reasoning_effort" => {
                if !self.models.supports_effort(&self.model, value) {
                    return Err(format!("{} does not offer the effort {value}", self.model));
                }
                self.effort = Some(value.to_owned());
            }
            _ => return Err(format!("Codex has no option {config_id}")),
        }
        Ok(self.models.options(&self.model, self.effort.as_deref()))
    }

    async fn stop_task(&mut self, _task_id: &str) -> Result<(), String> {
        Err("Codex has no background tasks to stop".into())
    }

    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let turn_id = self
            .driver
            .lock()
            .message_turns
            .get(message_id)
            .cloned()
            .unwrap_or_else(|| message_id.to_owned());
        self.driver
            .rpc
            .request(
                "thread/revert",
                json!({ "threadId": self.thread_id, "beforeTurnId": turn_id }),
            )
            .await?;
        self.driver.lock().rewound(&turn_id);
        Ok(None)
    }

    async fn closed(&self) -> String {
        self.process.ended().await
    }

    fn pid(&self) -> u32 {
        self.process.pid()
    }

    async fn release(self) -> Result<(Value, Released), String> {
        let Self {
            process,
            driver,
            reading,
            thread_id,
            cwd,
            models,
            model,
            effort,
            mode,
            account,
            servers,
        } = self;
        let (_, lines) = reading.stop().await.ok_or("Codex stopped")?;
        let session = std::mem::replace(&mut *driver.lock(), Session::new(String::new()));
        let state = CodexState {
            rpc: driver.rpc.state(),
            session,
            thread_id,
            cwd,
            models,
            model,
            effort,
            mode,
            account,
            servers,
        };
        let state = serde_json::to_value(state).map_err(|error| error.to_string())?;
        let released = process.release(lines).await?;
        Ok((state, released))
    }

    fn adopt(
        _launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let state: CodexState = serde_json::from_value(state).map_err(|error| error.to_string())?;
        let waiting = state.session.waiting_turn();
        let driver = Driver {
            rpc: Rpc::restored(writer, false, state.rpc),
            sink,
            session: Arc::new(Mutex::new(state.session)),
        };
        if let Some(turn) = waiting {
            driver.await_subagents(turn);
        }
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok(Self {
            process,
            driver,
            reading,
            thread_id: state.thread_id,
            cwd: state.cwd,
            models: state.models,
            model: state.model,
            effort: state.effort,
            mode: state.mode,
            account: state.account,
            servers: state.servers,
        })
    }

    fn responder(&self, reply: Value) -> Respond {
        self.driver.responder(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_blocks_become_codex_input() {
        let blocks = [
            json!({ "type": "text", "text": "look" }),
            json!({ "type": "resource", "resource": {
                "uri": "https://github.com/o/r/issues/1", "mimeType": "text/markdown", "text": "Issue",
            } }),
            json!({ "type": "resource_link", "name": "a.txt", "uri": "file:///w/a.txt" }),
            json!({ "type": "resource_link", "name": "shot.PNG", "uri": "file:///w/shot.PNG" }),
        ];
        let said = input(&blocks, true);
        assert_eq!(
            said[0],
            json!({ "type": "text", "text": "look", "text_elements": [] })
        );
        assert_eq!(
            said[1]["text"],
            "https://github.com/o/r/issues/1\n<context ref=\"https://github.com/o/r/issues/1\">\nIssue\n</context>"
        );
        assert_eq!(said[2]["text"], "[@a.txt](file:///w/a.txt)");
        assert_eq!(
            said[3],
            json!({ "type": "localImage", "path": "/w/shot.PNG" })
        );
        assert_eq!(
            input(&blocks, false)[3]["text"],
            "[@shot.PNG](file:///w/shot.PNG)"
        );
    }
}
