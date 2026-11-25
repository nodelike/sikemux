//! Agents that speak the Agent Client Protocol themselves, such as OMP, Grok
//! and Hermes: JSON-RPC, one message per line, on the agent's
//! standard input and output. Their session updates already have the shape
//! the chat reads, so they pass through as they come.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Error as AcpError, ImageContent, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, McpServer, Meta, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionResponse, ResumeSessionRequest,
    SelectedPermissionOutcome, SetSessionConfigOptionRequest, SetSessionModeRequest,
    AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES,
};
use agent_client_protocol::schema::ProtocolVersion;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::UpdateKind;
use sikemux_process::sync::Locked;
use sikemux_view::chat::StopReason;

use crate::acp::failure::{sounds_offline, Failure};
use crate::acp::pickers;
use crate::acp::{air, native, permission_mode_id};
use crate::agent_catalog::ChatDriver;
use crate::launch::PermissionMode;
use crate::server::chat::prompt::{command_parts, prompt_blocks};
use crate::sessions;
use sikemux_wire::protocol::ChatLaunch;

use super::super::Respond;
use super::antigravity;
use super::mcp_calls;
use super::parts::inline_picture;
use super::pending::{Answered, Purpose};
use super::process::{self, Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, Incoming, Rpc, RpcState, Sent, ANSWER_WAIT};
use super::servers;
use super::session::{Backend, Opened, Prompt, Sink, Steered, Telling, TurnEnd};

#[derive(Serialize, Deserialize)]
pub struct Asked {
    id: Value,
}

const COMPACT: &str = "compact";

/// How long a reply the agent began on its own stays quiet after its usage before it counts as over.
const UNPROMPTED_QUIET: Duration = Duration::from_secs(3);

/// The error code an agent turns a prompt, or a new session, down with when it needs a sign-in.
const AUTH_REQUIRED: i64 = -32000;

/// Shows a saved session of an agent that keeps one Sikemux can read, as the agent would
/// replay it on loading it.
pub fn replay_saved(launch: &ChatLaunch, session: &str, sink: &Telling) -> Result<(), String> {
    let agent = crate::agent_catalog::AgentKind::from_name(&launch.provider)
        .ok_or_else(|| format!("{} keeps no sessions Sikemux reads", launch.provider))?;
    let servers = server_names(launch);
    for mut update in crate::sessions::history(agent, session, &launch.cwd)? {
        if !crate::server::chat::prompt::drop_for_agent(&mut update) {
            continue;
        }
        mcp_calls::as_mcp_call(&mut update, &servers);
        sink.update(session, update);
    }
    Ok(())
}

fn is_antigravity(launch: &ChatLaunch) -> bool {
    crate::agent_catalog::AgentKind::from_name(&launch.provider)
        == Some(crate::agent_catalog::AgentKind::Antigravity)
}

/// Whether the chat runs on its agent's own free models, which come and go, rather than on a
/// provider Sikemux routes it to.
fn on_free_models(launch: &ChatLaunch) -> bool {
    launch.model_provider.is_none()
        && crate::agent_catalog::agent(&launch.provider).is_some_and(|entry| entry.free_models)
}

/// A turn the agent refused while on its free models, said with what the person can do next.
fn free_model_failure(said: String) -> TurnEnd {
    if sounds_offline(&said) {
        return TurnEnd::Refused(said);
    }
    TurnEnd::Failed {
        failure: Failure {
            details: Some(said),
            ..Failure::other(FREE_MODEL_FAILED)
        },
        response: None,
    }
}

const FREE_MODEL_FAILED: &str = "The model didn't answer. Free models come and go: pick another in the model menu, or chat with an agent you're signed in to.";

fn compact_word(launch: &ChatLaunch) -> &'static str {
    match crate::agent_catalog::agent(&launch.provider).and_then(|entry| entry.chat.as_ref()) {
        Some(ChatDriver::Acp { compact, .. }) => compact,
        _ => COMPACT,
    }
}

/// Whether an `available_commands_update` payload advertises compaction natively.
fn advertises_compact(update: &Value, word: &str) -> bool {
    update["sessionUpdate"] == "available_commands_update"
        && update["availableCommands"]
            .as_array()
            .is_some_and(|commands| {
                commands
                    .iter()
                    .any(|command| command["name"] == word || command["name"] == COMPACT)
            })
}

/// Offers `/compact` in the agent's command list whatever the agent calls it, and offers it
/// even where the agent takes it without listing it.
fn offer_compact(update: &mut Value, word: &str) {
    if update["sessionUpdate"] != "available_commands_update" {
        return;
    }
    let Some(commands) = update["availableCommands"].as_array_mut() else {
        return;
    };
    if let Some(own) = commands.iter_mut().find(|command| command["name"] == word) {
        own["name"] = json!(COMPACT);
    } else if !commands.iter().any(|command| command["name"] == COMPACT) {
        commands.insert(
            0,
            json!({ "name": COMPACT, "description": "Summarize the conversation to free up context." }),
        );
    }
}

/// The person's `/compact`, in the agent's own word for it.
fn in_own_words(text: String, word: &str) -> String {
    match command_parts(&text) {
        Some((name, rest)) if word != COMPACT && name.eq_ignore_ascii_case(COMPACT) => {
            format!("/{word} {rest}").trim_end().to_owned()
        }
        _ => text,
    }
}

fn server_names(launch: &ChatLaunch) -> Arc<[String]> {
    launch
        .mcp_servers
        .iter()
        .filter_map(|server| server["name"].as_str().map(str::to_owned))
        .collect()
}

/// An agent's error in words, its details included.
fn describe(error: &Value) -> String {
    serde_json::from_value::<AcpError>(error.clone())
        .map_or_else(|_| error_text(error), |error| error.to_string())
}

/// Why the agent would not open the chat's session, in its own words.
enum Refused {
    SignIn(String),
    Other(String),
}

impl Refused {
    fn of(error: &Value) -> Self {
        let said = describe(error);
        if error.get("code").and_then(Value::as_i64) == Some(AUTH_REQUIRED) {
            Self::SignIn(said)
        } else {
            Self::Other(said)
        }
    }
}

impl From<String> for Refused {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl From<&str> for Refused {
    fn from(error: &str) -> Self {
        Self::Other(error.to_owned())
    }
}

/// The largest picture handed to an ACP agent as itself. Agents differ in
/// whether they shrink pictures, so this stays within what the model takes as sent.
const MAX_IMAGE_BYTES: u64 = 3_750_000;

/// `blocks` with each attached picture also handed over as the picture itself,
/// after its link, for an agent that takes pictures.
fn with_pictures(blocks: Vec<ContentBlock>, images: bool) -> Vec<ContentBlock> {
    if !images {
        return blocks;
    }
    let mut sent = Vec::with_capacity(blocks.len());
    for block in blocks {
        let picture = match &block {
            ContentBlock::ResourceLink(link) => inline_picture(&link.uri, MAX_IMAGE_BYTES),
            _ => None,
        };
        sent.push(block);
        sent.extend(
            picture
                .map(|(media_type, data)| ContentBlock::Image(ImageContent::new(data, media_type))),
        );
    }
    sent
}

fn params(message: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(message).map_err(|error| error.to_string())
}

/// Sets `key` on an object, making null into one first.
fn set(value: &mut Value, key: &str, field: Value) {
    if value.is_null() {
        *value = json!({});
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(key.to_owned(), field);
    }
}

/// The turn last prompted, the last one asked to stop while it ran, and
/// whether the agent's output has closed.
#[derive(Default, Serialize, Deserialize)]
struct Turns {
    prompted: u64,
    /// The running turn's prompts still unanswered: a steer sent as a prompt adds one.
    unanswered: u32,
    cancelled: u64,
    gone: bool,
    /// Whether the agent is replying with no prompt open, as Antigravity does once a command
    /// it left running in the background finishes.
    unprompted: bool,
    /// Counts the unprompted reply's output, so an end waiting on quiet sees more come.
    #[serde(skip)]
    said: u64,
    /// Antigravity's session and the last step its log held when the running turn was
    /// prompted, so its end can tell an error it recorded since.
    #[serde(skip)]
    log: Option<(String, i64)>,
    #[serde(default)]
    native_compact: bool,
    #[serde(default)]
    compacting: bool,
    #[serde(default)]
    last_used: u64,
    #[serde(skip)]
    compact_done: Arc<tokio::sync::Notify>,
}

impl Turns {
    /// Whether `turn` runs on after one of its prompts was answered, because a
    /// later prompt carries it on.
    fn carried_on(&mut self, turn: u64) -> bool {
        if turn != self.prompted {
            return false;
        }
        self.unanswered = self.unanswered.saturating_sub(1);
        self.unanswered > 0
    }
}

/// What the reader, the turns waiting on answers and the chat's loop share.
#[derive(Clone)]
struct Driver {
    rpc: Rpc,
    sink: Sink,
    turns: Arc<Mutex<Turns>>,
    /// The names of the tool servers the chat handed its agent.
    servers: Arc<[String]>,
    /// The agent's own name for its compaction command.
    compact: &'static str,
    free_models: bool,
    antigravity: bool,
    /// Antigravity's subagent conversations whose cards the chat shows.
    subagents: antigravity::Taken,
    /// In-flight Antigravity gateway tool calls, by `toolCallId`.
    gateway_calls: Arc<antigravity::GatewayCalls>,
}

impl Driver {
    async fn call(
        &self,
        method: &str,
        message: impl Serialize,
        wait: Option<Duration>,
    ) -> Result<Value, String> {
        self.rpc
            .start(method, params(message)?)
            .map_err(|error| describe(&error))?
            .answer(wait)
            .await
            .map_err(|error| describe(&error))
    }

    fn answered(&self, turn: u64, answer: Result<Value, Value>) {
        if self.turns.locked().carried_on(turn) {
            return;
        }
        self.gateway_calls.clear();
        let end = match answer {
            Ok(response) => match response
                .get("_meta")
                .and_then(air::failure)
                .or_else(|| self.logged_error())
            {
                Some(failure) => TurnEnd::Failed {
                    failure,
                    response: Some(response),
                },
                None => TurnEnd::Completed(response),
            },
            // Hermes can crash out of a stopped turn and leave its session
            // refusing every prompt after.
            Err(_) if self.turns.locked().cancelled == turn => TurnEnd::Broken(
                "The agent failed while stopping, so its session was restarted".into(),
            ),
            // The agent is gone, and the chat says why once its process has
            // exited.
            Err(_) if self.turns.locked().gone => return,
            Err(error) if error.get("code").and_then(Value::as_i64) == Some(AUTH_REQUIRED) => {
                TurnEnd::Failed {
                    failure: Failure::sign_in(error_text(&error)),
                    response: None,
                }
            }
            Err(error) if self.free_models => free_model_failure(describe(&error)),
            Err(error) => TurnEnd::Refused(describe(&error)),
        };
        self.sink.end_turn(turn, end);
    }

    fn logged_error(&self) -> Option<Failure> {
        let (session, before) = self.turns.locked().log.take()?;
        sessions::antigravity::turn_error(&session, before).map(antigravity::turn_failure)
    }

    fn ask(&self, id: Value, mut request: Value) {
        if self.antigravity {
            antigravity::as_common_permission(&mut request, &self.servers, &self.gateway_calls);
        }
        self.sink
            .ask(request, Asked { id }, |reply| self.responder(reply));
    }

    fn responder(&self, reply: Asked) -> Respond {
        let driver = self.clone();
        Box::new(move |chosen| driver.answer_permission(reply, chosen))
    }

    /// Tells the agent the option the person chose for the permission
    /// request `reply` names, or that they never chose.
    fn answer_permission(&self, reply: Asked, chosen: Option<String>) {
        let outcome = match chosen {
            Some(option_id) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
            }
            None => RequestPermissionOutcome::Cancelled,
        };
        self.rpc
            .respond(reply.id, json!(RequestPermissionResponse::new(outcome)));
    }

    fn purpose_answered(&self, purpose: Purpose, result: Answered) {
        match purpose {
            Purpose::Prompt(turn) => self.answered(turn, result),
            Purpose::Steer {
                text,
                paths,
                person,
            } => {
                if result.is_ok_and(|answer| {
                    answer.get("outcome").and_then(Value::as_str) == Some(Steered::Injected.word())
                }) {
                    self.sink.prompted(&text, &paths, person);
                }
            }
            Purpose::StopTask(_) => {
                if let Err(error) = result {
                    self.sink.error(describe(&error));
                }
            }
            Purpose::Compact(_) => {
                let used = {
                    let mut turns = self.turns.locked();
                    turns.compacting = false;
                    turns.compact_done.notify_waiters();
                    turns.last_used
                };
                if result.is_ok_and(|answer| {
                    answer.get("stopReason").and_then(Value::as_str) != Some("cancelled")
                }) {
                    self.sink.compacted(used);
                }
                self.sink.compaction_over();
            }
            _ => {}
        }
    }
}

impl Driver {
    /// Opens the chat's work when the agent speaks with no prompt open, and closes it once
    /// the usage that ends each of its replies has stood a while with nothing after it.
    fn follow_unprompted(&self, update: &Value) {
        if self.sink.reloading() {
            return;
        }
        let kind = UpdateKind::of(update);
        let said = {
            let mut turns = self.turns.locked();
            if kind == Some(UpdateKind::UsageUpdate) {
                if !turns.unprompted {
                    return;
                }
                turns.said
            } else {
                let output = matches!(
                    kind,
                    Some(
                        UpdateKind::AgentMessageChunk
                            | UpdateKind::AgentThoughtChunk
                            | UpdateKind::ToolCall
                    )
                );
                if !output || turns.unanswered > 0 {
                    return;
                }
                turns.said += 1;
                if turns.unprompted {
                    return;
                }
                turns.unprompted = true;
                drop(turns);
                self.sink.work_started();
                return;
            }
        };
        let driver = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(UNPROMPTED_QUIET).await;
            let ended = {
                let mut turns = driver.turns.locked();
                let ended = turns.unprompted && turns.said == said;
                if ended {
                    turns.unprompted = false;
                }
                ended
            };
            if ended {
                driver.sink.work_ended(StopReason::EndTurn);
            }
        });
    }
}

impl Driver {
    fn follow_subagent(&self, parent: &str, card: &Value, prompt: String) {
        let Some(card) = card["subagentSessionId"].as_str() else {
            return;
        };
        let turns = self.turns.clone();
        tokio::spawn(antigravity::follow(
            self.sink.clone(),
            parent.to_owned(),
            card.to_owned(),
            prompt,
            self.subagents.clone(),
            move || turns.locked().gone,
        ));
    }
}

impl Reads for Driver {
    type Nudge = ();
    type Message = Value;

    fn message(&mut self, message: Value) {
        match self.rpc.read(message) {
            Some(Incoming::Notification { method, mut params })
                if method == CLIENT_METHOD_NAMES.session_update =>
            {
                let mut update = params
                    .get_mut("update")
                    .map(Value::take)
                    .unwrap_or_default();
                if !crate::server::chat::prompt::drop_for_agent(&mut update) {
                    return;
                }
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if self.antigravity {
                    if advertises_compact(&update, self.compact) {
                        self.turns.locked().native_compact = true;
                    }
                    if UpdateKind::of(&update) == Some(UpdateKind::UsageUpdate) {
                        if let Some(used) = update.get("used").and_then(Value::as_u64) {
                            let compacting = {
                                let mut turns = self.turns.locked();
                                turns.last_used = used;
                                turns.compacting
                            };
                            if !self.sink.reloading() && !compacting {
                                self.sink.called(used);
                            }
                            if compacting {
                                return self.sink.update(session_id, update);
                            }
                        }
                    } else if self.turns.locked().compacting {
                        return;
                    }
                    if let Some((card, prompt)) = antigravity::spawn(&update) {
                        if card.is_null() {
                            return;
                        }
                        if let Some(prompt) = prompt.filter(|_| !self.sink.reloading()) {
                            self.follow_subagent(session_id, &card, prompt);
                        }
                        return self.sink.update(session_id, card);
                    }
                    antigravity::as_common_call(
                        &mut update,
                        &self.servers,
                        Some(&self.gateway_calls),
                    );
                }
                mcp_calls::as_mcp_call(&mut update, &self.servers);
                offer_compact(&mut update, self.compact);
                self.follow_unprompted(&update);
                self.sink.update(session_id, update);
            }
            Some(Incoming::Request { id, method, params })
                if method == CLIENT_METHOD_NAMES.session_request_permission =>
            {
                self.ask(id, params);
            }
            Some(Incoming::Request { id, method, .. }) => self
                .rpc
                .refuse(id, &format!("Sikemux does not handle {method}")),
            Some(Incoming::Answer { purpose, result }) => self.purpose_answered(purpose, result),
            Some(Incoming::Notification { .. }) | None => {}
        }
    }

    fn closed(&mut self) {
        let was_compacting = {
            let mut turns = self.turns.locked();
            turns.gone = true;
            let was = std::mem::take(&mut turns.compacting);
            if was {
                turns.compact_done.notify_waiters();
            }
            was
        };
        if was_compacting {
            self.sink.compaction_over();
        }
        self.rpc.close();
    }
}

/// What an ACP chat knows of its session, as it hands it to a core that
/// takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    session_id: String,
    setup: Value,
    steering: bool,
    embedded_context: bool,
    /// Whether the agent takes attached pictures as pictures.
    images: bool,
    can_load: bool,
    /// Whether the session's models are picked through `session/set_model`
    /// rather than as a config option.
    model_outside_config: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcpState {
    rpc: RpcState,
    turns: Turns,
    #[serde(default)]
    gateway_calls: Vec<(String, String, Option<Value>)>,
    #[serde(flatten)]
    kept: Kept,
}

#[derive(Clone, Copy)]
enum Reopens {
    /// `session/load`, replaying its history.
    Load,
    /// `session/resume`, without its history.
    Resume,
    No,
}

/// A steer the agent has taken, or one it will answer whether it took.
enum Steering {
    Done(Steered),
    Answering(Sent),
}

pub struct Acp {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    launch: ChatLaunch,
    kept: Kept,
}

impl Acp {
    async fn begin(launch: &ChatLaunch, sink: Sink) -> Result<(Self, Opened), String> {
        let (args, extra_env) = if is_antigravity(launch) {
            (
                antigravity::launch_args(launch.permission_mode),
                antigravity::launch_env(&launch.env),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let (process, writer, lines) = Process::spawn(launch, args, extra_env)?;
        let driver = Driver {
            rpc: Rpc::new(writer, true),
            sink,
            turns: Arc::default(),
            servers: server_names(launch),
            compact: compact_word(launch),
            free_models: on_free_models(launch),
            antigravity: is_antigravity(launch),
            subagents: Default::default(),
            gateway_calls: Arc::default(),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        let failed = |error: String| process.start_error(error);

        let initialize = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(air::client_capabilities())
            .client_info(Implementation::new("sikemux", env!("CARGO_PKG_VERSION")));
        let initialized = driver
            .call(AGENT_METHOD_NAMES.initialize, initialize, None)
            .await
            .map_err(failed)?;
        let command =
            crate::agent_catalog::agent(&launch.provider).map_or("", |entry| entry.command);
        let sign_in = crate::acp::sign_in::methods(&initialized, command);
        let key_sign_in = crate::agent_catalog::AgentKind::from_name(&launch.provider)
            .and_then(|agent| agent.ops().key_sign_in(&launch.env));
        let signs_in = !sign_in.is_empty();
        let initialized: InitializeResponse =
            serde_json::from_value(initialized).map_err(|error| failed(error.to_string()))?;
        if let Some(agent) = &initialized.agent_info {
            driver.sink.cli_version(&agent.version);
        }
        let mut capabilities = params(&initialized.agent_capabilities)?;
        let steering = driver.antigravity || air::steering_supported(&params(&initialized.meta)?);
        set(&mut capabilities, "steering", json!(steering));
        set(&mut capabilities, "signInMethods", json!(sign_in));
        let reopens = if initialized.agent_capabilities.load_session {
            Reopens::Load
        } else if initialized
            .agent_capabilities
            .session_capabilities
            .resume
            .is_some()
        {
            Reopens::Resume
        } else {
            Reopens::No
        };

        let opened = match Self::open(&driver, launch, reopens).await {
            Err(Refused::SignIn(_)) if key_sign_in.is_some() => {
                let method = json!({ "methodId": key_sign_in });
                (driver
                    .call(AGENT_METHOD_NAMES.authenticate, method, None)
                    .await)
                    .map_err(failed)?;
                Self::open(&driver, launch, reopens).await
            }
            opened => opened,
        };
        let (session_id, setup) = match opened {
            Ok(opened) => opened,
            Err(Refused::SignIn(said)) if signs_in => {
                driver.sink.needs_sign_in();
                return Err(said);
            }
            Err(Refused::SignIn(error) | Refused::Other(error)) => return Err(failed(error)),
        };
        driver.sink.loaded();

        let mut acp = Self {
            process,
            driver,
            reading,
            launch: launch.clone(),
            kept: Kept {
                session_id,
                model_outside_config: native::models_outside_config(&setup),
                setup: native::with_model_config(setup),
                steering,
                embedded_context: initialized
                    .agent_capabilities
                    .prompt_capabilities
                    .embedded_context,
                images: initialized.agent_capabilities.prompt_capabilities.image,
                can_load: !matches!(reopens, Reopens::No),
            },
        };

        if let Some(mode_id) = permission_mode_id(
            &acp.launch.provider,
            launch.permission_mode,
            &acp.kept.setup,
        )? {
            acp.set_mode(mode_id).await?;
        }
        acp.apply_saved_choices(launch.model.as_deref(), launch.effort.as_deref())
            .await;

        let opened = Opened {
            session_id: acp.kept.session_id.clone(),
            capabilities,
            setup: acp.kept.setup.clone(),
        };
        Ok((acp, opened))
    }

    /// Starts agy again on the session, since whether it asks before acting is set as it starts.
    async fn relaunch(&mut self, mode: PermissionMode) -> Result<(), String> {
        let mut launch = self.launch.clone();
        launch.resume_id = Some(self.kept.session_id.clone());
        launch.permission_mode = mode;
        launch.model = pickers::current_choice(&self.kept.setup, "model").or(launch.model);
        launch.effort = pickers::effort_config_id(&self.kept.setup)
            .and_then(|id| pickers::current_choice(&self.kept.setup, id))
            .or(launch.effort);
        self.process.stop().await;
        self.driver.sink.hush();
        *self = Self::begin(&launch, self.driver.sink.clone()).await?.0;
        Ok(())
    }

    fn session_meta(driver: &Driver, launch: &ChatLaunch, servers: &[McpServer]) -> Option<Meta> {
        let mut meta = native::session_meta(&launch.provider);
        if driver.antigravity && servers::offers_sikemux_tools(servers) {
            if let Some(instructions) = driver.sink.tool_instructions() {
                meta.get_or_insert_with(Meta::new).insert(
                    "antigravity".into(),
                    json!({ "systemPrompt": { "append": instructions } }),
                );
            }
        }
        meta
    }

    async fn open_new(
        driver: &Driver,
        launch: &ChatLaunch,
        servers: Vec<McpServer>,
    ) -> Result<(String, Value), Refused> {
        let meta = Self::session_meta(driver, launch, &servers);
        let request = NewSessionRequest::new(&launch.cwd)
            .mcp_servers(servers)
            .meta(meta);
        let setup = driver
            .rpc
            .start(AGENT_METHOD_NAMES.session_new, params(request)?)
            .map_err(|error| describe(&error))?
            .answer(None)
            .await
            .map_err(|error| Refused::of(&error))?;
        let session_id = setup
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or("The agent opened a session without an id")?
            .to_owned();
        Ok((session_id, setup))
    }

    /// Opens the chat's session: the one it resumes, or a new one. A loaded
    /// session replays its history before it answers. An agent that cannot load one may
    /// still take it up again without its history, which the chat already shows; anything
    /// it replays is not shown again.
    async fn open(
        driver: &Driver,
        launch: &ChatLaunch,
        reopens: Reopens,
    ) -> Result<(String, Value), Refused> {
        let servers = servers::tool_servers(launch);
        let Some(existing) = launch.resume_id.clone() else {
            return Self::open_new(driver, launch, servers).await;
        };
        let meta = Self::session_meta(driver, launch, &servers);
        let (method, request) = match reopens {
            Reopens::Load => (
                AGENT_METHOD_NAMES.session_load,
                params(
                    LoadSessionRequest::new(existing.clone(), &launch.cwd)
                        .mcp_servers(servers.clone())
                        .meta(meta),
                )?,
            ),
            Reopens::Resume => {
                driver.sink.hush();
                (
                    AGENT_METHOD_NAMES.session_resume,
                    params(
                        ResumeSessionRequest::new(existing.clone(), &launch.cwd)
                            .mcp_servers(servers.clone())
                            .meta(meta),
                    )?,
                )
            }
            Reopens::No => return Err("This agent cannot load existing sessions".into()),
        };
        let loaded = driver
            .rpc
            .start(method, request)
            .map_err(|error| describe(&error))?
            .answer(None)
            .await;
        match loaded {
            Ok(setup) => Ok((existing, setup)),
            // A chat whose turns all failed may have nothing saved to load,
            // and starting again is all there is to do.
            Err(_) if driver.sink.reloading() && !driver.sink.has_answered() => {
                Self::open_new(driver, launch, servers).await
            }
            Err(error) => Err(Refused::of(&error)),
        }
    }

    async fn set_model(&mut self, model: &str) -> Result<(), String> {
        self.driver
            .call(
                native::SetSessionModel::METHOD,
                native::SetSessionModel {
                    session_id: self.kept.session_id.clone(),
                    model_id: model.to_owned(),
                },
                Some(ANSWER_WAIT),
            )
            .await?;
        native::select_model(&mut self.kept.setup, model);
        Ok(())
    }

    async fn set_option(&mut self, config_id: &str, value: &str) -> Result<(), String> {
        let answer = self
            .driver
            .call(
                AGENT_METHOD_NAMES.session_set_config_option,
                SetSessionConfigOptionRequest::new(
                    self.kept.session_id.clone(),
                    config_id.to_owned(),
                    value,
                ),
                Some(ANSWER_WAIT),
            )
            .await?;
        if let Some(options) = answer.get("configOptions") {
            set(&mut self.kept.setup, "configOptions", options.clone());
        }
        Ok(())
    }

    /// Carries the chat's saved model and effort into the session. A choice
    /// the agent no longer offers is skipped, since model lists change between
    /// launches and a stale one should not stop the chat from starting.
    async fn apply_saved_choices(&mut self, model: Option<&str>, effort: Option<&str>) {
        if let Some(model) = model.filter(|model| pickers::offers(&self.kept.setup, "model", model))
        {
            let applied = if self.kept.model_outside_config {
                self.set_model(model).await
            } else {
                self.set_option("model", model).await
            };
            if let Err(error) = applied {
                eprintln!("The agent did not take the saved model {model}: {error}");
            }
        }
        let Some(effort) = effort else {
            return;
        };
        let Some(config_id) = pickers::effort_config_id(&self.kept.setup)
            .filter(|id| pickers::offers(&self.kept.setup, id, effort))
            .map(str::to_owned)
        else {
            return;
        };
        if let Err(error) = self.set_option(&config_id, effort).await {
            eprintln!("The agent did not take the saved effort {effort}: {error}");
        }
    }

    /// Antigravity takes a prompt sent while a turn runs into that turn: it
    /// answers the earlier prompt at once and carries the turn on under the new one.
    fn prompt_into_turn(&self, prompt: Prompt) -> Result<Steered, String> {
        if !self.driver.sink.running() {
            return Ok(Steered::PromptRequired);
        }
        let blocks = prompt_blocks(
            prompt.text,
            prompt.paths,
            prompt.context,
            self.kept.embedded_context,
        )?;
        let blocks = with_pictures(blocks, self.kept.images);
        let request = PromptRequest::new(self.kept.session_id.clone(), blocks);
        let turn = {
            let mut turns = self.driver.turns.locked();
            turns.unanswered += 1;
            turns.prompted
        };
        self.driver.rpc.send_for(
            AGENT_METHOD_NAMES.session_prompt,
            params(request)?,
            Purpose::Prompt(turn),
        )?;
        Ok(Steered::Injected)
    }

    /// Writes a steer to the agent now, so a stop sent after it reaches the
    /// agent after it too.
    fn send_steer(&self, prompt: Prompt) -> Result<Sent, String> {
        if !self.kept.steering {
            return Err("this agent cannot take a message mid-turn".to_owned());
        }
        let purpose = Purpose::Steer {
            text: prompt.text.clone(),
            paths: prompt.paths.clone(),
            person: !prompt.letter,
        };
        let blocks = prompt_blocks(
            prompt.text,
            prompt.paths,
            prompt.context,
            self.kept.embedded_context,
        )?;
        let blocks = with_pictures(blocks, self.kept.images);
        let steer = air::Steer::new(self.kept.session_id.clone(), blocks);
        self.driver
            .rpc
            .start_for(air::Steer::METHOD, params(steer)?, Some(purpose))
            .map_err(|error| describe(&error))
    }

    async fn set_mode(&self, mode_id: &str) -> Result<(), String> {
        self.driver
            .call(
                AGENT_METHOD_NAMES.session_set_mode,
                SetSessionModeRequest::new(self.kept.session_id.clone(), mode_id.to_owned()),
                Some(ANSWER_WAIT),
            )
            .await
            .map(drop)
    }
}

impl Backend for Acp {
    type Reply = Asked;

    async fn start(
        launch: &ChatLaunch,
        _brief: Option<&str>,
        sink: Sink,
    ) -> Result<(Self, Opened), String> {
        Self::begin(launch, sink).await
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let compacting = {
            let turns = self.driver.turns.locked();
            turns
                .compacting
                .then(|| turns.compact_done.clone().notified_owned())
        };
        if let Some(done) = compacting {
            let _ = tokio::time::timeout(Duration::from_secs(60), done).await;
        }
        let blocks = prompt_blocks(
            in_own_words(prompt.text, self.driver.compact),
            prompt.paths,
            prompt.context,
            self.kept.embedded_context,
        )?;
        let blocks = with_pictures(blocks, self.kept.images);
        let request = PromptRequest::new(self.kept.session_id.clone(), blocks);
        {
            let mut turns = self.driver.turns.locked();
            turns.prompted = turn;
            turns.unanswered = 1;
            turns.unprompted = false;
            turns.log = self.driver.antigravity.then(|| {
                let session = self.kept.session_id.to_string();
                let last = sessions::antigravity::last_step(&session);
                (session, last)
            });
        }
        self.driver.rpc.send_for(
            AGENT_METHOD_NAMES.session_prompt,
            params(request)?,
            Purpose::Prompt(turn),
        )
    }

    async fn compact(&mut self) -> Result<(), String> {
        if !self.driver.antigravity {
            return Err("this agent cannot compact quietly".to_owned());
        }
        {
            let mut turns = self.driver.turns.locked();
            if !turns.native_compact {
                return Err("this Antigravity version does not advertise /compact".to_owned());
            }
            if self.driver.sink.running() || turns.unanswered > 0 || turns.unprompted {
                return Err("the chat is working".to_owned());
            }
            if turns.compacting {
                return Ok(());
            }
            turns.compacting = true;
        }
        let text = format!(
            "/{} {}",
            self.driver.compact,
            crate::server::chat::cap::IDLE_FOCUS
        );
        let request =
            PromptRequest::new(self.kept.session_id.clone(), vec![ContentBlock::from(text)]);
        self.driver
            .rpc
            .send_for(
                AGENT_METHOD_NAMES.session_prompt,
                params(request)?,
                Purpose::Compact(0),
            )
            .inspect_err(|_| {
                let mut turns = self.driver.turns.locked();
                turns.compacting = false;
                turns.compact_done.notify_waiters();
            })
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static {
        let steering = if self.driver.antigravity {
            self.prompt_into_turn(prompt).map(Steering::Done)
        } else {
            self.send_steer(prompt).map(Steering::Answering)
        };
        async move {
            let sent = match steering? {
                Steering::Done(steered) => return Ok(steered),
                Steering::Answering(sent) => sent,
            };
            let answer = sent.answer(None).await.map_err(|error| describe(&error))?;
            match answer.get("outcome").unwrap_or(&Value::Null) {
                outcome if outcome == Steered::Injected.word() => Ok(Steered::Injected),
                outcome if outcome == Steered::PromptRequired.word() => Ok(Steered::PromptRequired),
                outcome => Err(format!("The agent answered a steer with {outcome}")),
            }
        }
    }

    async fn cancel(&mut self) -> Result<(), String> {
        let sent = self.driver.rpc.notify(
            AGENT_METHOD_NAMES.session_cancel,
            params(CancelNotification::new(self.kept.session_id.clone()))?,
        );
        if self.driver.sink.running() {
            let mut turns = self.driver.turns.locked();
            turns.cancelled = turns.prompted;
        }
        if std::mem::take(&mut self.driver.turns.locked().unprompted) {
            self.driver.sink.work_ended(StopReason::Cancelled);
        }
        sent
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        if self.driver.antigravity
            && antigravity::launch_args(mode)
                != antigravity::launch_args(self.launch.permission_mode)
        {
            return self.relaunch(mode).await;
        }
        if let Some(mode_id) = permission_mode_id(&self.launch.provider, mode, &self.kept.setup)? {
            self.set_mode(mode_id).await?;
        }
        self.launch.permission_mode = mode;
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        if config_id == "model" && self.kept.model_outside_config {
            self.set_model(value).await?;
        } else {
            self.set_option(config_id, value).await?;
        }
        Ok(self
            .kept
            .setup
            .get("configOptions")
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn authenticate(&mut self, method_id: &str) -> Result<(), String> {
        self.driver
            .call(
                AGENT_METHOD_NAMES.authenticate,
                json!({ "methodId": method_id }),
                None,
            )
            .await
            .map(|_| ())
    }

    /// A background task outlives the turn that started it, so stopping one
    /// does not wait on the agent's answer.
    async fn stop_task(&mut self, task_id: &str) -> Result<(), String> {
        let stop = air::StopAsyncTask {
            session_id: self.kept.session_id.clone(),
            async_task_id: task_id.to_owned(),
        };
        self.driver.rpc.send_for(
            air::StopAsyncTask::METHOD,
            params(stop)?,
            Purpose::StopTask(task_id.to_owned()),
        )
    }

    async fn rewind(&mut self, _message_id: &str) -> Result<Option<String>, String> {
        Err("This agent cannot edit a sent message".into())
    }

    async fn closed(&self) -> String {
        self.process.ended().await
    }

    fn pid(&self) -> u32 {
        self.process.pid()
    }

    fn reloadable(&self) -> bool {
        self.kept.can_load
    }

    async fn release(self) -> Result<(Value, Released), String> {
        let Self {
            process,
            driver,
            reading,
            launch: _,
            kept,
        } = self;
        process::hand_over(process, reading, "The agent", |_| AcpState {
            rpc: driver.rpc.state(),
            turns: std::mem::take(&mut *driver.turns.locked()),
            gateway_calls: driver.gateway_calls.snapshot(),
            kept,
        })
        .await
    }

    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let state: AcpState = serde_json::from_value(state).map_err(|error| error.to_string())?;
        let driver = Driver {
            rpc: Rpc::restored(writer, state.rpc),
            sink,
            turns: Arc::new(Mutex::new(state.turns)),
            servers: server_names(launch),
            compact: compact_word(launch),
            free_models: on_free_models(launch),
            antigravity: is_antigravity(launch),
            subagents: Default::default(),
            gateway_calls: Arc::new(antigravity::GatewayCalls::restored(state.gateway_calls)),
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok(Self {
            process,
            driver,
            reading,
            launch: launch.clone(),
            kept: state.kept,
        })
    }

    fn responder(&self, reply: Asked) -> Respond {
        self.driver.responder(reply)
    }
}
