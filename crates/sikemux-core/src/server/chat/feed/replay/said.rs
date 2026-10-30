//! What a chat said, as another agent reads it, and the title it goes by.

use serde_json::Value;
use sikemux_fold::{ChatEvent, ChatEventKind, UpdateKind};

use super::Replay;

/// One thing said or done in a chat, as another agent reads it.
pub(crate) enum Said {
    /// What the agent was sent: by the person, or another agent's message.
    Asked(String),
    /// Text the agent wrote.
    Message(String),
    Tool {
        id: String,
        title: Option<String>,
        failed: bool,
    },
    TurnStarted,
    TurnEnded {
        cancelled: bool,
    },
    Failed(String),
}

impl Said {
    fn of(event: &ChatEvent) -> Option<Self> {
        let text = |pointer: &str| {
            event
                .payload
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match event.kind {
            ChatEventKind::Prompt => text("/text").map(Said::Asked),
            ChatEventKind::TurnStarted => Some(Said::TurnStarted),
            ChatEventKind::TurnCompleted => Some(Said::TurnEnded {
                cancelled: text("/stopReason").as_deref() == Some("cancelled"),
            }),
            ChatEventKind::Error => Some(Said::Failed(text("/message").unwrap_or_default())),
            ChatEventKind::SessionUpdate => match UpdateKind::within(&event.payload)? {
                UpdateKind::AgentMessageChunk => text("/update/content/text").map(Said::Message),
                UpdateKind::UserMessageChunk => text("/update/content/text").map(Said::Asked),
                UpdateKind::ToolCall | UpdateKind::ToolCallUpdate => Some(Said::Tool {
                    id: text("/update/toolCallId")?,
                    title: text("/update/title"),
                    failed: text("/update/status").as_deref() == Some("failed"),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Said::Asked(text) | Said::Message(text) | Said::Failed(text) => text.len(),
            Said::Tool { id, title, .. } => id.len() + title.as_ref().map_or(0, String::len),
            Said::TurnStarted | Said::TurnEnded { .. } => 0,
        }
    }
}

/// What a chat said from one point on, without its thinking, its subagents'
/// work or what its tools returned.
pub(crate) struct Excerpt {
    /// Names this run of the chat; its indexes start over in the next.
    pub feed: String,
    /// Each with its index, and whether a turn starts there.
    pub said: Vec<(u64, bool, Said)>,
    /// Some of what came after the point asked for is no longer kept.
    pub skipped: bool,
    /// Where reading on starts.
    pub next: u64,
}

/// The most of a chat's title that is kept.
const MAX_TITLE_CHARS: usize = 256;

/// A title as the chat shows it: one line of bounded length without markdown's marks, or
/// none when blank. Text that is only a code block keeps its characters.
pub(crate) fn title(text: &str) -> Option<String> {
    let words = sikemux_markdown::words(text);
    let text = if words.trim().is_empty() {
        text
    } else {
        &words
    };
    let title: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    (!title.is_empty()).then_some(title)
}

pub(crate) fn session_title(payload: &Value) -> Option<String> {
    let update = payload.get("update")?;
    if UpdateKind::of(update)? != UpdateKind::SessionInfoUpdate {
        return None;
    }
    title(update.get("title")?.as_str()?)
}

impl Replay {
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
