//! OpenCode's tool calls as the chat's: titled, kinded and placed as OpenCode's own ACP front
//! does, so a chat reads alike whichever way it runs.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_fold::UpdateKind;

/// A call OpenCode is making, as far as it has said.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Call {
    pub name: String,
    pub input: Value,
    pub metadata: Value,
}

/// The tool's own name, with the names OpenCode 1 used read as OpenCode 2's.
pub fn canonical(name: &str) -> String {
    let name = name.to_lowercase();
    match name.as_str() {
        "bash" => "shell".to_owned(),
        "task" => "subagent".to_owned(),
        "apply_patch" => "patch".to_owned(),
        _ => name,
    }
}

fn kind(name: &str) -> &'static str {
    match canonical(name).as_str() {
        "shell" => "execute",
        "webfetch" => "fetch",
        "edit" | "patch" | "write" => "edit",
        "grep" | "glob" | "context" => "search",
        "read" => "read",
        "subagent" => "think",
        _ => "other",
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|text| !text.is_empty())
}

pub fn title(name: &str, input: &Value) -> String {
    if canonical(name) == "shell" {
        if let Some(command) = text(input, "command").or(text(input, "cmd")) {
            return command.to_owned();
        }
    }
    name.to_owned()
}

fn absolute(path: &str, cwd: &Path) -> String {
    cwd.join(path).to_string_lossy().into_owned()
}

/// The files and folders the call touches.
pub fn locations(name: &str, input: &Value, cwd: &Path) -> Vec<Value> {
    let named = |key: &str| text(input, key).map(str::to_owned);
    let paths: Vec<String> = match canonical(name).as_str() {
        "shell" => vec![named("workdir")
            .or_else(|| named("cwd"))
            .unwrap_or_else(|| cwd.to_string_lossy().into_owned())],
        "read" | "edit" | "write" => named("path")
            .or_else(|| named("filePath"))
            .into_iter()
            .collect(),
        "external_directory" => named("filepath").into_iter().collect(),
        "grep" | "glob" | "context" => named("path").into_iter().collect(),
        _ => Vec::new(),
    };
    let mut seen = Vec::new();
    for path in paths {
        let path = absolute(&path, cwd);
        if !seen.contains(&path) {
            seen.push(path);
        }
    }
    seen.into_iter()
        .map(|path| json!({ "path": path }))
        .collect()
}

/// What the call was given, with the folder a command runs in when it names none.
fn raw_input(name: &str, input: &Value, cwd: &Path) -> Value {
    let mut input = input.clone();
    if canonical(name) == "shell"
        && text(&input, "cwd").is_none()
        && text(&input, "workdir").is_none()
    {
        if let Some(fields) = input.as_object_mut() {
            fields.insert("cwd".into(), json!(cwd));
        }
    }
    input
}

/// A call OpenCode has begun to write.
pub fn pending(id: &str, name: &str, cwd: &Path) -> Value {
    json!({
        "sessionUpdate": UpdateKind::ToolCall,
        "toolCallId": id,
        "name": name,
        "title": title(name, &Value::Null),
        "kind": kind(name),
        "status": "pending",
        "locations": locations(name, &Value::Null, cwd),
        "rawInput": {},
    })
}

/// A call whose input OpenCode has written, now running.
pub fn running(id: &str, call: &Call, cwd: &Path) -> Value {
    json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": id,
        "status": "in_progress",
        "kind": kind(&call.name),
        "title": title(&call.name, &call.input),
        "locations": locations(&call.name, &call.input, cwd),
        "rawInput": raw_input(&call.name, &call.input, cwd),
    })
}

/// What a tool said, as the chat's content blocks: its text, and the pictures it showed.
fn content(said: &Value) -> Vec<Value> {
    let mut texts = Vec::new();
    let mut pictures = Vec::new();
    for part in said.as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => texts.push(json!({
                "type": "content",
                "content": { "type": "text", "text": part["text"] },
            })),
            _ => {
                let Some((mime, data)) = part["uri"].as_str().and_then(data_picture) else {
                    continue;
                };
                pictures.push(json!({
                    "type": "content",
                    "content": { "type": "image", "mimeType": mime, "data": data },
                }));
            }
        }
    }
    texts.extend(pictures);
    texts
}

/// A `data:image/…;base64,…` address as its type and its data.
fn data_picture(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let mime = head.split(';').next()?;
    (mime.starts_with("image/") && head.ends_with(";base64")).then_some((mime, data))
}

fn raw_output(metadata: &Value) -> Value {
    if metadata.is_null() {
        json!({})
    } else {
        json!({ "metadata": metadata })
    }
}

pub fn completed(id: &str, call: &Call, said: &Value, metadata: &Value, cwd: &Path) -> Value {
    json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": id,
        "status": "completed",
        "locations": locations(&call.name, &call.input, cwd),
        "content": content(said),
        "rawOutput": raw_output(metadata),
    })
}

pub fn failed(
    id: &str,
    call: &Call,
    said: &Value,
    metadata: &Value,
    error: &str,
    cwd: &Path,
) -> Value {
    let mut blocks = content(said);
    blocks.push(json!({ "type": "content", "content": { "type": "text", "text": error } }));
    let mut raw = raw_output(metadata);
    if let Some(fields) = raw.as_object_mut() {
        fields.insert("error".to_owned(), json!(error));
    }
    json!({
        "sessionUpdate": UpdateKind::ToolCallUpdate,
        "toolCallId": id,
        "status": "failed",
        "kind": kind(&call.name),
        "title": title(&call.name, &call.input),
        "locations": locations(&call.name, &call.input, cwd),
        "rawInput": raw_input(&call.name, &call.input, cwd),
        "content": blocks,
        "rawOutput": raw,
    })
}

/// A permission OpenCode asks for, as the chat puts it to the person: the call it is for,
/// with the choices OpenCode takes.
pub fn permission(session: &str, asked: &Value, call: Option<&Call>, cwd: &Path) -> Value {
    let name = call.map_or_else(
        || text(asked, "action").unwrap_or("tool").to_owned(),
        |call| call.name.clone(),
    );
    let input = call.map_or_else(|| asked["metadata"].clone(), |call| call.input.clone());
    let id = asked
        .get("source")
        .and_then(|source| source.get("id"))
        .and_then(Value::as_str)
        .or(asked.get("id").and_then(Value::as_str))
        .unwrap_or_default();
    let mut locations = locations(&name, &input, cwd);
    if locations.is_empty()
        && matches!(
            text(asked, "action"),
            Some("read" | "edit" | "external_directory")
        )
    {
        for resource in asked["resources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let path = resource.strip_suffix("/*").unwrap_or(resource);
            if !path.is_empty() && !path.contains(['*', '?']) {
                locations.push(json!({ "path": absolute(path, cwd) }));
            }
        }
    }
    let title = permission_title(&name, &input).unwrap_or_else(|| title(&name, &input));
    let mut tool_call = Map::new();
    tool_call.insert("toolCallId".into(), json!(id));
    tool_call.insert("title".into(), json!(title));
    tool_call.insert("kind".into(), json!(kind(&name)));
    tool_call.insert("status".into(), json!("pending"));
    tool_call.insert("locations".into(), json!(locations));
    if call.is_some() {
        tool_call.insert("name".into(), json!(name));
        tool_call.insert("rawInput".into(), raw_input(&name, &input, cwd));
    }
    json!({
        "sessionId": session,
        "toolCall": tool_call,
        "options": [
            { "optionId": "once", "kind": "allow_once", "name": "Allow once" },
            { "optionId": "always", "kind": "allow_always", "name": "Always allow" },
            { "optionId": "reject", "kind": "reject_once", "name": "Reject" },
        ],
    })
}

fn permission_title(name: &str, input: &Value) -> Option<String> {
    let found = match canonical(name).as_str() {
        "external_directory" => text(input, "description")
            .or(text(input, "command"))
            .or(text(input, "parentDir")),
        "webfetch" => text(input, "url"),
        "websearch" => text(input, "query"),
        "grep" | "glob" => text(input, "pattern"),
        "read" | "edit" | "write" | "patch" => text(input, "path").or(text(input, "filePath")),
        _ => None,
    };
    found.map(str::to_owned)
}

pub fn is_todo(name: &str) -> bool {
    canonical(name) == "todowrite"
}

/// The chat's plan from OpenCode's todo list.
pub fn todo_plan(input: &Value) -> Value {
    let entries = input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| {
            sikemux_fold::update::plan_entry(
                todo["content"].as_str().unwrap_or_default(),
                todo["status"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    sikemux_fold::update::plan(entries)
}

/// A form OpenCode puts to the person, as the ask tool's questions: each field the question
/// in its description, its title the header, and its options the choices.
pub fn form_questions(form: &Value) -> Value {
    let questions: Vec<Value> = form["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|field| field["hidden"] != true)
        .map(|field| {
            let title = text(field, "title");
            let options: Vec<Value> = field["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| {
                    json!({
                        "label": text(option, "label").or(text(option, "value")).unwrap_or_default(),
                        "description": option["description"],
                    })
                })
                .collect();
            json!({
                "question": text(field, "description").or(title).or(text(form, "title")).unwrap_or_default(),
                "header": title,
                "options": options,
                "multiSelect": field["type"] == "multiselect",
            })
        })
        .collect();
    json!({ "questions": questions })
}
