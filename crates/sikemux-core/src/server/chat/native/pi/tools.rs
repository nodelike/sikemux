//! Pi's tool calls as the session updates the other agents send for the same work.

use serde_json::{json, Value};
use sikemux_fold::update::{self, diff_block, terminal_call, terminal_done, tool_update};

use super::super::json::text;
use super::super::words::{absolute, file_name, spoken};

fn path_of(args: &Value) -> Option<&str> {
    text(args, "path").or_else(|| text(args, "file_path"))
}

/// A tool Sikemux's own tools, or an MCP server's, are named by, as server and tool.
fn mcp_name(tool: &str) -> Option<(&str, &str)> {
    tool.strip_prefix("mcp__")?.split_once("__")
}

pub(super) fn is_bash(tool: &str) -> bool {
    matches!(tool, "bash" | "powershell")
}

fn edits(args: &Value, path: &str) -> Vec<Value> {
    let one = |edit: &Value| {
        let new = text(edit, "newText")?;
        Some(diff_block(path, Some(text(edit, "oldText")?), new))
    };
    match args["edits"].as_array() {
        Some(edits) => edits.iter().filter_map(one).collect(),
        None => one(args).into_iter().collect(),
    }
}

pub(super) fn call(id: &str, tool: &str, args: &Value, cwd: &str, status: &str) -> Value {
    let path = path_of(args);
    let base = |kind: &str, title: String| {
        let mut call = update::tool_call(id, kind, &title, status);
        call["rawInput"] = args.clone();
        if let Some(path) = path {
            call["locations"] = json!([{ "path": path }]);
        }
        call
    };
    if let Some((server, name)) = mcp_name(tool) {
        return update::mcp_call(id, status, server, name, args);
    }
    match tool {
        "bash" | "powershell" => {
            let command = text(args, "command").unwrap_or_default();
            terminal_call(
                id,
                status,
                command,
                json!({ "command": command, "cwd": cwd }),
            )
        }
        "read" => base("read", update::read_title(path.unwrap_or_default())),
        "ls" => base("read", format!("List {}", path.unwrap_or("."))),
        "grep" | "find" => base("search", update::search_title(text(args, "pattern"), path)),
        "web_search" => base("search", update::web_search_title(text(args, "query"))),
        "fetch_content" => base(
            "fetch",
            text(args, "url").map_or_else(|| "Fetch".to_owned(), |url| format!("Fetch {url}")),
        ),
        "write" | "edit" => {
            let mut call = base(
                "edit",
                path.map_or_else(
                    || update::EDITING_FILES.to_owned(),
                    |path| format!("Edit {}", file_name(path)),
                ),
            );
            let blocks = match path.map(|path| absolute(path, cwd)) {
                Some(path) if tool == "write" => text(args, "content")
                    .map(|new| vec![diff_block(&path, None, new)])
                    .unwrap_or_default(),
                Some(path) => edits(args, &path),
                None => Vec::new(),
            };
            if !blocks.is_empty() {
                call["content"] = Value::Array(blocks);
            }
            call
        }
        "Agent" => base(
            "other",
            text(args, "description").map_or_else(|| spoken(tool), str::to_owned),
        ),
        _ => base("other", spoken(tool)),
    }
}

/// The words a tool's result holds.
pub(super) fn output(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| text(block, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn blocks(result: &Value) -> Vec<Value> {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| match block["type"].as_str()? {
            "text" => text(block, "text").map(update::text_content),
            "image" => Some(update::content(block.clone())),
            _ => None,
        })
        .collect()
}

pub(super) fn done(id: &str, tool: &str, result: &Value, failed: bool) -> Value {
    let status = if failed { "failed" } else { "completed" };
    let said = output(result);
    if is_bash(tool) {
        return terminal_done(id, status, &said);
    }
    let mut update = tool_update(id, status);
    if mcp_name(tool).is_some() {
        update["rawOutput"] = json!({
            "result": (!failed).then(|| json!({ "content": result["content"] })),
            "error": failed.then(|| json!({ "message": said })),
        });
        return update;
    }
    if failed || !matches!(tool, "edit" | "write") {
        let blocks = blocks(result);
        if !blocks.is_empty() {
            update["content"] = Value::Array(blocks);
        }
        update["rawOutput"] = json!(said);
    }
    update
}

/// What a running command has printed so far.
pub(super) fn partial(id: &str, tool: &str, result: &Value) -> Option<Value> {
    if !is_bash(tool) {
        return None;
    }
    let said = output(result);
    (!said.is_empty()).then(|| terminal_done(id, "in_progress", &said))
}
