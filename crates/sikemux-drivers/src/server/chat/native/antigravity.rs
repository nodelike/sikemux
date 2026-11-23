//! Antigravity's tool calls under the names the chat reads, and the errors it ends turns on.
//! It titles a tool server's tool `Run mcp_<server>_<tool>`, or `MCP Tool <tool>` when it
//! reaches the tool through `call_mcp_tool`, calls a command `CommandLine`
//! and its label `toolSummary`, and heads what a command printed with when it ran. A subagent
//! it starts runs on detached, its steps only in a log of its own, which the subagent's card
//! follows.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use sikemux_fold::update::{
    agent_text, mcp_title, subagent_model, subagent_usage, text_content, thought, tool_call,
    tool_update,
};
use sikemux_fold::UpdateKind;
use sikemux_process::sync::Locked;

use super::super::states::{self, Subagent, UNNAMED_TASK};
use super::mcp_calls::split;
use super::session::Sink;
use crate::acp::failure::{Failure, FailureKind};
use crate::sessions::antigravity::{subagent_log, subagent_run, subagent_started, SubagentRun};

const PRINTED: &str = "Command output:\n";

pub(super) fn as_common_call(update: &mut Value, servers: &[String]) {
    if !UpdateKind::of(update).is_some_and(UpdateKind::is_tool) {
        return;
    }
    let summary = update["rawInput"]
        .as_object_mut()
        .and_then(|input| input.remove("toolSummary"));
    let named = update["name"]
        .as_str()
        .and_then(|name| name.strip_prefix("mcp_"))
        .and_then(|name| split(name, servers));
    if let Some((server, tool)) = named {
        update["title"] = json!(mcp_title(&server, &tool));
        return;
    }
    if let Some((server, tool, arguments)) = proxied(&update["rawInput"]) {
        update["title"] = json!(mcp_title(&server, &tool));
        update["kind"] = json!("other");
        update["rawInput"] = arguments;
        return;
    }
    if update["kind"] != "execute" {
        return;
    }
    if let Some(input) = update["rawInput"].as_object_mut() {
        rename(input, "CommandLine", "command");
        if let Some(summary) = summary {
            input.insert("description".to_owned(), summary);
        }
    }
    if let Some(output) = update.pointer_mut("/rawOutput/output") {
        trim_head(output);
    }
    if let Some(blocks) = update["content"].as_array_mut() {
        for block in blocks {
            if let Some(text) = block.pointer_mut("/content/text") {
                trim_head(text);
            }
        }
    }
}

const SPAWNS: &str = "invoke_subagent";

/// How often a subagent's card looks for its next steps.
const FOLLOW_EVERY: Duration = Duration::from_secs(1);

/// How long a subagent may take to appear once it was asked for.
const START_WAIT: Duration = Duration::from_secs(60);

/// The subagent conversations whose cards are already in the chat.
pub(super) type Taken = Arc<Mutex<Vec<String>>>;

/// The subagent an `invoke_subagent` call starts: the card shown in place of the call, and,
/// for the call's start, the prompt it was given. Its updates show nothing the card does not.
pub(super) fn spawn(update: &Value) -> Option<(Value, Option<String>)> {
    if update["name"] != SPAWNS || !UpdateKind::of(update).is_some_and(UpdateKind::is_tool) {
        return None;
    }
    let card = update["toolCallId"].as_str()?;
    if UpdateKind::of(update) != Some(UpdateKind::ToolCall) {
        return Some((Value::Null, None));
    }
    let input = &update["rawInput"];
    let prompt = input["Prompt"].as_str().unwrap_or_default().to_owned();
    let name = states::first_text(&[input["Role"].as_str(), input["TypeName"].as_str()])
        .unwrap_or_else(|| states::unnamed(card));
    let task = states::first_text(&[Some(&prompt)]).unwrap_or_else(|| UNNAMED_TASK.to_owned());
    Some((states::spawned(card, &name, &task), Some(prompt)))
}

/// Follows the subagent conversation `parent` started with `prompt` onto the card `card`
/// until it goes idle or is stopped, or `gone` says the chat's agent has.
pub(super) async fn follow(
    sink: Sink,
    parent: String,
    card: String,
    prompt: String,
    taken: Taken,
    gone: impl Fn() -> bool,
) {
    let mut waited = Duration::ZERO;
    let child = loop {
        if gone() {
            return sink.update(&parent, Subagent::Disconnected.update(&card));
        }
        let found = {
            let mut taken = taken.locked();
            let found = subagent_started(&parent, &prompt, &taken);
            taken.extend(found.clone());
            found
        };
        if let Some(child) = found {
            break child;
        }
        if waited >= START_WAIT {
            return sink.update(&parent, Subagent::Failed.update(&card));
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
        waited += FOLLOW_EVERY;
    };
    let Some(mut log) = subagent_log(&child).map(|path| ChildLog::new(path, card.clone())) else {
        return sink.update(&parent, Subagent::Failed.update(&card));
    };
    let mut idle = 0;
    let end = loop {
        let run = subagent_run(&child);
        for (in_card, update) in log.read() {
            sink.update(if in_card { &card } else { &parent }, update);
        }
        match run {
            SubagentRun::Killed => break Subagent::Cancelled,
            SubagentRun::Gone => break Subagent::Disconnected,
            SubagentRun::Idle if log.stepped => idle += 1,
            _ => idle = 0,
        }
        if idle >= 2 {
            break Subagent::Completed;
        }
        if gone() {
            break Subagent::Disconnected;
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
    };
    sink.update(&parent, end.update(&card));
}

/// What a subagent's log has said so far, read as it grows.
struct ChildLog {
    path: PathBuf,
    card: String,
    read: u64,
    rest: Vec<u8>,
    /// Its calls still waiting on the step that says how they went, oldest first.
    open: Vec<String>,
    tokens: u64,
    model: bool,
    /// Whether its model has taken a step yet, so an idle conversation is one that finished.
    stepped: bool,
}

impl ChildLog {
    fn new(path: PathBuf, card: String) -> Self {
        Self {
            path,
            card,
            read: 0,
            rest: Vec::new(),
            open: Vec::new(),
            tokens: 0,
            model: false,
            stepped: false,
        }
    }

    /// The updates its new whole lines make, each saying whether it goes onto the card's
    /// own feed or to the card itself in the chat that started it.
    fn read(&mut self) -> Vec<(bool, Value)> {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(self.read)).is_err() {
            return Vec::new();
        }
        let mut more = Vec::new();
        let Ok(count) = file.read_to_end(&mut more) else {
            return Vec::new();
        };
        self.read += count as u64;
        self.rest.extend(more);
        let Some(end) = self.rest.iter().rposition(|byte| *byte == b'\n') else {
            return Vec::new();
        };
        let whole: Vec<u8> = self.rest.drain(..=end).collect();
        String::from_utf8_lossy(&whole)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .flat_map(|step| self.step(&step))
            .collect()
    }

    fn step(&mut self, step: &Value) -> Vec<(bool, Value)> {
        let index = step["step_index"].as_i64().unwrap_or_default();
        match step["type"].as_str() {
            Some("PLANNER_RESPONSE") => self.planned(index, step),
            Some("GENERIC") => self
                .done(step)
                .into_iter()
                .map(|update| (true, update))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn planned(&mut self, index: i64, step: &Value) -> Vec<(bool, Value)> {
        self.stepped = true;
        let mut out = Vec::new();
        let id = format!("{}-{index}", self.card);
        if let Some(text) = said(&step["thinking"]) {
            out.push((true, thought(text, Some(&format!("{id}-thought")))));
        }
        if let Some(text) = said(&step["content"]) {
            out.push((true, agent_text(text, Some(&id))));
        }
        for (at, call) in step["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let call_id = format!("{id}-{at}");
            out.push((true, child_call(&call_id, call)));
            self.open.push(call_id);
        }
        if let Some(model) = step["model"].as_str().filter(|_| !self.model) {
            self.model = true;
            out.push((false, subagent_model(&self.card, model)));
        }
        let spent: u64 = ["input_tokens", "cache_read_tokens", "output_tokens"]
            .iter()
            .filter_map(|key| step[key].as_u64())
            .sum();
        if spent > 0 {
            self.tokens += spent;
            out.push((false, subagent_usage(&self.card, self.tokens)));
        }
        out
    }

    /// How the oldest call still open went.
    fn done(&mut self, step: &Value) -> Option<Value> {
        if self.open.is_empty() {
            return None;
        }
        let id = self.open.remove(0);
        let failed = step["status"] == "ERROR";
        let mut printed = step["error"]
            .as_str()
            .filter(|_| failed)
            .or_else(|| step["content"].as_str())
            .map(|text| json!(text))
            .unwrap_or_else(|| json!(""));
        trim_head(&mut printed);
        let text = printed.as_str().unwrap_or_default();
        let mut update = tool_update(&id, if failed { "failed" } else { "completed" });
        if let Some(fields) = update.as_object_mut() {
            fields.insert("rawOutput".to_owned(), json!({ "output": text }));
            fields.insert("content".to_owned(), json!([text_content(text)]));
        }
        Some(update)
    }
}

fn said(text: &Value) -> Option<&str> {
    text.as_str().filter(|text| !text.trim().is_empty())
}

/// A call a subagent's log names, its arguments each written there as JSON text.
fn child_call(id: &str, call: &Value) -> Value {
    let name = call["name"].as_str().unwrap_or("tool");
    let input: Map<String, Value> = call["args"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_else(|| value.clone());
            (key.clone(), value)
        })
        .collect();
    let command = input.get("CommandLine").and_then(Value::as_str);
    let mut update = match command {
        Some(command) => tool_call(id, "execute", command, "in_progress"),
        None => tool_call(id, "other", name, "in_progress"),
    };
    if let Some(fields) = update.as_object_mut() {
        fields.insert("name".to_owned(), json!(name));
        fields.insert("rawInput".to_owned(), Value::Object(input));
    }
    as_common_call(&mut update, &[]);
    update
}

/// The server, tool and arguments of a `call_mcp_tool` call, which reaches a tool of a
/// server from Antigravity's own settings that it left out of the model's tool list.
fn proxied(input: &Value) -> Option<(String, String, Value)> {
    let server = input["ServerName"].as_str()?.trim();
    let tool = input["ToolName"].as_str()?.trim();
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    let arguments = input.get("Arguments").cloned().unwrap_or_else(|| json!({}));
    Some((server.to_owned(), tool.to_owned(), arguments))
}

fn rename(input: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = input.remove(from) {
        input.insert(to.to_owned(), value);
    }
}

/// What a command printed, without the times and exit line Antigravity writes above it.
fn trim_head(text: &mut Value) {
    let Some(said) = text.as_str() else {
        return;
    };
    let printed = said
        .split_once(PRINTED)
        .map_or(said, |(_, printed)| printed);
    *text = json!(printed.replace("\r\n", "\n"));
}

/// The failure an error step names, such as `RESOURCE_EXHAUSTED (code 429): …` or
/// `Error 502, Message: …, Status: …`, titled by its message and kept whole in the details.
pub(super) fn turn_failure(line: String) -> Failure {
    let limited = [
        "(code 429)",
        "Error 429,",
        "RESOURCE_EXHAUSTED",
        "QUOTA_EXHAUSTED",
    ]
    .iter()
    .any(|sign| line.contains(sign));
    let message = line
        .split_once("): ")
        .or_else(|| line.split_once(", Message: "))
        .map(|(_, message)| message.split(", Status: ").next().unwrap_or(message))
        .map(|message| message.trim_start_matches("* ").trim())
        .filter(|message| !message.is_empty())
        .unwrap_or(&line)
        .to_owned();
    Failure {
        kind: if limited {
            FailureKind::Limit
        } else {
            FailureKind::Other
        },
        details: (message != line).then_some(line),
        ..Failure::other(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sikemux_tool_call_reads_as_the_tool() {
        let mut update = json!({
            "sessionUpdate": "tool_call", "toolCallId": "call_1",
            "name": "mcp_sikemux-tools_agent_list", "args": { "toolSummary": "List Sikemux agents" },
            "status": "in_progress", "kind": "other", "title": "Run mcp_sikemux-tools_agent_list",
            "rawInput": { "toolSummary": "List Sikemux agents" }
        });
        as_common_call(&mut update, &["sikemux-tools".to_owned()]);
        assert_eq!(update["title"], "mcp__sikemux-tools__agent_list");
        assert_eq!(update["rawInput"], json!({}));
    }

    #[test]
    fn a_command_reads_as_one() {
        let printed = "Created At: 2026-11-22T06:30:27+05:30\nCompleted At: 2026-11-22T06:30:28+05:30\nCommand exited with code 0.\n\nCommand output:\n./notes.txt:1:two\r\n";
        let mut update = json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "call_2", "name": "run_command",
            "status": "completed", "kind": "execute", "title": "grep -rn \"two\" .",
            "rawInput": { "CommandLine": "grep -rn \"two\" .", "WaitMsBeforeAsync": 5000, "toolSummary": "Search folder for two" },
            "rawOutput": { "output": printed, "exit_code": 0 },
            "content": [{ "type": "content", "content": { "type": "text", "text": printed } }]
        });
        as_common_call(&mut update, &[]);
        assert_eq!(update["rawInput"]["command"], "grep -rn \"two\" .");
        assert_eq!(update["rawInput"]["description"], "Search folder for two");
        assert_eq!(update["rawOutput"]["output"], "./notes.txt:1:two\n");
        assert_eq!(
            update["content"][0]["content"]["text"],
            "./notes.txt:1:two\n"
        );
    }
}
