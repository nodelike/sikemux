//! The notifications from Codex the chat acts on, read as they arrive, so a
//! field Codex left out is caught here rather than read as empty later. They
//! borrow from the notification, which streamed text arrives in many times a
//! second.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
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
#[derive(Clone, Copy, Deserialize)]
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

/// How a turn ended, all a subagent's ending is read for.
#[derive(Deserialize)]
pub(super) struct TurnEnded<'a> {
    #[serde(borrow)]
    pub turn: TurnStatus<'a>,
}

#[derive(Deserialize)]
pub(super) struct TurnStatus<'a> {
    /// `completed`, `interrupted` or `failed`.
    pub status: &'a str,
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

/// Where a message from Codex belongs: the thread and the turn it names, when it names them.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Place<'a> {
    pub thread_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
}

/// A turn of a thread's history, as `thread/read` and `thread/turns/list` answer; its
/// items are read by their kind.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryTurn<'a> {
    pub id: &'a str,
    /// `completed`, `interrupted` or `failed`, once it ended.
    pub status: Option<&'a str>,
    /// In seconds.
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
}

/// An item about Codex's subagents, which the chat routes to their own sessions.
pub(super) enum SubagentItem<'a> {
    Activity(SubagentActivity<'a>),
    Collaboration(Collaboration<'a>),
}

/// A subagent starting or stopping, said in the thread of the agent that runs it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubagentActivity<'a> {
    pub kind: &'a str,
    pub agent_thread_id: &'a str,
    /// Where it sits under the agent that started it, such as `/root/code_reviewer`.
    pub agent_path: &'a str,
}

/// A call one of Codex's agents made to start, message or wait on others.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Collaboration<'a> {
    pub tool: &'a str,
    pub status: &'a str,
    pub sender_thread_id: &'a str,
    pub receiver_thread_ids: Vec<&'a str>,
    pub prompt: Option<&'a str>,
    /// Each agent the call is about, by its thread, in the order Codex sent them.
    #[serde(deserialize_with = "in_order")]
    pub agents_states: Vec<(&'a str, AgentState<'a>)>,
}

#[derive(Deserialize)]
pub(super) struct AgentState<'a> {
    pub status: &'a str,
}

fn in_order<'de, D, K, V>(deserializer: D) -> Result<Vec<(K, V)>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    struct Entries<K, V>(PhantomData<(K, V)>);

    impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for Entries<K, V> {
        type Value = Vec<(K, V)>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }

    deserializer.deserialize_map(Entries(PhantomData))
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

/// `params` of `method` read as `T`, said in the core's log when Codex sent a shape Sikemux
/// cannot read.
pub(super) fn readable<'a, T: Deserialize<'a>>(
    method: &str,
    params: &'a Value,
) -> Result<T, String> {
    T::deserialize(params).map_err(|error| {
        eprintln!("sikemux core: Codex sent a {method} Sikemux cannot read: {error}");
        error.to_string()
    })
}

/// Where `method`'s message belongs, as far as it says.
pub(super) fn place<'a>(method: &str, params: &'a Value) -> Place<'a> {
    readable(method, params).unwrap_or_default()
}

/// What `item` of `method` says of itself, or None when Sikemux cannot read it.
pub(super) fn item_head<'a>(method: &str, item: &'a Value) -> Option<ItemHead<'a>> {
    readable(method, item).ok()
}

/// The subagent item `item` is, or None for any other item.
pub(super) fn subagent_item<'a>(method: &str, item: &'a Value) -> Option<SubagentItem<'a>> {
    match ItemHead::deserialize(item).ok()?.kind {
        "subAgentActivity" => readable(method, item).ok().map(SubagentItem::Activity),
        "collabAgentToolCall" => readable(method, item).ok().map(SubagentItem::Collaboration),
        _ => None,
    }
}

/// `method`'s notification, or None for one the chat does not act on.
pub(super) fn read<'a>(method: &'a str, params: &'a Value) -> Option<Notification<'a>> {
    fn parsed<'a, T: Deserialize<'a>>(
        method: &'a str,
        params: &'a Value,
        into: impl FnOnce(T) -> Notification<'a>,
    ) -> Notification<'a> {
        match readable(method, params) {
            Ok(read) => into(read),
            Err(error) => Notification::Unreadable { method, error },
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
