//! The messages Claude Code writes on its stream-json output, read once as
//! each line arrives, so a field it left out is reported there rather than
//! read as empty later.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Message {
    ControlResponse {
        response: ControlResponse,
    },
    ControlRequest {
        request_id: String,
        #[serde(default)]
        request: Value,
    },
    ControlCancelRequest {
        request_id: String,
    },
    StreamEvent {
        parent_tool_use_id: Option<String>,
        event: StreamEvent,
    },
    Assistant(Value),
    User(Value),
    Result(TurnResult),
    System(System),
    CommandLifecycle {
        state: Option<String>,
        command_uuid: Option<String>,
    },
    ConversationReset,
    #[serde(other)]
    Other,
}

/// Claude Code's answer to a request this side made.
#[derive(Deserialize)]
pub(crate) struct ControlResponse {
    pub request_id: String,
    pub subtype: Option<String>,
    pub error: Option<String>,
    pub response: Option<Value>,
}

/// A piece of the model's answer as it streams in.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamEvent {
    MessageStart {
        message: StartedMessage,
    },
    ContentBlockStart {
        content_block: Value,
    },
    ContentBlockDelta {
        #[serde(default)]
        index: u64,
        delta: Delta,
    },
    MessageDelta {
        usage: Option<OutputUsage>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(crate) struct StartedMessage {
    pub id: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(crate) struct OutputUsage {
    pub output_tokens: Option<u64>,
}

/// How a turn ended.
#[derive(Deserialize)]
pub(crate) struct TurnResult {
    pub subtype: Option<String>,
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    /// What the turn answered, or what went wrong.
    pub result: Option<String>,
    #[serde(default)]
    pub errors: Vec<Value>,
    /// The message the turn answers.
    pub user_message_uuid: Option<String>,
    /// Present on a turn background work started rather than a message.
    pub origin: Option<Value>,
    pub total_cost_usd: Option<f64>,
    /// Each model the turn used, by name.
    #[serde(rename = "modelUsage")]
    pub model_usage: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
pub(crate) struct System {
    pub claude_code_version: Option<String>,
    #[serde(flatten)]
    pub event: SystemEvent,
}

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(crate) enum SystemEvent {
    SessionStateChanged {
        state: Option<String>,
    },
    /// Claude Code opens every turn with this, including the ones background
    /// work starts.
    Init,
    Status {
        status: Option<String>,
        uuid: Option<String>,
        compact_result: Option<String>,
        compact_error: Option<String>,
    },
    ApiRetry {
        attempt: Option<u64>,
        error_status: Option<u64>,
    },
    CompactBoundary {
        compact_metadata: Option<CompactMetadata>,
    },
    /// Kept whole: a task started in the foreground is started again from it
    /// once it moves to the background.
    TaskStarted(Value),
    TaskProgress(Value),
    TaskUpdated {
        task_id: String,
        #[serde(default)]
        patch: TaskPatch,
    },
    TaskNotification {
        task_id: String,
        status: Option<String>,
        summary: Option<String>,
        output_file: Option<String>,
    },
    BackgroundTasksChanged {
        #[serde(default)]
        tasks: Vec<LiveTask>,
    },
    Informational {
        level: Option<String>,
        content: Option<String>,
    },
    ModelRefusalFallback {
        original_model: Option<String>,
        fallback_model: Option<String>,
    },
    CommandsChanged {
        commands: Option<Value>,
    },
    LocalCommandOutput {
        content: Option<String>,
    },
    PermissionDenied {
        tool_use_id: String,
        decision_reason: Option<String>,
        message: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(crate) struct CompactMetadata {
    pub post_tokens: Option<u64>,
}

#[derive(Default, Deserialize)]
pub(crate) struct TaskPatch {
    pub is_backgrounded: Option<bool>,
    pub status: Option<String>,
    pub output_file: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LiveTask {
    pub task_id: Option<String>,
}
