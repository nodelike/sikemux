//! Pi, spoken to over its RPC mode. Every event comes out as the session
//! updates the other agents send for the same work. Pi leaves permissions to
//! its extensions, so the extension Sikemux hands it holds the tool hook, whose
//! questions arrive here as extension dialogs.

mod commands;
mod config;
mod dialogs;
mod history;
mod state;
mod tools;
mod wire;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::update::{self, notice};
use sikemux_fold::UpdateKind;
use sikemux_process::sync::Locked;

use crate::launch::tools::TOOLS_SERVER;
use crate::launch::PermissionMode;
use crate::sessions::for_agent::context_text;
use agent_client_protocol::schema::v1::McpServer;
use sikemux_process::vars;
use sikemux_wire::protocol::ChatLaunch;

use super::super::Respond;
use super::commands as shared_commands;
use super::file_link;
use super::json::text;
use super::models::Models;
use super::parts::{parts, picture, Part};
use super::pending::{Answered, Purpose};
use super::process::{self, Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, RpcState, ANSWER_WAIT};
use super::rpc_driver::RpcSession;
use super::servers;
use super::session::{Backend, Opened, Prompt, Sink, Steered, TurnEnd};
use commands::Command;
use dialogs::{Asked, Dialog};
use state::{turn_id, Effect, Own, Session};
use wire::{Incoming, Wire};

/// Pi takes at most this much of one picture; larger ones go as a link.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Pi renews a sign-in and shuts its extensions down as it quits, so it gets a
/// moment to before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// The models Pi offers and the one it runs, from its `get_state` and `get_available_models` answers.
struct Offered<'a> {
    listed: Vec<config::Listed>,
    models: Models,
    model: String,
    level: Option<&'a str>,
}

fn offered<'a>(state: &'a Value, answer: &Value) -> Offered<'a> {
    let listed: Vec<config::Listed> =
        serde_json::from_value(answer["models"].clone()).unwrap_or_default();
    let current = state["model"].is_object().then(|| {
        format!(
            "{}/{}",
            state["model"]["provider"].as_str().unwrap_or(""),
            state["model"]["id"].as_str().unwrap_or("")
        )
    });
    let level = text(state, "thinkingLevel");
    let models = config::models(&listed, current.as_deref(), level);
    let model = current
        .or_else(|| models.default_model())
        .unwrap_or_default();
    Offered {
        listed,
        models,
        model,
        level,
    }
}

/// Asks Pi, with no session, which models it can run, as the pickers of a chat not yet started.
pub async fn read_options(command: tokio::process::Command) -> Result<Value, String> {
    let asked = format!(
        "{}\n{}\n",
        json!({ "type": "get_state", "id": "state" }),
        json!({ "type": "get_available_models", "id": "models" }),
    );
    let said = crate::agent_accounts::usage::ask(
        "pi",
        command,
        &["--mode", "rpc", "--no-session"],
        &asked,
    )
    .await?;
    let answer = |id: &str| {
        said.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| line["type"] == "response" && line["id"] == id && line["success"] == true)
            .map(|line| line["data"].clone())
    };
    let state = answer("state").unwrap_or(Value::Null);
    let listed = answer("models").ok_or("Pi named no models")?;
    let Offered {
        models,
        model,
        level,
        ..
    } = offered(&state, &listed);
    if models.listed.is_empty() {
        return Err("Pi has no models it can run".to_owned());
    }
    let effort = models.effort_for(&model, level);
    Ok(models.options("Pi", &model, effort.as_deref()))
}

#[derive(Clone)]
struct Driver {
    wire: Wire,
    sink: Sink,
    session: Arc<Mutex<Session>>,
}

impl Driver {
    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Update(session, update) => self.sink.update(&session, update),
                Effect::End(turn, end) => self.sink.end_turn(turn, end),
                Effect::WorkStarted => self.sink.work_started(),
                Effect::WorkEnded(reason) => self.sink.work_ended(reason),
                Effect::Interrupt(turn) => self.interrupt(turn),
                Effect::Own(Own::Send { command, purpose }) => self.wire.send_for(command, purpose),
                Effect::Own(Own::CloseDialogs(ids)) => {
                    for id in ids {
                        self.tell_cancelled(&id);
                    }
                    self.sink.withdraw_dialogs(None);
                }
            }
        }
    }

    fn session_id(&self) -> String {
        self.session.locked().session_id.clone()
    }

    fn tell_cancelled(&self, id: &str) {
        self.wire.tell(&json!({
            "type": "extension_ui_response",
            "id": id,
            "cancelled": true,
        }));
    }

    fn say(&self, said: &str) {
        self.sink
            .update(&self.session_id(), update::agent_text(said, None));
    }

    /// Queued messages would start a new run the moment the abort lands, so
    /// they go first.
    fn interrupt(&self, turn: String) {
        self.wire
            .send_for(json!({ "type": "clear_queue" }), Purpose::ClearQueue);
        self.wire
            .send_for(json!({ "type": "abort" }), Purpose::Interrupt(turn));
    }

    /// Sends a prompt now, or once a compaction under way has ended.
    fn send_prompt(&self, command: Value, purpose: Purpose) {
        let mut session = self.session.locked();
        session.touched();
        if session.is_compacting() {
            session.hold(command, purpose);
        } else {
            session.sent_prompt = Some(command.clone());
            drop(session);
            self.wire.send_for(command, purpose);
        }
    }

    fn answered(&self, purpose: Purpose, result: Answered) {
        let effects = match (purpose, result) {
            (Purpose::Prompt(_), Ok(answer)) => {
                let disposition = text(&answer, "disposition").unwrap_or("started");
                let mut session = self.session.locked();
                session.sent_prompt = None;
                session.accepted(disposition)
            }
            (Purpose::Prompt(turn), Err(error)) => self.prompt_refused(turn, error),
            (
                Purpose::Steer {
                    text,
                    paths,
                    person,
                },
                Ok(_),
            ) => {
                self.sink.prompted(&text, &paths, person);
                self.session.locked().probe_if_settling()
            }
            (Purpose::Steer { text, .. }, Err(error)) => {
                eprintln!("sikemux core: Pi refused a message: {}", error_text(&error));
                self.sink.letter_unread(text);
                self.session.locked().probe_if_settling()
            }
            (Purpose::Interrupt(turn), Ok(_)) => {
                let mut session = self.session.locked();
                let mut effects = session.interrupted(&turn);
                effects.extend(session.close_dialogs());
                effects
            }
            (Purpose::Settle(generation), state) => self.session.locked().probed(generation, state),
            (Purpose::Compact(turn), result) => {
                let result = result.map_err(|error| error_text(&error));
                self.session.locked().compact_answered(turn, result)
            }
            (Purpose::Usage, Ok(stats)) => self.session.locked().stats(stats),
            (Purpose::Entries(message), Ok(read)) => {
                self.session.locked().entries(&read, message);
                Vec::new()
            }
            (Purpose::Entries(_), Err(_)) => {
                self.session.locked().leaf = None;
                Vec::new()
            }
            (Purpose::ClearQueue, Ok(cleared)) => {
                self.queue_cleared(&cleared);
                Vec::new()
            }
            _ => Vec::new(),
        };
        self.apply(effects);
    }

    fn prompt_refused(&self, turn: u64, error: Value) -> Vec<Effect> {
        let said = error_text(&error);
        let mut session = self.session.locked();
        if said.contains(state::COMPACTING) && session.is_compacting() {
            if let Some(command) = session.sent_prompt.take() {
                session.hold(command, Purpose::Prompt(turn));
                return Vec::new();
            }
        }
        session.turns.abandon(turn);
        vec![Effect::End(
            turn,
            TurnEnd::Failed {
                failure: state::failure(&said),
                response: None,
            },
        )]
    }

    fn queue_cleared(&self, cleared: &Value) {
        let texts: Vec<&str> = ["steering", "followUp"]
            .iter()
            .flat_map(|key| cleared[key].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        if texts.is_empty() {
            return;
        }
        let title = format!(
            "Stop cleared {} queued message{} Pi had not read",
            texts.len(),
            if texts.len() == 1 { "" } else { "s" },
        );
        let detail = texts
            .iter()
            .filter(|said| !said.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n\n");
        self.sink.update(
            &self.session_id(),
            notice(
                "info",
                &title,
                Some(detail.as_str()).filter(|detail| !detail.is_empty()),
            ),
        );
    }

    fn extension_ui(&self, event: &Value) {
        match event["method"].as_str() {
            Some("notify") => {
                let severity = match event["notifyType"].as_str() {
                    Some("error") => "error",
                    Some("warning") => "warning",
                    _ => "info",
                };
                if let Some(said) = text(event, "message") {
                    self.sink
                        .update(&self.session_id(), notice(severity, said, None));
                }
                return;
            }
            Some("select" | "confirm" | "input" | "editor") => {}
            _ => return,
        }
        let (session_id, cwd, allowed) = {
            let session = self.session.locked();
            if session.stopped {
                drop(session);
                self.tell_cancelled(event["id"].as_str().unwrap_or_default());
                return;
            }
            (
                session.session_id.clone(),
                session.cwd.clone(),
                session.allowed.clone(),
            )
        };
        let dialog = dialogs::read(&session_id, &cwd, event, |key| allowed.contains(key));
        if let (Some(id), Some(timeout)) = (text(event, "id"), event["timeout"].as_u64()) {
            self.expire_after(id.to_owned(), Duration::from_millis(timeout));
        }
        match dialog {
            Some(Dialog::Allowed(id)) => self.wire.tell(&json!({
                "type": "extension_ui_response",
                "id": id,
                "confirmed": true,
            })),
            Some(Dialog::Permission(request, asked)) => {
                self.sink.ask(request, asked, |asked| self.responder(asked));
            }
            Some(Dialog::Question(request, asked)) => {
                self.sink
                    .ask_person(request, asked, |asked| self.responder(asked));
            }
            Some(Dialog::Words(words)) => self.ask_for_words(words),
            None => self.tell_cancelled(event["id"].as_str().unwrap_or_default()),
        }
    }

    /// Pi answers a dialog itself when its timeout passes, so its question comes down then.
    fn expire_after(&self, id: String, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        let driver = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            driver.sink.withdraw_permission(&dialogs::card_id(&id));
            if driver.session.locked().word_dialogs.remove(&id).is_some() {
                driver.sink.withdraw_dialogs(Some(&id));
            }
        });
    }

    fn ask_for_words(&self, words: dialogs::Words) {
        let choices = words.choices();
        self.session
            .locked()
            .word_dialogs
            .insert(words.id.clone(), words.prefill.clone());
        let asked = self
            .sink
            .ask_dialog(words.id.clone(), &words.title, &words.text, choices);
        if let Err(error) = asked {
            eprintln!("sikemux core: Pi's dialog was not put to the person: {error}");
            self.session.locked().word_dialogs.remove(&words.id);
            self.tell_cancelled(&words.id);
        }
    }

    fn responder(&self, asked: Asked) -> Respond {
        let driver = self.clone();
        Box::new(move |chosen| {
            let (record, remember) = dialogs::response(asked, chosen.as_deref());
            if let Some(remember) = remember {
                driver.session.locked().allowed.insert(remember);
            }
            driver.wire.tell(&record);
        })
    }
}

impl Reads for Driver {
    type Nudge = ();
    type Message = Value;

    fn message(&mut self, message: Value) {
        match self.wire.read(message) {
            Some(Incoming::Answer { purpose, result }) => self.answered(purpose, result),
            Some(Incoming::Event(event)) if event["type"] == "extension_ui_request" => {
                self.extension_ui(&event)
            }
            Some(Incoming::Event(event)) => {
                let effects = self.session.locked().handle(&event);
                self.apply(effects);
            }
            None => {}
        }
    }

    fn closed(&mut self) {
        self.wire.close();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    models: Models,
    context_windows: Vec<(String, u64)>,
    model: String,
    effort: Option<String>,
    mode: PermissionMode,
    commands: Value,
    has_hook: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handed {
    wire: RpcState,
    session: Session,
    #[serde(flatten)]
    kept: Kept,
}

pub struct Pi {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    kept: Kept,
}

/// What Pi's extension is told through its environment: the chat's permission
/// mode, and how to reach Sikemux's tools.
fn extension_env(launch: &ChatLaunch) -> Vec<(String, String)> {
    let mut env = vec![(
        vars::PI_PERMISSIONS.to_owned(),
        config::permissions(launch.permission_mode).to_owned(),
    )];
    let tools = servers::tool_servers(launch)
        .into_iter()
        .find_map(|server| match server {
            McpServer::Stdio(server) if server.name == TOOLS_SERVER => Some(server),
            _ => None,
        });
    if let Some(server) = tools {
        env.push((
            vars::TOOLS_MCP_COMMAND.to_owned(),
            server.command.to_string_lossy().into_owned(),
        ));
        env.push((
            vars::TOOLS_MCP_ARGS.to_owned(),
            serde_json::to_string(&server.args).unwrap_or_default(),
        ));
        env.extend(
            server
                .env
                .into_iter()
                .map(|variable| (variable.name, variable.value)),
        );
    }
    env
}

fn attachment(name: &str, uri: &str, takes_images: bool) -> Result<String, Value> {
    let image = takes_images
        .then(|| picture(uri))
        .flatten()
        .and_then(|(path, media_type)| {
            if std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES {
                return None;
            }
            Some(json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(std::fs::read(&path).ok()?),
                "mimeType": media_type,
            }))
        });
    image.map_or_else(|| Ok(file_link(Some(name), uri)), Err)
}

/// A message as Pi takes it: the words, with what was attached after them so a
/// leading slash command stays first, and pictures apart.
fn message_of(parts: Vec<Part>, takes_images: bool) -> (String, Vec<Value>) {
    let mut said = Vec::new();
    let mut images = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => said.push(text),
            Part::File { name, uri } => match attachment(&name, &uri, takes_images) {
                Ok(link) => said.push(link),
                Err(image) => images.push(image),
            },
            Part::Context { uri, text } => said.push(format!(
                "{}\n{}",
                file_link(None, &uri),
                context_text(&uri, &text)
            )),
        }
    }
    (said.join("\n\n"), images)
}

impl Pi {
    fn wire(&self) -> &Wire {
        &self.driver.wire
    }

    fn session_id(&self) -> String {
        self.driver.session_id()
    }

    fn prompt_command(&self, prompt: &Prompt) -> Result<Value, String> {
        let takes_images = self.kept.models.takes_images(&self.kept.model);
        let (message, images) = message_of(parts(prompt)?, takes_images);
        // Steering asks Pi to queue the message if a run it began by itself is under way.
        let mut command =
            json!({ "type": "prompt", "message": message, "streamingBehavior": "steer" });
        if !images.is_empty() {
            command["images"] = json!(images);
        }
        Ok(command)
    }

    fn begin_turn(&self, turn: u64, prompt: &Prompt) {
        let id = turn_id(turn);
        let mut session = self.driver.session.locked();
        session.turns.begin(
            turn,
            Some(&id),
            prompt.message_id.clone(),
            crate::server::chat::feed::title(&prompt.text),
        );
        session.turn_message = prompt.message_id.clone();
        session.stopped = false;
    }

    async fn command(
        &mut self,
        turn: u64,
        prompt: &Prompt,
        command: Command,
    ) -> Result<(), String> {
        match command {
            Command::Compact(instructions) => {
                self.begin_turn(turn, prompt);
                let mut compact = json!({ "type": "compact" });
                if let Some(instructions) = instructions {
                    compact["customInstructions"] = json!(instructions);
                }
                self.wire().send_for(compact, Purpose::Compact(turn));
                return Ok(());
            }
            Command::Name(name) => {
                self.wire()
                    .request(json!({ "type": "set_session_name", "name": name }))
                    .await?;
                self.driver.session.locked().turns.titled = true;
                self.driver
                    .sink
                    .update(&self.session_id(), update::session_title(&name));
            }
            Command::Status => self.status().await,
            Command::Hint(said) => self.driver.say(&said),
        }
        self.driver.sink.end_turn(
            turn,
            TurnEnd::Completed(update::turn_answer(sikemux_view::chat::StopReason::EndTurn)),
        );
        Ok(())
    }

    async fn status(&mut self) {
        let stats = self
            .wire()
            .request(json!({ "type": "get_session_stats" }))
            .await
            .unwrap_or(Value::Null);
        let context = &stats["contextUsage"];
        let window = context["contextWindow"]
            .as_u64()
            .and_then(|size| shared_commands::window_left(size, context["tokens"].as_u64()?));
        let tokens = stats["tokens"]["total"].as_u64().map(|total| {
            format!(
                "{} total  ({} input, {} output)",
                shared_commands::count(total),
                shared_commands::count(stats["tokens"]["input"].as_u64().unwrap_or(0)),
                shared_commands::count(stats["tokens"]["output"].as_u64().unwrap_or(0)),
            )
        });
        let model = match &self.kept.effort {
            Some(effort) => format!("{} ({effort} thinking)", self.kept.model),
            None => self.kept.model.clone(),
        };
        let cwd = self.driver.session.locked().cwd.clone();
        let session = self.session_id();
        self.driver
            .say(&shared_commands::status_text(shared_commands::Status {
                model,
                cwd: &cwd,
                approval: match self.kept.mode {
                    PermissionMode::Bypass => "never asks",
                    _ => "asks before commands and edits",
                },
                sandbox: "none",
                account: None,
                session: &session,
                tokens,
                window,
            }));
    }

    fn context_window(&self, model: &str) -> u64 {
        self.kept
            .context_windows
            .iter()
            .find(|(slug, _)| slug == model)
            .map_or(0, |(_, size)| *size)
    }

    async fn apply_level(&mut self, wanted: Option<String>) -> Result<(), String> {
        let chosen = self
            .kept
            .models
            .effort_for(&self.kept.model, wanted.as_deref());
        match chosen {
            Some(level) if wanted.is_some() => {
                self.wire().request(config::set_level(&level)).await?;
                self.kept.effort = Some(level);
            }
            _ => {
                let state = self.wire().request(json!({ "type": "get_state" })).await?;
                self.kept.effort = text(&state, "thinkingLevel")
                    .filter(|level| self.kept.models.supports_effort(&self.kept.model, level))
                    .map(str::to_owned);
            }
        }
        Ok(())
    }

    fn options(&self) -> Value {
        if self.kept.models.listed.is_empty() {
            return config::no_models();
        }
        self.kept
            .models
            .options("Pi", &self.kept.model, self.kept.effort.as_deref())
    }

    async fn switch_model(&mut self, slug: &str) -> Result<(), String> {
        let (provider, id) =
            config::split_slug(slug).ok_or_else(|| format!("{slug} is not a Pi model"))?;
        let chosen = self.wire().request(config::set_model(provider, id)).await?;
        self.kept.model = slug.to_owned();
        let window = chosen["contextWindow"]
            .as_u64()
            .unwrap_or_else(|| self.context_window(slug));
        self.driver.session.locked().context_window = window;
        Ok(())
    }

    async fn offer_commands(&mut self) {
        self.kept.commands = self
            .wire()
            .request(json!({ "type": "get_commands" }))
            .await
            .unwrap_or(Value::Null);
        self.driver.sink.update(
            &self.session_id(),
            json!({
                "sessionUpdate": UpdateKind::AvailableCommandsUpdate,
                "availableCommands": commands::available(&self.kept.commands),
            }),
        );
    }

    async fn replay(&self, state: &Value) {
        let driver = &self.driver;
        let session_id = self.session_id();
        let read = self
            .wire()
            .request(json!({ "type": "get_entries" }))
            .await
            .unwrap_or(Value::Null);
        let entries = read["entries"].as_array().map_or(&[][..], Vec::as_slice);
        let leaf = read["leafId"].as_str();
        let branch = history::branch(entries, leaf);
        let title = text(state, "sessionName")
            .map(str::to_owned)
            .or_else(|| history::first_prompt(&branch));
        if let Some(title) = title {
            driver
                .sink
                .update(&session_id, update::session_title(&title));
            driver.session.locked().turns.titled = true;
        }
        let cwd = driver.session.locked().cwd.clone();
        for update in history::replay(&branch, &cwd) {
            driver.sink.update(&session_id, update);
        }
        driver.session.locked().leaf = leaf.map(str::to_owned);
    }
}

impl Backend for Pi {
    type Reply = Asked;

    async fn start(
        launch: &ChatLaunch,
        _brief: Option<&str>,
        sink: Sink,
    ) -> Result<(Self, Opened), String> {
        let extension = launch.env.get(vars::PI_EXTENSION).map(String::as_str);
        if extension.is_none() && launch.permission_mode != PermissionMode::Bypass {
            return Err(
                "Sikemux's Pi extension is missing, so Pi could not ask before running tools"
                    .to_owned(),
            );
        }
        let (mut process, writer, lines) = Process::spawn(
            launch,
            config::arguments(launch, extension),
            extension_env(launch),
        )?;
        process.stop_gently(SHUTDOWN_GRACE);
        sink.withdraw_dialogs(None);
        let cwd = launch.cwd.to_string_lossy().into_owned();
        let driver = Driver {
            wire: Wire::new(writer),
            sink,
            session: Arc::new(Mutex::new(Session::new(String::new(), cwd))),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        let wire = driver.wire.clone();
        let state = wire
            .request(json!({ "type": "get_state" }))
            .await
            .map_err(|error| process.start_error(error))?;
        let session_id = text(&state, "sessionFile")
            .or_else(|| text(&state, "sessionId"))
            .ok_or("Pi started without a session")?
            .to_owned();
        driver.session.locked().session_id = session_id.clone();

        let answer = wire
            .request(json!({ "type": "get_available_models" }))
            .await
            .unwrap_or(Value::Null);
        let Offered {
            listed,
            models,
            model,
            level,
        } = offered(&state, &answer);
        let context_windows = listed
            .iter()
            .map(|model| (model.slug(), model.context_window))
            .collect();
        let window = state["model"]["contextWindow"].as_u64().unwrap_or(0);
        driver.session.locked().context_window = window;

        let mut pi = Self {
            process,
            driver,
            reading,
            kept: Kept {
                effort: models.effort_for(&model, level),
                models,
                context_windows,
                model,
                mode: launch.permission_mode,
                commands: Value::Null,
                has_hook: extension.is_some(),
            },
        };
        let switch = launch
            .model
            .as_deref()
            .filter(|wanted| pi.kept.models.offers(wanted) && *wanted != pi.kept.model);
        if let Some(wanted) = switch {
            pi.switch_model(wanted)
                .await
                .map_err(|error| pi.process.start_error(error))?;
        }
        let level = launch
            .effort
            .clone()
            .filter(|wanted| pi.kept.effort.as_ref() != Some(wanted));
        if level.is_some() || switch.is_some() {
            pi.apply_level(level)
                .await
                .map_err(|error| pi.process.start_error(error))?;
        }
        if launch.resume_id.is_some() {
            pi.replay(&state).await;
        }
        pi.offer_commands().await;
        let opened = Opened {
            session_id,
            capabilities: Opened::native_capabilities(),
            setup: json!({ "configOptions": pi.options() }),
        };
        Ok((pi, opened))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        if let Some(command) = commands::parse(&prompt.text) {
            return self.command(turn, &prompt, command).await;
        }
        let command = self.prompt_command(&prompt)?;
        self.begin_turn(turn, &prompt);
        self.driver.send_prompt(command, Purpose::Prompt(turn));
        Ok(())
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static {
        let driver = self.driver.clone();
        let steered = (|| -> Result<_, String> {
            if driver.session.locked().turns.active().is_none() {
                return Ok(None);
            }
            let command = self.prompt_command(&prompt)?;
            let purpose = Purpose::Steer {
                text: prompt.text.clone(),
                paths: prompt.paths.clone(),
                person: !prompt.letter,
            };
            let mut session = driver.session.locked();
            session.touched();
            if session.is_compacting() {
                session.hold(command, purpose);
                return Ok(Some(None));
            }
            drop(session);
            driver
                .wire
                .start(command, Some(purpose))
                .map(|sent| Some(Some(sent)))
        })();
        async move {
            let Some(sent) = steered? else {
                return Ok(Steered::PromptRequired);
            };
            let Some(sent) = sent else {
                return Ok(Steered::Injected);
            };
            let answer = sent.answer(Some(ANSWER_WAIT)).await;
            match answer {
                Ok(_) => {
                    let effects = driver.session.locked().probe_if_settling();
                    driver.apply(effects);
                    Ok(Steered::Injected)
                }
                Err(error) => {
                    let said = error_text(&error);
                    if said.contains("not running") || said.contains("idle") {
                        Ok(Steered::PromptRequired)
                    } else {
                        Err(said)
                    }
                }
            }
        }
    }

    async fn cancel(&mut self) -> Result<(), String> {
        let effects = {
            let mut session = self.driver.session.locked();
            session.drop_held();
            session.stopped = true;
            session.cancel()
        };
        self.driver.apply(effects);
        Ok(())
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        if !self.kept.has_hook {
            return Err(
                "Sikemux's Pi extension is missing, so permissions cannot change".to_owned(),
            );
        }
        self.wire()
            .request(json!({
                "type": "prompt",
                "message": format!("/sikemux-permission-mode {}", config::permissions(mode)),
            }))
            .await?;
        self.kept.mode = mode;
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                if !self.kept.models.offers(value) {
                    return Err(format!("Pi does not offer the model {value}"));
                }
                let wanted = self.kept.effort.clone();
                self.switch_model(value).await?;
                self.apply_level(wanted).await?;
            }
            "reasoning_effort" => {
                if !self.kept.models.supports_effort(&self.kept.model, value) {
                    return Err(format!("{} does not think at {value}", self.kept.model));
                }
                self.wire().request(config::set_level(value)).await?;
                self.kept.effort = Some(value.to_owned());
            }
            _ => return Err(format!("Pi has no option {config_id}")),
        }
        Ok(self.options())
    }

    async fn stop_task(&mut self, _task_id: &str) -> Result<(), String> {
        Ok(())
    }

    /// Pi goes back by forking at a user message into a new session file, so
    /// the chat goes on there. A message replayed from history is known by its entry.
    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let entry = {
            let session = self.driver.session.locked();
            session
                .message_entries
                .get(message_id)
                .cloned()
                .unwrap_or_else(|| message_id.to_owned())
        };
        let forked = self
            .wire()
            .request(json!({ "type": "fork", "entryId": entry }))
            .await?;
        if forked["cancelled"] == true {
            return Err("A Pi extension stopped this message being edited".to_owned());
        }
        let state = self.wire().request(json!({ "type": "get_state" })).await?;
        let moved = text(&state, "sessionFile")
            .ok_or("Pi went back without naming the session it made")?
            .to_owned();
        self.driver.session.locked().forked(moved.clone());
        Ok(Some(moved))
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
            driver: _,
            reading,
            kept,
        } = self;
        process::hand_over(process, reading, "Pi", |driver: Driver| Handed {
            wire: driver.wire.state(),
            session: std::mem::take(&mut *driver.session.locked()),
            kept,
        })
        .await
    }

    fn adopt(
        _launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let Handed {
            wire,
            session,
            kept,
        } = serde_json::from_value(state).map_err(|error| error.to_string())?;
        let driver = Driver {
            wire: Wire::restored(writer, wire),
            sink,
            session: Arc::new(Mutex::new(session)),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok(Self {
            process,
            driver,
            reading,
            kept,
        })
    }

    fn answer_dialog(&mut self, id: &str, answer: String, choice: Option<usize>) {
        let Some(prefill) = self.driver.session.locked().word_dialogs.remove(id) else {
            return;
        };
        self.wire()
            .tell(&dialogs::words_response(id, prefill, answer, choice));
    }

    fn responder(&self, reply: Asked) -> Respond {
        self.driver.responder(reply)
    }
}
