//! Claude Code, driven over its stream-json control protocol: the protocol
//! Anthropic's own Agent SDK speaks to the `claude` binary.

mod apps;
mod compaction;
mod config;
mod control;
mod failure;
mod messages;
mod permission;
mod reader;
mod replay;
#[cfg(test)]
mod rows;
mod tools;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::acp::pickers;
use crate::agent_catalog::AgentKind;
use crate::launch::PermissionMode;
use crate::sessions::{self, roots::account_root};
use sikemux_wire::protocol::ChatLaunch;

use super::super::cap;
use super::super::feed::Reach;
use super::super::Respond;
use super::file_link;
use super::parts::{parts, Part};
use super::pending::Purpose;
use super::process::{self, Lines, Process, Reading, Released, Writer};
use super::servers;
use super::session::{Backend, Opened, Prompt, Sink, Steered, Telling};
use crate::sessions::for_agent::context_text;
pub(crate) use compaction::AutoCompact;
pub(crate) use config::saved_window;
use control::Control;
use reader::{Asking, Reader, ReaderState, Shared, Turn};

/// Claude Code tells the model it writes to a terminal and offers its own Artifact tool, so
/// charts went to claude.ai links the person never asked for.
const SHOWING_PAGES: &str = "Your replies show in Sikemux's chat, not a terminal. Show a chart, diagram, mockup or comparison as an HTML page with page_preview then page_show; publish a claude.ai Artifact only when the person asks for a link to share.";

enum Start<'a> {
    New(&'a str),
    Resume(&'a str),
}

/// One run of the `claude` process. Dropping it stops the process.
struct Running {
    process: Process,
    control: Control,
    apps: apps::Apps,
    asking: Asking,
    reading: Reading<Reader>,
    task_stops: mpsc::UnboundedSender<String>,
}

/// What a Claude chat hands a core that takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeState {
    control: Vec<(String, Purpose)>,
    shared: Shared,
    reader: ReaderState,
    #[serde(flatten)]
    kept: Kept,
}

/// What a Claude chat knows of its session, as it hands it to a core that
/// takes it over.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    session_id: String,
    models: Vec<Value>,
    model: String,
    effort: String,
    fast: bool,
    /// What the person calls each message they sent, against the id Claude
    /// Code knows it by. An edited message is sent again under a new id.
    aliases: HashMap<String, String>,
}

pub(crate) struct Claude {
    launch: ChatLaunch,
    running: Running,
    shared: Arc<Mutex<Shared>>,
    kept: Kept,
}

/// The transcript Claude keeps of `session_id` for the account `launch` runs on.
fn transcript(launch: &ChatLaunch, session_id: &str) -> Option<PathBuf> {
    let root = account_root(AgentKind::Claude, None, &launch.env)?;
    let cwd = launch.cwd.to_string_lossy();
    sessions::transcript::find(AgentKind::Claude, &root, Some(&cwd), session_id)
}

/// The models Claude offers, from its answer to `initialize` with no session saved.
pub(crate) async fn read_options(command: tokio::process::Command) -> Result<Value, String> {
    const ASKED: &str = "sikemux-options";
    let request = json!({
        "type": "control_request",
        "request_id": ASKED,
        "request": { "subtype": "initialize", "hooks": {} },
    });
    let said = crate::agent_accounts::usage::ask(
        "claude",
        command,
        crate::agent_accounts::usage::CLAUDE_STREAM_ARGS,
        &format!("{request}\n"),
    )
    .await?;
    let models = crate::agent_accounts::usage::control_answer(&said, ASKED)
        .and_then(|answer| answer.get("models")?.as_array().cloned())
        .ok_or("Claude named no models")?;
    Ok(config::options(
        &models,
        config::DEFAULT,
        config::DEFAULT,
        false,
    ))
}

/// Tells `sink` what the saved chat of `session_id` holds that the chat does not show yet: the
/// whole of it, or for a chat whose history already holds it, only what Claude wrote past the
/// point the history reaches.
pub(crate) fn replay_saved(
    launch: &ChatLaunch,
    session_id: &str,
    sink: &Telling,
) -> Result<(), String> {
    let Some(path) = transcript(launch, session_id) else {
        return Ok(());
    };
    let mut unread = None;
    let (records, reach) = if sink.reloading() {
        let Some(Reach::Bytes(from)) = sink.reach(session_id) else {
            return Ok(());
        };
        let (records, reach) = replay::read_since(&path, from)?;
        if !records.is_empty() {
            sink.loaded();
        }
        (records, reach)
    } else {
        let (newest, len) = replay::read_newest(&path, NEWEST_BYTES)?;
        let (records, older) = match newest.before {
            Some(_) => match replay::from_turn_start(newest.records) {
                (from, older) if from.is_empty() => (older, Vec::new()),
                cut => cut,
            },
            None => (newest.records, Vec::new()),
        };
        if newest.before.is_some() || !older.is_empty() {
            sink.leave_room();
            unread = Some(Unread {
                path: path.clone(),
                session_id: session_id.to_owned(),
                cwd: launch.cwd.clone(),
                start: newest.start,
                before: newest.before,
                pending: older,
                window: NEWEST_BYTES * 2,
            });
        }
        (records, len)
    };
    let records = replay::with_subagents(&path, records);
    for (session, update) in replay::replay(&records, session_id, &launch.cwd) {
        sink.update(&session, update);
    }
    sink.reached(session_id, Reach::Bytes(reach));
    if let Some(mut unread) = unread {
        sink.tell_earlier(Box::new(move || unread.next()));
    }
    Ok(())
}

/// How much of the end of a saved chat is read when a chat opens on one it never ran. What
/// came before is read as a client pages back to it, in parts that grow up to
/// [`MAX_EARLIER_BYTES`].
const NEWEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EARLIER_BYTES: u64 = 64 * 1024 * 1024;

/// What a chat opened on a saved session has not read of it yet: the transcript before the
/// bytes from `start` on, whose active branch goes on from the message `before`, and the
/// messages read already that wait for the start of their turn.
struct Unread {
    path: PathBuf,
    session_id: String,
    cwd: PathBuf,
    start: u64,
    before: Option<String>,
    pending: Vec<Value>,
    window: u64,
}

impl Unread {
    /// The updates of the next part back, from the start of a turn, or none once the whole
    /// transcript has been read.
    fn next(&mut self) -> Result<Option<Vec<(String, Value)>>, String> {
        loop {
            let records = match self.before.clone() {
                Some(before) => {
                    let stretch =
                        replay::read_before(&self.path, self.start, &before, self.window)?;
                    self.window = (self.window * 2).min(MAX_EARLIER_BYTES);
                    self.start = stretch.start;
                    self.before = stretch.before;
                    let mut records = stretch.records;
                    records.append(&mut self.pending);
                    if self.before.is_some() {
                        let (from, older) = replay::from_turn_start(records);
                        self.pending = older;
                        from
                    } else {
                        records
                    }
                }
                None if self.pending.is_empty() => return Ok(None),
                None => std::mem::take(&mut self.pending),
            };
            if records.is_empty() {
                continue;
            }
            let records = replay::with_subagents(&self.path, records);
            let mut updates = replay::replay(&records, &self.session_id, &self.cwd);
            for (_, update) in &mut updates {
                super::session::slim(update);
            }
            return Ok(Some(updates));
        }
    }
}

/// The model the saved chat of `session_id` last ran on, read from the newest part of it.
fn saved_model(launch: &ChatLaunch, session_id: &str) -> Option<String> {
    let path = transcript(launch, session_id)?;
    replay::newest(&path, replay::resumed_model)
        .inspect_err(|error| eprintln!("sikemux core: Claude's saved model was not read: {error}"))
        .ok()
        .flatten()
}

/// The chat's tool servers as Claude Code's `--mcp-config` file.
fn mcp_config(tools: &[McpServer]) -> Option<String> {
    let tools: serde_json::Map<String, Value> = tools
        .iter()
        .filter_map(|server| {
            let (name, config) = match server {
                McpServer::Http(http) => (
                    &http.name,
                    json!({ "type": "http", "url": http.url, "headers": servers::headers(&http.headers) }),
                ),
                McpServer::Sse(sse) => (
                    &sse.name,
                    json!({ "type": "sse", "url": sse.url, "headers": servers::headers(&sse.headers) }),
                ),
                McpServer::Stdio(stdio) => (
                    &stdio.name,
                    json!({
                        "type": "stdio",
                        "command": stdio.command,
                        "args": stdio.args,
                        "env": servers::variables(&stdio.env),
                    }),
                ),
                _ => return None,
            };
            Some((name.clone(), config))
        })
        .collect();
    (!tools.is_empty()).then(|| json!({ "mcpServers": tools }).to_string())
}

/// What the person wrote, as the content of a Claude user message. Files go
/// as links Claude reads itself, and handed-over text follows the message.
fn content(prompt: &Prompt) -> Result<Vec<Value>, String> {
    let text = |text: String| json!({ "type": "text", "text": text });
    let mut content = Vec::new();
    let mut handed = Vec::new();
    for part in parts(prompt)? {
        match part {
            Part::Text(said) => content.push(text(said)),
            Part::File { name, uri } => content.push(text(file_link(Some(&name), &uri))),
            Part::Context { uri, text: body } => {
                content.push(text(file_link(None, &uri)));
                handed.push(text(format!("\n{}", context_text(&uri, &body))));
            }
        }
    }
    content.extend(handed);
    Ok(content)
}

fn user_message(uuid: &str, content: Vec<Value>, priority: Option<&str>) -> Value {
    let mut message = json!({
        "type": "user",
        "session_id": "",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "uuid": uuid,
    });
    if let Some(priority) = priority {
        message["priority"] = json!(priority);
    }
    message
}

/// Writes the prompt Sikemux adds to a file Claude reads once as it starts,
/// rather than putting it on the command line, where a `pkill -f` pattern
/// matching words in it would stop the agent. Removed when dropped.
fn prompt_file(prompt: &str) -> Result<tempfile::TempPath, String> {
    let mut file = tempfile::Builder::new()
        .prefix("sikemux-prompt-")
        .suffix(".md")
        .tempfile()
        .map_err(|error| {
            format!("Claude did not start: its prompt could not be written: {error}")
        })?;
    std::io::Write::write_all(&mut file, prompt.as_bytes()).map_err(|error| {
        format!("Claude did not start: its prompt could not be written: {error}")
    })?;
    Ok(file.into_temp_path())
}

impl Claude {
    async fn spawn(
        launch: &ChatLaunch,
        sink: &Sink,
        shared: &Arc<Mutex<Shared>>,
        session_id: &str,
        start: Start<'_>,
        model: Option<&str>,
        brief: Option<&str>,
    ) -> Result<(Running, Value), String> {
        let mode = config::permission_mode(launch.permission_mode);
        let mut args: Vec<String> = [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--permission-prompt-tool",
            "stdio",
            "--include-partial-messages",
            "--replay-user-messages",
            "--setting-sources=user,project,local",
            "--allow-dangerously-skip-permissions",
            "--prompt-suggestions",
            "--thinking-display",
            "summarized",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend(["--permission-mode".to_owned(), mode.to_owned()]);
        let capped = sink.capped();
        if capped {
            args.extend([
                "--autocompact".to_owned(),
                format!("{}k", cap::CLAUDE_WINDOW / 1000),
            ]);
        }
        if let Some(settings) = crate::git_guard::shim::claude_settings() {
            args.extend(["--settings".to_owned(), settings]);
        }
        if let Some(model) = model {
            args.extend(["--model".to_owned(), model.to_owned()]);
        }
        let tools = servers::tool_servers(launch);
        let sikemux_lines: &[&str] = if servers::offers_sikemux_tools(&tools) {
            &[crate::server::gestures::PERSONALITY, SHOWING_PAGES]
        } else {
            &[]
        };
        let appended: Vec<&str> = sikemux_lines.iter().copied().chain(brief).collect();
        let _prompt_file = if appended.is_empty() {
            None
        } else {
            let file = prompt_file(&appended.join("\n\n"))?;
            args.extend([
                "--append-system-prompt-file".to_owned(),
                file.to_string_lossy().into_owned(),
            ]);
            Some(file)
        };
        if let Some(config) = mcp_config(&tools) {
            args.extend(["--mcp-config".to_owned(), config]);
        }
        match start {
            Start::New(id) => args.push(format!("--session-id={id}")),
            Start::Resume(id) => args.push(format!("--resume={id}")),
        }
        let env = [
            sikemux_process::vars::CLAUDE_ENTRYPOINT,
            ("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS", "1"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()));
        let (process, writer, lines) = Process::spawn(launch, args, env)?;
        let control = Control::new(writer);
        let apps = apps::Apps::default();
        let mut reader = Reader::new(
            sink.clone(),
            control.clone(),
            launch.cwd.clone(),
            session_id.to_owned(),
            shared.clone(),
            apps.clone(),
        );
        if let Some(model) = model {
            reader.note_model(model);
        }
        reader.compacts(AutoCompact::of(launch, capped));
        let asking = reader.asking();
        let (reading, task_stops) = Reading::start(reader, lines);
        let running = Running {
            process,
            control: control.clone(),
            apps,
            asking,
            reading,
            task_stops,
        };
        let initialized = tokio::select! {
            answer = control.request(json!({
                "subtype": "initialize",
                "hooks": cap::claude_hooks(capped),
                "forwardSubagentText": true,
                "perTaskStopAffordance": true,
            })) => answer,
            ended = running.process.ended() => Err(ended),
        };
        match initialized {
            Ok(response) => Ok((running, response)),
            Err(error) => Err(format!(
                "Claude did not start: {}",
                running.process.start_error(error)
            )),
        }
    }

    fn writer(&self) -> &Writer {
        self.running.control.writer()
    }

    /// Asks Claude for a short name for a new chat from its first message, as
    /// Claude Code names its own, while the first turn runs.
    fn name_chat(&self, first: &str) {
        let description: String = first.chars().take(1_000).collect();
        if description.trim().chars().count() < 10 {
            return;
        }
        let _ = self.running.control.send_for(
            json!({ "subtype": "generate_session_title", "description": description, "persist": true }),
            Purpose::Title,
        );
    }

    fn options(&self) -> Value {
        config::options(
            &self.kept.models,
            &self.kept.model,
            &self.kept.effort,
            self.kept.fast,
        )
    }

    /// Claude Code's Agent SDK runs fast only when the flag settings ask for it, whatever the
    /// person's own settings say.
    async fn apply_fast(&self) -> Result<(), String> {
        self.running
            .control
            .request(json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": self.kept.fast } }))
            .await
            .map(drop)
    }

    async fn apply_effort(&self) -> Result<(), String> {
        let level = if self.kept.effort == config::DEFAULT {
            Value::Null
        } else {
            json!(self.kept.effort)
        };
        self.running
            .control
            .request(
                json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": level } }),
            )
            .await
            .map(drop)
    }

    /// The id Claude Code gets for the prompt's message. One already sent, before an edit or
    /// by an earlier run of the session, gets a fresh one, since Claude Code drops a message
    /// under an id its transcript already has.
    fn uuid_for(&mut self, prompt: &Prompt) -> String {
        let Some(message_id) = prompt.message_id.as_deref() else {
            return Uuid::new_v4().to_string();
        };
        let used = prompt.resent
            || self.kept.aliases.contains_key(message_id)
            || self.shared.locked().sent.contains(message_id);
        let uuid = if used || Uuid::parse_str(message_id).is_err() {
            Uuid::new_v4().to_string()
        } else {
            message_id.to_owned()
        };
        self.kept
            .aliases
            .insert(message_id.to_owned(), uuid.clone());
        uuid
    }

    fn inject(&self, prompt: Prompt) -> Result<Steered, String> {
        let content = content(&prompt)?;

        let uuid = Uuid::new_v4().to_string();
        {
            let mut shared = self.shared.locked();
            if shared.turn.is_none() {
                return Ok(Steered::PromptRequired);
            }
            shared.sent.insert(uuid.clone());
            if let Some(turn) = shared.turn.as_mut() {
                turn.messages.insert(uuid.clone());
                turn.waiting.insert(uuid.clone());
                if prompt.letter {
                    turn.letters.insert(uuid.clone(), prompt.text.clone());
                }
            }
        }
        // Claude Code takes it in after the turn's next tool call, or once the turn ends.
        self.writer()
            .send(&user_message(&uuid, content, Some("next")))?;
        Ok(Steered::Injected)
    }
}

impl Backend for Claude {
    type Reply = reader::Asked;

    const INSTRUCTIONS: bool = true;

    const EFFORT_MID_TURN: bool = true;

    async fn start(
        launch: &ChatLaunch,
        brief: Option<&str>,
        sink: Sink,
    ) -> Result<(Self, Opened), String> {
        let mode = config::permission_mode(launch.permission_mode);
        let shared = Arc::new(Mutex::new(Shared {
            mode: mode.to_owned(),
            untitled: launch.resume_id.is_none().then(String::new),
            ..Shared::default()
        }));
        let session_id = launch
            .resume_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        // Claude takes a moment to start, so the saved chat is read and
        // replayed while it does.
        let replaying = launch.resume_id.is_some().then(|| {
            let (launch, id, sink) = (launch.clone(), session_id.clone(), sink.clone());
            tokio::task::spawn_blocking(move || replay_saved(&launch, &id, &sink))
        });
        let start = match launch.resume_id.as_deref() {
            Some(id) => Start::Resume(id),
            None => Start::New(&session_id),
        };
        let routed = launch.model_provider.is_some();
        // A picked "default" is Claude's own default model, which only `--model default` gets:
        // without it Claude runs the model the person's settings name instead.
        let wanted = launch
            .model
            .clone()
            .filter(|model| !(routed && model == config::DEFAULT));
        let (running, initialized) = Self::spawn(
            launch,
            &sink,
            &shared,
            &session_id,
            start,
            wanted.as_deref(),
            brief,
        )
        .await?;
        if let Some(replaying) = replaying {
            replaying.await.map_err(|error| error.to_string())??;
        }
        let mut models: Vec<Value> = initialized
            .get("models")
            .and_then(Value::as_array)
            .filter(|_| !routed)
            .cloned()
            .unwrap_or_default();
        if let Some(wanted) = wanted.as_deref().filter(|_| routed) {
            config::provider_model(&mut models, wanted);
        }
        // A resumed chat goes on with the model it last ran, unless the person chose one;
        // without either it runs what the person's Claude settings name, and the pickers say so.
        let settings = running
            .control
            .request(json!({ "subtype": "get_settings" }))
            .await
            .ok()
            .and_then(|settings| settings.get("effective").cloned())
            .unwrap_or(Value::Null);
        let from_settings = settings
            .get("model")
            .and_then(Value::as_str)
            .and_then(|named| config::settings_model(&mut models, named));
        let chosen = wanted.filter(|model| config::offers_model(&models, model));
        let resumed = match (&chosen, &launch.resume_id) {
            (None, Some(id)) => {
                let (launch, id) = (launch.clone(), id.clone());
                tokio::task::spawn_blocking(move || saved_model(&launch, &id))
                    .await
                    .ok()
                    .flatten()
            }
            _ => None,
        }
        .and_then(|resolved| config::row_for_resolved(&models, &resolved));
        let switch_to = resumed.clone().filter(|_| chosen.is_none());
        let model = chosen
            .or(resumed)
            .or(from_settings)
            .unwrap_or_else(|| config::DEFAULT.to_owned());
        let levels = config::effort_levels(&models, &model);
        let chosen_effort = launch
            .effort
            .clone()
            .filter(|effort| levels.contains(effort));
        let apply_effort = chosen_effort.is_some();
        let effort = chosen_effort
            .or_else(|| {
                settings
                    .get("effortLevel")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .filter(|effort| levels.contains(effort))
            })
            .unwrap_or_else(|| config::DEFAULT.to_owned());
        let lifetimes = config::cache_lifetimes(&settings, &launch.env, routed);
        let fast = settings.get("fastMode") == Some(&json!(true))
            && settings.get("fastModePerSessionOptIn") != Some(&json!(true));
        let claude = Self {
            launch: launch.clone(),
            running,
            shared,
            kept: Kept {
                session_id: session_id.clone(),
                models,
                model,
                effort,
                fast,
                aliases: HashMap::new(),
            },
        };
        if let Some(model) = switch_to {
            claude
                .running
                .control
                .request(json!({ "subtype": "set_model", "model": model }))
                .await?;
        }
        if apply_effort {
            claude.apply_effort().await?;
        }
        if !lifetimes.is_empty() {
            let _ = claude
                .running
                .control
                .request(json!({ "subtype": "apply_flag_settings", "settings": lifetimes }))
                .await;
        }
        if fast {
            claude.apply_fast().await?;
        }
        if let Some(commands) = initialized.get("commands") {
            sink.update(&session_id, reader::available_commands(commands));
        }
        let setup = json!({ "configOptions": claude.options() });
        Ok((
            claude,
            Opened {
                session_id,
                capabilities: Opened::native_capabilities(),
                setup,
            },
        ))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let content = content(&prompt)?;
        let uuid = self.uuid_for(&prompt);
        let first = {
            let mut shared = self.shared.locked();
            shared.sent.insert(uuid.clone());
            shared.turn = Some(Turn {
                number: turn,
                messages: HashSet::from([uuid.clone()]),
                waiting: HashSet::new(),
                letters: HashMap::new(),
                cancelled: false,
                failure: None,
            });
            shared.untitled.take().is_some()
        };
        let sent = self.writer().send(&user_message(&uuid, content, None));
        if sent.is_err() {
            self.shared.locked().turn = None;
        } else if first {
            self.name_chat(&prompt.text);
        }
        sent
    }

    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static {
        std::future::ready(self.inject(prompt))
    }

    fn mcp_app(
        &mut self,
        request: crate::server::mcp_apps::ServerRequest,
    ) -> impl std::future::Future<Output = Result<Value, String>> + Send + 'static {
        let (apps, control) = (self.running.apps.clone(), self.running.control.clone());
        async move { apps.serve(&control, request).await }
    }

    async fn cancel(&mut self) -> Result<(), String> {
        {
            let mut shared = self.shared.locked();
            shared.quiet = None;
            shared.hushed = false;
        }
        let running = match self.shared.locked().turn.as_mut() {
            Some(turn) => {
                turn.cancelled = true;
                true
            }
            None => false,
        };
        if !running {
            return Ok(());
        }
        self.running
            .control
            .request(json!({ "subtype": "interrupt", "cancel_queued": true }))
            .await
            .map(drop)
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), String> {
        let mode = config::permission_mode(mode);
        self.running
            .control
            .request(json!({ "subtype": "set_permission_mode", "mode": mode }))
            .await?;
        self.shared.locked().mode = mode.to_owned();
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                if self.launch.model_provider.is_some() {
                    config::provider_model(&mut self.kept.models, value);
                }
                if !config::offers_model(&self.kept.models, value) {
                    return Err(format!("Invalid value for config option model: {value}"));
                }
                self.running
                    .control
                    .request(json!({ "subtype": "set_model", "model": value }))
                    .await?;
                self.kept.model = value.to_owned();
                let levels = config::effort_levels(&self.kept.models, value);
                if self.kept.effort != config::DEFAULT && !levels.contains(&self.kept.effort) {
                    self.kept.effort = config::DEFAULT.to_owned();
                    self.apply_effort().await?;
                }
            }
            "effort" => {
                let levels = config::effort_levels(&self.kept.models, &self.kept.model);
                if value != config::DEFAULT && !levels.iter().any(|level| level == value) {
                    return Err(format!("Invalid value for config option effort: {value}"));
                }
                self.kept.effort = value.to_owned();
                self.apply_effort().await?;
            }
            "speed" => {
                let fast = match value {
                    config::FAST if config::supports_fast(&self.kept.models, &self.kept.model) => {
                        true
                    }
                    pickers::STANDARD => false,
                    _ => return Err(format!("Invalid value for config option speed: {value}")),
                };
                self.kept.fast = fast;
                self.apply_fast().await?;
            }
            _ => return Err(format!("Unknown config option: {config_id}")),
        }
        Ok(self.options())
    }

    async fn compact(&mut self) -> Result<(), String> {
        let uuid = Uuid::new_v4().to_string();
        {
            let mut shared = self.shared.locked();
            if shared.turn.is_some() || shared.quiet.is_some() {
                return Ok(());
            }
            shared.sent.insert(uuid.clone());
            shared.quiet = Some(uuid.clone());
        }
        let command = json!({ "type": "text", "text": format!("/compact {}", cap::IDLE_FOCUS) });
        self.writer()
            .send(&user_message(&uuid, vec![command], None))
            .inspect_err(|_| self.shared.locked().quiet = None)
    }

    async fn handoff(&mut self) -> Result<String, String> {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(120);
        const LOOK: std::time::Duration = std::time::Duration::from_millis(200);
        let uuid = Uuid::new_v4().to_string();
        {
            let mut shared = self.shared.locked();
            if shared.turn.is_some() || shared.quiet.is_some() {
                return Err("Claude is at work".to_owned());
            }
            shared.sent.insert(uuid.clone());
            shared.quiet = Some(uuid.clone());
            shared.hushed = true;
            shared.handed = None;
        }
        let unhush = |shared: &Arc<Mutex<Shared>>| {
            let mut shared = shared.locked();
            shared.quiet = None;
            shared.hushed = false;
        };
        // Tools it is not already allowed are refused without asking, so the turn only writes.
        let refusing = self
            .running
            .control
            .request(json!({ "subtype": "set_permission_mode", "mode": "dontAsk" }))
            .await;
        if let Err(error) = refusing {
            unhush(&self.shared);
            return Err(error);
        }
        let ask = json!({ "type": "text", "text": super::super::rotation::HANDOFF_ASK });
        if let Err(error) = self.writer().send(&user_message(&uuid, vec![ask], None)) {
            unhush(&self.shared);
            let mode = self.shared.locked().mode.clone();
            let _ = self.running.control.send_for(
                json!({ "subtype": "set_permission_mode", "mode": mode }),
                Purpose::Mode,
            );
            return Err(error);
        }
        // Past the wait the turn stays hushed: whatever it says late never reaches the chat.
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(note) = self.shared.locked().handed.take() {
                    return note;
                }
                tokio::time::sleep(LOOK).await;
            }
        })
        .await
        .map_err(|_| "Claude wrote no handoff note in time".to_owned())
    }

    async fn stop_task(&mut self, task_id: &str) -> Result<(), String> {
        self.running
            .control
            .request(json!({ "subtype": "stop_task", "task_id": task_id }))
            .await?;
        let _ = self.running.task_stops.send(task_id.to_owned());
        Ok(())
    }

    async fn rewind(&mut self, message_id: &str) -> Result<Option<String>, String> {
        let uuid = self
            .kept
            .aliases
            .get(message_id)
            .cloned()
            .unwrap_or_else(|| message_id.to_owned());
        let path = transcript(&self.launch, &self.kept.session_id)
            .ok_or("Claude has not saved this chat yet")?;
        // Claude Code refuses to drop turns it thinks the person has not seen.
        let newest = replay::newest(&path, |records| {
            records
                .iter()
                .rev()
                .filter(|record| record.get("type").and_then(Value::as_str) == Some("user"))
                .find_map(|record| {
                    record
                        .get("uuid")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        })?;
        let mut request = json!({
            "subtype": "rewind_conversation",
            "target_message_uuid": uuid,
            "interrupt_if_running": true,
        });
        if let Some(newest) = newest {
            request["last_seen_user_message_uuid"] = json!(newest);
        }
        let rewound = self.running.control.request(request).await?;
        if rewound.get("rewound").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        Err(match rewound.get("reason").and_then(Value::as_str) {
            Some("target_not_found") => "That message is no longer part of this chat".into(),
            _ => format!(
                "Claude could not go back to that message: {}",
                rewound
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("it gave no reason")
            ),
        })
    }

    async fn closed(&self) -> String {
        self.running.process.ended().await
    }

    fn reach(&self) -> Option<Reach> {
        let path = transcript(&self.launch, &self.kept.session_id)?;
        Some(Reach::Bytes(std::fs::metadata(path).ok()?.len()))
    }

    fn pid(&self) -> u32 {
        self.running.process.pid()
    }

    async fn release(self) -> Result<(Value, Released), String> {
        let Self {
            launch: _,
            running,
            shared: _,
            kept,
        } = self;
        let Running {
            process,
            control,
            asking: _,
            apps: _,
            reading,
            task_stops: _,
        } = running;
        process::hand_over(process, reading, "Claude", |reader| {
            let (reader, shared) = reader.into_state();
            let shared = std::mem::take(&mut *shared.locked());
            ClaudeState {
                control: control.state(),
                shared,
                reader,
                kept,
            }
        })
        .await
    }

    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        (process, writer, lines): (Process, Writer, Lines),
    ) -> Result<Self, String> {
        let state: ClaudeState =
            serde_json::from_value(state).map_err(|error| error.to_string())?;
        let control = Control::restored(writer, state.control);
        let shared = Arc::new(Mutex::new(state.shared));
        let apps = apps::Apps::default();
        let reader = Reader::restored(
            sink,
            control.clone(),
            shared.clone(),
            apps.clone(),
            state.reader,
        );
        let asking = reader.asking();
        let (reading, task_stops) = Reading::start(reader, lines);
        Ok(Self {
            launch: launch.clone(),
            running: Running {
                process,
                control,
                apps,
                asking,
                reading,
                task_stops,
            },
            shared,
            kept: state.kept,
        })
    }

    fn responder(&self, reply: reader::Asked) -> Respond {
        self.running.asking.responder(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_servers_become_claudes_mcp_config() {
        let config = mcp_config(&[
            serde_json::from_value(json!({ "name": "sikemux-tools", "command": "/bin/tools", "args": ["--x"], "env": [{ "name": "K", "value": "v" }] })).unwrap(),
            serde_json::from_value(json!({ "type": "http", "name": "web", "url": "https://x", "headers": [] })).unwrap(),
        ])
        .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(
            config["mcpServers"]["sikemux-tools"]["command"],
            "/bin/tools"
        );
        assert_eq!(config["mcpServers"]["sikemux-tools"]["env"]["K"], "v");
        assert_eq!(config["mcpServers"]["web"]["type"], "http");
        assert_eq!(mcp_config(&[]), None);
    }

    #[test]
    fn files_go_as_links_and_handed_text_after_the_message() {
        let prompt = Prompt {
            message_id: None,
            text: "look".into(),
            paths: vec!["/tmp/a.rs".into()],
            context: vec![sikemux_wire::protocol::ChatContext {
                uri: "jira://X-1".into(),
                title: "X-1".into(),
                text: "the issue".into(),
            }],
            letter: false,
            resent: false,
        };
        let content = content(&prompt).unwrap();
        let texts: Vec<&str> = content
            .iter()
            .map(|block| block["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts[0], "look");
        assert!(texts.contains(&"[@a.rs](file:///tmp/a.rs)"));
        assert!(texts
            .last()
            .unwrap()
            .contains("<context ref=\"jira://X-1\">"));
    }
}
