//! Agents that speak the Agent Client Protocol themselves, such as OpenCode,
//! OMP, Grok and Hermes: JSON-RPC, one message per line, on the agent's
//! standard input and output. Their session updates already have the shape
//! the chat reads, so they pass through as they come.

use sikemux_fold::UpdateKind;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, Error as AcpError, Implementation, InitializeRequest, InitializeResponse,
    LoadSessionRequest, McpServer, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome,
    SetSessionConfigOptionRequest, SetSessionModeRequest, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES,
};
use agent_client_protocol::schema::ProtocolVersion;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::acp::{air, native, permission_mode_id};
use crate::launch::PermissionMode;
use crate::protocol::ChatLaunch;
use crate::server::chat::failure::Failure;
use crate::server::chat::prompt::prompt_blocks;

use super::super::Respond;
use super::pending::{Answered, Purpose};
use super::process::{Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, Incoming, Rpc, RpcState, Sent, ANSWER_WAIT};
use super::session::{Backend, Opened, Prompt, Sink, TurnEnd};
use super::{mcp_calls, servers};

/// The error code an agent turns a prompt down with when it needs a sign-in.
const AUTH_REQUIRED: i64 = -32000;

/// An agent's error in words, its details included.
/// Shows a saved session of an agent that keeps one Sikemux can read, as the agent would
/// replay it on loading it.
pub(super) fn replay_saved(launch: &ChatLaunch, session: &str, sink: &Sink) -> Result<(), String> {
    let agent = crate::agent_catalog::AgentKind::from_name(&launch.provider)
        .ok_or_else(|| format!("{} keeps no sessions Sikemux reads", launch.provider))?;
    let servers = server_names(launch);
    for mut update in crate::sessions::history(agent, session, &launch.cwd)? {
        mcp_calls::as_mcp_call(&mut update, &servers);
        sink.update(session, update);
    }
    sink.update(
        session,
        json!({ "sessionUpdate": UpdateKind::HistoryLoaded }),
    );
    Ok(())
}

fn server_names(launch: &ChatLaunch) -> Arc<[String]> {
    launch
        .mcp_servers
        .iter()
        .filter_map(|server| server["name"].as_str().map(str::to_owned))
        .collect()
}

fn describe(error: &Value) -> String {
    serde_json::from_value::<AcpError>(error.clone())
        .map_or_else(|_| error_text(error), |error| error.to_string())
}

fn params(message: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(message).map_err(|error| error.to_string())
}

/// The turn last prompted, the last one asked to stop while it ran, and
/// whether the agent's output has closed.
#[derive(Default, Serialize, Deserialize)]
struct Turns {
    prompted: u64,
    cancelled: u64,
    gone: bool,
}

/// What the reader, the turns waiting on answers and the chat's loop share.
#[derive(Clone)]
struct Driver {
    rpc: Rpc,
    sink: Sink,
    turns: Arc<Mutex<Turns>>,
    /// The names of the tool servers the chat handed its agent.
    servers: Arc<[String]>,
}

impl Driver {
    /// Sends a request and waits on its answer, for at most `wait`.
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

    /// Settles turn `turn` with the agent's answer to its prompt.
    fn answered(&self, turn: u64, answer: Result<Value, Value>) {
        let end = match answer {
            Ok(response) => match response.get("_meta").and_then(air::failure) {
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
            Err(error) if error["code"] == AUTH_REQUIRED => TurnEnd::Failed {
                failure: Failure::sign_in(error_text(&error)),
                response: None,
            },
            Err(error) => TurnEnd::Refused(describe(&error)),
        };
        self.sink.end_turn(turn, end);
    }

    fn ask(&self, id: Value, request: Value) {
        let reply = json!({ "id": id });
        self.sink.ask(request, reply.clone(), self.responder(reply));
    }

    fn responder(&self, reply: Value) -> Respond {
        let driver = self.clone();
        Box::new(move |chosen| driver.answer_permission(&reply, chosen))
    }

    /// Tells the agent the option the person chose for the permission
    /// request `reply` names, or that they never chose.
    fn answer_permission(&self, reply: &Value, chosen: Option<String>) {
        let outcome = match chosen {
            Some(option_id) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
            }
            None => RequestPermissionOutcome::Cancelled,
        };
        self.rpc.respond(
            reply["id"].clone(),
            json!(RequestPermissionResponse::new(outcome)),
        );
    }

    /// The answer to a request sent with `purpose`.
    fn purpose_answered(&self, purpose: Purpose, result: Answered) {
        match purpose {
            Purpose::Prompt(turn) => self.answered(turn, result),
            Purpose::Steer { text, paths } => {
                if result.is_ok_and(|answer| answer["outcome"] == "injected") {
                    self.sink.prompted(&text, &paths);
                }
            }
            Purpose::StopTask(_) => {
                if let Err(error) = result {
                    self.sink.error(describe(&error));
                }
            }
            _ => {}
        }
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
                mcp_calls::as_mcp_call(&mut update, &self.servers);
                let session_id = params["sessionId"].as_str().unwrap_or_default();
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
        self.turns.locked().gone = true;
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
    #[serde(flatten)]
    kept: Kept,
}

/// How an agent takes up a session it ran before.
#[derive(Clone, Copy)]
enum Reopens {
    /// `session/load`, replaying its history.
    Load,
    /// `session/resume`, without its history.
    Resume,
    No,
}

pub(crate) struct Acp {
    process: Process,
    driver: Driver,
    reading: Reading<Driver>,
    provider: String,
    kept: Kept,
}

impl Acp {
    async fn open_new(
        driver: &Driver,
        launch: &ChatLaunch,
        servers: Vec<McpServer>,
    ) -> Result<(String, Value), String> {
        let setup = driver
            .call(
                AGENT_METHOD_NAMES.session_new,
                NewSessionRequest::new(&launch.cwd)
                    .mcp_servers(servers)
                    .meta(native::session_meta(&launch.provider)),
                None,
            )
            .await?;
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
    ) -> Result<(String, Value), String> {
        let servers = servers::tool_servers(launch);
        let Some(existing) = launch.resume_id.clone() else {
            return Self::open_new(driver, launch, servers).await;
        };
        let (method, request) = match reopens {
            Reopens::Load => (
                AGENT_METHOD_NAMES.session_load,
                params(
                    LoadSessionRequest::new(existing.clone(), &launch.cwd)
                        .mcp_servers(servers.clone())
                        .meta(native::session_meta(&launch.provider)),
                )?,
            ),
            Reopens::Resume => {
                driver.sink.hush();
                (
                    AGENT_METHOD_NAMES.session_resume,
                    params(
                        ResumeSessionRequest::new(existing.clone(), &launch.cwd)
                            .mcp_servers(servers.clone())
                            .meta(native::session_meta(&launch.provider)),
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
            Err(error) => Err(describe(&error)),
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
            self.kept.setup["configOptions"] = options.clone();
        }
        Ok(())
    }

    /// Carries the chat's saved model and effort into the session. A choice
    /// the agent no longer offers is skipped, since model lists change between
    /// launches and a stale one should not stop the chat from starting.
    async fn apply_saved_choices(&mut self, model: Option<&str>, effort: Option<&str>) {
        if let Some(model) = model.filter(|model| native::offers(&self.kept.setup, "model", model))
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
        let Some(config_id) = native::effort_config_id(&self.kept.setup)
            .filter(|id| native::offers(&self.kept.setup, id, effort))
            .map(str::to_owned)
        else {
            return;
        };
        if let Err(error) = self.set_option(&config_id, effort).await {
            eprintln!("The agent did not take the saved effort {effort}: {error}");
        }
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
        };
        let blocks = prompt_blocks(
            prompt.text,
            prompt.paths,
            prompt.context,
            self.kept.embedded_context,
        )?;
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
    async fn start(launch: &ChatLaunch, sink: Sink) -> Result<(Self, Opened), String> {
        let (process, writer, lines) =
            Process::spawn(launch, Vec::<String>::new(), Vec::<(String, String)>::new())?;
        let driver = Driver {
            rpc: Rpc::new(writer, true),
            sink,
            turns: Arc::default(),
            servers: server_names(launch),
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
        let initialized: InitializeResponse =
            serde_json::from_value(initialized).map_err(|error| failed(error.to_string()))?;
        if let Some(agent) = &initialized.agent_info {
            driver.sink.cli_version(&agent.version);
        }
        let mut capabilities = params(&initialized.agent_capabilities)?;
        let steering = air::steering_supported(&params(&initialized.meta)?);
        capabilities["steering"] = json!(steering);
        capabilities["signInMethods"] = json!(sign_in);
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

        let (session_id, setup) = Self::open(&driver, launch, reopens).await.map_err(failed)?;
        driver.sink.loaded();

        let mut acp = Self {
            process,
            driver,
            reading,
            provider: launch.provider.clone(),
            kept: Kept {
                session_id,
                model_outside_config: native::models_outside_config(&setup),
                setup: native::with_model_config(setup),
                steering,
                embedded_context: initialized
                    .agent_capabilities
                    .prompt_capabilities
                    .embedded_context,
                can_load: !matches!(reopens, Reopens::No),
            },
        };

        if let Some(mode_id) =
            permission_mode_id(&acp.provider, launch.permission_mode, &acp.kept.setup)?
        {
            acp.set_mode(mode_id).await?;
            if let Some(modes) = acp
                .kept
                .setup
                .get_mut("modes")
                .and_then(Value::as_object_mut)
            {
                modes.insert("currentModeId".into(), json!(mode_id));
            }
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

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let blocks = prompt_blocks(
            prompt.text,
            prompt.paths,
            prompt.context,
            self.kept.embedded_context,
        )?;
        let request = PromptRequest::new(self.kept.session_id.clone(), blocks);
        self.driver.turns.locked().prompted = turn;
        self.driver.rpc.send_for(
            AGENT_METHOD_NAMES.session_prompt,
            params(request)?,
            Purpose::Prompt(turn),
        )
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send + 'static {
        let sent = self.send_steer(prompt);
        async move {
            let answer = sent?.answer(None).await.map_err(|error| describe(&error))?;
            Ok(answer["outcome"].as_str().unwrap_or_default().to_owned())
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
        sent
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        match permission_mode_id(&self.provider, mode, &self.kept.setup)? {
            Some(mode_id) => self.set_mode(mode_id).await,
            None => Ok(()),
        }
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        if config_id == "model" && self.kept.model_outside_config {
            self.set_model(value).await?;
        } else {
            self.set_option(config_id, value).await?;
        }
        Ok(self.kept.setup["configOptions"].clone())
    }

    /// A background task outlives the turn that started it, so stopping one
    /// does not wait on the agent's answer.
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
            provider: _,
            kept,
        } = self;
        let (driver_back, lines) = reading.stop().await.ok_or("The agent stopped")?;
        drop(driver_back);
        let turns = std::mem::take(&mut *driver.turns.locked());
        let state = AcpState {
            rpc: driver.rpc.state(),
            turns,
            kept,
        };
        let state = serde_json::to_value(state).map_err(|error| error.to_string())?;
        let released = process.release(lines).await?;
        Ok((state, released))
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
        };
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok(Self {
            process,
            driver,
            reading,
            provider: launch.provider.clone(),
            kept: state.kept,
        })
    }

    fn responder(&self, reply: Value) -> Respond {
        self.driver.responder(reply)
    }
}
