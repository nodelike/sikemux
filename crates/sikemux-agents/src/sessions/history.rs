//! A saved OMP or OpenCode session as the updates its agent sends when it loads
//! it, so a chat taken up again shows its conversation before the agent starts.

use sikemux_fold::UpdateKind;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// A transcript larger than this is left for the agent to replay once it starts.
const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;

/// How the agents' tools read in a chat: running a command, reading, editing, searching.
fn tool_kind(name: &str) -> &'static str {
    match name {
        "bash" | "shell" => "execute",
        "read" => "read",
        "write" | "edit" | "patch" | "multiedit" => "edit",
        "grep" | "glob" | "find" | "list" | "ls" | "search" => "search",
        "webfetch" | "fetch" | "web_search" | "websearch" => "fetch",
        _ => "other",
    }
}

fn chunk(kind: UpdateKind, message_id: &str, content: Value) -> Value {
    json!({ "sessionUpdate": kind, "messageId": message_id, "content": content })
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// Each content block as a tool call's result shows it.
fn wrapped(blocks: &Value) -> Value {
    let blocks = blocks.as_array().cloned().unwrap_or_default();
    Value::Array(
        blocks
            .into_iter()
            .map(|block| json!({ "type": "content", "content": block }))
            .collect(),
    )
}

/// An OMP transcript's active branch: from its last entry back through each one's parent.
pub(super) fn omp(path: &Path, cwd: &Path) -> Result<Vec<Value>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_TRANSCRIPT_BYTES {
        return Err("the transcript is too long to show before the agent starts".into());
    }
    let entries: Vec<Value> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect();
    let by_id: HashMap<&str, &Value> = entries
        .iter()
        .filter_map(|entry| Some((entry.get("id")?.as_str()?, entry)))
        .collect();
    let mut branch = Vec::new();
    let mut at = entries.iter().rev().find(|entry| entry.get("id").is_some());
    while let Some(entry) = at {
        branch.push(entry);
        at = entry["parentId"]
            .as_str()
            .and_then(|parent| by_id.get(parent).copied());
        if branch.len() > entries.len() {
            break;
        }
    }
    branch.reverse();
    let mut updates = Vec::new();
    for entry in branch {
        let message = &entry["message"];
        let id = entry["id"].as_str().unwrap_or_default();
        match message["role"].as_str() {
            Some("user") => omp_user(message, id, &mut updates),
            Some("assistant") => omp_assistant(message, id, cwd, &mut updates),
            Some("toolResult") => updates.push(omp_result(message)),
            _ => {}
        }
    }
    Ok(updates)
}

fn omp_user(message: &Value, id: &str, updates: &mut Vec<Value>) {
    if let Some(said) = message["content"].as_str() {
        updates.push(chunk(UpdateKind::UserMessageChunk, id, text(said)));
        return;
    }
    for block in message["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => updates.push(chunk(
                UpdateKind::UserMessageChunk,
                id,
                text(block["text"].as_str().unwrap_or_default()),
            )),
            Some("image") => updates.push(chunk(
                UpdateKind::UserMessageChunk,
                id,
                json!({ "type": "image", "data": block["data"], "mimeType": block["mimeType"] }),
            )),
            _ => {}
        }
    }
}

fn omp_assistant(message: &Value, id: &str, cwd: &Path, updates: &mut Vec<Value>) {
    for block in message["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("thinking") => updates.push(chunk(
                UpdateKind::AgentThoughtChunk,
                id,
                text(block["thinking"].as_str().unwrap_or_default()),
            )),
            Some("text") => updates.push(chunk(
                UpdateKind::AgentMessageChunk,
                id,
                text(block["text"].as_str().unwrap_or_default()),
            )),
            Some("toolCall") => updates.push(omp_call(block, cwd)),
            _ => {}
        }
    }
}

/// A tool call titled as OMP titles it live: by what the model said it was doing, else
/// `$ command`, `read: path` or the tool's name.
fn omp_call(block: &Value, cwd: &Path) -> Value {
    let name = block["name"].as_str().unwrap_or_default();
    let input = &block["arguments"];
    let path = input["path"].as_str();
    let to_device = path.is_some_and(|path| path.starts_with("xd://"));
    let intent = block["intent"]
        .as_str()
        .or(input["i"].as_str())
        .filter(|intent| !intent.trim().is_empty());
    let (title, kind) = match (name, path) {
        (_, Some(path)) if to_device => (path.to_owned(), "execute"),
        (_, _) if intent.is_some() => (
            intent.unwrap_or_default().to_owned(),
            if name == "bash" {
                "execute"
            } else {
                tool_kind(name)
            },
        ),
        ("bash", _) => (
            format!("$ {}", input["command"].as_str().unwrap_or_default()),
            "execute",
        ),
        (_, Some(path)) => (format!("{name}: {path}"), tool_kind(name)),
        _ => (name.to_owned(), tool_kind(name)),
    };
    let mut call = json!({
        "sessionUpdate": UpdateKind::ToolCall,
        "toolCallId": block["id"],
        "title": title,
        "kind": kind,
        "status": "pending",
        "rawInput": input,
    });
    if let Some(path) = path.filter(|_| !to_device) {
        call["locations"] = json!([{ "path": cwd.join(path) }]);
    }
    call
}

fn omp_result(message: &Value) -> Value {
    let failed = message["isError"].as_bool().unwrap_or(false);
    json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": message["toolCallId"],
        "status": if failed { "failed" } else { "completed" },
        "rawOutput": { "content": message["content"], "details": message["details"] },
        "content": wrapped(&message["content"]),
    })
}

/// An OpenCode session's messages from whichever of its databases holds it.
pub(super) fn opencode(
    databases: &[impl AsRef<Path>],
    session_id: &str,
) -> Result<Vec<Value>, String> {
    for db in databases {
        let messages = opencode_messages(db.as_ref(), session_id)?;
        if !messages.is_empty() {
            return Ok(messages
                .iter()
                .flat_map(|(id, kind, data)| opencode_message(id, kind, data))
                .collect());
        }
    }
    Ok(Vec::new())
}

/// The id, type and contents of each message OpenCode 2 kept of the session, in order.
pub(super) fn opencode_messages(
    db: &Path,
    session_id: &str,
) -> Result<Vec<(String, String, Value)>, String> {
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Ok(Vec::new());
    };
    let read = || -> rusqlite::Result<Vec<(String, String, String)>> {
        let mut statement = conn.prepare(
            "SELECT id, type, data FROM session_message WHERE session_id = ?1 ORDER BY seq",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.map_while(Result::ok).collect())
    };
    Ok(read()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|(id, kind, data)| Some((id, kind, serde_json::from_str(&data).ok()?)))
        .collect())
}

fn opencode_message(id: &str, kind: &str, data: &Value) -> Vec<Value> {
    match kind {
        "user" => data["text"]
            .as_str()
            .map(|said| chunk(UpdateKind::UserMessageChunk, id, text(said)))
            .into_iter()
            .collect(),
        "assistant" => (data["content"].as_array().into_iter().flatten())
            .flat_map(|block| opencode_block(id, block))
            .collect(),
        _ => Vec::new(),
    }
}

fn opencode_block(id: &str, block: &Value) -> Vec<Value> {
    let said = block["text"].as_str().unwrap_or_default();
    match block["type"].as_str() {
        Some("text") => vec![chunk(UpdateKind::AgentMessageChunk, id, text(said))],
        Some("reasoning") => vec![chunk(UpdateKind::AgentThoughtChunk, id, text(said))],
        Some("tool") => opencode_tool(block),
        _ => Vec::new(),
    }
}

/// A tool call titled as OpenCode titles it live: a command by itself, else the tool's name.
fn opencode_tool(block: &Value) -> Vec<Value> {
    let name = block["name"].as_str().unwrap_or_default();
    let state = &block["state"];
    let input = &state["input"];
    let title = match name {
        "bash" | "shell" => input["command"].as_str().unwrap_or(name),
        _ => name,
    };
    let mut call = json!({
        "sessionUpdate": UpdateKind::ToolCall,
        "toolCallId": block["id"],
        "title": title,
        "kind": tool_kind(name),
        "status": "pending",
        "rawInput": input,
    });
    if let Some(path) = input["filePath"].as_str().or(input["path"].as_str()) {
        call["locations"] = json!([{ "path": path }]);
    }
    let (status, said, raw) = match state["status"].as_str() {
        Some("completed") => (
            "completed",
            (state["content"].as_array().into_iter().flatten())
                .filter(|block| block["type"] == "text")
                .cloned()
                .collect(),
            json!({ "metadata": state["metadata"] }),
        ),
        Some("error") => {
            let message = state["error"]["message"].as_str().unwrap_or_default();
            (
                "failed",
                vec![text(message)],
                json!({ "error": state["error"] }),
            )
        }
        _ => return vec![call],
    };
    let result = json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": block["id"],
        "status": status,
        "rawOutput": raw,
        "content": wrapped(&Value::Array(said)),
    });
    vec![call, result]
}
