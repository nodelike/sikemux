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
        let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let parts = opencode_parts(&conn, session_id).map_err(|error| error.to_string())?;
        if !parts.is_empty() {
            return Ok(parts
                .iter()
                .flat_map(|(id, role, part)| opencode_part(id, role, part))
                .collect());
        }
    }
    Ok(Vec::new())
}

fn opencode_parts(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<(String, String, Value)>> {
    let mut statement = conn.prepare(
        "SELECT m.id, json_extract(m.data, '$.role'), p.data FROM part p \
         JOIN message m ON m.id = p.message_id WHERE p.session_id = ?1 \
         ORDER BY m.time_created, m.id, p.time_created, p.id",
    )?;
    let rows = statement.query_map([session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, String>(2)?,
        ))
    })?;
    Ok(rows
        .map_while(Result::ok)
        .filter_map(|(id, role, data)| Some((id, role, serde_json::from_str(&data).ok()?)))
        .collect())
}

fn opencode_part(id: &str, role: &str, part: &Value) -> Vec<Value> {
    let said = part["text"].as_str().unwrap_or_default();
    match (part["type"].as_str(), role) {
        (Some("text"), _) if part["synthetic"].as_bool() == Some(true) => Vec::new(),
        (Some("text"), "user") => vec![chunk(UpdateKind::UserMessageChunk, id, text(said))],
        (Some("text"), _) => vec![chunk(UpdateKind::AgentMessageChunk, id, text(said))],
        (Some("reasoning"), _) => vec![chunk(UpdateKind::AgentThoughtChunk, id, text(said))],
        (Some("tool"), _) => opencode_tool(part),
        _ => Vec::new(),
    }
}

/// A tool call titled as OpenCode titles it live: a command by itself, else the tool's name.
fn opencode_tool(part: &Value) -> Vec<Value> {
    let name = part["tool"].as_str().unwrap_or_default();
    let state = &part["state"];
    let input = &state["input"];
    let title = match name {
        "bash" => input["command"].as_str().unwrap_or(name),
        _ => state["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .unwrap_or(name),
    };
    let mut call = json!({
        "sessionUpdate": UpdateKind::ToolCall,
        "toolCallId": part["callID"],
        "title": title,
        "kind": tool_kind(name),
        "status": "pending",
        "rawInput": input,
    });
    if let Some(path) = input["filePath"].as_str().or(input["path"].as_str()) {
        call["locations"] = json!([{ "path": path }]);
    }
    let status = match state["status"].as_str() {
        Some("completed") => "completed",
        Some("error") => "failed",
        _ => return vec![call],
    };
    let output = state["output"]
        .as_str()
        .or(state["error"].as_str())
        .unwrap_or_default();
    let result = json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": part["callID"],
        "status": status,
        "rawOutput": { "output": output },
        "content": wrapped(&json!([text(output)])),
    });
    vec![call, result]
}
