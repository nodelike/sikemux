//! OpenCode, spoken to over the HTTP API and event stream of an `opencode serve` of the chat's
//! own. OpenCode's events come out as the session updates the other agents send for the same
//! work, translated as OpenCode's own ACP front translates them.

mod options;
mod state;
mod tools;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::agent_catalog::AgentKind;
use crate::launch::PermissionMode;
use crate::server::chat::prompt::command_parts;
use crate::sessions::for_agent::context_text;
use crate::sessions::opencode_server::{
    new_password, ready_url, Client, Refused, PASSWORD_ENV, READY_WAIT, SERVE_ARGS,
};
use sikemux_wire::protocol::ChatLaunch;

use super::super::Respond;
use super::file_link;
use super::mcp_calls::as_mcp_call;
use super::parts::{parts, Part};
use super::process::{self, Lines, Process, Reading, Reads, Released, Writer};
use super::servers;
use super::session::{Backend, Opened, Prompt, Sink, Steered};
use options::{model_ref, read_ref, BUILD, EFFORT_ID, MODE_ID};
pub use options::{read_options, Catalog};
use state::{Effect, Session};

/// What answers OpenCode on a permission it asked for.
#[derive(Serialize, Deserialize)]
pub struct Asked {
    session: String,
    id: String,
}

/// Reads what the server prints after the line saying where it listens, which is nothing the
/// chat needs, so its output never fills.
struct Quiet;

impl Reads for Quiet {
    type Nudge = ();
    type Message = Value;

    fn message(&mut self, _message: Value) {}

    fn closed(&mut self) {}
}

/// What the chat's loop, the event reader and the answers to the person share.
#[derive(Clone)]
struct Driver {
    client: Client,
    sink: Sink,
    session: Arc<Mutex<Session>>,
    servers: Arc<[String]>,
}

impl Driver {
    fn session_id(&self) -> String {
        self.session.locked().id.clone()
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/session/{}{rest}", self.session_id())
    }

    /// OpenCode took `prompt` as the message `sent`, kept for going back to before it.
    fn sent(&self, prompt: &Prompt, sent: &Value) {
        if let (Some(ours), Some(theirs)) = (&prompt.message_id, sent["id"].as_str()) {
            let mut session = self.session.locked();
            session.messages.insert(ours.clone(), theirs.to_owned());
        }
    }

    fn handle(&self, event: &Value) {
        let effects = self.session.locked().handle(event);
        self.apply(effects);
    }

    fn apply(&self, effects: Vec<Effect>) {
        let session = self.session_id();
        for effect in effects {
            match effect {
                Effect::Update(mut update) => {
                    as_mcp_call(&mut update, &self.servers);
                    self.sink.update(&session, update);
                }
                Effect::In(child, mut update) => {
                    as_mcp_call(&mut update, &self.servers);
                    self.sink.update(&child, update);
                }
                Effect::End(turn, end) => self.sink.end_turn(turn, end),
                Effect::WorkStarted => self.sink.work_started(),
                Effect::WorkEnded(reason) => self.sink.work_ended(reason),
                Effect::Ask {
                    session,
                    request,
                    id,
                } => {
                    let reply = Asked { session, id };
                    self.sink.ask(request, reply, |reply| self.responder(reply));
                }
                Effect::Question {
                    session,
                    form,
                    input,
                } => self.put_question(session, form, &input),
                Effect::Compacted => {
                    self.sink.compacted(0);
                    self.sink.compaction_over();
                }
                Effect::CompactionOver => self.sink.compaction_over(),
                Effect::Settled(id) => self.sink.withdraw_permission(&id),
                Effect::Called(context) => self.sink.called(context),
                Effect::Selected => {}
            }
        }
    }

    /// Puts OpenCode's form to the person as the ask tool's questions, and tells OpenCode what
    /// Sikemux said in place of the answer, which reaches it later as a message.
    fn put_question(&self, session: String, form: String, input: &Value) {
        let told = self.sink.question(input);
        let message: String = url::form_urlencoded::byte_serialize(told.as_bytes()).collect();
        let path = format!("/api/session/{session}/form/{form}?message={message}");
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(error) = client.delete(&path).await {
                eprintln!("sikemux core: OpenCode did not take the note on its question: {error}");
            }
        });
    }

    fn responder(&self, reply: Asked) -> Respond {
        let client = self.client.clone();
        Box::new(move |chosen| {
            let decision = match chosen.as_deref() {
                Some("once") => "once",
                Some("always") => "always",
                _ => "reject",
            };
            tokio::spawn(async move {
                let path = format!(
                    "/api/session/{}/permission/{}/reply",
                    reply.session, reply.id
                );
                if let Err(error) = client.post(&path, json!({ "decision": decision })).await {
                    eprintln!("sikemux core: OpenCode did not take the person's answer: {error}");
                }
            });
        })
    }

    /// Follows the server's events into the chat until they end.
    async fn follow(&self) -> Result<JoinHandle<()>, String> {
        let (events, mut heard) = mpsc::unbounded_channel();
        self.client.follow(events).await?;
        let driver = self.clone();
        Ok(tokio::spawn(async move {
            while let Some(event) = heard.recv().await {
                driver.handle(&event);
            }
        }))
    }

    /// What happened in the session while nobody was following it, read from its log.
    async fn catch_up(&self) {
        let after = self.session.locked().seq;
        let path = format!(
            "/api/experimental/session/{}/log?after={after}",
            self.session_id()
        );
        match self.client.get(&path).await {
            Ok(Value::Array(events)) => events.iter().for_each(|event| self.handle(event)),
            Ok(_) => {}
            Err(error) => {
                eprintln!("sikemux core: OpenCode's session log could not be read: {error}")
            }
        }
    }
}

/// What a chat on OpenCode keeps across a hand-over to another core.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    url: String,
    password: String,
    catalog: Catalog,
    model: String,
    effort: Option<String>,
    mode: String,
    commands: Vec<String>,
    permission_mode: PermissionMode,
    session: Session,
}

pub struct Opencode {
    process: Process,
    reading: Reading<Quiet>,
    driver: Driver,
    catalog: Catalog,
    model: String,
    effort: Option<String>,
    mode: String,
    commands: Vec<String>,
    permission_mode: PermissionMode,
    events: JoinHandle<()>,
}

fn refused(error: Refused) -> String {
    error.message
}

/// The session's permission rules for a chat in `mode` on an agent whose own rules are `agent`.
/// OpenCode applies a session's rules after its agent's, and the last matching rule wins. YOLO
/// turns every ask into an allow and keeps what the agent denies. Normal asks before commands
/// and edits, placed after the agent's catch-all so the narrower rules after it, such as the
/// person's own, still decide.
fn rules(mode: PermissionMode, agent: &[Value]) -> Value {
    let rules: Vec<Value> = match mode {
        PermissionMode::Bypass => std::iter::once(
            json!({ "action": "*", "resource": "*", "effect": "allow" }),
        )
        .chain(agent.iter().map(|rule| match rule["effect"] == "ask" {
            true => {
                json!({ "action": rule["action"], "resource": rule["resource"], "effect": "allow" })
            }
            false => rule.clone(),
        }))
        .collect(),
        PermissionMode::WorkspaceWrite | PermissionMode::Auto => {
            let catch_all = agent
                .iter()
                .position(|rule| rule["action"] == "*" && rule["resource"] == "*")
                .map_or(0, |at| at + 1);
            let (base, narrower) = agent.split_at(catch_all);
            base.iter()
                .cloned()
                .chain([
                    json!({ "action": "shell", "resource": "*", "effect": "ask" }),
                    json!({ "action": "edit", "resource": "*", "effect": "ask" }),
                    json!({ "action": "sikemux-tools_process_start", "resource": "*", "effect": "ask" }),
                    json!({ "action": "sikemux-tools_trigger_add", "resource": "*", "effect": "ask" }),
                ])
                .chain(narrower.iter().cloned())
                .collect()
        }
    };
    Value::Array(rules)
}

/// Each tool server the chat hands its agent, as OpenCode registers one for a folder.
fn tool_configs(launch: &ChatLaunch) -> Vec<(String, Value)> {
    use agent_client_protocol::schema::v1::McpServer;
    servers::tool_servers(launch)
        .into_iter()
        .filter_map(|server| match server {
            McpServer::Stdio(stdio) => {
                let mut command = vec![stdio.command.to_string_lossy().into_owned()];
                command.extend(stdio.args);
                Some((
                    stdio.name,
                    json!({
                        "type": "local",
                        "command": command,
                        "environment": servers::variables(&stdio.env),
                    }),
                ))
            }
            McpServer::Http(http) => Some((
                http.name,
                json!({
                    "type": "remote",
                    "url": http.url,
                    "headers": servers::headers(&http.headers),
                    "oauth": false,
                }),
            )),
            _ => None,
        })
        .collect()
}

fn server_names(launch: &ChatLaunch) -> Arc<[String]> {
    launch
        .mcp_servers
        .iter()
        .filter_map(|server| server["name"].as_str().map(str::to_owned))
        .collect()
}

const COMPACT: &str = "compact";

/// The commands the chat offers: OpenCode's own, after `/compact`, which it runs by a route of
/// its own rather than listing.
fn offered_commands(commands: &[Value]) -> Value {
    let compact =
        json!({ "name": COMPACT, "description": "Summarize the conversation to free up context." });
    let own = commands.iter().filter_map(|command| {
        Some(json!({
            "name": command["name"].as_str()?,
            "description": command["description"].as_str().unwrap_or_default(),
        }))
    });
    json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": std::iter::once(compact).chain(own).collect::<Vec<_>>(),
    })
}

/// The person's message as OpenCode takes it: its text, and the files they attached.
fn prompt_body(prompt: &Prompt) -> Result<Value, String> {
    let mut text = Vec::new();
    let mut files = Vec::new();
    for part in parts(prompt)? {
        match part {
            Part::Text(said) => text.push(said),
            Part::File { name, uri } => {
                text.push(file_link(Some(&name), &uri));
                files.push(json!({ "uri": uri, "name": name }));
            }
            Part::Context { uri, text: said } => text.push(format!(
                "{}\n{}",
                file_link(None, &uri),
                context_text(&uri, &said)
            )),
        }
    }
    Ok(json!({ "text": text.join("\n"), "files": files }))
}

impl Opencode {
    fn options(&self) -> Value {
        self.catalog
            .rows(&self.model, self.effort.as_deref(), Some(&self.mode))
    }

    async fn register_tools(client: &Client, launch: &ChatLaunch) -> Result<(), String> {
        for (name, config) in tool_configs(launch) {
            let path = format!("/api/experimental/mcp/{}", servers::config_key(&name));
            client
                .put(&path, json!({ "config": config }))
                .await
                .map_err(|error| {
                    format!("OpenCode did not take the tool server {name}: {error}")
                })?;
        }
        Ok(())
    }

    /// Opens the chat's session: the one it resumes, with its history told the chat, or a new
    /// one on the model, effort and mode the chat starts with.
    async fn open(
        client: &Client,
        sink: &Sink,
        launch: &ChatLaunch,
        catalog: &Catalog,
        model: &str,
        effort: Option<&str>,
        mode: PermissionMode,
    ) -> Result<Value, String> {
        if let Some(existing) = &launch.resume_id {
            let path = format!("/api/session/{existing}");
            match client.get(&path).await {
                Ok(session) => {
                    let agent = session
                        .get("agent")
                        .and_then(Value::as_str)
                        .unwrap_or(BUILD);
                    let permissions =
                        json!({ "permissions": rules(mode, catalog.permissions(agent)) });
                    client.patch(&path, permissions).await.map_err(refused)?;
                    replay(sink, launch, existing);
                    return Ok(session);
                }
                Err(error)
                    if error.status == Some(404) && sink.reloading() && !sink.has_answered() => {}
                Err(error) => return Err(error.message),
            }
        }
        let mut body = json!({
            "location": { "directory": launch.cwd },
            "agent": BUILD,
            "permissions": rules(mode, catalog.permissions(BUILD)),
        });
        if let (Some(model), Some(body)) = (model_ref(model, effort), body.as_object_mut()) {
            body.insert("model".to_owned(), model);
        }
        if !catalog.offers_mode(BUILD) {
            body.as_object_mut().map(|body| body.remove("agent"));
        }
        client.post("/api/session", body).await.map_err(refused)
    }

    /// Puts the session on the model, effort and mode the chat picked.
    async fn select(&mut self) -> Result<(), String> {
        let model = model_ref(&self.model, self.effort.as_deref())
            .ok_or_else(|| format!("{} names no provider", self.model))?;
        self.driver
            .client
            .post(&self.driver.path("/model"), json!({ "model": model }))
            .await
            .map_err(refused)?;
        self.driver.session.locked().window = self.catalog.window(&self.model);
        Ok(())
    }

    /// Gives the session the permission rules for the chat's mode on the agent it runs.
    async fn put_rules(&self) -> Result<(), String> {
        let rules = rules(self.permission_mode, self.catalog.permissions(&self.mode));
        self.driver
            .client
            .patch(&self.driver.path(""), json!({ "permissions": rules }))
            .await
            .map(drop)
            .map_err(refused)
    }

    async fn read_commands(client: &Client) -> Result<Vec<Value>, String> {
        let listed = client.get("/api/command").await.map_err(refused)?;
        Ok(listed.as_array().cloned().unwrap_or_default())
    }

    /// Where the person's message goes: `/compact` and OpenCode's own commands have routes of
    /// their own, and anything else is a prompt.
    fn request(&self, prompt: &Prompt) -> Result<(&'static str, Value), String> {
        match command_parts(&prompt.text) {
            Some((name, _)) if name.eq_ignore_ascii_case(COMPACT) => Ok(("/compact", json!({}))),
            Some((name, rest)) if self.commands.iter().any(|command| command == name) => {
                Ok(("/command", json!({ "name": name, "text": rest })))
            }
            _ => Ok(("/prompt", prompt_body(prompt)?)),
        }
    }
}

/// Tells the chat the saved session it takes up again, as OpenCode would replay it.
fn replay(sink: &Sink, launch: &ChatLaunch, session: &str) {
    let names = server_names(launch);
    let Ok(history) = crate::sessions::history(AgentKind::Opencode, session, &launch.cwd) else {
        return;
    };
    for mut update in history {
        if !crate::server::chat::prompt::drop_for_agent(&mut update) {
            continue;
        }
        as_mcp_call(&mut update, &names);
        sink.update(session, update);
    }
}

/// Shows a saved session before OpenCode starts, as it would replay it.
pub fn replay_saved(
    launch: &ChatLaunch,
    session: &str,
    sink: &super::session::Telling,
) -> Result<(), String> {
    let names = server_names(launch);
    for mut update in crate::sessions::history(AgentKind::Opencode, session, &launch.cwd)? {
        if !crate::server::chat::prompt::drop_for_agent(&mut update) {
            continue;
        }
        as_mcp_call(&mut update, &names);
        sink.update(session, update);
    }
    Ok(())
}

async fn ready(process: &Process, lines: &mut Lines) -> Result<String, String> {
    let line: Option<Value> = tokio::time::timeout(READY_WAIT, lines.next())
        .await
        .map_err(|_| process.start_error("OpenCode's server did not start in time".to_owned()))?;
    let line = line.ok_or_else(|| process.start_error("OpenCode's server stopped".to_owned()))?;
    ready_url(&line)
}

impl Backend for Opencode {
    type Reply = Asked;

    async fn start(
        launch: &ChatLaunch,
        _brief: Option<&str>,
        sink: Sink,
    ) -> Result<(Self, Opened), String> {
        let password = new_password();
        let (process, _writer, mut lines) = Process::spawn(
            launch,
            SERVE_ARGS.map(str::to_owned),
            [(PASSWORD_ENV.to_owned(), password.clone())],
        )?;
        let url = ready(&process, &mut lines).await?;
        let failed = |error: String| process.start_error(error);
        let client = Client::new(url, password, &launch.cwd);
        let (catalog, (), commands) = tokio::try_join!(
            Catalog::read(&client),
            Self::register_tools(&client, launch),
            Self::read_commands(&client),
        )
        .map_err(failed)?;
        let model = catalog.model(launch.model.as_deref());
        let effort = catalog.effort(&model, launch.effort.as_deref());
        let driver = Driver {
            client: client.clone(),
            sink: sink.clone(),
            session: Arc::new(Mutex::new(Session::new(String::new(), launch.cwd.clone()))),
            servers: server_names(launch),
        };
        let events = driver.follow().await.map_err(failed)?;
        let opened = Self::open(
            &client,
            &sink,
            launch,
            &catalog,
            &model,
            effort.as_deref(),
            launch.permission_mode,
        )
        .await
        .map_err(failed)?;
        let session_id = opened
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| failed("OpenCode opened a session without an id".to_owned()))?
            .to_owned();
        {
            let mut session = driver.session.locked();
            session.id = session_id.clone();
            session.cost = opened.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
            session.titled = opened
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| !title.is_empty());
        }
        sink.loaded();
        sink.update(&session_id, offered_commands(&commands));
        let (reading, _) = Reading::start(Quiet, lines);
        let mode = opened
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or(BUILD)
            .to_owned();
        let mut opencode = Self {
            process,
            reading,
            driver,
            catalog,
            model: model.clone(),
            effort,
            mode,
            commands: commands
                .iter()
                .filter_map(|command| command["name"].as_str().map(str::to_owned))
                .collect(),
            permission_mode: launch.permission_mode,
            events,
        };
        let on = opened.get("model").and_then(read_ref);
        if on
            .as_ref()
            .map(|(on, effort)| (on.as_str(), effort.as_deref()))
            != Some((model.as_str(), opencode.effort.as_deref()))
        {
            if let Err(error) = opencode.select().await {
                eprintln!("sikemux core: OpenCode did not take the chat's model: {error}");
            }
        } else {
            opencode.driver.session.locked().window = opencode.catalog.window(&model);
        }
        let opened = Opened {
            session_id,
            capabilities: Opened::native_capabilities(),
            setup: json!({ "configOptions": opencode.options() }),
        };
        Ok((opencode, opened))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let (route, body) = self.request(&prompt)?;
        let effects = self.driver.session.locked().send(turn);
        self.driver.apply(effects);
        let sent = self
            .driver
            .client
            .post(&self.driver.path(route), body)
            .await;
        match sent.map_err(refused) {
            Ok(sent) => self.driver.sent(&prompt, &sent),
            Err(said) => {
                let effects = self.driver.session.locked().refused(turn, said);
                self.driver.apply(effects);
            }
        }
        Ok(())
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static {
        let running = self.driver.session.locked().running();
        let body = prompt_body(&prompt);
        let driver = self.driver.clone();
        async move {
            if !running {
                return Ok(Steered::PromptRequired);
            }
            let mut body = body?;
            if let Some(body) = body.as_object_mut() {
                body.insert("delivery".to_owned(), json!("steer"));
                body.insert("resume".to_owned(), json!(false));
            }
            let sent = driver
                .client
                .post(&driver.path("/prompt"), body)
                .await
                .map_err(refused)?;
            driver.sent(&prompt, &sent);
            driver
                .sink
                .prompted(&prompt.text, &prompt.paths, !prompt.letter);
            Ok(Steered::Injected)
        }
    }

    async fn cancel(&mut self) -> Result<(), String> {
        self.driver.session.locked().stop();
        self.driver
            .client
            .post(&self.driver.path("/interrupt"), json!({}))
            .await
            .map(drop)
            .map_err(refused)
    }

    async fn compact(&mut self) -> Result<(), String> {
        self.driver.session.locked().compact_quietly();
        self.driver
            .client
            .post(&self.driver.path("/compact"), json!({}))
            .await
            .map(drop)
            .map_err(refused)
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        self.permission_mode = mode;
        self.put_rules().await
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                self.effort = self.catalog.effort(value, self.effort.as_deref());
                self.model = value.to_owned();
                self.select().await?;
            }
            EFFORT_ID => {
                self.effort = Some(value.to_owned());
                self.select().await?;
            }
            MODE_ID => {
                self.driver
                    .client
                    .post(&self.driver.path("/agent"), json!({ "agent": value }))
                    .await
                    .map_err(refused)?;
                self.mode = value.to_owned();
                self.put_rules().await?;
            }
            _ => return Err(format!("OpenCode has no option {config_id}")),
        }
        Ok(self.options())
    }

    async fn stop_task(&mut self, _task_id: &str) -> Result<(), String> {
        Err("OpenCode runs no background task Sikemux can stop".to_owned())
    }

    /// OpenCode goes back by reverting the session to before the message, which also puts
    /// back the files its turns changed. A message replayed from history goes by OpenCode's id.
    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let message = {
            let session = self.driver.session.locked();
            session
                .messages
                .get(message_id)
                .cloned()
                .unwrap_or_else(|| message_id.to_owned())
        };
        let client = &self.driver.client;
        client
            .post(
                &self.driver.path("/revert/stage"),
                json!({ "messageID": message, "files": true }),
            )
            .await
            .map_err(refused)?;
        client
            .post(&self.driver.path("/revert/commit"), json!({}))
            .await
            .map(drop)
            .map_err(refused)?;
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
            reading,
            driver,
            catalog,
            model,
            effort,
            mode,
            commands,
            permission_mode,
            events,
        } = self;
        events.abort();
        let session = std::mem::take(&mut *driver.session.locked());
        let client = driver.client;
        process::hand_over(process, reading, "OpenCode", |_| Kept {
            url: client.url().to_owned(),
            password: client.password().to_owned(),
            catalog,
            model,
            effort,
            mode,
            commands,
            permission_mode,
            session,
        })
        .await
    }

    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, _writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let kept: Kept = serde_json::from_value(state).map_err(|error| error.to_string())?;
        let client = Client::new(kept.url, kept.password, &launch.cwd);
        let driver = Driver {
            client: client.clone(),
            sink,
            session: Arc::new(Mutex::new(kept.session)),
            servers: server_names(launch),
        };
        let (reading, _) = Reading::start(Quiet, lines);
        let follower = driver.clone();
        let events = tokio::spawn(async move {
            match follower.follow().await {
                Ok(following) => {
                    follower.catch_up().await;
                    let _ = following.await;
                }
                Err(error) => eprintln!(
                    "sikemux core: OpenCode's events could not be followed again: {error}"
                ),
            }
        });
        Ok(Self {
            process,
            reading,
            driver,
            catalog: kept.catalog,
            model: kept.model,
            effort: kept.effort,
            mode: kept.mode,
            commands: kept.commands,
            permission_mode: kept.permission_mode,
            events,
        })
    }

    fn responder(&self, reply: Asked) -> Respond {
        self.driver.responder(reply)
    }
}
