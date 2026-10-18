//! What Muse's view items look like in the chat: the ACP session updates the
//! other agents send for the same kind of work, so the app reads them alike.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};

use super::super::super::states::{self, Subagent, Task};
use super::super::json::{text, text_or_empty};
use super::super::patch;

/// A tool call's argument JSON. Muse hands it over as the model wrote it,
/// which is not always valid JSON.
pub(super) fn args(item: &Value) -> Value {
    item["args"]
        .as_str()
        .and_then(|args| serde_json::from_str(args).ok())
        .unwrap_or(Value::Null)
}

/// The first of `keys` that holds text in `value`.
fn pick<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| text(value, key))
}

const PATH_KEYS: [&str; 4] = ["path", "file_path", "filePath", "file"];

fn command_of(args: &Value) -> Option<String> {
    for key in ["command", "cmd", "script"] {
        match &args[key] {
            Value::String(command) if !command.is_empty() => return Some(command.clone()),
            Value::Array(words) => {
                let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
                if !words.is_empty() {
                    return Some(words.join(" "));
                }
            }
            _ => {}
        }
    }
    None
}

fn status(item: &Value) -> &'static str {
    match text_or_empty(item, "status") {
        "completed" => "completed",
        "inProgress" | "" => "in_progress",
        _ => "failed",
    }
}

fn settled(item: &Value) -> bool {
    status(item) != "in_progress"
}

pub(super) fn content(block: Value) -> Value {
    json!({ "type": "content", "content": block })
}

fn text_content(said: &str) -> Value {
    content(json!({ "type": "text", "text": said }))
}

pub(super) fn agent_text(said: &str, message_id: Option<&str>) -> Value {
    let mut update = json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": said },
    });
    if let Some(id) = message_id {
        update["messageId"] = json!(id);
    }
    update
}

fn thought(said: &str, message_id: &str) -> Value {
    json!({
        "sessionUpdate": "agent_thought_chunk",
        "content": { "type": "text", "text": said },
        "messageId": message_id,
    })
}

/// A tool's name read as words, such as `cron_create` as "Cron create".
fn spoken(tool: &str) -> String {
    let words = tool.replace(['_', '-'], " ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => "Tool".to_owned(),
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The tools whose calls the chat leaves out: Muse's to-do list, which the chat
/// shows as the plan, and its questions, which the chat turns off.
fn hidden(tool: &str) -> bool {
    matches!(
        tool,
        "write_todos" | "request_user_input" | "snooze_reminder"
    )
}

fn edits(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file")
}

/// The update that first shows a tool call.
fn tool_call(item: &Value, cwd: &str) -> Value {
    let id = text_or_empty(item, "itemId");
    let tool = text_or_empty(item, "tool");
    let args = args(item);
    let path = pick(&args, &PATH_KEYS);
    let base = |kind: &str, title: String| {
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "kind": kind,
            "title": title,
            "status": status(item),
            "rawInput": args,
        })
    };
    let with_path = |mut call: Value| {
        if let Some(path) = path {
            call["locations"] = json!([{ "path": path }]);
        }
        call
    };
    if let Some(rest) = tool.strip_prefix("mcp__") {
        let (server, name) = rest.split_once("__").unwrap_or((rest, ""));
        return json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "kind": "other",
            "title": tool,
            "status": status(item),
            "rawInput": { "server": server, "tool": name, "arguments": args },
            "_meta": { "is_mcp_tool_call": true },
        });
    }
    match tool {
        "bash" | "shell" => {
            let command = command_of(&args).unwrap_or_default();
            let cwd = pick(&args, &["cwd", "workdir", "directory"]).unwrap_or(cwd);
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "kind": "execute",
                "title": command,
                "status": status(item),
                "content": [{ "type": "terminal", "terminalId": id }],
                "rawInput": { "command": command, "cwd": cwd },
                "_meta": { "terminal_info": { "cwd": cwd, "terminal_id": id } },
            })
        }
        "bash_input" => base("execute", "Send input to a running command".to_owned()),
        "read_file" => with_path(base(
            "read",
            format!("Read file '{}'", path.unwrap_or_default()),
        )),
        "write_file" | "edit_file" => {
            let mut call = with_path(base(
                "edit",
                path.map_or_else(
                    || "Editing files".to_owned(),
                    |path| format!("Edit {}", file_name(path)),
                ),
            ));
            let blocks = argument_diff(tool, &args, path, cwd);
            if !blocks.is_empty() {
                call["content"] = Value::Array(blocks);
            }
            call
        }
        "search" => {
            let query = pick(&args, &["pattern", "query", "regex", "glob"]);
            let title = match (query, path) {
                (Some(query), Some(path)) => format!("Search for '{query}' in {path}"),
                (Some(query), None) => format!("Search for '{query}'"),
                (None, Some(path)) => format!("Search in '{path}'"),
                (None, None) => "Search".to_owned(),
            };
            base("search", title)
        }
        "web_search" => base(
            "search",
            pick(&args, &["query", "q"]).map_or_else(
                || "Web search".to_owned(),
                |query| format!("Web search: {query}"),
            ),
        ),
        "web_fetch" => base(
            "fetch",
            pick(&args, &["url", "uri"])
                .map_or_else(|| "Fetch".to_owned(), |url| format!("Fetch {url}")),
        ),
        "read_skill" => base(
            "read",
            pick(&args, &["skill", "skill_id", "name", "id"]).map_or_else(
                || "Read skill".to_owned(),
                |skill| format!("Read skill {skill}"),
            ),
        ),
        "read_memory" => base("read", "Read memory".to_owned()),
        "add_memory" | "edit_memory" => base("edit", spoken(tool)),
        "workflow" => base(
            "other",
            pick(&args, &["objective", "name", "description"])
                .map_or_else(|| "Workflow".to_owned(), |name| format!("Workflow: {name}")),
        ),
        _ => base("other", spoken(tool)),
    }
}

/// An edit's change as the arguments describe it, for while it runs and when
/// Muse keeps no patch for it.
fn argument_diff(tool: &str, args: &Value, path: Option<&str>, cwd: &str) -> Vec<Value> {
    let Some(path) = path else {
        return Vec::new();
    };
    let path = absolute(path, cwd);
    if tool == "write_file" {
        return pick(args, &["content", "contents", "text"])
            .map(|new| {
                vec![json!({ "type": "diff", "path": path, "oldText": null, "newText": new })]
            })
            .unwrap_or_default();
    }
    let old = pick(args, &["old_string", "old_text", "old", "search"]);
    let new = pick(args, &["new_string", "new_text", "new", "replace"]);
    match (old, new) {
        (Some(old), Some(new)) => {
            vec![json!({ "type": "diff", "path": path, "oldText": old, "newText": new })]
        }
        _ => Vec::new(),
    }
}

fn absolute(path: &str, cwd: &str) -> String {
    if path.starts_with('/') || cwd.is_empty() {
        path.to_owned()
    } else {
        Path::new(cwd).join(path).to_string_lossy().into_owned()
    }
}

/// What a finished tool call printed.
fn output(item: &Value) -> &str {
    text_or_empty(item, "visibleOutput")
}

fn tool_done(item: &Value) -> Value {
    let id = text_or_empty(item, "itemId");
    let tool = text_or_empty(item, "tool");
    let said = output(item);
    if matches!(tool, "bash" | "shell") {
        return json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": status(item),
            "rawOutput": { "formatted_output": said, "exit_code": item["exitCode"] },
            "_meta": {
                "terminal_exit": { "exit_code": item["exitCode"], "signal": item["exitSignal"], "terminal_id": id },
            },
        });
    }
    let mut update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": id,
        "status": status(item),
    });
    if tool.starts_with("mcp__") {
        let failed = status(item) == "failed";
        update["rawOutput"] = json!({
            "result": (!failed).then(|| json!({ "content": [{ "type": "text", "text": said }] })),
            "error": failed.then(|| json!({ "message": said })),
        });
        return update;
    }
    if !said.is_empty() && !edits(tool) {
        update["content"] = json!([text_content(said)]);
        update["rawOutput"] = json!(said);
    } else if status(item) == "failed" {
        if let Some(reason) = pick(item, &["failureReason", "reason", "visibleOutput"]) {
            update["content"] = json!([text_content(reason)]);
        }
    }
    update
}

/// A stored patch's files, as the file before and after each change. `read`
/// gives a file as it is now, after the edit.
pub(super) fn patch_blocks(
    patch_text: &str,
    cwd: &str,
    read: &impl Fn(&str) -> Option<String>,
) -> Vec<Value> {
    let mut blocks = Vec::new();
    for file in patch_files(patch_text) {
        let path = absolute(&file.path, cwd);
        match file.change {
            Change::Add => {
                let new = file.lines('+');
                blocks.push(json!({ "type": "diff", "path": path, "oldText": null, "newText": new, "_meta": { "kind": "add" } }));
            }
            Change::Delete => {
                let old = file.lines('-');
                blocks.push(json!({ "type": "diff", "path": path, "oldText": old, "newText": "", "_meta": { "kind": "delete" } }));
            }
            Change::Update => {
                let Some(current) = read(&path) else {
                    continue;
                };
                let hunks = file.hunks.join("\n");
                let block = match patch::revert(&current, &hunks) {
                    Some(original) => {
                        json!({ "type": "diff", "path": path, "oldText": original, "newText": current })
                    }
                    None => match patch::apply(&current, &hunks) {
                        Some(patched) => {
                            json!({ "type": "diff", "path": path, "oldText": current, "newText": patched })
                        }
                        None => continue,
                    },
                };
                blocks.push(block);
            }
        }
    }
    blocks
}

#[derive(Debug, PartialEq)]
enum Change {
    Add,
    Delete,
    Update,
}

struct PatchFile {
    path: String,
    change: Change,
    hunks: Vec<String>,
}

impl PatchFile {
    /// The text on one side of a whole-file change, from its marked lines.
    fn lines(&self, mark: char) -> String {
        let mut said: String = self
            .hunks
            .iter()
            .filter_map(|line| line.strip_prefix(mark))
            .map(|line| format!("{line}\n"))
            .collect();
        if self
            .hunks
            .iter()
            .any(|line| line.starts_with("\\ No newline"))
        {
            said.pop();
        }
        said
    }
}

fn side(header: &str) -> Option<&str> {
    let path = header.split('\t').next()?.trim();
    (path != "/dev/null").then(|| {
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path)
    })
}

/// The files of a unified diff, each with its hunk lines.
fn patch_files(patch_text: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut lines = patch_text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(old) = line.strip_prefix("--- ") {
            let Some(new) = lines.peek().and_then(|next| next.strip_prefix("+++ ")) else {
                continue;
            };
            let (old, new) = (side(old), side(new));
            lines.next();
            let (path, change) = match (old, new) {
                (None, Some(new)) => (new, Change::Add),
                (Some(old), None) => (old, Change::Delete),
                (_, Some(new)) => (new, Change::Update),
                (None, None) => continue,
            };
            files.push(PatchFile {
                path: path.to_owned(),
                change,
                hunks: Vec::new(),
            });
            continue;
        }
        if line.starts_with("diff ") {
            continue;
        }
        if let Some(file) = files.last_mut() {
            if !file.hunks.is_empty() || line.starts_with("@@") {
                file.hunks.push(line.to_owned());
            }
        }
    }
    files
}

/// A finished item as it is replayed from the session's history. Subagents
/// are left to the caller, which opens their cards.
pub(super) fn replayed(item: &Value, cwd: &str) -> Vec<Value> {
    let id = text_or_empty(item, "itemId");
    match text_or_empty(item, "kind") {
        "agentMessage" => text(item, "text")
            .map(|said| vec![agent_text(said, Some(id))])
            .unwrap_or_default(),
        "reasoning" => reasoning_text(item)
            .map(|said| vec![thought(&said, id)])
            .unwrap_or_default(),
        "toolCall" if hidden(text_or_empty(item, "tool")) => Vec::new(),
        "toolCall" => {
            let mut call = tool_call(item, cwd);
            call["status"] = json!("in_progress");
            vec![call, tool_done(item)]
        }
        "userShell" => user_shell(item, true),
        "compaction" => vec![compaction(item, "tool_call")],
        "webSearch" => vec![web_search(item)],
        _ => Vec::new(),
    }
}

fn reasoning_text(item: &Value) -> Option<String> {
    let summary: Vec<&str> = item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|part| !part.trim().is_empty())
        .collect();
    let said = if summary.is_empty() {
        text_or_empty(item, "text").to_owned()
    } else {
        summary.join("\n\n")
    };
    (!said.trim().is_empty()).then_some(said)
}

fn user_shell(item: &Value, done: bool) -> Vec<Value> {
    let id = text_or_empty(item, "itemId");
    let command = text_or_empty(item, "commandText");
    let mut updates = vec![json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "kind": "execute",
        "title": command,
        "status": "in_progress",
        "content": [{ "type": "terminal", "terminalId": id }],
        "rawInput": { "command": command },
        "_meta": { "terminal_info": { "terminal_id": id } },
    })];
    if done && settled(item) {
        updates.push(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": status(item),
            "rawOutput": { "formatted_output": output(item), "exit_code": item["exitCode"] },
            "_meta": { "terminal_exit": { "exit_code": item["exitCode"], "signal": item["exitSignal"], "terminal_id": id } },
        }));
    }
    updates
}

fn compaction(item: &Value, session_update: &str) -> Value {
    let mut update = json!({
        "sessionUpdate": session_update,
        "toolCallId": item["itemId"],
        "title": "Compact conversation",
        "status": status(item),
        "_meta": { "contextCompaction": { "version": 1 } },
    });
    if session_update == "tool_call" {
        update["kind"] = json!("think");
    }
    update
}

fn web_search(item: &Value) -> Value {
    let query = pick(item, &["label", "text", "fallbackText"]);
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["itemId"],
        "kind": "search",
        "title": query.map_or_else(|| "Web search".to_owned(), |query| format!("Web search: {query}")),
        "status": status(item),
    })
}

/// A subagent's card name, from the file that defines it, such as
/// `.agents/code-reviewer.md` read as "Code reviewer".
pub(super) fn subagent_name(item: &Value) -> String {
    let from_path = text(item, "agentPath").and_then(|path| {
        let stem = file_name(path).trim_end_matches(".md");
        (!stem.is_empty()).then(|| spoken(stem))
    });
    from_path
        .or_else(|| text(item, "role").map(spoken))
        .unwrap_or_else(|| states::unnamed(text_or_empty(item, "subagentId")))
}

pub(super) fn subagent_task(item: &Value) -> String {
    states::first_text(&[text(item, "objective"), text(item, "fallbackText")])
        .unwrap_or_else(|| states::UNNAMED_TASK.to_owned())
}

/// How a subagent item says its work stands, once it has ended.
fn subagent_end(item: &Value) -> Option<Subagent> {
    match text_or_empty(item, "status") {
        "completed" => Some(Subagent::Completed),
        "cancelled" => Some(Subagent::Cancelled),
        "inProgress" | "" => None,
        _ => Some(Subagent::Failed),
    }
}

pub(super) fn subagent_result(item: &Value) -> Option<String> {
    let result = &item["result"];
    states::first_text(&[text(result, "text"), text(result, "summary")])
}

/// One subagent with a card in the chat.
#[derive(serde::Serialize, serde::Deserialize)]
struct Child {
    parent: String,
    ended: bool,
    /// Whether anything of its own conversation reached its card.
    spoke: bool,
}

/// A tool call that runs on after its turn.
#[derive(serde::Serialize, serde::Deserialize)]
struct Background {
    ended: bool,
}

/// What follows from an item beyond the updates it makes.
#[derive(Debug, PartialEq)]
pub(super) enum Follow {
    /// A subagent's conversation to listen to, now that it has one.
    Subagent(String),
    /// A subagent ended without its conversation reaching its card: read it,
    /// or else show what it answered.
    ReadSubagent {
        child: String,
        result: Option<String>,
    },
    /// An edit's stored patch, to show as the files before and after.
    Patch {
        session: String,
        call: String,
        output_ref: String,
    },
}

/// A message or reasoning arriving in pieces.
#[derive(serde::Serialize, serde::Deserialize)]
struct Streaming {
    thought: bool,
    /// Whether any piece of it has reached the chat.
    said: bool,
}

/// What the chat has been shown of the items streaming in.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Items {
    streaming: HashMap<String, Streaming>,
    shown: HashSet<String>,
    children: HashMap<String, Child>,
    /// The subagent each subagent item stands for.
    subagent_items: HashMap<String, String>,
    tasks: HashMap<String, Background>,
}

impl Items {
    pub fn is_child(&self, session: &str) -> bool {
        self.children.contains_key(session)
    }

    pub fn child_spoke(&mut self, session: &str) {
        if let Some(child) = self.children.get_mut(session) {
            child.spoke = true;
        }
    }

    pub fn delta(&mut self, session: &str, params: &Value) -> Option<Value> {
        let id = text_or_empty(params, "itemId");
        let delta = text_or_empty(params, "delta");
        let field = text(params, "field").unwrap_or("text");
        let streaming = self.streaming.get_mut(id)?;
        if delta.is_empty() || !(field == "text" || field.starts_with("summary")) {
            return None;
        }
        streaming.said = true;
        if streaming.thought {
            return Some(thought(delta, id));
        }
        self.child_spoke(session);
        Some(agent_text(delta, Some(id)))
    }

    /// The updates for an item that opened or changed while still running.
    pub fn opened(
        &mut self,
        session: &str,
        item: &Value,
        cwd: &str,
        follow: &mut Vec<Follow>,
    ) -> Vec<(String, Value)> {
        let id = text_or_empty(item, "itemId").to_owned();
        let mut out = Vec::new();
        match text_or_empty(item, "kind") {
            kind @ ("agentMessage" | "reasoning") => {
                self.streaming.entry(id).or_insert(Streaming {
                    thought: kind == "reasoning",
                    said: false,
                });
            }
            "toolCall" if hidden(text_or_empty(item, "tool")) => {}
            "toolCall" => {
                if self.shown.insert(id.clone()) {
                    self.child_spoke(session);
                    out.push((session.to_owned(), tool_call(item, cwd)));
                }
                self.background(session, item, &mut out);
            }
            "userShell" if self.shown.insert(id.clone()) => {
                out.extend(
                    user_shell(item, false)
                        .into_iter()
                        .map(|update| (session.to_owned(), update)),
                );
            }
            "compaction" if self.shown.insert(id.clone()) => {
                out.push((session.to_owned(), compaction(item, "tool_call")));
            }
            "webSearch" if self.shown.insert(id) => {
                out.push((session.to_owned(), web_search(item)));
            }
            "subagent" => self.subagent(session, item, &mut out, follow),
            _ => {}
        }
        out
    }

    /// The updates for an item that finished.
    pub fn completed(
        &mut self,
        session: &str,
        item: &Value,
        cwd: &str,
        follow: &mut Vec<Follow>,
    ) -> Vec<(String, Value)> {
        let id = text_or_empty(item, "itemId").to_owned();
        let at = |updates: Vec<Value>| -> Vec<(String, Value)> {
            updates
                .into_iter()
                .map(|update| (session.to_owned(), update))
                .collect()
        };
        match text_or_empty(item, "kind") {
            "agentMessage" | "reasoning" => {
                let said = self
                    .streaming
                    .remove(&id)
                    .is_some_and(|streaming| streaming.said);
                if said {
                    return Vec::new();
                }
                if item["kind"] == "agentMessage" {
                    self.child_spoke(session);
                }
                at(replayed(item, cwd))
            }
            "toolCall" if hidden(text_or_empty(item, "tool")) => Vec::new(),
            "toolCall" => {
                let mut out = Vec::new();
                if self.shown.remove(&id) {
                    out.push((session.to_owned(), tool_done(item)));
                } else {
                    self.child_spoke(session);
                    out.extend(at(replayed(item, cwd)));
                }
                self.background(session, item, &mut out);
                if edits(text_or_empty(item, "tool")) && status(item) == "completed" {
                    if let Some(output_ref) = item["patchRef"]["id"].as_str() {
                        follow.push(Follow::Patch {
                            session: session.to_owned(),
                            call: id,
                            output_ref: output_ref.to_owned(),
                        });
                    }
                }
                out
            }
            "userShell" => {
                if self.shown.remove(&id) {
                    user_shell(item, true)
                        .into_iter()
                        .skip(1)
                        .map(|update| (session.to_owned(), update))
                        .collect()
                } else {
                    at(user_shell(item, true))
                }
            }
            "compaction" => {
                if self.shown.remove(&id) {
                    at(vec![compaction(item, "tool_call_update")])
                } else {
                    at(vec![compaction(item, "tool_call")])
                }
            }
            "webSearch" => {
                if self.shown.remove(&id) {
                    at(vec![
                        json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status(item) }),
                    ])
                } else {
                    at(vec![web_search(item)])
                }
            }
            "subagent" => {
                let mut out = Vec::new();
                self.subagent(session, item, &mut out, follow);
                out
            }
            _ => Vec::new(),
        }
    }

    /// A tool call that went on in the background is a task of the chat's.
    fn background(&mut self, session: &str, item: &Value, out: &mut Vec<(String, Value)>) {
        let id = text_or_empty(item, "itemId");
        if item["background"] != true && !self.tasks.contains_key(id) {
            return;
        }
        if !self.tasks.contains_key(id) {
            let tool = text_or_empty(item, "tool");
            let task_type = if matches!(tool, "bash" | "shell") {
                "shell"
            } else if tool == "monitor" {
                "monitor"
            } else {
                "other"
            };
            let name = command_of(&args(item)).unwrap_or_else(|| spoken(tool));
            self.tasks
                .insert(id.to_owned(), Background { ended: false });
            out.push((
                session.to_owned(),
                json!({
                    "sessionUpdate": "async_task_spawned",
                    "asyncTaskId": id,
                    "name": name,
                    "taskType": task_type,
                    "description": name,
                    "showInTranscript": true,
                    "canStop": true,
                    "toolCallId": id,
                }),
            ));
        }
        let task = self.tasks.get_mut(id);
        if let Some(task) = task.filter(|task| !task.ended && settled(item)) {
            task.ended = true;
            let state = match status(item) {
                "completed" => Task::Completed,
                _ if text_or_empty(item, "status") == "cancelled" => Task::Stopped,
                _ => Task::Failed,
            };
            out.push((session.to_owned(), state.update(id)));
        }
    }

    fn subagent(
        &mut self,
        session: &str,
        item: &Value,
        out: &mut Vec<(String, Value)>,
        follow: &mut Vec<Follow>,
    ) {
        let item_id = text_or_empty(item, "itemId").to_owned();
        let child = match item["childSessionId"].as_str() {
            Some(child) => child.to_owned(),
            None => return,
        };
        self.subagent_items.insert(item_id, child.clone());
        if !self.children.contains_key(&child) {
            out.push((
                session.to_owned(),
                states::spawned(&child, &subagent_name(item), &subagent_task(item)),
            ));
            self.children.insert(
                child.clone(),
                Child {
                    parent: session.to_owned(),
                    ended: false,
                    spoke: false,
                },
            );
            follow.push(Follow::Subagent(child.clone()));
        }
        let Some(card) = self.children.get_mut(&child) else {
            return;
        };
        match subagent_end(item) {
            Some(state) if !card.ended => {
                card.ended = true;
                if !card.spoke {
                    follow.push(Follow::ReadSubagent {
                        child: child.clone(),
                        result: subagent_result(item),
                    });
                }
                out.push((card.parent.clone(), state.update(&child)));
            }
            None if card.ended => {
                card.ended = false;
                out.push((card.parent.clone(), Subagent::Running.update(&child)));
            }
            _ => {}
        }
    }

    /// Ends every subagent card still open, as `state`.
    pub fn finish_outstanding(&mut self, state: Subagent) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for (id, child) in &mut self.children {
            if !child.ended {
                child.ended = true;
                out.push((child.parent.clone(), state.update(id)));
            }
        }
        out
    }
}
