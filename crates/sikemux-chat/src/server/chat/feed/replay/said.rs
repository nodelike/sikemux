//! What a chat said, as another agent reads it, and the title it goes by.

use serde_json::Value;
pub use sikemux_drivers::server::chat::feed::title;
use sikemux_fold::{ChatEventKind, UpdateKind};

use super::Replay;
use crate::sessions::transcript::Said;

/// What a chat said from one point on, without its thinking, its subagents'
/// work or what its tools returned.
pub struct Excerpt {
    /// Names this run of the chat; its indexes start over in the next.
    pub feed: String,
    /// Each with its index, and whether a turn starts there.
    pub said: Vec<(u64, bool, Said)>,
    /// Some of what came after the point asked for is no longer kept.
    pub skipped: bool,
    /// Where reading on starts.
    pub next: u64,
}

const RUNNING_TOOLS: &[&str] = &[
    "process_start",
    "process_restart",
    "browser",
    "sikemux-tools__sim",
];
const READING_COMMANDS: &[&str] = &[
    "awk", "cat", "cd", "cut", "diff", "echo", "file", "find", "git", "grep", "head", "less", "ls",
    "more", "nl", "printf", "pwd", "rg", "sed", "sort", "stat", "tail", "test", "tr", "tree",
    "true", "ugrep", "uniq", "wc", "which",
];

fn tool_runs(update: &Value) -> bool {
    let title = update
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match update.get("kind").and_then(Value::as_str) {
        Some("execute") => command_runs(title),
        _ => RUNNING_TOOLS.iter().any(|tool| title.contains(tool)),
    }
}

/// Whether any part of a shell command is more than reading.
fn command_runs(command: &str) -> bool {
    command
        .split(['\n', ';', '|', '&'])
        .filter_map(|part| part.split_whitespace().find(|word| !word.contains('=')))
        .any(|program| !READING_COMMANDS.contains(&program.trim_matches('`')))
}

pub fn session_title(payload: &Value) -> Option<String> {
    let update = payload.get("update")?;
    if UpdateKind::of(update)? != UpdateKind::SessionInfoUpdate {
        return None;
    }
    title(update.get("title")?.as_str()?)
}

impl Replay {
    /// What the person wrote, oldest first: the newest messages that fit in `budget`
    /// characters, each counted with the three a list line adds, and each past `longest`
    /// characters cut and marked `[cut]`. Only messages the record marks as the person's count.
    pub(in super::super) fn persons_words(&self, budget: usize, longest: usize) -> Vec<String> {
        const LINE: usize = 3;
        const CUT: &str = " [cut]";
        let mut said = Vec::new();
        let mut left = budget;
        for entry in self.entries.iter().rev() {
            if entry.event.kind != ChatEventKind::Prompt {
                continue;
            }
            if entry.event.payload.get("person") != Some(&Value::Bool(true)) {
                continue;
            }
            let Some(text) = entry.event.payload.get("text").and_then(Value::as_str) else {
                continue;
            };
            let text = text.trim();
            let mut kept: String = text.chars().take(longest).collect();
            if kept.len() < text.len() {
                kept.push_str(CUT);
            }
            if kept.is_empty() || said.last() == Some(&kept) {
                continue;
            }
            let Some(rest) = left.checked_sub(kept.chars().count() + LINE) else {
                break;
            };
            left = rest;
            said.push(kept);
        }
        said.reverse();
        said
    }

    /// The newest `count` exchanges with the person, oldest first: each message of theirs and
    /// what the agent wrote back in that turn. Each side past `longest` characters keeps its
    /// start for the person's and its end for the agent's, marked `[cut]`. Only a message the
    /// record marks as the person's opens an exchange.
    pub(in super::super) fn exchanges(
        &self,
        count: usize,
        longest: usize,
    ) -> Vec<(String, String)> {
        let mut exchanges: Vec<(String, String)> = Vec::new();
        let mut open = false;
        for entry in &self.entries {
            let payload = &entry.event.payload;
            match entry.event.kind {
                ChatEventKind::Prompt => {
                    open = payload.get("person") == Some(&Value::Bool(true));
                    if open {
                        let text = payload.get("text").and_then(Value::as_str);
                        exchanges.push((text.unwrap_or_default().trim().to_owned(), String::new()));
                    }
                }
                ChatEventKind::SessionUpdate if open && !self.work.in_subagent(payload) => {
                    if UpdateKind::within(payload) != Some(UpdateKind::AgentMessageChunk) {
                        continue;
                    }
                    let said = payload
                        .pointer("/update/content/text")
                        .and_then(Value::as_str);
                    if let (Some(said), Some((_, answer))) = (said, exchanges.last_mut()) {
                        answer.push_str(said);
                    }
                }
                _ => {}
            }
        }
        let first = exchanges.len().saturating_sub(count);
        let start = |text: &str| {
            let text = text.trim();
            let kept: String = text.chars().take(longest).collect();
            if kept.len() < text.len() {
                format!("{kept} [cut]")
            } else {
                kept
            }
        };
        let end = |text: &str| {
            let text = text.trim();
            let skip = text.chars().count().saturating_sub(longest);
            let kept: String = text.chars().skip(skip).collect();
            if skip > 0 {
                format!("[cut] {kept}")
            } else {
                kept
            }
        };
        exchanges[first..]
            .iter()
            .map(|(asked, answered)| (start(asked), end(answered)))
            .collect()
    }

    /// Every exchange with the person it holds, oldest first and whole, and the shell commands
    /// the agent ran, each with whether it failed, once it has ended.
    pub(in super::super) fn handed_over(&self) -> super::super::HandedOver {
        let mut exchanges: Vec<(String, String)> = Vec::new();
        let mut commands: Vec<(String, String, Option<bool>)> = Vec::new();
        let mut open = false;
        for entry in &self.entries {
            let payload = &entry.event.payload;
            match entry.event.kind {
                ChatEventKind::Prompt => {
                    open = payload.get("person") == Some(&Value::Bool(true));
                    if open {
                        let text = payload.get("text").and_then(Value::as_str);
                        exchanges.push((text.unwrap_or_default().trim().to_owned(), String::new()));
                    }
                }
                ChatEventKind::SessionUpdate if !self.work.in_subagent(payload) => {
                    let Some(update) = payload.get("update") else {
                        continue;
                    };
                    let text = |key: &str| update.get(key).and_then(Value::as_str);
                    match UpdateKind::within(payload) {
                        Some(UpdateKind::AgentMessageChunk) if open => {
                            let said = update.pointer("/content/text").and_then(Value::as_str);
                            if let (Some(said), Some((_, answer))) = (said, exchanges.last_mut()) {
                                answer.push_str(said);
                            }
                        }
                        Some(UpdateKind::ToolCall) if text("kind") == Some("execute") => {
                            if let (Some(id), Some(title)) = (text("toolCallId"), text("title")) {
                                commands.push((id.to_owned(), title.trim().to_owned(), None));
                            }
                        }
                        Some(UpdateKind::ToolCallUpdate) => {
                            let ended = match text("status") {
                                Some("completed") => Some(false),
                                Some("failed") => Some(true),
                                _ => None,
                            };
                            let id = text("toolCallId");
                            let ran = commands
                                .iter_mut()
                                .rev()
                                .find(|(each, ..)| Some(each.as_str()) == id);
                            if let (Some(ended), Some((.., failed))) = (ended, ran) {
                                *failed = Some(ended);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        exchanges.retain(|(asked, answered)| !asked.is_empty() || !answered.trim().is_empty());
        let commands = commands
            .into_iter()
            .map(|(_, title, failed)| (title, failed));
        (exchanges, commands.collect())
    }

    /// What the agent wrote last in its latest turn: its text after its last
    /// tool call, or before it when it wrote nothing after.
    pub(in super::super) fn last_answer(&self) -> Option<String> {
        let mut pieces: Vec<&str> = Vec::new();
        for entry in self.entries.iter().rev() {
            match entry.event.kind {
                ChatEventKind::TurnStarted | ChatEventKind::Prompt => break,
                ChatEventKind::SessionUpdate => {}
                _ => continue,
            }
            let payload = &entry.event.payload;
            if self.work.in_subagent(payload) {
                continue;
            }
            match UpdateKind::within(payload) {
                Some(UpdateKind::UserMessageChunk) => break,
                Some(UpdateKind::ToolCall | UpdateKind::ToolCallUpdate) if !pieces.is_empty() => {
                    break
                }
                Some(UpdateKind::AgentMessageChunk) => {
                    if let Some(text) = payload
                        .pointer("/update/content/text")
                        .and_then(Value::as_str)
                    {
                        pieces.push(text);
                    }
                }
                _ => {}
            }
        }
        let answer: String = pieces.into_iter().rev().collect();
        let answer = answer.trim();
        (!answer.is_empty()).then(|| answer.to_owned())
    }

    /// Whether the chat ran anything beyond reading: a command that does more
    /// than look at files, or a process, browser or simulator tool.
    pub(in super::super) fn ran_something(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(entry.event.kind, ChatEventKind::SessionUpdate)
                && UpdateKind::within(&entry.event.payload) == Some(UpdateKind::ToolCall)
                && entry.event.payload.get("update").is_some_and(tool_runs)
        })
    }

    /// What the chat said from index `from` on, or over its last `turns`
    /// turns, stopping at a turn once past `max_bytes`. A message still
    /// streaming in is left for the next read.
    pub(in super::super) fn excerpt(
        &self,
        from: Option<u64>,
        turns: usize,
        max_bytes: usize,
    ) -> (Vec<(u64, bool, Said)>, bool, u64) {
        let start = match from {
            Some(from) => self.entries.partition_point(|entry| entry.index < from),
            None => self
                .entries
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, entry)| entry.starts_turn)
                .take(turns)
                .last()
                .map_or(0, |(position, _)| position),
        };
        let skipped =
            from.is_some_and(|from| self.entries.front().is_some_and(|first| first.index > from));
        let mut said = Vec::new();
        let mut bytes = 0;
        for (position, entry) in self.entries.iter().enumerate().skip(start) {
            if bytes >= max_bytes && entry.starts_turn && !said.is_empty() {
                return (said, skipped, entry.index);
            }
            if self.work.in_subagent(&entry.event.payload) {
                continue;
            }
            let Some(item) = Said::of(&entry.event) else {
                continue;
            };
            let streaming = position + 1 == self.entries.len() && self.in_turn;
            if streaming && matches!(item, Said::Message(_)) {
                return (said, skipped, entry.index);
            }
            bytes += item.bytes();
            said.push((entry.index, entry.starts_turn, item));
        }
        (said, skipped, self.next_index)
    }
}
