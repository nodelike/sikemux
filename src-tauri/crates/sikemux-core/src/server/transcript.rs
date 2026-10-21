//! What an agent running in a terminal said, read from the session file its
//! CLI writes: Claude Code's transcript or Codex's rollout. Both only grow, so
//! a byte offset into one is a cursor.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::chat::feed::Said;
use crate::agent_catalog::AgentKind;

/// How much of a file's end is read when no cursor says where to start.
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// How much one read takes from a cursor on.
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// The session file of `session`, under the agent's account folder or the
/// CLI's own.
pub(crate) fn locate(kind: AgentKind, home: Option<&Path>, session: &str) -> Option<PathBuf> {
    let home = home.map(Path::to_path_buf).or_else(|| kind.home_root())?;
    crate::sessions::transcript::find(kind, &home, None, session)
}

/// What a session file said from one offset on.
pub(crate) struct Excerpt {
    /// Each with the offset of its line, and whether a turn starts there.
    pub said: Vec<(u64, bool, Said)>,
    /// Where reading on starts.
    pub next: u64,
    /// Everything the file holds has been read.
    pub whole: bool,
    /// The offset asked for is past the file's end, so it is not the file the
    /// cursor came from and this is its last turns instead.
    pub restarted: bool,
}

/// What the file said from `from` on, or over its last `turns` turns.
pub(crate) fn read(
    cli: AgentKind,
    path: &Path,
    from: Option<u64>,
    turns: usize,
) -> std::io::Result<Excerpt> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let restarted = from.is_some_and(|from| from > len);
    let from = from.filter(|from| *from <= len);
    let start = from.unwrap_or_else(|| len.saturating_sub(TAIL_BYTES));
    let end = match from {
        Some(from) => len.min(from + MAX_READ_BYTES),
        None => len,
    };
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end - start).read_to_end(&mut bytes)?;
    let mut offset = start;
    let mut rest = bytes.as_slice();
    if from.is_none() && start > 0 {
        let cut = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(rest.len(), |at| at + 1);
        offset += cut as u64;
        rest = &rest[cut..];
    }
    let mut said: Vec<(u64, bool, Said)> = Vec::new();
    while let Some(at) = rest.iter().position(|byte| *byte == b'\n') {
        let line = &rest[..at];
        if let Ok(value) = serde_json::from_slice::<Value>(line) {
            let mut items = Vec::new();
            match cli {
                AgentKind::Claude => claude(&value, &mut items),
                AgentKind::Codex => codex(&value, &mut items),
                _ => {}
            }
            for (starts, item) in items {
                push(&mut said, cli, offset, starts, item);
            }
        }
        offset += at as u64 + 1;
        rest = &rest[at + 1..];
    }
    if from.is_none() {
        let starts: Vec<usize> = said
            .iter()
            .enumerate()
            .filter(|(_, (_, starts, _))| *starts)
            .map(|(position, _)| position)
            .collect();
        if let Some(first) = starts
            .len()
            .checked_sub(turns)
            .and_then(|skip| starts.get(skip))
        {
            said.drain(..*first);
        }
    }
    Ok(Excerpt {
        said,
        next: offset,
        whole: offset == len,
        restarted,
    })
}

/// Adds `item`, unless it repeats the one before it: a Codex rollout can
/// carry the same message both as an event and as a finished item. Claude
/// writes nothing where a turn ends, so its next prompt ends the one before.
fn push(
    said: &mut Vec<(u64, bool, Said)>,
    cli: AgentKind,
    offset: u64,
    mut starts: bool,
    item: Said,
) {
    if let Some((_, _, last)) = said.last() {
        let repeat = match (last, &item) {
            (Said::Asked(last), Said::Asked(text)) | (Said::Message(last), Said::Message(text)) => {
                last == text
            }
            _ => false,
        };
        if repeat {
            return;
        }
        if matches!(last, Said::TurnStarted) && matches!(item, Said::Asked(_)) {
            starts = false;
        }
        let open = !matches!(last, Said::TurnEnded { .. } | Said::Failed(_));
        if cli == AgentKind::Claude && starts && open {
            said.push((offset, false, Said::TurnEnded { cancelled: false }));
        }
    }
    said.push((offset, starts, item));
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty())
}

/// The text parts of a message's content, joined.
fn texts(content: &Value) -> Option<String> {
    if let Some(text) = text(content) {
        return Some(text.to_owned());
    }
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("text" | "Text" | "input_text" | "output_text")
            )
        })
        .filter_map(|part| text(part.get("text")?))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// `text` without the tags Claude Code wraps a long paste in.
fn unpasted(text: &str) -> String {
    let mut text = text.to_owned();
    for tag in ["<pasted_content", "</pasted_content"] {
        while let Some(start) = text.find(tag) {
            let Some(length) = text[start..].find('>') else {
                break;
            };
            text.replace_range(start..=start + length, "");
        }
    }
    text.trim().to_owned()
}

fn prompt(text: String, items: &mut Vec<(bool, Said)>) {
    let text = unpasted(&text);
    let start = text.as_str();
    if start.starts_with("[Request interrupted") {
        items.push((false, Said::TurnEnded { cancelled: true }));
    } else if !start.starts_with("<command-") && !start.starts_with("<local-command") {
        items.push((true, Said::Asked(text)));
    }
}

fn claude(record: &Value, items: &mut Vec<(bool, Said)>) {
    let flag = |key: &str| record.get(key).and_then(Value::as_bool) == Some(true);
    if flag("isSidechain") || flag("isMeta") {
        return;
    }
    let content = &record["message"]["content"];
    match record.get("type").and_then(Value::as_str) {
        Some("user") => {
            for block in content.as_array().into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) == Some("tool_result")
                    && block.get("is_error").and_then(Value::as_bool) == Some(true)
                {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                        items.push((
                            false,
                            Said::Tool {
                                id: id.to_owned(),
                                title: None,
                                failed: true,
                            },
                        ));
                    }
                }
            }
            if let Some(text) = texts(content) {
                prompt(text, items);
            }
        }
        Some("assistant") => {
            for block in content.as_array().into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(text) {
                            items.push((false, Said::Message(text.to_owned())));
                        }
                    }
                    Some("tool_use") => {
                        let (Some(id), Some(name)) = (
                            block.get("id").and_then(Value::as_str),
                            block.get("name").and_then(Value::as_str),
                        ) else {
                            continue;
                        };
                        items.push((
                            false,
                            Said::Tool {
                                id: id.to_owned(),
                                title: Some(claude_title(name, &block["input"])),
                                failed: false,
                            },
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn claude_title(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str);
    let detail = match name {
        "Bash" => field("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "Grep" | "Glob" => field("pattern"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Task" | "Agent" => field("description"),
        _ => None,
    };
    let name = name.rsplit("__").next().unwrap_or(name);
    match detail {
        Some(detail) => format!("{name} {detail}"),
        None => name.to_owned(),
    }
}

fn codex(record: &Value, items: &mut Vec<(bool, Said)>) {
    let payload = &record["payload"];
    let kind = payload.get("type").and_then(Value::as_str);
    match record.get("type").and_then(Value::as_str) {
        Some("event_msg") => match kind {
            Some("task_started") => items.push((true, Said::TurnStarted)),
            Some("user_message") => {
                if let Some(text) = payload.get("message").and_then(text) {
                    prompt(text.to_owned(), items);
                }
            }
            Some("agent_message") => {
                if let Some(text) = payload.get("message").and_then(text) {
                    items.push((false, Said::Message(text.to_owned())));
                }
            }
            Some("item_completed") => {
                let item = &payload["item"];
                let said = texts(&item["content"]);
                match (item.get("type").and_then(Value::as_str), said) {
                    (Some("UserMessage"), Some(text)) => prompt(text, items),
                    (Some("AgentMessage"), Some(text)) => items.push((false, Said::Message(text))),
                    _ => {}
                }
            }
            Some("task_complete") => {
                let error = &payload["error"];
                let failure = text(error)
                    .or_else(|| error.get("message").and_then(text))
                    .map(str::to_owned);
                items.push((
                    false,
                    match failure {
                        Some(failure) => Said::Failed(failure),
                        None => Said::TurnEnded { cancelled: false },
                    },
                ));
            }
            Some("turn_aborted") => items.push((false, Said::TurnEnded { cancelled: true })),
            Some("error") => {
                if let Some(message) = payload.get("message").and_then(text) {
                    items.push((false, Said::Failed(message.to_owned())));
                }
            }
            _ => {}
        },
        Some("response_item") => {
            let call = matches!(
                kind,
                Some("function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call")
            );
            let id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str);
            if let (true, Some(id)) = (call, id) {
                items.push((
                    false,
                    Said::Tool {
                        id: id.to_owned(),
                        title: Some(codex_title(payload)),
                        failed: false,
                    },
                ));
            }
        }
        _ => {}
    }
}

fn command_line(command: &Value) -> Option<String> {
    match command {
        Value::String(command) => Some(command.clone()),
        Value::Array(words) => {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            let shell = matches!(words.as_slice(), [_, "-lc" | "-c", _]);
            Some(if shell {
                words[2].to_owned()
            } else {
                words.join(" ")
            })
        }
        _ => None,
    }
}

fn codex_title(payload: &Value) -> String {
    let name = match payload.get("type").and_then(Value::as_str) {
        Some("local_shell_call") => "shell",
        Some("web_search_call") => "web search",
        _ => payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool"),
    };
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok());
    let command = arguments
        .as_ref()
        .and_then(|arguments| arguments.get("cmd").or_else(|| arguments.get("command")))
        .or_else(|| {
            payload
                .get("action")
                .and_then(|action| action.get("command"))
        })
        .and_then(command_line);
    match command {
        Some(command) => format!("{name} {command}"),
        None => name.to_owned(),
    }
}
