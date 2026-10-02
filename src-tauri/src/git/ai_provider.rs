use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::ai::{AI_COMMAND_TIMEOUT, AI_RESPONSE_LIMIT};
use super::{kill_and_reap_process, read_bounded_pipe};

#[derive(Clone, Copy, Debug)]
pub(super) enum GitAiProvider {
    Hermes,
    Codex,
    Claude,
}

impl GitAiProvider {
    pub(super) fn parse(raw: Option<String>) -> Result<Self, String> {
        match raw
            .as_deref()
            .unwrap_or("hermes")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "hermes" => Ok(Self::Hermes),
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            other => Err(format!("unknown local AI provider: {other}")),
        }
    }

    fn bin(self) -> &'static str {
        match self {
            Self::Hermes => "hermes",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(super) fn default_model(self) -> &'static str {
        match self {
            Self::Hermes => "openai/gpt-5.5",
            Self::Codex => "gpt-5.5",
            Self::Claude => "sonnet",
        }
    }
}

#[derive(Clone, Copy)]
enum AiOutputFormat {
    Text,
    ClaudeJson,
}

struct AiStreamDecoder {
    format: AiOutputFormat,
    pending: Vec<u8>,
    message: String,
}

impl AiStreamDecoder {
    fn new(format: AiOutputFormat) -> Self {
        Self {
            format,
            pending: Vec::new(),
            message: String::new(),
        }
    }

    fn emit(
        &mut self,
        text: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        self.message.push_str(text);
        if let Some(channel) = on_chunk {
            channel
                .send(text.to_string())
                .map_err(|e| format!("Commit message stream closed: {e}"))?;
        }
        Ok(())
    }

    fn apply_snapshot(
        &mut self,
        text: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        if let Some(suffix) = text.strip_prefix(&self.message) {
            self.emit(suffix, on_chunk)
        } else {
            // The command's final snapshot is authoritative. The frontend
            // replaces its streamed draft with the returned cleaned message.
            self.message.clear();
            self.message.push_str(text);
            Ok(())
        }
    }

    fn structured_line(
        &mut self,
        line: &str,
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            return Ok(());
        }
        let event: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("Local AI returned invalid streaming JSON: {e}"))?;
        match self.format {
            AiOutputFormat::ClaudeJson => {
                if event.get("type").and_then(|value| value.as_str()) == Some("stream_event") {
                    let delta = event.pointer("/event/delta");
                    if delta
                        .and_then(|value| value.get("type"))
                        .and_then(|value| value.as_str())
                        == Some("text_delta")
                    {
                        if let Some(text) = delta
                            .and_then(|value| value.get("text"))
                            .and_then(|value| value.as_str())
                        {
                            self.emit(text, on_chunk)?;
                        }
                    }
                } else if event.get("type").and_then(|value| value.as_str()) == Some("result") {
                    if let Some(text) = event.get("result").and_then(|value| value.as_str()) {
                        self.apply_snapshot(text, on_chunk)?;
                    }
                }
            }
            AiOutputFormat::Text => unreachable!("text output is not line decoded"),
        }
        Ok(())
    }

    fn push(
        &mut self,
        bytes: &[u8],
        on_chunk: Option<&tauri::ipc::Channel<String>>,
    ) -> Result<(), String> {
        self.pending.extend_from_slice(bytes);
        if matches!(self.format, AiOutputFormat::Text) {
            loop {
                match std::str::from_utf8(&self.pending) {
                    Ok(text) => {
                        let text = text.to_string();
                        self.pending.clear();
                        return self.emit(&text, on_chunk);
                    }
                    Err(error) if error.valid_up_to() > 0 => {
                        let valid = error.valid_up_to();
                        let text = std::str::from_utf8(&self.pending[..valid])
                            .expect("validated UTF-8 prefix")
                            .to_string();
                        self.pending.drain(..valid);
                        self.emit(&text, on_chunk)?;
                    }
                    Err(error) if error.error_len().is_none() => return Ok(()),
                    Err(error) => {
                        return Err(format!("Local AI returned invalid UTF-8: {error}"));
                    }
                }
            }
        }

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            let line = std::str::from_utf8(&line)
                .map_err(|e| format!("Local AI returned invalid UTF-8: {e}"))?;
            self.structured_line(line, on_chunk)?;
        }
        Ok(())
    }

    fn finish(mut self, on_chunk: Option<&tauri::ipc::Channel<String>>) -> Result<String, String> {
        if !self.pending.is_empty() {
            match self.format {
                AiOutputFormat::Text => {
                    let text = std::str::from_utf8(&self.pending)
                        .map_err(|e| format!("Local AI returned incomplete UTF-8: {e}"))?
                        .to_string();
                    self.pending.clear();
                    self.emit(&text, on_chunk)?;
                }
                AiOutputFormat::ClaudeJson => {
                    let line = std::str::from_utf8(&self.pending)
                        .map_err(|e| format!("Local AI returned invalid UTF-8: {e}"))?
                        .to_string();
                    self.pending.clear();
                    self.structured_line(&line, on_chunk)?;
                }
            }
        }
        Ok(self.message)
    }
}

fn command_candidates(name: &str) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![
        name.to_string(),
        format!("{home}/.local/bin/{name}"),
        format!("{home}/.cargo/bin/{name}"),
        format!("{home}/.opencode/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
    ];
    if let Some(path) = crate::system::find_executable(name) {
        candidates.insert(0, path.to_string_lossy().into_owned());
    }
    candidates.dedup();
    candidates
}

#[derive(Debug, PartialEq)]
enum CodexAppServerEvent {
    Initialized,
    ThreadStarted(String),
    TurnStarted,
    AgentMessageDelta(String),
    AgentMessageCompleted(String),
    TurnCompleted,
    Error(String),
    Ignore,
}

fn codex_error_message(value: &serde_json::Value) -> Option<String> {
    value
        .get("message")
        .and_then(|message| message.as_str())
        .map(str::to_string)
}

fn parse_codex_app_server_event(line: &str) -> Result<CodexAppServerEvent, String> {
    let event: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| format!("Codex app-server returned invalid JSON: {error}"))?;

    if let Some(error) = event.get("error") {
        return Ok(CodexAppServerEvent::Error(
            codex_error_message(error).unwrap_or_else(|| error.to_string()),
        ));
    }

    match event.get("id").and_then(|id| id.as_u64()) {
        Some(0) => return Ok(CodexAppServerEvent::Initialized),
        Some(1) => {
            let thread_id = event
                .pointer("/result/thread/id")
                .and_then(|id| id.as_str())
                .ok_or_else(|| "Codex app-server did not return a thread id".to_string())?;
            return Ok(CodexAppServerEvent::ThreadStarted(thread_id.to_string()));
        }
        Some(2) => return Ok(CodexAppServerEvent::TurnStarted),
        _ => {}
    }

    let method = event.get("method").and_then(|method| method.as_str());
    match method {
        Some("item/agentMessage/delta") => event
            .pointer("/params/delta")
            .and_then(|delta| delta.as_str())
            .map(|delta| CodexAppServerEvent::AgentMessageDelta(delta.to_string()))
            .ok_or_else(|| "Codex app-server emitted a text delta without text".to_string()),
        Some("item/completed")
            if event
                .pointer("/params/item/type")
                .and_then(|kind| kind.as_str())
                == Some("agentMessage") =>
        {
            event
                .pointer("/params/item/text")
                .and_then(|text| text.as_str())
                .map(|text| CodexAppServerEvent::AgentMessageCompleted(text.to_string()))
                .ok_or_else(|| {
                    "Codex app-server completed an assistant message without text".to_string()
                })
        }
        Some("turn/completed") => {
            let status = event
                .pointer("/params/turn/status")
                .and_then(|status| status.as_str())
                .unwrap_or("failed");
            if status == "completed" {
                Ok(CodexAppServerEvent::TurnCompleted)
            } else {
                let detail = event
                    .pointer("/params/turn/error/message")
                    .and_then(|message| message.as_str())
                    .unwrap_or(status);
                Ok(CodexAppServerEvent::Error(format!(
                    "Codex turn {status}: {detail}"
                )))
            }
        }
        Some("error") => {
            let will_retry = event
                .pointer("/params/willRetry")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if will_retry {
                Ok(CodexAppServerEvent::Ignore)
            } else {
                let error = event
                    .pointer("/params/error")
                    .and_then(codex_error_message)
                    .unwrap_or_else(|| "Codex turn failed".to_string());
                Ok(CodexAppServerEvent::Error(error))
            }
        }
        _ => Ok(CodexAppServerEvent::Ignore),
    }
}

fn write_codex_app_server_message(
    stdin: &mut impl Write,
    message: &serde_json::Value,
) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, message)
        .map_err(|error| format!("Could not encode Codex app-server request: {error}"))?;
    stdin
        .write_all(b"\n")
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("Could not write to Codex app-server: {error}"))
}

fn read_codex_app_server_lines(
    mut stdout: impl Read,
    sender: std::sync::mpsc::Sender<Result<String, String>>,
    exceeded: Arc<AtomicBool>,
) {
    let mut total = 0usize;
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => {
                if !pending.is_empty() {
                    let line = String::from_utf8(pending).map_err(|error| {
                        format!("Codex app-server returned invalid UTF-8: {error}")
                    });
                    let _ = sender.send(line);
                }
                return;
            }
            Ok(read) => {
                total = total.saturating_add(read);
                if total > AI_RESPONSE_LIMIT {
                    exceeded.store(true, Ordering::Release);
                    let _ = sender.send(Err("Codex app-server output exceeds 1 MiB limit".into()));
                    return;
                }
                pending.extend_from_slice(&chunk[..read]);
                while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut line = pending.drain(..=newline).collect::<Vec<_>>();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let line = String::from_utf8(line).map_err(|error| {
                        format!("Codex app-server returned invalid UTF-8: {error}")
                    });
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = sender.send(Err(format!("Could not read Codex app-server: {error}")));
                return;
            }
        }
    }
}

fn run_codex_app_server_candidate(
    bin: &str,
    repo: &str,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let repo = Path::new(repo)
        .canonicalize()
        .map_err(|error| format!("Could not resolve repository path: {error}"))?;
    let repo = repo.to_string_lossy().into_owned();
    let mut command = sikemux_process::user_environment::command(bin);
    command
        .current_dir(&repo)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("codex failed to start: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stdout unavailable".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stderr unavailable".into());
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            kill_and_reap_process(&mut child);
            return Err("codex stdin unavailable".into());
        }
    };

    let output_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_exceeded = Arc::clone(&output_exceeded);
    let (line_sender, line_receiver) = std::sync::mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        read_codex_app_server_lines(stdout, line_sender, stdout_exceeded)
    });
    let stderr_exceeded = Arc::clone(&output_exceeded);
    let stderr_reader = std::thread::spawn(move || read_bounded_pipe(stderr, stderr_exceeded));

    let result = (|| -> Result<String, String> {
        write_codex_app_server_message(
            &mut stdin,
            &serde_json::json!({
                "id": 0,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "sikemux",
                        "title": "Sikemux",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;

        let deadline = Instant::now() + AI_COMMAND_TIMEOUT;
        let mut final_message = None;
        loop {
            if output_exceeded.load(Ordering::Acquire) {
                return Err("Local AI output exceeds its safety limit".into());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "codex timed out after {}s",
                    AI_COMMAND_TIMEOUT.as_secs()
                ));
            }
            match line_receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(line)) => match parse_codex_app_server_event(line.trim_end())? {
                    CodexAppServerEvent::Initialized => {
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({"method": "initialized"}),
                        )?;
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({
                                "id": 1,
                                "method": "thread/start",
                                "params": {
                                    "model": model,
                                    "cwd": repo,
                                    "approvalPolicy": "never",
                                    "sandbox": "read-only",
                                    "ephemeral": true,
                                    "serviceName": "sikemux",
                                    "developerInstructions": "Answer directly from the supplied prompt. Do not call tools or inspect the workspace. Return only the requested commit message."
                                }
                            }),
                        )?;
                    }
                    CodexAppServerEvent::ThreadStarted(thread_id) => {
                        write_codex_app_server_message(
                            &mut stdin,
                            &serde_json::json!({
                                "id": 2,
                                "method": "turn/start",
                                "params": {
                                    "threadId": thread_id,
                                    "input": [{"type": "text", "text": prompt}]
                                }
                            }),
                        )?;
                    }
                    CodexAppServerEvent::AgentMessageDelta(delta) => {
                        if let Some(channel) = &on_chunk {
                            channel.send(delta).map_err(|error| {
                                format!("Commit message stream closed: {error}")
                            })?;
                        }
                    }
                    CodexAppServerEvent::AgentMessageCompleted(message) => {
                        final_message = Some(message);
                    }
                    CodexAppServerEvent::TurnCompleted => {
                        return final_message
                            .filter(|message| !message.trim().is_empty())
                            .ok_or_else(|| "codex returned no commit message".to_string());
                    }
                    CodexAppServerEvent::Error(error) => {
                        return Err(format!("codex failed: {error}"))
                    }
                    CodexAppServerEvent::TurnStarted | CodexAppServerEvent::Ignore => {}
                },
                Ok(Err(error)) => return Err(error),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => match child.try_wait() {
                    Ok(Some(status)) => {
                        return Err(format!(
                            "codex app-server exited unexpectedly with {status}"
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => return Err(format!("codex failed: {error}")),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("codex app-server closed its output unexpectedly".into());
                }
            }
        }
    })();

    drop(stdin);
    kill_and_reap_process(&mut child);
    let _ = stdout_reader.join();
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Codex app-server stderr reader panicked".to_string())?
        .map_err(|error| error.to_string())?;

    result.map_err(|error| {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        if detail.is_empty() || error.contains(&detail) {
            error
        } else {
            format!("{error}: {detail}")
        }
    })
}

fn run_codex_app_server(
    repo: &str,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    for bin in command_candidates("codex") {
        match run_codex_app_server_candidate(&bin, repo, model, prompt, on_chunk.clone()) {
            Err(error) if error.starts_with("codex failed to start: No such file") => continue,
            result => return result,
        }
    }
    Err("codex is not installed or could not be found on PATH".into())
}

fn read_ai_pipe(
    mut pipe: impl Read,
    format: AiOutputFormat,
    on_chunk: Option<tauri::ipc::Channel<String>>,
    exceeded: Arc<AtomicBool>,
) -> Result<String, String> {
    let mut decoder = AiStreamDecoder::new(format);
    let mut total = 0usize;
    let mut chunk = [0_u8; 4096];
    let mut decode_error = None;
    loop {
        let read = pipe.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            if let Some(error) = decode_error {
                return Err(error);
            }
            return decoder.finish(on_chunk.as_ref());
        }
        total = total.saturating_add(read);
        if total > AI_RESPONSE_LIMIT {
            exceeded.store(true, Ordering::Release);
            return Err("Local AI response exceeds 1 MiB limit".into());
        }
        if decode_error.is_none() {
            if let Err(error) = decoder.push(&chunk[..read], on_chunk.as_ref()) {
                decode_error = Some(error);
            }
        }
    }
}

fn run_ai_candidate<F>(
    provider: GitAiProvider,
    on_chunk: Option<tauri::ipc::Channel<String>>,
    mut build: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> (Command, Option<Vec<u8>>, AiOutputFormat),
{
    for bin in command_candidates(provider.bin()) {
        let (mut command, input, format) = build(&bin);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{} failed to start: {error}", provider.bin())),
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                kill_and_reap_process(&mut child);
                return Err(format!("{} stdout unavailable", provider.bin()));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                kill_and_reap_process(&mut child);
                return Err(format!("{} stderr unavailable", provider.bin()));
            }
        };

        let output_exceeded = Arc::new(AtomicBool::new(false));
        let stdout_exceeded = Arc::clone(&output_exceeded);
        let stream_channel = on_chunk.clone();
        let stdout_reader = std::thread::spawn(move || {
            read_ai_pipe(stdout, format, stream_channel, stdout_exceeded)
        });
        let stderr_exceeded = Arc::clone(&output_exceeded);
        let stderr_reader = std::thread::spawn(move || read_bounded_pipe(stderr, stderr_exceeded));
        let stdin_writer = input.map(|bytes| {
            let stdin = child.stdin.take();
            std::thread::spawn(move || {
                stdin
                    .ok_or_else(|| "Local AI stdin unavailable".to_string())
                    .and_then(|mut stdin| stdin.write_all(&bytes).map_err(|e| e.to_string()))
            })
        });

        let deadline = Instant::now() + AI_COMMAND_TIMEOUT;
        let status = loop {
            if output_exceeded.load(Ordering::Acquire) {
                kill_and_reap_process(&mut child);
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("Local AI output exceeds its safety limit".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    kill_and_reap_process(&mut child);
                    if let Some(writer) = stdin_writer {
                        let _ = writer.join();
                    }
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(format!(
                        "{} timed out after {}s",
                        provider.bin(),
                        AI_COMMAND_TIMEOUT.as_secs()
                    ));
                }
                Err(error) => {
                    kill_and_reap_process(&mut child);
                    return Err(format!("{} failed: {error}", provider.bin()));
                }
            }
        };
        if let Some(writer) = stdin_writer {
            writer
                .join()
                .map_err(|_| "Local AI stdin writer panicked".to_string())??;
        }
        let message = stdout_reader
            .join()
            .map_err(|_| "Local AI stdout reader panicked".to_string())?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| "Local AI stderr reader panicked".to_string())?
            .map_err(|e| e.to_string())?;
        if !status.success() {
            let detail = String::from_utf8_lossy(&stderr).trim().to_string();
            return Err(if detail.is_empty() {
                format!("{} exited with {status}", provider.bin())
            } else {
                format!("{} failed: {detail}", provider.bin())
            });
        }
        let message = message?;
        if message.trim().is_empty() {
            return Err(format!("{} returned no commit message", provider.bin()));
        }
        return Ok(message);
    }
    Err(format!(
        "{} is not installed or could not be found on PATH",
        provider.bin()
    ))
}

pub(super) fn run_ai_commit_model(
    repo: &str,
    provider: GitAiProvider,
    model: &str,
    prompt: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let model = if model.trim().is_empty() {
        provider.default_model()
    } else {
        model.trim()
    };
    match provider {
        GitAiProvider::Hermes => run_ai_candidate(provider, on_chunk, |bin| {
            let mut command = sikemux_process::user_environment::command(bin);
            command
                .current_dir(repo)
                .args(["chat", "-Q", "-m", model, "-t", "safe", "-q", prompt]);
            (command, None, AiOutputFormat::Text)
        }),
        GitAiProvider::Codex => run_codex_app_server(repo, model, prompt, on_chunk),
        GitAiProvider::Claude => run_ai_candidate(provider, on_chunk, |bin| {
            let mut command = sikemux_process::user_environment::command(bin);
            command.current_dir(repo).args([
                "--print",
                "--model",
                model,
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--no-session-persistence",
                "--tools",
                "",
                "--setting-sources",
                "user",
                "--strict-mcp-config",
            ]);
            (
                command,
                Some(prompt.as_bytes().to_vec()),
                AiOutputFormat::ClaudeJson,
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_claude_partial_stream_across_single_byte_chunks() {
        let stream = concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"feat(git): \"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"stream ✨\"}}}\n",
            "{\"type\":\"result\",\"result\":\"feat(git): stream ✨\"}\n",
        );
        let mut decoder = AiStreamDecoder::new(AiOutputFormat::ClaudeJson);
        for byte in stream.as_bytes() {
            decoder
                .push(std::slice::from_ref(byte), None)
                .expect("stream chunk");
        }

        assert_eq!(
            decoder.finish(None).expect("finish"),
            "feat(git): stream ✨"
        );
    }

    #[test]
    fn decodes_codex_app_server_token_delta_and_final_message() {
        let delta = parse_codex_app_server_event(
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"item","delta":"fix(ui): "}}"#,
        )
        .expect("delta");
        let completed = parse_codex_app_server_event(
            r#"{"method":"item/completed","params":{"threadId":"thread","turnId":"turn","completedAtMs":1,"item":{"id":"item","type":"agentMessage","text":"fix(ui): stream token output"}}}"#,
        )
        .expect("completed message");
        assert_eq!(
            delta,
            CodexAppServerEvent::AgentMessageDelta("fix(ui): ".into())
        );
        assert_eq!(
            completed,
            CodexAppServerEvent::AgentMessageCompleted("fix(ui): stream token output".into())
        );
    }

    #[test]
    fn decodes_codex_app_server_handshake_and_turn_completion() {
        assert_eq!(
            parse_codex_app_server_event(r#"{"id":0,"result":{"userAgent":"test"}}"#)
                .expect("initialize"),
            CodexAppServerEvent::Initialized
        );
        assert_eq!(
            parse_codex_app_server_event(r#"{"id":1,"result":{"thread":{"id":"thread-123"}}}"#)
                .expect("thread"),
            CodexAppServerEvent::ThreadStarted("thread-123".into())
        );
        assert_eq!(
            parse_codex_app_server_event(
                r#"{"method":"turn/completed","params":{"threadId":"thread-123","turn":{"id":"turn","items":[],"status":"completed"}}}"#
            )
            .expect("turn"),
            CodexAppServerEvent::TurnCompleted
        );
    }

    #[test]
    fn text_decoder_preserves_split_utf8() {
        let text = "feat(git): stream ✨";
        let mut decoder = AiStreamDecoder::new(AiOutputFormat::Text);
        for byte in text.as_bytes() {
            decoder
                .push(std::slice::from_ref(byte), None)
                .expect("text chunk");
        }

        assert_eq!(decoder.finish(None).expect("finish"), text);
    }
}
