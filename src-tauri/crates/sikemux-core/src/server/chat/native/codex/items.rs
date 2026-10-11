//! What Codex's thread items look like in the chat: the ACP session updates
//! Codex's ACP adapter sends for each, so the app reads them alike.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use super::patch;

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn status(item: &Value) -> &'static str {
    match text(item, "status") {
        "completed" => "completed",
        "failed" | "declined" | "interrupted" => "failed",
        _ => "in_progress",
    }
}

/// Settled: completed, or failed for anything else.
fn ended(item: &Value) -> &'static str {
    if item["status"] == "completed" {
        "completed"
    } else {
        "failed"
    }
}

/// A shell command without the `zsh -lc '…'` Codex wraps it in.
pub(super) fn strip_shell(command: &str) -> &str {
    let rest = without_shell(command).unwrap_or(command);
    rest.strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .unwrap_or(rest)
}

fn without_shell(command: &str) -> Option<&str> {
    let rest = command.strip_prefix("/bin/").unwrap_or(command);
    let shell = ["bash", "zsh", "sh"]
        .iter()
        .find(|shell| rest.starts_with(**shell))?;
    let after = &rest[shell.len()..];
    let rest = after.trim_start();
    if rest.len() == after.len() {
        return None;
    }
    if let Some(flags) = rest.strip_prefix('-') {
        let end = flags
            .find(|character: char| character != 'l' && character != 'c')
            .unwrap_or(flags.len());
        let after_flags = &flags[end..];
        let spaced = after_flags.trim_start();
        if end > 0 && spaced.len() < after_flags.len() {
            return Some(spaced);
        }
    }
    Some(rest)
}

pub(super) fn content(block: Value) -> Value {
    json!({ "type": "content", "content": block })
}

pub(super) fn agent_text(text: &str, message_id: Option<&str>, phase: Option<&Value>) -> Value {
    let mut update = json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": text },
    });
    if let Some(id) = message_id {
        update["messageId"] = json!(id);
    }
    if let Some(phase) = phase.filter(|phase| !phase.is_null()) {
        update["_meta"] = json!({ "codex": { "phase": phase } });
    }
    update
}

pub(super) fn thought(text: &str, message_id: Option<&str>) -> Value {
    let mut update = json!({
        "sessionUpdate": "agent_thought_chunk",
        "content": { "type": "text", "text": text },
    });
    if let Some(id) = message_id {
        update["messageId"] = json!(id);
    }
    update
}

fn search_title(query: Option<&str>, path: Option<&str>) -> String {
    match (
        query.filter(|q| !q.is_empty()),
        path.filter(|p| !p.is_empty()),
    ) {
        (Some(query), Some(path)) => format!("Search for '{query}' in {path}"),
        (Some(query), None) => format!("Search for '{query}'"),
        (None, Some(path)) => format!("Search in '{path}'"),
        (None, None) => "Search".to_owned(),
    }
}

/// Whether the command shows as a terminal rather than as a read, list or search.
fn runs_in_terminal(item: &Value) -> bool {
    match item["commandActions"].as_array().map(Vec::as_slice) {
        Some([action]) => action["type"] == "unknown",
        _ => true,
    }
}

fn terminal_call(id: &str, status: &str, title: &str, command: &Value, cwd: &Value) -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "kind": "execute",
        "title": title,
        "status": status,
        "content": [{ "type": "terminal", "terminalId": id }],
        "rawInput": { "command": command, "cwd": cwd },
        "_meta": { "terminal_info": { "cwd": cwd, "terminal_id": id } },
    })
}

fn command_call(item: &Value) -> Value {
    let id = text(item, "id");
    let status = status(item);
    let cwd = &item["cwd"];
    if let Some([action]) = item["commandActions"].as_array().map(Vec::as_slice) {
        let path = action["path"].as_str();
        let base = |kind: &str, title: String| {
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "status": status,
                "kind": kind,
                "title": title,
            })
        };
        match text(action, "type") {
            "read" => {
                let mut call = base("read", format!("Read file '{}'", path.unwrap_or_default()));
                call["locations"] = json!([{ "path": action["path"] }]);
                return call;
            }
            "search" => return base("search", search_title(action["query"].as_str(), path)),
            "listFiles" => {
                let title = path.filter(|path| !path.is_empty()).map_or_else(
                    || "List files".to_owned(),
                    |path| format!("List files in '{path}'"),
                );
                return base("read", title);
            }
            _ => {
                let command = text(action, "command");
                return terminal_call(id, status, strip_shell(command), &action["command"], cwd);
            }
        }
    }
    let command = text(item, "command");
    terminal_call(id, status, strip_shell(command), &item["command"], cwd)
}

fn command_done(item: &Value) -> Option<Value> {
    if item["status"] == "inProgress" {
        return None;
    }
    let id = text(item, "id");
    let mut update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": id,
        "status": ended(item),
        "rawOutput": {
            "formatted_output": item["aggregatedOutput"].as_str().unwrap_or_default(),
            "exit_code": item["exitCode"],
        },
    });
    if runs_in_terminal(item) {
        update["_meta"] = json!({
            "terminal_exit": { "exit_code": item["exitCode"], "signal": null, "terminal_id": id },
        });
    }
    Some(update)
}

pub(super) fn read_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn diff_block(path: &str, old: Option<String>, new: String, kind: &str) -> Value {
    json!({ "type": "diff", "path": path, "oldText": old, "newText": new, "_meta": { "kind": kind } })
}

/// One change of an edit as the file before and after. An update is worked out
/// against the file as it is now, before or after the edit; when it fits
/// neither, the change shows no diff.
fn change_block(change: &Value, read: &impl Fn(&str) -> Option<String>) -> Option<Value> {
    let path = text(change, "path");
    let diff = text(change, "diff");
    match change["kind"]["type"].as_str()? {
        "add" => Some(diff_block(path, None, diff.to_owned(), "add")),
        "delete" => Some(diff_block(
            path,
            Some(diff.to_owned()),
            String::new(),
            "delete",
        )),
        "update" => {
            let moved_to = change["kind"]["move_path"].as_str();
            if let Some(current) = read(path) {
                if let Some(patched) = patch::apply(&current, diff) {
                    return Some(diff_block(
                        moved_to.unwrap_or(path),
                        Some(current),
                        patched,
                        "update",
                    ));
                }
                let original = patch::revert(&current, diff)?;
                return Some(diff_block(path, Some(original), current, "update"));
            }
            let moved_to = moved_to?;
            let current = read(moved_to)?;
            let original = patch::revert(&current, diff)?;
            Some(diff_block(moved_to, Some(original), current, "update"))
        }
        _ => None,
    }
}

fn edit_call(item: &Value, status: &str, read: &impl Fn(&str) -> Option<String>) -> Value {
    let blocks: Vec<Value> = item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|change| change_block(change, read))
        .collect();
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "title": "Editing files",
        "kind": "edit",
        "status": status,
        "content": blocks,
    })
}

fn mcp_input(item: &Value) -> Value {
    json!({ "server": item["server"], "tool": item["tool"], "arguments": item["arguments"] })
}

fn mcp_output(item: &Value) -> Option<Value> {
    (!item["result"].is_null() || !item["error"].is_null())
        .then(|| json!({ "result": item["result"], "error": item["error"] }))
}

/// Codex's adapter titles these `mcp.server.tool`. The chat names a tool server
/// call `mcp__server__tool` and draws it as one, with Sikemux's own tools
/// getting their own rows, so it is titled that way here.
fn mcp_call(item: &Value) -> Value {
    let mut call = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "kind": "other",
        "title": format!("mcp__{}__{}", text(item, "server"), text(item, "tool")),
        "status": status(item),
        "rawInput": mcp_input(item),
        "_meta": { "is_mcp_tool_call": true },
    });
    if let Some(output) = mcp_output(item) {
        call["rawOutput"] = output;
    }
    call
}

fn mcp_done(item: &Value) -> Value {
    let mut update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": item["id"],
        "status": ended(item),
        "rawInput": mcp_input(item),
    });
    if let Some(output) = mcp_output(item) {
        update["rawOutput"] = output;
    }
    update
}

fn dynamic_call(item: &Value) -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "kind": "execute",
        "title": item["tool"],
        "status": status(item),
        "rawInput": { "arguments": item["arguments"] },
    })
}

fn status_update(item: &Value) -> Value {
    json!({ "sessionUpdate": "tool_call_update", "toolCallId": item["id"], "status": ended(item) })
}

fn web_search_title(item: &Value) -> String {
    let query = item["query"].as_str().filter(|query| !query.is_empty());
    let action = &item["action"];
    let searched = || {
        query.map_or_else(
            || "Web search".to_owned(),
            |query| format!("Web search: {query}"),
        )
    };
    match action["type"].as_str() {
        None => searched(),
        Some("search") => {
            let queries: Vec<&str> = action["queries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|query| !query.is_empty())
                .collect();
            let joined = queries.join(", ");
            let query = action["query"]
                .as_str()
                .or((!queries.is_empty()).then_some(joined.as_str()))
                .or(query);
            query.map_or_else(
                || "Web search".to_owned(),
                |query| format!("Web search: {query}"),
            )
        }
        Some("open_page" | "openPage") => action["url"]
            .as_str()
            .map_or_else(|| "Open page".to_owned(), |url| format!("Open page: {url}")),
        Some("find_in_page" | "findInPage") => {
            let pattern = action["pattern"]
                .as_str()
                .map(|pattern| format!(" for '{pattern}'"))
                .unwrap_or_default();
            let url = action["url"]
                .as_str()
                .map(|url| format!(" in {url}"))
                .unwrap_or_default();
            format!("Find in page{pattern}{url}").trim().to_owned()
        }
        Some(_) => "Web search".to_owned(),
    }
}

fn web_search_input(item: &Value) -> Value {
    json!({ "type": item["type"], "id": item["id"], "query": item["query"], "action": item["action"] })
}

fn image_view(item: &Value) -> Value {
    let path = text(item, "path");
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "kind": "read",
        "title": format!("View Image {path}"),
        "status": "completed",
        "content": [content(json!({ "type": "resource_link", "name": path, "uri": path }))],
        "locations": [{ "path": path }],
        "rawInput": { "path": path },
    })
}

fn image_status(item: &Value, terminal: bool) -> &'static str {
    match text(item, "status") {
        "failed" => "failed",
        "completed" => "completed",
        "generating" | "in_progress" | "inProgress" | "incomplete" if !terminal => "in_progress",
        _ => "completed",
    }
}

fn image_content(item: &Value) -> Vec<Value> {
    let mut blocks = Vec::new();
    if let Some(prompt) = item["revisedPrompt"]
        .as_str()
        .filter(|prompt| !prompt.trim().is_empty())
    {
        blocks.push(content(
            json!({ "type": "text", "text": format!("Revised prompt: {prompt}") }),
        ));
    }
    let result = text(item, "result");
    if !result.trim().is_empty() {
        let mut image = json!({ "type": "image", "data": result, "mimeType": "image/png" });
        if let Some(saved) = item["savedPath"]
            .as_str()
            .filter(|saved| !saved.trim().is_empty())
        {
            image["uri"] = json!(saved);
        }
        blocks.push(content(image));
    }
    blocks
}

fn image_output(item: &Value) -> Value {
    let mut output = json!({
        "status": item["status"],
        "revisedPrompt": item["revisedPrompt"],
        "result": item["result"],
    });
    if let Some(saved) = item.get("savedPath") {
        output["savedPath"] = saved.clone();
    }
    output
}

fn image_generation(item: &Value, terminal: bool) -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "kind": "other",
        "title": "Image generation",
        "status": image_status(item, terminal),
        "content": image_content(item),
        "rawOutput": image_output(item),
    })
}

fn compaction(item: &Value, session_update: &str, status: &str) -> Value {
    let mut update = json!({
        "sessionUpdate": session_update,
        "toolCallId": item["id"],
        "title": "Compact conversation",
        "status": status,
        "_meta": { "contextCompaction": { "version": 1 } },
    });
    if session_update == "tool_call" {
        update["kind"] = json!("think");
    }
    update
}

fn collaboration(item: &Value, session_update: &str) -> Value {
    json!({
        "sessionUpdate": session_update,
        "toolCallId": item["id"],
        "kind": "other",
        "title": item["tool"],
        "status": status(item),
        "rawInput": {
            "prompt": item["prompt"],
            "senderThreadId": item["senderThreadId"],
            "receiverThreadIds": item["receiverThreadIds"],
            "agentsStates": item["agentsStates"],
            "model": item["model"],
            "reasoningEffort": item["reasoningEffort"],
            "status": item["status"],
        },
        "_meta": { "codex": { "collaboration": {
            "tool": item["tool"],
            "senderThreadId": item["senderThreadId"],
            "receiverThreadIds": item["receiverThreadIds"],
        } } },
    })
}

fn plan_text(item: &Value) -> Option<Value> {
    let text = text(item, "text");
    (!text.is_empty()).then(|| agent_text(text, item["id"].as_str(), Some(&json!("final_answer"))))
}

fn reasoning_parts(item: &Value) -> Vec<&str> {
    let summary = item["summary"].as_array().filter(|parts| !parts.is_empty());
    summary
        .or_else(|| item["content"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// A user message's inputs as the chat shows them on replay.
pub(super) fn user_blocks(item: &Value) -> Vec<Value> {
    let link = |name: Option<&str>, uri: &str| match name.filter(|name| !name.is_empty()) {
        Some(name) => format!("[@{name}]({uri})"),
        None => match uri.strip_prefix("file://") {
            Some(path) => format!("[@{}]({uri})", path.rsplit('/').next().unwrap_or(path)),
            None => uri.to_owned(),
        },
    };
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|input| {
            let said = match text(input, "type") {
                "text" => Some(text(input, "text").to_owned()).filter(|said| !said.is_empty()),
                "image" => Some(link(Some("image"), text(input, "url"))),
                "localImage" => {
                    let path = text(input, "path");
                    let uri = if path.starts_with("file://") {
                        path.to_owned()
                    } else {
                        format!("file://{path}")
                    };
                    Some(link(None, &uri))
                }
                "skill" => Some(format!(
                    "skill:{} ({})",
                    text(input, "name"),
                    text(input, "path")
                )),
                _ => None,
            }?;
            Some(json!({ "type": "text", "text": said }))
        })
        .collect()
}

/// A finished item as it is replayed from the thread's history.
pub(super) fn replayed(item: &Value, read: &impl Fn(&str) -> Option<String>) -> Vec<Value> {
    match text(item, "type") {
        "agentMessage" => vec![agent_text(
            text(item, "text"),
            item["id"].as_str(),
            Some(&item["phase"]),
        )],
        "reasoning" => reasoning_parts(item)
            .into_iter()
            .map(|part| thought(part, item["id"].as_str()))
            .collect(),
        "fileChange" => vec![edit_call(item, status(item), read)],
        "commandExecution" => std::iter::once(command_call(item))
            .chain(command_done(item))
            .collect(),
        "mcpToolCall" => vec![mcp_call(item)],
        "dynamicToolCall" => vec![dynamic_call(item)],
        "webSearch" => vec![json!({
            "sessionUpdate": "tool_call",
            "toolCallId": item["id"],
            "kind": "search",
            "title": web_search_title(item),
            "status": "completed",
            "rawInput": { "query": item["query"], "action": item["action"] },
        })],
        "imageView" => vec![image_view(item)],
        "imageGeneration" => vec![image_generation(item, false)],
        "enteredReviewMode" | "exitedReviewMode" => {
            let entered = item["type"] == "enteredReviewMode";
            let said = format!(
                "{} review mode: {}",
                if entered { "Entered" } else { "Exited" },
                text(item, "review")
            );
            vec![agent_text(&said, None, None)]
        }
        "contextCompaction" => vec![compaction(item, "tool_call", "completed")],
        "plan" => plan_text(item).into_iter().collect(),
        "collabAgentToolCall" => vec![collaboration(item, "tool_call")],
        _ => Vec::new(),
    }
}

/// What the chat has been shown of the items streaming in.
#[derive(Default)]
pub(super) struct Items {
    phases: HashMap<String, Value>,
    streamed_reasoning: HashSet<String>,
    shown: HashSet<String>,
}

impl Items {
    pub fn message_delta(&self, item_id: &str, delta: &str) -> Value {
        agent_text(delta, Some(item_id), self.phases.get(item_id))
    }

    pub fn thought_delta(&mut self, item_id: &str, delta: &str) -> Value {
        self.streamed_reasoning.insert(item_id.to_owned());
        thought(delta, Some(item_id))
    }

    pub fn started(&mut self, item: &Value) -> Vec<Value> {
        let id = text(item, "id").to_owned();
        let call = match text(item, "type") {
            "agentMessage" => {
                self.phases.insert(id, item["phase"].clone());
                return Vec::new();
            }
            "fileChange" => edit_call(item, status(item), &read_file),
            "commandExecution" => command_call(item),
            "mcpToolCall" => mcp_call(item),
            "dynamicToolCall" => dynamic_call(item),
            "webSearch" => json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "kind": "search",
                "title": web_search_title(item),
                "status": "in_progress",
                "rawInput": web_search_input(item),
            }),
            "imageView" => image_view(item),
            "imageGeneration" => json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "kind": "other",
                "title": "Image generation",
                "status": "in_progress",
                "rawInput": { "id": id },
            }),
            "contextCompaction" => compaction(item, "tool_call", "in_progress"),
            "collabAgentToolCall" => collaboration(item, "tool_call"),
            _ => return Vec::new(),
        };
        self.shown.insert(id);
        vec![call]
    }

    /// The updates for a finished item. One whose start never reached the
    /// chat, such as a command run before the chat loaded, is shown whole.
    pub fn completed(&mut self, item: &Value) -> Vec<Value> {
        let id = text(item, "id");
        let kind = text(item, "type");
        if kind == "agentMessage" {
            self.phases.insert(id.to_owned(), item["phase"].clone());
            return Vec::new();
        }
        if kind == "reasoning" {
            if self.streamed_reasoning.remove(id) {
                return Vec::new();
            }
            let joined = reasoning_parts(item)
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            return if joined.is_empty() {
                Vec::new()
            } else {
                vec![thought(&joined, Some(id))]
            };
        }
        match kind {
            "plan" => return plan_text(item).into_iter().collect(),
            "exitedReviewMode" => {
                let review = text(item, "review").trim();
                return if review.is_empty() {
                    Vec::new()
                } else {
                    vec![agent_text(review, None, None)]
                };
            }
            _ => {}
        }
        if !self.shown.remove(id) {
            return match kind {
                "imageGeneration" => vec![image_generation(item, true)],
                "contextCompaction" => vec![compaction(item, "tool_call", "completed")],
                _ => replayed(item, &read_file),
            };
        }
        let update = match kind {
            "fileChange" | "dynamicToolCall" => status_update(item),
            "mcpToolCall" => mcp_done(item),
            "commandExecution" => match command_done(item) {
                Some(update) => update,
                None => return Vec::new(),
            },
            "imageView" => return Vec::new(),
            "imageGeneration" => json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": image_status(item, true),
                "content": image_content(item),
                "rawOutput": image_output(item),
            }),
            "webSearch" => json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "title": web_search_title(item),
                "status": "completed",
                "rawInput": web_search_input(item),
            }),
            "contextCompaction" => compaction(item, "tool_call_update", "completed"),
            "collabAgentToolCall" => collaboration(item, "tool_call_update"),
            _ => return Vec::new(),
        };
        vec![update]
    }
}

/// A turn's plan as the chat shows it.
pub(super) fn plan(params: &Value) -> Value {
    let entries: Vec<Value> = params["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            let status = match text(step, "status") {
                "inProgress" => "in_progress",
                other => other,
            };
            json!({ "content": step["step"], "status": status, "priority": "medium" })
        })
        .collect();
    json!({ "sessionUpdate": "plan", "entries": entries })
}

/// The context window filling up, when Codex says how full it is.
pub(super) fn usage(params: &Value) -> Option<Value> {
    let usage = &params["tokenUsage"];
    let used = usage["last"]["totalTokens"].as_u64()?;
    let size = usage["modelContextWindow"].as_u64()?;
    Some(json!({ "sessionUpdate": "usage_update", "used": used, "size": size }))
}

/// The tokens a turn used, as Codex's adapter answers a prompt with them.
pub(super) fn turn_usage(token_usage: &Value) -> Option<Value> {
    let last = token_usage.get("last")?;
    let count = |key: &str| last[key].as_u64().unwrap_or(0);
    let mut usage = Map::new();
    usage.insert("totalTokens".into(), json!(count("totalTokens")));
    usage.insert(
        "inputTokens".into(),
        json!(count("inputTokens").saturating_sub(count("cachedInputTokens"))),
    );
    usage.insert("cachedReadTokens".into(), json!(count("cachedInputTokens")));
    usage.insert("outputTokens".into(), json!(count("outputTokens")));
    usage.insert(
        "thoughtTokens".into(),
        json!(count("reasoningOutputTokens")),
    );
    Some(Value::Object(usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_files(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_shell_wrapper_is_taken_off_a_command() {
        assert_eq!(strip_shell("/bin/zsh -lc 'ls -a'"), "ls -a");
        assert_eq!(strip_shell("bash -c 'echo hi'"), "echo hi");
        assert_eq!(strip_shell("sh 'x'"), "x");
        assert_eq!(strip_shell("git status"), "git status");
        assert_eq!(strip_shell("shellcheck x"), "shellcheck x");
    }

    #[test]
    fn a_terminal_command_runs_and_reports_its_output() {
        let mut items = Items::default();
        let mut item = json!({
            "type": "commandExecution", "id": "exec-1", "command": "/bin/zsh -lc 'cargo test'",
            "cwd": "/work", "status": "inProgress",
            "commandActions": [{ "type": "unknown", "command": "cargo test" }],
            "aggregatedOutput": null, "exitCode": null,
        });
        let started = items.started(&item);
        assert_eq!(
            started[0],
            json!({
                "sessionUpdate": "tool_call", "toolCallId": "exec-1", "kind": "execute",
                "title": "cargo test", "status": "in_progress",
                "content": [{ "type": "terminal", "terminalId": "exec-1" }],
                "rawInput": { "command": "cargo test", "cwd": "/work" },
                "_meta": { "terminal_info": { "cwd": "/work", "terminal_id": "exec-1" } },
            })
        );
        item["status"] = json!("failed");
        item["aggregatedOutput"] = json!("error[E0308]");
        item["exitCode"] = json!(101);
        let done = items.completed(&item);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(done[0]["status"], "failed");
        assert_eq!(
            done[0]["rawOutput"],
            json!({ "formatted_output": "error[E0308]", "exit_code": 101 })
        );
        assert_eq!(done[0]["_meta"]["terminal_exit"]["exit_code"], 101);
    }

    #[test]
    fn reads_lists_and_searches_are_named_for_what_they_touch() {
        let action = |action: Value| {
            command_call(&json!({
                "type": "commandExecution", "id": "c", "command": "x", "cwd": "/w",
                "status": "completed", "commandActions": [action],
            }))
        };
        let read =
            action(json!({ "type": "read", "command": "cat a", "name": "a", "path": "/w/a" }));
        assert_eq!(read["kind"], "read");
        assert_eq!(read["title"], "Read file '/w/a'");
        assert_eq!(read["locations"], json!([{ "path": "/w/a" }]));
        assert!(read.get("rawInput").is_none());
        let search =
            action(json!({ "type": "search", "command": "rg x", "query": "x", "path": null }));
        assert_eq!(search["kind"], "search");
        assert_eq!(search["title"], "Search for 'x'");
        let list = action(json!({ "type": "listFiles", "command": "ls", "path": null }));
        assert_eq!(list["title"], "List files");
        assert_eq!(list["kind"], "read");
    }

    #[test]
    fn a_persisted_listing_replays_as_a_call_and_its_output() {
        let item = json!({
            "type": "commandExecution", "id": "exec-f544", "command": "/bin/zsh -lc 'ls -a'",
            "cwd": "/repo", "processId": "45637", "source": "unifiedExecStartup", "status": "completed",
            "commandActions": [{ "type": "listFiles", "command": "ls -a", "path": null }],
            "aggregatedOutput": ".\n..\n.git\n", "exitCode": 0, "durationMs": 0,
        });
        let updates = replayed(&item, &no_files);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0]["title"], "List files");
        assert_eq!(updates[1]["rawOutput"]["formatted_output"], ".\n..\n.git\n");
        assert!(updates[1].get("_meta").is_none());
    }

    #[test]
    fn tool_server_calls_carry_their_arguments_and_result() {
        let mut items = Items::default();
        let mut item = json!({
            "type": "mcpToolCall", "id": "call_1", "server": "sikemux-tools", "tool": "browser_navigate",
            "status": "inProgress", "arguments": { "url": "https://x" }, "result": null, "error": null,
        });
        let started = items.started(&item);
        assert_eq!(started[0]["title"], "mcp__sikemux-tools__browser_navigate");
        assert_eq!(started[0]["rawInput"]["arguments"]["url"], "https://x");
        assert!(started[0].get("rawOutput").is_none());
        item["status"] = json!("completed");
        item["result"] = json!({ "content": [{ "type": "text", "text": "ok" }] });
        let done = items.completed(&item);
        assert_eq!(done[0]["status"], "completed");
        assert_eq!(done[0]["rawOutput"]["result"]["content"][0]["text"], "ok");
        assert_eq!(done[0]["rawOutput"]["error"], Value::Null);
    }

    #[test]
    fn an_edit_shows_the_file_before_and_after() {
        let item = json!({
            "type": "fileChange", "id": "exec-2", "status": "inProgress",
            "changes": [
                { "path": "/w/a.txt", "kind": { "type": "update", "move_path": null },
                  "diff": "@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n" },
                { "path": "/w/new.txt", "kind": { "type": "add" }, "diff": "hello\n" },
                { "path": "/w/old.txt", "kind": { "type": "delete" }, "diff": "bye\n" },
                { "path": "/w/gone.txt", "kind": { "type": "update", "move_path": null },
                  "diff": "@@ -1 +1 @@\n-a\n+b\n" },
            ],
        });
        let read = |path: &str| (path == "/w/a.txt").then(|| "one\ntwo\n".to_owned());
        let call = edit_call(&item, "in_progress", &read);
        assert_eq!(call["title"], "Editing files");
        assert_eq!(call["kind"], "edit");
        let blocks = call["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["oldText"], "one\ntwo\n");
        assert_eq!(blocks[0]["newText"], "one\nTWO\n");
        assert_eq!(
            blocks[1],
            json!({
                "type": "diff", "path": "/w/new.txt", "oldText": null, "newText": "hello\n",
                "_meta": { "kind": "add" },
            })
        );
        assert_eq!(blocks[2]["oldText"], "bye\n");
        assert_eq!(blocks[2]["newText"], "");

        let applied = |path: &str| (path == "/w/a.txt").then(|| "one\nTWO\n".to_owned());
        let replayed = edit_call(&item, "completed", &applied);
        assert_eq!(replayed["content"][0]["oldText"], "one\ntwo\n");
        assert_eq!(replayed["content"][0]["newText"], "one\nTWO\n");
    }

    #[test]
    fn streamed_text_carries_its_message_and_phase() {
        let mut items = Items::default();
        items.started(
            &json!({ "type": "agentMessage", "id": "msg_1", "text": "", "phase": "final_answer" }),
        );
        assert_eq!(
            items.message_delta("msg_1", "Hi"),
            json!({
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "Hi" },
                "messageId": "msg_1",
                "_meta": { "codex": { "phase": "final_answer" } },
            })
        );
        assert!(items.message_delta("msg_2", "x").get("_meta").is_none());
    }

    #[test]
    fn reasoning_is_sent_once_whether_streamed_or_not() {
        let mut items = Items::default();
        let item =
            json!({ "type": "reasoning", "id": "rs_1", "summary": ["a", "b"], "content": [] });
        let whole = items.completed(&item);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0]["content"]["text"], "a\n\nb");
        assert_eq!(whole[0]["sessionUpdate"], "agent_thought_chunk");
        items.thought_delta("rs_2", "a");
        assert!(items
            .completed(
                &json!({ "type": "reasoning", "id": "rs_2", "summary": ["a"], "content": [] })
            )
            .is_empty());
    }

    #[test]
    fn a_finished_item_never_started_here_is_shown_whole() {
        let mut items = Items::default();
        let done = items.completed(&json!({
            "type": "mcpToolCall", "id": "call_9", "server": "s", "tool": "t", "status": "failed",
            "arguments": {}, "result": null, "error": { "message": "boom" },
        }));
        assert_eq!(done[0]["sessionUpdate"], "tool_call");
        assert_eq!(done[0]["status"], "failed");
        assert_eq!(done[0]["rawOutput"]["error"]["message"], "boom");
    }

    #[test]
    fn web_searches_are_titled_for_their_action() {
        let search = |action: Value| web_search_title(&json!({ "query": "q", "action": action }));
        assert_eq!(search(Value::Null), "Web search: q");
        assert_eq!(
            search(json!({ "type": "search", "query": null, "queries": ["a", "b"] })),
            "Web search: a, b"
        );
        assert_eq!(
            search(json!({ "type": "open_page", "url": "https://x" })),
            "Open page: https://x"
        );
        assert_eq!(
            search(json!({ "type": "find_in_page", "url": "https://x", "pattern": "p" })),
            "Find in page for 'p' in https://x"
        );
        assert_eq!(search(json!({ "type": "other" })), "Web search");
    }

    #[test]
    fn plans_usage_and_user_inputs_take_the_adapters_shapes() {
        let update = plan(&json!({
            "explanation": "x",
            "plan": [{ "step": "Read", "status": "completed" }, { "step": "Fix", "status": "inProgress" }],
        }));
        assert_eq!(
            update["entries"][1],
            json!({ "content": "Fix", "status": "in_progress", "priority": "medium" })
        );
        assert_eq!(
            usage(
                &json!({ "tokenUsage": { "last": { "totalTokens": 900 }, "total": {}, "modelContextWindow": 200000 } })
            ),
            Some(json!({ "sessionUpdate": "usage_update", "used": 900, "size": 200000 }))
        );
        assert_eq!(
            usage(
                &json!({ "tokenUsage": { "last": { "totalTokens": 9 }, "modelContextWindow": null } })
            ),
            None
        );
        let blocks = user_blocks(&json!({ "content": [
            { "type": "text", "text": "look", "text_elements": [] },
            { "type": "localImage", "path": "/w/shot.png" },
            { "type": "image", "url": "https://x/i.png" },
            { "type": "skill", "name": "deploy", "path": "/s/deploy" },
            { "type": "mention", "name": "app", "path": "app://x" },
        ] }));
        let said: Vec<_> = blocks
            .iter()
            .map(|block| block["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            said,
            [
                "look",
                "[@shot.png](file:///w/shot.png)",
                "[@image](https://x/i.png)",
                "skill:deploy (/s/deploy)"
            ]
        );
    }
}
