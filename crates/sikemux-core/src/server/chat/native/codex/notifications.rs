//! The notifications from Codex the chat acts on, read as they arrive, so a
//! field Codex left out is caught here rather than read as empty later. They
//! borrow from the notification, which streamed text arrives in many times a
//! second.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) enum Notification<'a> {
    MessageDelta(Delta<'a>),
    ReasoningDelta(Delta<'a>),
    ReasoningPartAdded(OfItem<'a>),
    ItemStarted(ItemEvent<'a>),
    ItemCompleted(ItemEvent<'a>),
    TurnStarted(TurnStarted<'a>),
    TurnCompleted(TurnCompleted<'a>),
    PlanUpdated(PlanUpdated),
    Error(ErrorNotice<'a>),
    TokenUsage(TokenUsage),
    ThreadNamed(ThreadNamed),
    McpServerStatus(McpServerStatus<'a>),
    ModelRerouted(ModelRerouted<'a>),
    Compacted,
    Warning(Warning<'a>),
    ConfigWarning(ConfigWarning<'a>),
    /// A notification the chat needs that Codex sent in a shape Sikemux
    /// cannot read.
    Unreadable {
        method: &'a str,
        error: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Delta<'a> {
    pub item_id: &'a str,
    pub delta: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OfItem<'a> {
    pub item_id: &'a str,
}

pub(super) struct ItemEvent<'a> {
    pub thread_id: Option<&'a str>,
    pub item: ItemHead<'a>,
    /// The item as Codex sent it, read further by its kind.
    pub whole: &'a Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemFields<'a> {
    thread_id: Option<&'a str>,
    #[serde(borrow)]
    item: ItemHead<'a>,
}

/// What every item says of itself; the rest is read by the item's kind.
#[derive(Deserialize)]
pub(super) struct ItemHead<'a> {
    pub id: &'a str,
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub server: Option<&'a str>,
}

#[derive(Deserialize)]
pub(super) struct TurnStarted<'a> {
    #[serde(borrow)]
    pub turn: TurnId<'a>,
}

#[derive(Deserialize)]
pub(super) struct TurnId<'a> {
    pub id: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnCompleted<'a> {
    pub thread_id: Option<&'a str>,
    pub turn: EndedTurn,
}

/// A turn Codex says ended, and how.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct EndedTurn {
    pub id: String,
    /// `completed`, `interrupted` or `failed`.
    pub status: String,
    pub error: Option<Value>,
}

#[derive(Deserialize)]
pub(super) struct PlanUpdated {
    #[serde(default)]
    pub plan: Vec<PlanStep>,
}

#[derive(Deserialize)]
pub(super) struct PlanStep {
    pub step: String,
    pub status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ErrorNotice<'a> {
    pub turn_id: Option<&'a str>,
    #[serde(default)]
    pub will_retry: bool,
    pub error: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenUsage {
    pub token_usage: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadNamed {
    pub thread_name: Value,
}

#[derive(Deserialize)]
pub(super) struct McpServerStatus<'a> {
    pub name: &'a str,
    pub status: &'a str,
    pub error: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelRerouted<'a> {
    pub from_model: &'a str,
    pub to_model: &'a str,
    pub reason: &'a str,
}

#[derive(Deserialize)]
pub(super) struct Warning<'a> {
    pub message: &'a str,
}

#[derive(Deserialize)]
pub(super) struct ConfigWarning<'a> {
    pub summary: &'a str,
    pub details: Option<&'a str>,
}

impl<'a> ItemFields<'a> {
    fn with(self, params: &'a Value) -> ItemEvent<'a> {
        ItemEvent {
            thread_id: self.thread_id,
            item: self.item,
            whole: &params["item"],
        }
    }
}

/// `method`'s notification, or None for one the chat does not act on.
pub(super) fn read<'a>(method: &'a str, params: &'a Value) -> Option<Notification<'a>> {
    fn parsed<'a, T: Deserialize<'a>>(
        method: &'a str,
        params: &'a Value,
        into: impl FnOnce(T) -> Notification<'a>,
    ) -> Notification<'a> {
        match T::deserialize(params) {
            Ok(read) => into(read),
            Err(error) => {
                eprintln!("sikemux core: Codex sent a {method} Sikemux cannot read: {error}");
                Notification::Unreadable {
                    method,
                    error: error.to_string(),
                }
            }
        }
    }
    Some(match method {
        "item/agentMessage/delta" => parsed(method, params, Notification::MessageDelta),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            parsed(method, params, Notification::ReasoningDelta)
        }
        "item/reasoning/summaryPartAdded" => {
            parsed(method, params, Notification::ReasoningPartAdded)
        }
        "item/started" => parsed(method, params, |fields: ItemFields<'a>| {
            Notification::ItemStarted(fields.with(params))
        }),
        "item/completed" => parsed(method, params, |fields: ItemFields<'a>| {
            Notification::ItemCompleted(fields.with(params))
        }),
        "turn/started" => parsed(method, params, Notification::TurnStarted),
        "turn/completed" => parsed(method, params, Notification::TurnCompleted),
        "turn/plan/updated" => parsed(method, params, Notification::PlanUpdated),
        "error" => parsed(method, params, Notification::Error),
        "thread/tokenUsage/updated" => parsed(method, params, Notification::TokenUsage),
        "thread/name/updated" => parsed(method, params, Notification::ThreadNamed),
        "mcpServer/startupStatus/updated" => parsed(method, params, Notification::McpServerStatus),
        "model/rerouted" => parsed(method, params, Notification::ModelRerouted),
        "thread/compacted" => Notification::Compacted,
        "warning" => parsed(method, params, Notification::Warning),
        "configWarning" => parsed(method, params, Notification::ConfigWarning),
        _ => return None,
    })
}
