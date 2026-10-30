//! The notifications from Muse the chat acts on, read once as they arrive, so
//! a field Muse left out is caught here rather than read as empty later.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) enum Notification {
    /// An item opened, changed or finished.
    Item {
        stage: Stage,
        params: ItemParams,
    },
    ItemDelta(ItemDelta),
    TurnStarted(TurnStarted),
    TurnCompleted(TurnCompleted),
    RetryScheduled(RetryScheduled),
    ModelRouteUnserved(InSession),
    TokenUsage(TokenUsage),
    ContextUsage(ContextUsage),
    NameChanged(NameChanged),
    TodoListChanged(TodoListChanged),
    /// A notification the chat needs that Muse sent in a shape Sikemux
    /// cannot read.
    Unreadable {
        method: String,
        error: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Opened,
    Completed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InSession {
    pub session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemParams {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub item: Value,
}

impl ItemParams {
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id
            .as_deref()
            .or_else(|| self.item["turnId"].as_str())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemDelta {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub item_id: String,
    pub delta: String,
    /// Which of the item's texts grows: `text`, unless Muse says otherwise.
    pub field: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnStarted {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnCompleted {
    pub session_id: String,
    pub turn_id: String,
    /// How the turn ended: `completed`, `cancelled`, or how it failed.
    pub terminal: String,
    pub usage: Option<TurnUsage>,
    pub error: Option<Value>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RetryScheduled {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub next_attempt: u64,
    pub max_attempts: u64,
    pub reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenUsage {
    pub session_id: String,
    pub cumulative: Totals,
}

/// The session's running token totals.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Totals {
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContextUsage {
    pub session_id: String,
    #[serde(flatten)]
    pub window: Window,
}

/// How full the context window is.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Window {
    pub used_tokens: Option<u64>,
    pub window_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NameChanged {
    pub session_id: String,
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TodoListChanged {
    pub session_id: String,
    #[serde(default)]
    pub items: Vec<Todo>,
}

#[derive(Deserialize)]
pub(super) struct Todo {
    pub text: String,
    pub status: String,
}

impl Notification {
    /// The session the notification is about, when the chat reads it.
    pub fn session_id(&self) -> Option<&str> {
        Some(match self {
            Self::Item { params, .. } => &params.session_id,
            Self::ItemDelta(delta) => &delta.session_id,
            Self::TurnStarted(started) => &started.session_id,
            Self::TurnCompleted(completed) => &completed.session_id,
            Self::RetryScheduled(retry) => &retry.session_id,
            Self::ModelRouteUnserved(unserved) => &unserved.session_id,
            Self::TokenUsage(usage) => &usage.session_id,
            Self::ContextUsage(usage) => &usage.session_id,
            Self::NameChanged(named) => &named.session_id,
            Self::TodoListChanged(todos) => &todos.session_id,
            Self::Unreadable { .. } => return None,
        })
    }
}

/// `method`'s notification, or None for one the chat does not act on.
pub(super) fn read(method: &str, params: Value) -> Option<Notification> {
    fn parsed<T: serde::de::DeserializeOwned>(
        method: &str,
        params: Value,
        into: impl FnOnce(T) -> Notification,
    ) -> Notification {
        match serde_json::from_value(params) {
            Ok(read) => into(read),
            Err(error) => {
                eprintln!("sikemux core: Muse sent a {method} Sikemux cannot read: {error}");
                Notification::Unreadable {
                    method: method.to_owned(),
                    error: error.to_string(),
                }
            }
        }
    }
    let item = |stage| move |params| Notification::Item { stage, params };
    Some(match method {
        "item/started" | "item/updated" => parsed(method, params, item(Stage::Opened)),
        "item/completed" => parsed(method, params, item(Stage::Completed)),
        "item/delta" => parsed(method, params, Notification::ItemDelta),
        "turn/started" => parsed(method, params, Notification::TurnStarted),
        "turn/completed" => parsed(method, params, Notification::TurnCompleted),
        "turn/retryScheduled" => parsed(method, params, Notification::RetryScheduled),
        "session/modelRouteUnserved" => parsed(method, params, Notification::ModelRouteUnserved),
        "session/tokenUsage" => parsed(method, params, Notification::TokenUsage),
        "session/contextUsage" => parsed(method, params, Notification::ContextUsage),
        "session/nameChanged" => parsed(method, params, Notification::NameChanged),
        "session/todoListChanged" => parsed(method, params, Notification::TodoListChanged),
        _ => return None,
    })
}
