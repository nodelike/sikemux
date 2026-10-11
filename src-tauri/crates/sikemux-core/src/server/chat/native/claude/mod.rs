//! Claude Code, driven over its stream-json control protocol: the protocol
//! Anthropic's own Agent SDK speaks to the `claude` binary.

mod config;
mod control;
mod permission;
mod reader;
mod replay;
mod tools;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::protocol::ChatLaunch;

use super::process::{Process, Writer};
use super::session::{Backend, Opened, Prompt, Sink};
use control::Control;
use reader::{Reader, Shared, Turn};

/// Where a session starts: a new one under a chosen id, or a saved one,
/// cut back to a message when it is resumed to edit what came after.
enum Start<'a> {
    New(&'a str),
    Resume { id: &'a str, at: Option<&'a str> },
}

/// One run of the `claude` process. Dropping it stops the process.
struct Running {
    process: Process,
    control: Control,
    reader: JoinHandle<()>,
    task_stops: tokio::sync::mpsc::UnboundedSender<String>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub(crate) struct Claude {
    launch: ChatLaunch,
    sink: Sink,
    running: Running,
    shared: Arc<Mutex<Shared>>,
    session_id: String,
    models: Vec<Value>,
    model: String,
    effort: String,
    /// What the person calls each message they sent, against the id Claude
    /// Code knows it by. An edited message is sent again under a new id.
    aliases: HashMap<String, String>,
}

fn config_dir(launch: &ChatLaunch) -> PathBuf {
    crate::acp::account::directory("claude", &launch.env)
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

/// The ACP tool server list as Claude Code's `--mcp-config` file.
fn mcp_config(servers: &[Value]) -> Option<String> {
    let pairs = |list: Option<&Value>| -> serde_json::Map<String, Value> {
        list.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pair| {
                Some((
                    pair.get("name")?.as_str()?.to_owned(),
                    pair.get("value")?.clone(),
                ))
            })
            .collect()
    };
    let servers: serde_json::Map<String, Value> = servers
        .iter()
        .filter_map(|server| {
            let name = server.get("name")?.as_str()?.to_owned();
            let config = match server.get("type").and_then(Value::as_str) {
                Some(kind @ ("http" | "sse")) => json!({
                    "type": kind,
                    "url": server.get("url")?,
                    "headers": pairs(server.get("headers")),
                }),
                _ => json!({
                    "type": "stdio",
                    "command": server.get("command")?,
                    "args": server.get("args").cloned().unwrap_or_else(|| json!([])),
                    "env": pairs(server.get("env")),
                }),
            };
            Some((name, config))
        })
        .collect();
    (!servers.is_empty()).then(|| json!({ "mcpServers": servers }).to_string())
}

/// What the person wrote, as the content of a Claude user message. Files go
/// as links Claude reads itself, and handed-over text follows the message.
fn content(prompt: &Prompt) -> Result<Vec<Value>, String> {
    let blocks = crate::acp::prompt_blocks(
        prompt.text.clone(),
        prompt.paths.clone(),
        prompt.context.clone(),
        true,
    )?;
    let link = |uri: &str| match uri
        .strip_prefix("file://")
        .and_then(|path| Path::new(path).file_name()?.to_str().map(str::to_owned))
    {
        Some(name) => format!("[@{name}]({uri})"),
        None => uri.to_owned(),
    };
    let mut content = Vec::new();
    let mut handed = Vec::new();
    for block in blocks {
        let block = serde_json::to_value(block).map_err(|error| error.to_string())?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => content.push(json!({ "type": "text", "text": block["text"] })),
            Some("resource_link") => {
                let uri = block["uri"].as_str().unwrap_or_default();
                content.push(json!({ "type": "text", "text": link(uri) }));
            }
            Some("resource") => {
                let uri = block.pointer("/resource/uri").and_then(Value::as_str).unwrap_or_default();
                let text = block.pointer("/resource/text").and_then(Value::as_str);
                content.push(json!({ "type": "text", "text": link(uri) }));
                if let Some(text) = text {
                    handed.push(json!({
                        "type": "text",
                        "text": format!("\n<context ref=\"{uri}\">\n{text}\n</context>"),
                    }));
                }
            }
            Some("image") => content.push(json!({
                "type": "image",
                "source": { "type": "base64", "data": block["data"], "media_type": block["mimeType"] },
            })),
            _ => {}
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

impl Claude {
    /// Starts `claude` on `start` and shakes hands with it.
    async fn spawn(
        launch: &ChatLaunch,
        sink: &Sink,
        shared: &Arc<Mutex<Shared>>,
        session_id: &str,
        start: Start<'_>,
        mode: &str,
        model: Option<&str>,
    ) -> Result<(Running, Value), String> {
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
            "--thinking-display",
            "summarized",
            "--disallowedTools",
            "AskUserQuestion",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend(["--permission-mode".to_owned(), mode.to_owned()]);
        if let Some(model) = model.filter(|model| *model != config::DEFAULT) {
            args.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(config) = mcp_config(&launch.mcp_servers) {
            args.extend(["--mcp-config".to_owned(), config]);
        }
        match start {
            Start::New(id) => args.push(format!("--session-id={id}")),
            Start::Resume { id, at } => {
                args.push(format!("--resume={id}"));
                if let Some(at) = at {
                    args.push(format!("--resume-session-at={at}"));
                }
            }
        }
        let env = [
            ("CLAUDE_CODE_ENTRYPOINT", "sdk-ts"),
            ("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS", "1"),
            ("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()));
        let (process, writer, mut lines) = Process::spawn(launch, args, env)?;
        let control = Control::new(writer);
        let (task_stops, mut stopped) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut reader = Reader::new(
            sink.clone(),
            control.clone(),
            launch.cwd.clone(),
            session_id.to_owned(),
            shared.clone(),
        );
        if let Some(model) = model {
            reader.note_model(model);
        }
        let reading = tokio::spawn(async move {
            loop {
                tokio::select! {
                    message = lines.next() => match message {
                        Some(message) => reader.handle(message),
                        None => break,
                    },
                    Some(task) = stopped.recv() => reader.task_stopped(&task),
                }
            }
            reader.close();
        });
        let running = Running {
            process,
            control: control.clone(),
            reader: reading,
            task_stops,
        };
        let initialized = tokio::select! {
            answer = control.request(json!({
                "subtype": "initialize",
                "hooks": {},
                "forwardSubagentText": true,
                "perTaskStopAffordance": true,
            })) => answer,
            ended = running.process.ended() => Err(ended),
        };
        match initialized {
            Ok(response) => Ok((running, response)),
            Err(error) => {
                let said = running.process.stderr();
                Err(if said.is_empty() {
                    format!("Claude did not start: {error}")
                } else {
                    format!("Claude did not start: {said}")
                })
            }
        }
    }

    fn writer(&self) -> &Writer {
        self.running.control.writer()
    }

    fn options(&self) -> Value {
        config::options(&self.models, &self.model, &self.effort)
    }

    async fn apply_effort(&self) -> Result<(), String> {
        let level = if self.effort == config::DEFAULT {
            Value::Null
        } else {
            json!(self.effort)
        };
        self.running
            .control
            .request(
                json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": level } }),
            )
            .await
            .map(drop)
    }

    /// The id Claude Code gets for message `message_id`. One it was already
    /// sent under, before an edit, gets a fresh one.
    fn uuid_for(&mut self, message_id: Option<&str>) -> String {
        let Some(message_id) = message_id else {
            return Uuid::new_v4().to_string();
        };
        let used = self.aliases.contains_key(message_id)
            || self
                .shared
                .lock()
                .map(|shared| shared.sent.contains(message_id))
                .unwrap_or(true);
        let uuid = if used || Uuid::parse_str(message_id).is_err() {
            Uuid::new_v4().to_string()
        } else {
            message_id.to_owned()
        };
        self.aliases.insert(message_id.to_owned(), uuid.clone());
        uuid
    }

    fn mode(&self) -> String {
        self.shared
            .lock()
            .map(|shared| shared.mode.clone())
            .unwrap_or_default()
    }
}

impl Backend for Claude {
    async fn start(launch: &ChatLaunch, sink: Sink) -> Result<(Self, Opened), String> {
        let mode = config::permission_mode(&launch.permission_mode)?;
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
            let (dir, id, cwd, sink) = (
                config_dir(launch),
                session_id.clone(),
                launch.cwd.clone(),
                sink.clone(),
            );
            tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
                let Some(path) = replay::transcript_path(&dir, &id) else {
                    return Ok(None);
                };
                let records = replay::with_subagents(&path, replay::read_chain(&path)?);
                for (session, update) in replay::replay(&records, &id, &cwd) {
                    sink.update(&session, update);
                }
                Ok(replay::resumed_model(&records))
            })
        });
        let start = match launch.resume_id.as_deref() {
            Some(id) => Start::Resume { id, at: None },
            None => Start::New(&session_id),
        };
        let wanted = launch
            .model
            .clone()
            .filter(|model| model != config::DEFAULT);
        let (running, initialized) = Self::spawn(
            launch,
            &sink,
            &shared,
            &session_id,
            start,
            mode,
            wanted.as_deref(),
        )
        .await?;
        let resumed_model = match replaying {
            Some(replaying) => replaying.await.map_err(|error| error.to_string())??,
            None => None,
        };
        let mut models: Vec<Value> = initialized
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Without a choice of its own the chat runs what the person's Claude
        // settings name, and the pickers say so.
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
        let resumed = resumed_model
            .as_deref()
            .and_then(|resolved| config::row_for_resolved(&models, resolved));
        let switch_to = (chosen.is_none() && from_settings.is_none())
            .then(|| resumed.clone())
            .flatten();
        let model = chosen
            .or(from_settings)
            .or(resumed)
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
        let claude = Self {
            launch: launch.clone(),
            sink: sink.clone(),
            running,
            shared,
            session_id: session_id.clone(),
            models,
            model,
            effort,
            aliases: HashMap::new(),
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
        if let Some(commands) = initialized.get("commands") {
            sink.update(&session_id, reader::available_commands(commands));
        }
        let setup = json!({
            "modes": config::modes(mode),
            "configOptions": claude.options(),
        });
        let capabilities = json!({
            "loadSession": true,
            "steering": true,
            "editing": true,
            "restoreFiles": true,
            "promptCapabilities": { "image": true, "embeddedContext": true },
        });
        Ok((
            claude,
            Opened {
                session_id,
                capabilities,
                setup,
            },
        ))
    }

    async fn prompt(&mut self, turn: u64, prompt: Prompt) -> Result<(), String> {
        let content = content(&prompt)?;
        let uuid = self.uuid_for(prompt.message_id.as_deref());
        if let Ok(mut shared) = self.shared.lock() {
            if let Some(untitled) = shared.untitled.as_mut().filter(|said| said.is_empty()) {
                untitled.push_str(&prompt.text);
            }
            shared.sent.insert(uuid.clone());
            shared.turn = Some(Turn {
                number: turn,
                latest: uuid.clone(),
                cancelled: false,
                failure: None,
            });
        }
        let sent = self.writer().send(&user_message(&uuid, content, None));
        if sent.is_err() {
            if let Ok(mut shared) = self.shared.lock() {
                shared.turn = None;
            }
        }
        sent
    }

    async fn steer(&mut self, prompt: Prompt) -> Result<String, String> {
        let content = content(&prompt)?;
        let uuid = Uuid::new_v4().to_string();
        {
            let Ok(mut shared) = self.shared.lock() else {
                return Err("The chat's state is unavailable".into());
            };
            if shared.turn.is_none() {
                return Ok("promptRequired".into());
            }
            shared.sent.insert(uuid.clone());
            if let Some(turn) = shared.turn.as_mut() {
                turn.latest = uuid.clone();
            }
        }
        self.writer()
            .send(&user_message(&uuid, content, Some("now")))?;
        Ok("injected".into())
    }

    async fn cancel(&mut self) -> Result<(), String> {
        let running = match self.shared.lock() {
            Ok(mut shared) => match shared.turn.as_mut() {
                Some(turn) => {
                    turn.cancelled = true;
                    true
                }
                None => false,
            },
            Err(_) => false,
        };
        if !running {
            return Ok(());
        }
        self.running
            .control
            .request(json!({ "subtype": "interrupt" }))
            .await
            .map(drop)
    }

    async fn set_permission_mode(&mut self, mode: &str) -> Result<(), String> {
        let mode = config::permission_mode(mode)?;
        self.running
            .control
            .request(json!({ "subtype": "set_permission_mode", "mode": mode }))
            .await?;
        if let Ok(mut shared) = self.shared.lock() {
            shared.mode = mode.to_owned();
        }
        Ok(())
    }

    async fn set_config(&mut self, config_id: &str, value: &str) -> Result<Value, String> {
        match config_id {
            "model" => {
                if !config::offers_model(&self.models, value) {
                    return Err(format!("Invalid value for config option model: {value}"));
                }
                self.running
                    .control
                    .request(json!({ "subtype": "set_model", "model": value }))
                    .await?;
                self.model = value.to_owned();
                let levels = config::effort_levels(&self.models, value);
                if self.effort != config::DEFAULT && !levels.contains(&self.effort) {
                    self.effort = config::DEFAULT.to_owned();
                    self.apply_effort().await?;
                }
            }
            "effort" => {
                let levels = config::effort_levels(&self.models, &self.model);
                if value != config::DEFAULT && !levels.iter().any(|level| level == value) {
                    return Err(format!("Invalid value for config option effort: {value}"));
                }
                self.effort = value.to_owned();
                self.apply_effort().await?;
            }
            _ => return Err(format!("Unknown config option: {config_id}")),
        }
        Ok(self.options())
    }

    async fn stop_task(&mut self, task_id: &str) -> Result<(), String> {
        self.running
            .control
            .request(json!({ "subtype": "stop_task", "task_id": task_id }))
            .await?;
        let _ = self.running.task_stops.send(task_id.to_owned());
        Ok(())
    }

    async fn rewind(
        &mut self,
        message_id: &str,
        restore_files: bool,
    ) -> Result<Option<String>, String> {
        let uuid = self
            .aliases
            .get(message_id)
            .cloned()
            .unwrap_or_else(|| message_id.to_owned());
        if restore_files {
            let rewound = self
                .running
                .control
                .request(
                    json!({ "subtype": "rewind_files", "user_message_id": uuid, "dry_run": false }),
                )
                .await?;
            if rewound.get("canRewind").and_then(Value::as_bool) == Some(false) {
                let reason = rewound
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude has no record of the files before that message");
                return Err(format!("The files could not be put back: {reason}"));
            }
        }
        let path = replay::transcript_path(&config_dir(&self.launch), &self.session_id)
            .ok_or("Claude has not saved this chat yet")?;
        let records = replay::read_chain(&path)?;
        if !records
            .iter()
            .any(|record| record.get("uuid").and_then(Value::as_str) == Some(uuid.as_str()))
        {
            return Err("That message is no longer part of this chat".into());
        }
        let at = replay::rewind_point(&records, &uuid);
        let renamed = at.is_none().then(|| Uuid::new_v4().to_string());
        let session_id = renamed.clone().unwrap_or_else(|| self.session_id.clone());
        let start = match at.as_deref() {
            Some(at) => Start::Resume {
                id: &self.session_id,
                at: Some(at),
            },
            None => Start::New(&session_id),
        };
        let mode = self.mode();
        let model = (self.model != config::DEFAULT).then(|| self.model.clone());
        let (running, _) = Self::spawn(
            &self.launch,
            &self.sink,
            &self.shared,
            &session_id,
            start,
            &mode,
            model.as_deref(),
        )
        .await?;
        self.running = running;
        self.session_id = session_id;
        if self.effort != config::DEFAULT {
            self.apply_effort().await?;
        }
        Ok(renamed)
    }

    async fn closed(&self) -> String {
        self.running.process.ended().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_servers_become_claudes_mcp_config() {
        let config = mcp_config(&[
            json!({ "name": "sikemux-tools", "command": "/bin/tools", "args": ["--x"], "env": [{ "name": "K", "value": "v" }] }),
            json!({ "type": "http", "name": "web", "url": "https://x", "headers": [] }),
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
            context: vec![crate::protocol::ChatContext {
                uri: "jira://X-1".into(),
                title: "X-1".into(),
                text: "the issue".into(),
            }],
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
