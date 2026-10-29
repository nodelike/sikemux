//! Codex, spoken to over its app server: JSON-RPC, one message per line, on
//! `codex app-server`'s standard input and output. Every thread item comes out
//! as the ACP session updates Codex's ACP adapter would send for it.

mod approvals;
mod commands;
mod config;
mod history;
mod items;
mod notifications;
mod state;
mod subagents;

use sikemux_fold::UpdateKind;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::launch::PermissionMode;
use crate::protocol::ChatLaunch;

use super::super::feed::Reach;
use super::super::Respond;
use super::file_link;
use super::models::Models;
use super::parts::{context_text, parts, picture, Part};
use super::pending::{Answered, Purpose};
use super::process::{Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, Incoming, Rpc, RpcState, Sent, ANSWER_WAIT};
use super::servers;
use super::session::{self, Backend, Opened, Prompt, Sink, TurnEnd};
use approvals::Question;
use commands::Command;
use state::{user_input_told, Effect, Reply, Session};

/// What the reader and the chat's loop share.
#[derive(Clone)]
struct Driver {
    rpc: Rpc,
    sink: Sink,
    session: Arc<Mutex<Session>>,
}

impl Driver {
    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Update(session, update) => self.sink.update(&session, update),
                Effect::End(turn, end) => self.sink.end_turn(turn, end),
                Effect::WorkStarted if !self.sink.running() => self.sink.work_started(),
                Effect::WorkStarted => {}
                Effect::WorkEnded(stop_reason) => self.sink.work_ended(stop_reason),
                Effect::Interrupt(turn_id) => self.interrupt(turn_id),
            }
        }
    }

    /// Asks Codex to stop the turn with this id. Codex never answers an
    /// interrupt for a turn that is not running.
    fn interrupt(&self, turn_id: String) {
        let thread_id = self.session.locked().thread_id.clone();
        let _ = self.rpc.send_for(
            "turn/interrupt",
            json!({ "threadId": thread_id, "turnId": turn_id }),
            Purpose::Interrupt(turn_id),
        );
    }

    /// The answer to a request sent with `purpose`.
    fn purpose_answered(&self, purpose: Purpose, result: Answered) {
        if result.is_err() {
            return;
        }
        match purpose {
            Purpose::Interrupt(turn_id) => {
                let effects = self.session.locked().interrupted(&turn_id);
                self.apply(effects);
            }
            Purpose::Steer { text, paths } => self.sink.prompted(&text, &paths),
            _ => {}
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
                json!({ "sessionUpdate": UpdateKind::ToolCallUpdate, "toolCallId": call, "status": "in_progress" }),
            );
        }
        self.rpc
            .respond(reply["id"].clone(), question.answer(chosen.as_deref()));
    }

    fn notification(&self, method: &str, params: &Value) {
        let effects = self.session.locked().handle(method, params);
        self.apply(effects);
    }

    fn request(&self, id: Value, method: &str, params: &Value) {
        let reply = self.session.locked().question(method, params);
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
                let reply = json!({ "id": id, "question": question });
                self.sink
                    .ask(question.request, reply.clone(), self.responder(reply));
            }
        }
    }
}

impl Reads for Driver {
    type Nudge = ();
    type Message = Value;

    fn message(&mut self, message: Value) {
        match self.rpc.read(message) {
            Some(Incoming::Notification { method, params }) => self.notification(&method, &params),
            Some(Incoming::Request { id, method, params }) => self.request(id, &method, &params),
            Some(Incoming::Answer { purpose, result }) => self.purpose_answered(purpose, result),
            None => {}
        }
    }

    fn closed(&mut self) {
        self.rpc.close();
    }
}

/// What a Codex chat knows of its session, as it hands it to a core that
/// takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    thread_id: String,
    cwd: String,
    models: Models,
    model: String,
    effort: Option<String>,
    mode: PermissionMode,
    account: Value,
    servers: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexState {
    rpc: RpcState,
    session: Session,
    #[serde(flatten)]
    kept: Kept,
}

pub(crate) struct Codex {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    kept: Kept,
}

/// The models Codex offers, asked of a short-lived app server with no thread.
pub(crate) async fn read_options(command: tokio::process::Command) -> Result<Value, String> {
    let params = json!({ "cursor": null, "limit": null, "includeHidden": false });
    let listed =
        crate::agent_accounts::usage::codex_ask(command, "model/list", params, "models").await?;
    let models = config::models(listed["data"].as_array().cloned().unwrap_or_default());
    let model = models.default_model().ok_or("Codex named no models")?;
    let effort = models.effort_for(&model, None);
    Ok(models.options("Codex", &model, effort.as_deref()))
}

async fn list_models(rpc: &Rpc) -> Result<Models, String> {
    let pages = rpc
        .pages(
            "model/list",
            |cursor| json!({ "cursor": cursor, "limit": null, "includeHidden": false }),
        )
        .await?;
    Ok(config::models(data(pages)))
}

async fn list_turns(rpc: &Rpc, thread_id: &str) -> Result<Vec<Value>, String> {
    let pages = rpc
        .pages("thread/turns/list", |cursor| {
            json!({
                "threadId": thread_id,
                "cursor": cursor,
                "itemsView": "full",
                "sortDirection": "asc",
            })
        })
        .await?;
    Ok(data(pages))
}

/// Whether Codex refused to resume a thread because it has no such thread any more.
fn thread_gone(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("thread")
        && [
            "not found",
            "unknown thread",
            "no such thread",
            "does not exist",
            "missing thread",
            "no rollout",
        ]
        .iter()
        .any(|said| error.contains(said))
}

/// The turns of `thread_id` after `latest`, oldest first, read from the newest back; none when
/// the thread no longer holds `latest`.
async fn turns_since(rpc: &Rpc, thread_id: &str, latest: &str) -> Result<Vec<Value>, String> {
    let mut newer = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut page = rpc
            .request(
                "thread/turns/list",
                json!({
                    "threadId": thread_id,
                    "cursor": cursor,
                    "itemsView": "full",
                    "sortDirection": "desc",
                }),
            )
            .await?;
        let listed = match page["data"].take() {
            Value::Array(listed) => listed,
            _ => Vec::new(),
        };
        for turn in listed {
            if turn["id"] == latest {
                newer.reverse();
                return Ok(newer);
            }
            newer.push(turn);
        }
        match page["nextCursor"].as_str().filter(|next| cursor != *next) {
            Some(next) => cursor = next.into(),
            None => return Ok(Vec::new()),
        }
    }
}

/// What each page listed, in order.
fn data(pages: Vec<Value>) -> Vec<Value> {
    pages
        .into_iter()
        .flat_map(|mut page| match page["data"].take() {
            Value::Array(listed) => listed,
            _ => Vec::new(),
        })
        .collect()
}

/// The turns of every subagent thread a history started, and theirs.
async fn subagent_histories(rpc: &Rpc, turns: &[Value]) -> HashMap<String, Vec<Value>> {
    let read = |thread: String| async move {
        list_turns(rpc, &thread).await.unwrap_or_else(|error| {
            eprintln!("sikemux core: Codex subagent history {thread} could not be read: {error}");
            Vec::new()
        })
    };
    session::subagent_histories(
        history::subagent_threads(turns),
        read,
        history::subagent_threads,
    )
    .await
}

/// Codex's input for the prompt, the way Codex's adapter writes it, with
/// attached pictures handed over as pictures when the model reads them.
fn input(parts: Vec<Part>, images: bool) -> Vec<Value> {
    let text = |text: String| json!({ "type": "text", "text": text, "text_elements": [] });
    parts
        .into_iter()
        .map(|part| match part {
            Part::Text(said) => text(said),
            Part::File { name, uri } => match picture(&uri).filter(|_| images) {
                Some((path, _)) => json!({ "type": "localImage", "path": path }),
                None => text(file_link(Some(&name), &uri)),
            },
            Part::Context { uri, text: body } => text(format!(
                "{}\n{}",
                file_link(None, &uri),
                context_text(&uri, &body)
            )),
        })
        .collect()
}

impl Codex {
    /// Writes a steer to Codex now, so a stop sent after it reaches Codex
    /// after it too. None when no turn is running.
    fn send_steer(&self, prompt: &Prompt) -> Result<Option<Sent>, String> {
        let Some(turn_id) = self.driver.session.locked().turns.active() else {
            return Ok(None);
        };
        let input = self.prompt_input(prompt)?;
        self.driver
            .rpc
            .start_for(
                "turn/steer",
                json!({ "threadId": self.kept.thread_id, "expectedTurnId": turn_id, "input": input }),
                Some(Purpose::Steer {
                    text: prompt.text.clone(),
                    paths: prompt.paths.clone(),
                }),
            )
            .map(Some)
            .map_err(|error| error_text(&error))
    }

    fn prompt_input(&self, prompt: &Prompt) -> Result<Vec<Value>, String> {
        Ok(input(
            parts(prompt)?,
            self.kept.models.takes_images(&self.kept.model),
        ))
    }

    fn say(&self, text: &str) {
        self.driver.sink.update(
            &self.kept.thread_id,
            sikemux_fold::update::agent_text(text, None),
        );
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
        self.driver.session.locked().turns.begin(
            turn,
            None,
            prompt.message_id.clone(),
            history::title(&prompt.text),
        );
        match self.driver.rpc.request(method, params).await {
            Ok(result) => {
                if let Some(id) = result["turn"]["id"].as_str() {
                    let effects = self.driver.session.locked().started(turn, id);
                    self.driver.apply(effects);
                }
                Ok(())
            }
            Err(error) => {
                self.driver.session.locked().turns.abandon(turn);
                Err(error)
            }
        }
    }

    async fn command(&self, turn: u64, prompt: &Prompt, command: Command) -> Result<(), String> {
        let rpc = &self.driver.rpc;
        let thread = json!(self.kept.thread_id);
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
                    .request("skills/list", json!({ "cwds": [self.kept.cwd] }))
                    .await?;
                self.say(&commands::skills_text(&skills));
            }
            Command::Mcp => {
                let statuses = rpc
                    .request("mcpServerStatus/list", json!({ "threadId": thread }))
                    .await?;
                let statuses = statuses["data"].as_array().cloned().unwrap_or_default();
                self.say(&commands::servers_text(&statuses, &self.kept.servers));
            }
            Command::Status => {
                let token_usage = self.driver.session.locked().token_usage.clone();
                self.say(&commands::status_text(&commands::Status {
                    model: &self.kept.model,
                    cwd: &self.kept.cwd,
                    approval: config::approval_policy(self.kept.mode),
                    sandbox: config::sandbox_name(self.kept.mode),
                    account: &self.kept.account,
                    session: &self.kept.thread_id,
                    token_usage: token_usage.as_ref(),
                }));
            }
            Command::Usage(text) => self.say(&text),
        }
        self.answered(turn);
        Ok(())
    }

    /// Tells the chat what the resumed thread holds that it does not show yet: the whole of
    /// it, or for a chat whose history already holds it, only the turns past the one the
    /// history reaches.
    async fn replay_history(&self, thread: &Value) -> Result<(), String> {
        let rpc = &self.driver.rpc;
        let sink = &self.driver.sink;
        let thread_id = &self.kept.thread_id;
        let turns = if sink.reloading() {
            self.driver.session.locked().turns.titled = true;
            let Some(Reach::Turn(latest)) = sink.reach(thread_id) else {
                return Ok(());
            };
            let turns = turns_since(rpc, thread_id, &latest).await?;
            if turns.is_empty() {
                return Ok(());
            }
            sink.loaded();
            turns
        } else {
            let turns = list_turns(rpc, thread_id).await?;
            if let Some(title) = history::thread_title(thread, &turns) {
                sink.update(
                    thread_id,
                    json!({ "sessionUpdate": UpdateKind::SessionInfoUpdate, "title": title }),
                );
                self.driver.session.locked().turns.titled = true;
            }
            turns
        };
        let children = subagent_histories(rpc, &turns).await;
        let mut out = Vec::new();
        history::replay(
            &self.kept.thread_id,
            &turns,
            &children,
            &items::read_file,
            &mut out,
        );
        for (session, update) in out {
            sink.update(&session, update);
        }
        self.driver
            .session
            .locked()
            .turns
            .message_turns
            .extend(history::message_turns(&turns));
        if let Some(latest) = turns.last().and_then(|turn| turn["id"].as_str()) {
            sink.reached(thread_id, Reach::Turn(latest.to_owned()));
        }
        Ok(())
    }

    /// The turn the app's message `message_id` began, looked for in the thread's history from
    /// its newest turn back when this run of Codex did not see it.
    async fn turn_of(&self, message_id: &str) -> Result<String, String> {
        if let Some(turn) = self
            .driver
            .session
            .locked()
            .turns
            .message_turns
            .get(message_id)
        {
            return Ok(turn.clone());
        }
        let mut cursor = Value::Null;
        loop {
            let page = self
                .driver
                .rpc
                .request(
                    "thread/turns/list",
                    json!({
                        "threadId": self.kept.thread_id,
                        "cursor": cursor,
                        "itemsView": "full",
                        "sortDirection": "desc",
                    }),
                )
                .await?;
            let listed = page["data"].as_array().cloned().unwrap_or_default();
            if listed.iter().any(|turn| turn["id"] == message_id) {
                return Ok(message_id.to_owned());
            }
            if let Some(turn) = history::message_turns(&listed).remove(message_id) {
                return Ok(turn);
            }
            match page["nextCursor"].as_str().filter(|next| cursor != *next) {
                Some(next) => cursor = next.into(),
                None => return Ok(message_id.to_owned()),
            }
        }
    }

    async fn offer_commands(&self) {
        let skills = self
            .driver
            .rpc
            .request("skills/list", json!({ "cwds": [self.kept.cwd] }))
            .await
            .unwrap_or(Value::Null);
        self.driver.sink.update(
            &self.kept.thread_id,
            json!({
                "sessionUpdate": UpdateKind::AvailableCommandsUpdate,
                "availableCommands": commands::available(&skills),
            }),
        );
    }
}

impl Backend for Codex {
    async fn start(launch: &ChatLaunch, sink: Sink) -> Result<(Self, Opened), String> {
        let mode = launch.permission_mode;
        let (process, writer, lines) = Process::spawn(
            launch,
            ["app-server".to_owned()],
            Vec::<(String, String)>::new(),
        )?;
        let rpc = Rpc::new(writer, false);
        let driver = Driver {
            rpc: rpc.clone(),
            sink,
            session: Arc::new(Mutex::new(Session::new(String::new()))),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        let failed = |error: String| process.start_error(error);

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
        let models = if launch.model_provider.is_some() {
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
        let session_config =
            config::session_config(&cwd, &servers::tool_servers(launch), &configured);
        let servers: Vec<String> = session_config["mcp_servers"]
            .as_object()
            .map(|servers| servers.keys().cloned().collect())
            .unwrap_or_default();
        let start_thread = || {
            rpc.request(
                "thread/start",
                json!({ "cwd": cwd, "config": session_config }),
            )
        };
        let mut lost = false;
        let opened = match &launch.resume_id {
            Some(thread_id) => {
                let resumed = rpc
                    .request(
                        "thread/resume",
                        json!({ "threadId": thread_id, "cwd": cwd, "config": session_config, "excludeTurns": true }),
                    )
                    .await;
                match resumed {
                    Err(error) if thread_gone(&error) => {
                        lost = true;
                        start_thread().await
                    }
                    resumed => resumed,
                }
            }
            None => start_thread().await,
        }
        .map_err(failed)?;
        let thread = &opened["thread"];
        let thread_id = thread["id"]
            .as_str()
            .ok_or("Codex opened a thread without an id")?
            .to_owned();
        {
            let mut session = driver.session.locked();
            *session = Session::new(thread_id.clone());
            session.turns.titled = thread["name"].as_str().and_then(history::title).is_some();
            session.starting_servers = servers.iter().cloned().collect();
        }

        let model = launch
            .model
            .clone()
            .filter(|model| models.offers(model))
            .or_else(|| opened["model"].as_str().map(str::to_owned))
            .or_else(|| models.default_model())
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
            kept: Kept {
                thread_id: thread_id.clone(),
                cwd,
                models,
                model,
                effort,
                mode,
                account,
                servers,
            },
        };
        if lost {
            codex.driver.sink.loaded();
            codex.driver.sink.update(
                &codex.kept.thread_id,
                super::super::restart_notice(
                    "Codex no longer had this conversation",
                    "It started a new one. The chat above stays, but Codex does not remember it.",
                ),
            );
        } else if launch.resume_id.is_some() {
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
                "configOptions": codex.kept.models.options("Codex", &codex.kept.model, codex.kept.effort.as_deref()),
            }),
        };
        Ok((codex, opened))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let input = self.prompt_input(&prompt)?;
        if let Some(command) = commands::parse(&prompt.text) {
            return self.command(turn, &prompt, command).await;
        }
        let summary = if self.kept.account["type"] == "apiKey"
            || !self.kept.models.reasons(&self.kept.model)
        {
            "none"
        } else {
            "auto"
        };
        let params = json!({
            "threadId": self.kept.thread_id,
            "input": input,
            "approvalPolicy": config::approval_policy(self.kept.mode),
            "approvalsReviewer": "user",
            "sandboxPolicy": config::sandbox_policy(self.kept.mode),
            "model": self.kept.model,
            "effort": self.kept.effort,
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
        let effects = self.driver.session.locked().cancel();
        self.driver.apply(effects);
        Ok(())
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        self.kept.mode = mode;
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                if !self.kept.models.offers(value) {
                    return Err(format!("Codex does not offer the model {value}"));
                }
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
                self.kept.effort = Some(value.to_owned());
            }
            _ => return Err(format!("Codex has no option {config_id}")),
        }
        Ok(self
            .kept
            .models
            .options("Codex", &self.kept.model, self.kept.effort.as_deref()))
    }

    async fn stop_task(&mut self, _task_id: &str) -> Result<(), String> {
        Err("Codex has no background tasks to stop".into())
    }

    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let turn_id = self.turn_of(message_id).await?;
        self.driver
            .rpc
            .request(
                "thread/revert",
                json!({ "threadId": self.kept.thread_id, "beforeTurnId": turn_id }),
            )
            .await?;
        self.driver.session.locked().turns.rewound(&turn_id);
        Ok(None)
    }

    async fn closed(&self) -> String {
        self.process.ended().await
    }

    fn reach(&self) -> Option<Reach> {
        self.driver
            .session
            .locked()
            .latest_turn
            .clone()
            .map(Reach::Turn)
    }

    fn pid(&self) -> u32 {
        self.process.pid()
    }

    async fn release(self) -> Result<(Value, Released), String> {
        let Self {
            process,
            driver,
            reading,
            kept,
        } = self;
        let (_, lines) = reading.stop().await.ok_or("Codex stopped")?;
        let session = std::mem::replace(&mut *driver.session.locked(), Session::new(String::new()));
        let state = CodexState {
            rpc: driver.rpc.state(),
            session,
            kept,
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
        let driver = Driver {
            rpc: Rpc::restored(writer, state.rpc),
            sink,
            session: Arc::new(Mutex::new(state.session)),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok(Self {
            process,
            driver,
            reading,
            kept: state.kept,
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
    fn prompt_parts_become_codex_input() {
        let parts = || {
            vec![
                Part::Text("look".into()),
                Part::Context {
                    uri: "https://github.com/o/r/issues/1".into(),
                    text: "Issue".into(),
                },
                Part::File {
                    name: "a.txt".into(),
                    uri: "file:///w/a.txt".into(),
                },
                Part::File {
                    name: "shot.PNG".into(),
                    uri: "file:///w/shot.PNG".into(),
                },
            ]
        };
        let said = input(parts(), true);
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
            input(parts(), false)[3]["text"],
            "[@shot.PNG](file:///w/shot.PNG)"
        );
    }
}
