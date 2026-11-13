//! The kinds of update a chat agent's session says, each named once: ACP's own,
//! and the ones Sikemux adds for what ACP has no word for. Drivers name the
//! kind they send and the feed, the core and the fold match on it, so a kind
//! cannot be spelled two ways.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_view::chat::{
    ChiefWeek, GoalMarker, KeptRule, OverruledDecision, RuleChange, StopReason,
};

use crate::ChatEventKind;

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
    AccountSwitched,
    /// The core started a fresh session under the chat.
    SessionRotated,
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
    QueuedMessages,
    EarlierTurnsLost,
    MessageRewound,
    /// What the agent predicts the person will send next, once a turn of theirs has ended.
    PromptSuggestion,
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
            Self::SessionRotated => "session_rotated",
            Self::Notice => "notice",
            Self::TaskNotice => "task_notice",
            Self::SubagentSpawned => "subagent_spawned",
            Self::SubagentStateUpdate => "subagent_state_update",
            Self::SubagentUsage => "subagent_usage",
            Self::AsyncTaskSpawned => "async_task_spawned",
            Self::AsyncTaskProgress => "async_task_progress",
            Self::AsyncTaskStateUpdate => "async_task_state_update",
            Self::QueuedMessages => "queued_messages",
            Self::EarlierTurnsLost => "earlier_turns_lost",
            Self::MessageRewound => "message_rewound",
            Self::PromptSuggestion => "prompt_suggestion",
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
            "session_rotated" => Self::SessionRotated,
            "notice" => Self::Notice,
            "task_notice" => Self::TaskNotice,
            "subagent_spawned" => Self::SubagentSpawned,
            "subagent_state_update" => Self::SubagentStateUpdate,
            "subagent_usage" => Self::SubagentUsage,
            "async_task_spawned" => Self::AsyncTaskSpawned,
            "async_task_progress" => Self::AsyncTaskProgress,
            "async_task_state_update" => Self::AsyncTaskStateUpdate,
            "queued_messages" => Self::QueuedMessages,
            "earlier_turns_lost" => Self::EarlierTurnsLost,
            "message_rewound" => Self::MessageRewound,
            "prompt_suggestion" => Self::PromptSuggestion,
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

pub fn tool_call(id: &str, kind: &str, title: &str, status: &str) -> Value {
    json!({
        "sessionUpdate": UpdateKind::ToolCall,
        "toolCallId": id,
        "kind": kind,
        "title": title,
        "status": status,
    })
}

pub fn tool_update(id: &str, status: &str) -> Value {
    json!({ "sessionUpdate": UpdateKind::ToolCallUpdate, "toolCallId": id, "status": status })
}

/// A status word as an agent writes it, as the chat reads one.
pub fn tool_status(word: &str) -> &'static str {
    match word {
        "completed" => "completed",
        "inProgress" | "in_progress" | "pending" | "" => "in_progress",
        _ => "failed",
    }
}

/// A command run in a terminal, with `input` saying what ran and where.
pub fn terminal_call(id: &str, status: &str, title: &str, input: Value) -> Value {
    let mut call = tool_call(id, "execute", title, status);
    call["rawInput"] = input;
    call
}

/// A command that finished, having printed `output`.
pub fn terminal_done(id: &str, status: &str, output: &str) -> Value {
    let mut update = tool_update(id, status);
    update["rawOutput"] = json!({ "formatted_output": output });
    update
}

/// How the chat names a tool server's tool, which it draws as one.
pub fn mcp_title(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// A call of a tool server's tool, with `arguments` as the model wrote them.
pub fn mcp_call(id: &str, status: &str, server: &str, tool: &str, arguments: &Value) -> Value {
    let mut call = tool_call(id, "other", &mcp_title(server, tool), status);
    call["rawInput"] = mcp_input(server, tool, arguments);
    call
}

pub fn mcp_input(server: &str, tool: &str, arguments: &Value) -> Value {
    json!({ "server": server, "tool": tool, "arguments": arguments })
}

pub fn search_title(query: Option<&str>, path: Option<&str>) -> String {
    match (
        query.filter(|query| !query.is_empty()),
        path.filter(|path| !path.is_empty()),
    ) {
        (Some(query), Some(path)) => format!("Search for '{query}' in {path}"),
        (Some(query), None) => format!("Search for '{query}'"),
        (None, Some(path)) => format!("Search in '{path}'"),
        (None, None) => "Search".to_owned(),
    }
}

pub fn read_title(path: &str) -> String {
    format!("Read file '{path}'")
}

pub fn web_search_title(query: Option<&str>) -> String {
    query.filter(|query| !query.is_empty()).map_or_else(
        || "Web search".to_owned(),
        |query| format!("Web search: {query}"),
    )
}

/// The title of an edit that names no one file.
pub const EDITING_FILES: &str = "Editing files";

/// A file before and after an edit, with no `old` for a new file.
pub fn diff_block(path: &str, old: Option<&str>, new: &str) -> Value {
    json!({ "type": "diff", "path": path, "oldText": old, "newText": new })
}

pub fn plan(entries: Vec<Value>) -> Value {
    json!({ "sessionUpdate": UpdateKind::Plan, "entries": entries })
}

/// One step of a plan, with its status as the agent writes it.
pub fn plan_entry(content: &str, status: &str) -> Value {
    let status = match status {
        "inProgress" | "in_progress" => "in_progress",
        "completed" | "cancelled" => "completed",
        _ => "pending",
    };
    json!({ "content": content, "status": status, "priority": "medium" })
}

/// What the agent calls the session; null when it named it nothing after all.
pub fn session_title(title: impl Serialize) -> Value {
    json!({ "sessionUpdate": UpdateKind::SessionInfoUpdate, "title": title })
}

/// What the agent predicts the person will send next.
pub fn prompt_suggestion(suggestion: &str) -> Value {
    json!({ "sessionUpdate": UpdateKind::PromptSuggestion, "suggestion": suggestion })
}

/// How full the context window is.
pub fn usage(used: u64, size: u64) -> Value {
    json!({ "sessionUpdate": UpdateKind::UsageUpdate, "used": used, "size": size })
}

/// The tokens a turn used, as an agent counts them.
pub struct TurnUsage {
    pub total: u64,
    /// Counting the ones read from the cache.
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thought: u64,
}

impl TurnUsage {
    /// As a turn's answer holds it, under `usage`.
    pub fn answer(&self) -> Value {
        json!({
            "totalTokens": self.total,
            "inputTokens": self.input.saturating_sub(self.cached),
            "cachedReadTokens": self.cached,
            "outputTokens": self.output,
            "thoughtTokens": self.thought,
        })
    }
}

pub fn turn_answer(reason: StopReason) -> Value {
    json!({ "stopReason": reason })
}

/// Set on an error event said while a turn is at work and goes on, such as a task
/// that could not be stopped.
pub const TURN_GOES_ON: &str = "turnGoesOn";

/// Whether an event of `kind` with `payload` ends the turn at work.
pub fn ends_turn(kind: ChatEventKind, payload: &Value) -> bool {
    match kind {
        ChatEventKind::TurnCompleted => true,
        ChatEventKind::Error => payload.get(TURN_GOES_ON).is_none(),
        _ => false,
    }
}

/// Why the turn with the answer `answer` ended.
pub fn stop_reason(answer: &Value) -> Option<StopReason> {
    StopReason::deserialize(answer.get("stopReason")?).ok()
}

pub fn subagent_state(session: &str, state: &str) -> Value {
    json!({ "sessionUpdate": UpdateKind::SubagentStateUpdate, "subagentSessionId": session, "state": state })
}

pub fn subagent_usage(session: &str, tokens: u64) -> Value {
    json!({ "sessionUpdate": UpdateKind::SubagentUsage, "subagentSessionId": session, "tokens": tokens })
}

pub fn content(block: Value) -> Value {
    json!({ "type": "content", "content": block })
}

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

/// The line in a chat where the goal `marker` began or ended.
pub fn goal_notice(title: &str, marker: GoalMarker) -> Value {
    let mut update = notice("info", title, None);
    update["goal"] = json!(marker);
    update
}

/// The line in the chief's chat that says the person overruled a decision.
pub fn overruled_notice(title: &str, overruled: OverruledDecision) -> Value {
    let mut update = notice("info", title, None);
    update["overruled"] = json!(overruled);
    update
}

/// The line in the chief's chat that counts its week.
pub fn week_notice(title: &str, week: ChiefWeek) -> Value {
    let mut update = notice("info", title, None);
    update["week"] = json!(week);
    update
}

/// The line in the chief's chat that says a rule of the person's is now a check.
pub fn rule_change_notice(title: &str, rule_change: RuleChange) -> Value {
    let mut update = notice("info", title, None);
    update["ruleChange"] = json!(rule_change);
    update
}

/// The line in a chat that says a rule was kept from what the person said there.
pub fn rule_notice(rule: KeptRule) -> Value {
    let title = match rule.undone {
        true => "Kept as a rule, then undone",
        false => "Kept as a rule",
    };
    let mut update = notice("info", title, None);
    update["rule"] = json!(rule);
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
        "_meta": { "sikemux": { "compaction": true } },
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

/// The id of a compaction [`compaction`] says has finished.
pub fn finished_compaction(update: &Value) -> Option<&str> {
    let marked = sikemux_meta(update)?.get("compaction") == Some(&Value::Bool(true));
    (marked && update.get("status").and_then(Value::as_str) == Some("completed"))
        .then(|| update.get("toolCallId").and_then(Value::as_str))
        .flatten()
}

/// Whether a replayed update began a turn the agent started by itself, and when.
pub fn opens_turn(update: &Value) -> Option<Option<u64>> {
    let kept = sikemux_meta(update)?;
    (kept.get("opensTurn") == Some(&Value::Bool(true)))
        .then(|| kept.get("at").and_then(Value::as_u64))
}
