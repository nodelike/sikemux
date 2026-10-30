//! The kinds of update a chat agent's session says, each named once: ACP's own,
//! and the ones Sikemux adds for what ACP has no word for. Drivers name the
//! kind they send and the feed, the core and the fold match on it, so a kind
//! cannot be spelled two ways.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    AgentMessageChunk,
    AgentThoughtChunk,
    UserMessageChunk,
    ToolCall,
    ToolCallUpdate,
    Plan,
    AvailableCommandsUpdate,
    CurrentModeUpdate,
    ConfigOptionUpdate,
    UsageUpdate,
    SessionInfoUpdate,
    /// The chat moved to another of the person's accounts.
    AccountSwitched,
    /// Something the agent wants the person to know, outside its answer.
    Notice,
    TaskNotice,
    SubagentSpawned,
    SubagentStateUpdate,
    /// The tokens a subagent has used so far.
    SubagentUsage,
    AsyncTaskSpawned,
    AsyncTaskProgress,
    AsyncTaskStateUpdate,
    /// The session's saved history has all been told.
    HistoryLoaded,
    QueuedMessages,
    EarlierTurnsLost,
    MessageRewound,
}

impl UpdateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentMessageChunk => "agent_message_chunk",
            Self::AgentThoughtChunk => "agent_thought_chunk",
            Self::UserMessageChunk => "user_message_chunk",
            Self::ToolCall => "tool_call",
            Self::ToolCallUpdate => "tool_call_update",
            Self::Plan => "plan",
            Self::AvailableCommandsUpdate => "available_commands_update",
            Self::CurrentModeUpdate => "current_mode_update",
            Self::ConfigOptionUpdate => "config_option_update",
            Self::UsageUpdate => "usage_update",
            Self::SessionInfoUpdate => "session_info_update",
            Self::AccountSwitched => "account_switched",
            Self::Notice => "notice",
            Self::TaskNotice => "task_notice",
            Self::SubagentSpawned => "subagent_spawned",
            Self::SubagentStateUpdate => "subagent_state_update",
            Self::SubagentUsage => "subagent_usage",
            Self::AsyncTaskSpawned => "async_task_spawned",
            Self::AsyncTaskProgress => "async_task_progress",
            Self::AsyncTaskStateUpdate => "async_task_state_update",
            Self::HistoryLoaded => "history_loaded",
            Self::QueuedMessages => "queued_messages",
            Self::EarlierTurnsLost => "earlier_turns_lost",
            Self::MessageRewound => "message_rewound",
        }
    }

    pub fn parse(kind: &str) -> Option<Self> {
        Some(match kind {
            "agent_message_chunk" => Self::AgentMessageChunk,
            "agent_thought_chunk" => Self::AgentThoughtChunk,
            "user_message_chunk" => Self::UserMessageChunk,
            "tool_call" => Self::ToolCall,
            "tool_call_update" => Self::ToolCallUpdate,
            "plan" => Self::Plan,
            "available_commands_update" => Self::AvailableCommandsUpdate,
            "current_mode_update" => Self::CurrentModeUpdate,
            "config_option_update" => Self::ConfigOptionUpdate,
            "usage_update" => Self::UsageUpdate,
            "session_info_update" => Self::SessionInfoUpdate,
            "account_switched" => Self::AccountSwitched,
            "notice" => Self::Notice,
            "task_notice" => Self::TaskNotice,
            "subagent_spawned" => Self::SubagentSpawned,
            "subagent_state_update" => Self::SubagentStateUpdate,
            "subagent_usage" => Self::SubagentUsage,
            "async_task_spawned" => Self::AsyncTaskSpawned,
            "async_task_progress" => Self::AsyncTaskProgress,
            "async_task_state_update" => Self::AsyncTaskStateUpdate,
            "history_loaded" => Self::HistoryLoaded,
            "queued_messages" => Self::QueuedMessages,
            "earlier_turns_lost" => Self::EarlierTurnsLost,
            "message_rewound" => Self::MessageRewound,
            _ => return None,
        })
    }

    /// The kind of `update`, one session update as ACP shapes it.
    pub fn of(update: &Value) -> Option<Self> {
        Self::parse(update.get("sessionUpdate")?.as_str()?)
    }

    /// The kind of the update in `notification`, a session update and the session it is for.
    pub fn within(notification: &Value) -> Option<Self> {
        Self::of(notification.get("update")?)
    }

    /// Whether it sets how the chat stands rather than saying something in it, so a
    /// client shown only the chat's end still needs the last of it.
    pub fn standing(self) -> bool {
        matches!(
            self,
            Self::AvailableCommandsUpdate
                | Self::CurrentModeUpdate
                | Self::ConfigOptionUpdate
                | Self::UsageUpdate
                | Self::SessionInfoUpdate
                | Self::Plan
        )
    }

    pub fn is_tool(self) -> bool {
        matches!(self, Self::ToolCall | Self::ToolCallUpdate)
    }
}

/// The update that sets the card of the subagent in `session` to `state`.
pub fn subagent_state(session: &str, state: &str) -> Value {
    json!({ "sessionUpdate": UpdateKind::SubagentStateUpdate, "subagentSessionId": session, "state": state })
}

/// The update that says how many tokens the subagent in `session` has used.
pub fn subagent_usage(session: &str, tokens: u64) -> Value {
    json!({ "sessionUpdate": UpdateKind::SubagentUsage, "subagentSessionId": session, "tokens": tokens })
}

/// A tool call's content holding `block`.
pub fn content(block: Value) -> Value {
    json!({ "type": "content", "content": block })
}

/// A tool call's content holding `text`.
pub fn text_content(text: &str) -> Value {
    content(json!({ "type": "text", "text": text }))
}

/// A piece of what the agent said, in the message `message_id` when it has one.
pub fn agent_text(text: &str, message_id: Option<&str>) -> Value {
    chunk(UpdateKind::AgentMessageChunk, text, message_id)
}

/// A piece of the agent's thinking, in the message `message_id` when it has one.
pub fn thought(text: &str, message_id: Option<&str>) -> Value {
    chunk(UpdateKind::AgentThoughtChunk, text, message_id)
}

fn chunk(kind: UpdateKind, text: &str, message_id: Option<&str>) -> Value {
    let mut update = json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } });
    if let Some(id) = message_id {
        update["messageId"] = json!(id);
    }
    update
}

/// A note in the chat from the agent or about it, at `severity` ("info",
/// "warning" or "error"), with more on it below the title when there is any.
pub fn notice(severity: &str, title: &str, description: Option<&str>) -> Value {
    let mut update =
        json!({ "sessionUpdate": UpdateKind::Notice, "severity": severity, "title": title });
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        update["description"] = json!(description);
    }
    update
}

/// The tool call row that stands for the agent compacting what it remembers, opened
/// when `opens`, else brought to `status`, with what went wrong when it failed.
pub fn compaction(id: &str, opens: bool, status: &str, error: Option<&str>) -> Value {
    let mut update = json!({
        "sessionUpdate": if opens { UpdateKind::ToolCall } else { UpdateKind::ToolCallUpdate },
        "toolCallId": id,
        "title": "Compact conversation",
        "status": status,
    });
    if opens {
        update["kind"] = json!("think");
    }
    if let Some(error) = error {
        update["content"] = json!([text_content(error)]);
    }
    update
}

/// Marks a replayed update with when it first happened, in Unix milliseconds,
/// and whether it began a turn the agent started by itself, so a client
/// rebuilding the chat tells its turns apart.
pub fn stamp_replayed(update: &mut Value, at: Option<u64>, opens_turn: bool) {
    let Some(fields) = update
        .as_object_mut()
        .filter(|_| at.is_some() || opens_turn)
    else {
        return;
    };
    let meta = fields.entry("_meta").or_insert_with(|| json!({}));
    let Some(sikemux) = meta
        .as_object_mut()
        .map(|meta| meta.entry("sikemux").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(at) = at {
        sikemux.insert("at".into(), json!(at));
    }
    if opens_turn {
        sikemux.insert("opensTurn".into(), json!(true));
    }
}

fn sikemux_meta(update: &Value) -> Option<&Map<String, Value>> {
    update.get("_meta")?.get("sikemux")?.as_object()
}

/// When a replayed update first happened, as [`stamp_replayed`] marked it.
pub fn stamped_at(update: &Value) -> Option<u64> {
    sikemux_meta(update)?.get("at").and_then(Value::as_u64)
}

/// Whether a replayed update began a turn the agent started by itself, and when.
pub fn opens_turn(update: &Value) -> Option<Option<u64>> {
    let kept = sikemux_meta(update)?;
    (kept.get("opensTurn") == Some(&Value::Bool(true)))
        .then(|| kept.get("at").and_then(Value::as_u64))
}
