//! A chat as the core folds it from what its agent says, and the changes to
//! it clients apply. Clients draw this; none of them reads the agent's own
//! messages. Events, attachments and pages carry it as JSON, so a field added
//! here reaches the phone with an over-the-air update.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use sikemux_wire::protocol::ChatContext;

use crate::slice::SliceState;

/// A chat as a client opening it is sent: how it stands, its latest rows, and
/// the rows of the subagents those name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatView {
    pub head: ChatHead,
    pub rows: Vec<ChatMessage>,
    /// Each subagent's own rows, by its session.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subagents: BTreeMap<String, Vec<ChatMessage>>,
    /// Earlier rows start before this event, and `AcpHistory` pages back to them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_before: Option<u64>,
}

/// Rows from before or after what a client holds, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPage {
    pub rows: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subagents: BTreeMap<String, Vec<ChatMessage>>,
    /// What to page back from for the rows before these; none at the chat's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_before: Option<u64>,
    /// What to page on from for the rows after these; none once they reach the chat's end,
    /// which the client then follows live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer_after: Option<u64>,
}

/// One change to a chat a client holds. `in` names the subagent session whose
/// rows it changes; without it, the chat's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatChange {
    /// These fields of the head change; the rest stay. A delivery that catches a client up
    /// opens with one naming the number it follows, rather than one less than its own.
    Head {
        set: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follows: Option<u64>,
    },
    /// A new row goes at the end, or a row already held is replaced whole.
    Row {
        row: ChatMessage,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        within: Option<String>,
    },
    /// A new part goes at the end of the row, or a part already held is replaced.
    Part {
        row: String,
        part: ChatPart,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        within: Option<String>,
    },
    /// Text written on to the end of a text or thought part, and what that
    /// changes in its markdown tree.
    Text {
        row: String,
        part: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        md: Option<Value>,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        within: Option<String>,
    },
    /// These fields of the row change; its parts stay.
    Set {
        row: String,
        set: Value,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        within: Option<String>,
    },
    /// Only these rows stay, in this order: a client catching up drops the rest.
    Keep { rows: Vec<String> },
    /// The row and every row after it go.
    Drop { from: String },
    /// Everything held is replaced.
    View { view: Box<ChatView> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatConnection {
    #[default]
    Connecting,
    Starting,
    Initializing,
    Ready,
    Stopped,
    Error,
}

/// What a chat is doing, which each client words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatActivity {
    /// Going at a request again, as `Retrying` says.
    Retrying {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        connection: bool,
        /// What went wrong, in the agent's words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Running the call this row draws.
    Call {
        row: Box<ToolRow>,
    },
    Thinking,
    /// The agent's program is starting, with something sent to it.
    Starting,
    /// Its session is being reached, with something sent to it.
    Connecting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    /// A reason a newer agent gives.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub priority: PlanPriority,
    #[serde(default)]
    pub status: PlanStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    #[default]
    Medium,
    Low,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    /// A status a newer agent gives, taken as still under way.
    #[serde(other)]
    Unknown,
}

impl ToolStatus {
    pub fn running(self) -> bool {
        !matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// What the chat's agent can do beyond taking a prompt.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCapabilities {
    /// It takes a message in the middle of a turn.
    #[serde(default)]
    pub steering: bool,
    /// A sent message can be edited, which takes the chat back to it.
    #[serde(default)]
    pub editing: bool,
    /// The ways the person can sign its account in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sign_in_methods: Vec<SignInMethod>,
}

/// A way an agent says it can be signed in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SignInMethod {
    /// Run the agent's own program again with `args` and `env`, in a terminal
    /// the person can see. A zero exit means it worked.
    Terminal {
        id: String,
        name: String,
        description: Option<String>,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Ask the agent to sign in by itself, through `authenticate`.
    Agent {
        id: String,
        name: String,
        description: Option<String>,
    },
}

/// How a chat stands, apart from what it said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub connection: ChatConnection,
    pub running: bool,
    /// A turn has run in this session, so the provider keeps it.
    pub turned: bool,
    pub permission_mode: sikemux_wire::protocol::PermissionMode,
    /// A change of the permission mode is on its way to the agent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changing_mode: bool,
    /// A turn has begun and its first row has not arrived yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_opening: Option<TurnStart>,
    /// The agent is retrying a request the network or its service failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrying: Option<Retrying>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ChatFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ContextUsage>,
    /// How often the agent has compacted the conversation to free its context, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compactions: Option<Compactions>,
    /// The account smart accounts started the chat on, and why, shown before its first message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<AccountMove>,
    /// The steps the agent plans for the turn, once it has said them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<PlanEntry>>,
    pub commands: Vec<ChatCommandInfo>,
    pub capabilities: ChatCapabilities,
    pub setup: Value,
    /// The model the session runs on, and the ones it can switch to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::workspace::ConfigPicker>,
    /// How hard the model thinks. An agent that always offers it has one
    /// with no choices until its session says them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::workspace::ConfigPicker>,
    /// Standard speed or one of the faster tiers the agent offers for the model, while it
    /// offers any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<crate::workspace::ConfigPicker>,
    pub permissions: Vec<PermissionAsk>,
    pub tasks: Vec<AsyncTask>,
    /// The chat's own subagents still at work.
    pub running_subagents: Vec<SubagentCard>,
    /// What the person sent mid-turn, waiting to go out once the turn ends.
    pub queued: Vec<QueuedMessage>,
    /// What viewers of the chat's share link wrote, waiting for the person to send or dismiss.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guests: Vec<GuestMessage>,
    /// The triggers that send this chat a message when their time comes.
    #[serde(default)]
    pub triggers: Vec<ChatTrigger>,
    /// The host no longer keeps the turns before the first row.
    pub earlier_lost: bool,
    /// Why the agent's session ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<SessionEnded>,
    /// The tool part still running at the end of the last row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<String>,
    /// What the line under the last row says the chat is doing; none while a permission card
    /// already says what the turn waits on, or while nothing is happening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<ChatActivity>,
    /// How its turn stands; none before its first turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<crate::turn::TurnState>,
    /// It is its project's chief, @Chief: the one chat of the project every other agent there
    /// works under.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chief: bool,
    /// The project's chief by agent id, on every other chat of a project that has one: what the
    /// person types here reaches the chief in its next digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chief_hears: Option<String>,
    /// On the chief's chat, what happened while the person was away, from when they come back
    /// after an hour or more until every call in it is answered, or, with none in it, until they
    /// next write to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<ChiefAway>,
    /// The pages its latest turns changed, as the core took them, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<crate::shown::TurnShown>,
    /// Its workers' finished work, oldest first; empty for a chat that leads nobody.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<crate::shown::ResultRow>,
    /// What the agent predicts the person will send next, from the turn that ended last; none
    /// while a turn runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// What the next message hands over to an agent that takes the chat over from another, or
    /// starts it as a fork; none once sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
    /// The chat's prompt cache has expired with a big history in it, so its next message reads
    /// all of it again; none while the cache is warm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired_cache: Option<ExpiredCache>,
    /// What the person has written and not sent, as the host keeps it for every app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<ChatDraft>,
    /// The goal the chat works on now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<ChatGoal>,
}

/// A chat whose prompt cache has expired, and what its next message does about it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpiredCache {
    /// The tokens the next message reads again.
    pub tokens: u64,
    /// When the cache expired, in Unix milliseconds.
    pub at: u64,
    /// The chat is summarised before the next message goes; off, it keeps its full history.
    pub summarise: bool,
}

/// The goal a chat works on, as its composer shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGoal {
    pub project: String,
    pub card: u64,
    pub name: String,
    pub state: SliceState,
    /// How it stands, in order: one step per gate of a goal of one slice, else one per slice.
    /// None for a goal with no slice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<SliceState>,
    /// Its steps are gates rather than slices.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub of_gates: bool,
    /// The chat's agent only just added it, and the person can still take that back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undoable: bool,
}

/// A chat's unsent draft: its text, what the apps keep beside it, and its revision, which a
/// `setDraft` names as the one it was written over.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDraft {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<DraftExtra>,
    pub rev: u64,
}

/// What a draft keeps beside its text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftExtra {
    /// The agents, files, chats and issues the text names, over the words that name them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<DraftToken>,
    /// The files sent with the text, by path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// The links and texts sent with the text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ChatContext>,
    /// The issues and pull requests the tokens name, by the token's id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tracked: BTreeMap<String, ChatContext>,
    /// What this build does not read, kept so that saving the draft loses none of it.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

impl DraftExtra {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A chip over the draft's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftToken {
    /// Where it starts in the text, in UTF-8 bytes.
    pub start: usize,
    /// Where it ends in the text, in UTF-8 bytes.
    pub end: usize,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Starts the key the core keeps a chat's draft under, before its agent's id.
pub const CHAT_DRAFT: &str = "chat:";

/// The composer at the bottom of the goals' Home, which keeps its draft as a chat's does but
/// belongs to no agent.
pub const START_COMPOSER: &str = "goals-start";

pub fn chat_draft_key(agent: &str) -> String {
    format!("{CHAT_DRAFT}{agent}")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEnded {
    /// `requested`, `exited` after it was up, or `failed` before it got there.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStart {
    /// Unix milliseconds, when the core knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retrying {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    /// The connection was lost, rather than the service answering with an error.
    pub connection: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    SignIn,
    Limit,
    Network,
    Other,
    /// A kind a newer core sends.
    #[serde(other)]
    Unknown,
}

/// Why the last turn failed, when it was the account's or the network's doing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFailure {
    pub kind: FailureKind,
    /// What the agent itself said, behind the plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// The profile id of the account it failed on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// When the core sends the stopped prompt again, once a usage limit resets, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumes_at: Option<u64>,
    /// Another agent with room to take the chat over, when every account of its own is out of
    /// usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_on_with: Option<String>,
}

/// How full the session's context window is, as the agent last reported it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used: f64,
    pub size: f64,
    /// Where the agent will compact the conversation on its own, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<UsageCost>,
}

impl ContextUsage {
    /// What the meter fills toward: where the agent compacts, else its whole window.
    pub fn limit(&self) -> f64 {
        self.compact_at.unwrap_or(self.size)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compactions {
    pub count: u32,
    /// When the last one finished, in Unix milliseconds.
    pub last_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub amount: f64,
    pub currency: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCommandInfo {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAsk {
    pub request_id: String,
    pub session_id: String,
    pub tool_call: ToolCall,
    /// The change the call asks to make, counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffSummary>,
    pub options: Vec<PermissionOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionKind,
}

/// What choosing a permission request's option does, as ACP names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    /// A kind an agent sent that ACP does not name.
    #[serde(other)]
    Unknown,
}

impl PermissionKind {
    pub fn is_allow(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }

    pub fn is_reject(self) -> bool {
        matches!(self, Self::RejectOnce | Self::RejectAlways)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Running,
    Paused,
    Completed,
    Failed,
    Stopped,
}

/// A trigger that sends a chat a message when its time comes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTrigger {
    pub id: u64,
    pub title: String,
    pub when: TriggerWhen,
    /// The next time it fires, in Unix milliseconds; none for a webhook, which fires on each
    /// request, or a CI trigger, which fires when CI ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TriggerWhen {
    /// Once, at `at` in Unix milliseconds.
    Once {
        at: u64,
    },
    Every {
        seconds: u64,
    },
    /// Each day at `minute` past midnight on the host's clock, on `weekdays`
    /// counted from Sunday as 0, or on every day when there are none.
    Daily {
        minute: u32,
        weekdays: Vec<u8>,
    },
    /// On each request to `url`, the address senders post to.
    Webhook {
        url: String,
    },
    /// Once, when CI on `on` ends: a commit or a run, as the agent named it.
    Ci {
        on: String,
    },
}

/// Work the agent left running in the background.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncTask {
    pub async_task_id: String,
    pub name: String,
    pub task_type: String,
    pub description: String,
    pub state: TaskState,
    pub can_stop: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
    /// The subagent that started it, when the chat itself did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The agents a workflow has started so far.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<WorkflowAgent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    pub total_tokens: u64,
    pub tool_uses: u64,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowAgentState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgent {
    pub index: u64,
    pub label: String,
    pub state: WorkflowAgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
}

/// A message waiting behind the running turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<SentContext>,
}

/// A message a viewer of the chat's share link wrote, which reaches the agent only once the
/// person sends it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestMessage {
    pub id: String,
    /// The name the viewer gave, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub text: String,
    /// When it came, in epoch ms.
    pub at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatRole,
    /// What the agent knows a message the person sent by, for editing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub parts: Vec<ChatPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// What the person sent with it, named rather than whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<SentContext>,
    /// A sent message's text read in order, its words and the chips its draft wrote into it,
    /// when it holds any chip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<SentSpan>,
    /// The messages from other agents and answers it is made of, when that is all it holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub letters: Vec<AgentLetter>,
    /// When the person sent it, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<u64>,
    /// When the turn it closed finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// The agent began a turn with it, without a prompt before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens_turn: Option<TurnStart>,
    /// What the last message of an answer says beneath it. A prompt's time is `sent_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RowMeta>,
    /// The finished turn it belongs to, when that turn folds its work away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold: Option<WorkFold>,
    /// The run of messages between agents it belongs to, which folds to one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamFold>,
    /// What the claims in an answer rest on, by the `why` links it holds: a link that names none
    /// of these shows nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chips: Vec<WhyChip>,
    /// The sentences of an answer that rested on none of its `why` links, taken out of its text;
    /// clients count the sentences, not the entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<Unsourced>,
    /// The index of the chat event that opened it, where a client pages from: `chatPage`'s
    /// `before`, `after` and `around` take it. Clients read it here, never out of the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_event: Option<u64>,
}

/// The line under the last message of an answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowMeta {
    /// The first message of the answer it closes.
    pub from: String,
    /// Tokens a second the answer was written at, estimated from its characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// When the answer's turn finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// How long the turn took, from the prompt that started it to its finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took: Option<u64>,
}

/// A finished turn, whose work folds away behind the reply it closed with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkFold {
    /// The turn's first message, which names the fold.
    pub id: String,
    /// The turn's last message.
    pub last: String,
    /// Where the reply starts among this message's parts: past its end when the
    /// reply starts in a later message, and 0 when it started in an earlier one.
    pub from: u64,
    /// Which of this message's parts stay in view while the fold is closed.
    pub shown: Vec<u64>,
    pub calls: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took: Option<u64>,
    /// Everything folded is thinking.
    pub thought: bool,
}

/// A row of a run of messages between agents. A run ends where something stays in view, so a
/// long job reads as its answers with one line between each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamFold {
    /// The run's first row, which names the fold, though a client may not hold that row.
    pub id: String,
    /// Which of this row's parts stay in view while the fold is closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<u64>,
    /// What the fold's line says, on the run's last row, so it comes with any window that holds
    /// some of the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<TeamLine>,
}

/// The one line a run of messages between agents folds to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamLine {
    /// Who sent the messages in it, in the order they first did, the first four.
    pub agents: Vec<TeamAgent>,
    /// How many others sent some.
    pub more: u64,
    /// What it holds that says something: the agents' messages and what the chat's agent wrote.
    pub messages: u64,
    /// The row the fold opens on; those before it wait behind `earlier`.
    pub from: String,
    /// How many of `messages` come before `from`.
    pub earlier: u64,
    /// The latest message in it, on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<TeamSaid>,
    /// What someone in it needs of the person, which takes over from `latest` until it is settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<TeamNeed>,
    /// How long it lasted, from its first row to its last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgent {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    pub cli: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamSaid {
    pub name: String,
    pub text: String,
}

/// What an agent in a run needs of the person, as the core knows it: never read from what it wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamNeed {
    pub kind: TeamNeedKind,
    pub name: String,
    /// What it asks or what went wrong, in a line, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamNeedKind {
    /// It asked the person something and has no answer yet.
    Asks,
    /// It waits on the person for something else, such as a permission.
    Waits,
    /// Its last turn failed.
    Failed,
    /// It ended or never started.
    Stopped,
}

/// Where an `@` mention of another agent points.
pub const AGENT_URI: &str = "sikemux://agent/";
/// An earlier chat of the project, as `<cli>/<session id>`.
pub const EARLIER_URI: &str = "sikemux://earlier/";
/// An agent a message woke up.
pub const WOKEN_URI: &str = "sikemux://woken/";
/// Something from the person's stash, by its id.
pub const STASH_URI: &str = "sikemux://stash/";
/// An answer the person gave to a question asked in an agent's reply.
pub const ANSWER_URI: &str = "sikemux://answer/";
/// An earlier answer the person replies to, by its message id; its text is the words quoted.
pub const REPLY_URI: &str = "sikemux://reply/";
/// What a side chat is told about the chat it was opened beside, by that chat's agent id.
pub const SIDE_URI: &str = "sikemux://side/";
/// The most of an answer a reply quotes.
pub const QUOTE_MAX: usize = 1_200;
/// What a claim in an agent's answer rests on, as `why` gave it: a commit, the person's words in
/// a chat, a call or a rule.
pub const WHY_URI: &str = "sikemux://why/";

/// What one `why` link in an answer names, checked by the core to be in the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyChip {
    /// The link as the answer holds it.
    pub href: String,
    pub kind: WhyKind,
    /// When it was said, made or answered, where the core knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// A change that is on the project's branch, rather than only in a checkout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub landed: bool,
    pub open: WhyOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhyKind {
    /// The person's own words in a chat.
    Said,
    /// A change, by its commit.
    Change,
    /// A call the person answered.
    Call,
    /// One of the person's rules.
    Rule,
    /// What an agent or a lead decided.
    Decision,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// Where a chip goes when it is opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WhyOpen {
    /// The agent's chat, at the event `index` of its run `feed` when the core could place it.
    Chat {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feed: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u64>,
    },
    Commit {
        sha: String,
    },
    Call {
        id: String,
    },
    /// The rule on the Playbook's Rules page.
    Rule {
        id: u64,
    },
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// What `chatLeftOut` answers: the text parts of an answer with the sentences left out of them
/// back in place.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLeftOut {
    pub parts: Vec<RestoredPart>,
}

/// A text part's whole tree with its left-out sentences back where they stood, each an inline
/// element `{"t": "faint", "c": [...]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredPart {
    pub part: String,
    pub md: PartTree,
}

/// Sentences taken out of a text part of an answer together, and where they stood.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unsourced {
    pub part: String,
    /// The byte of the part's text, as it is now, where it stood.
    pub at: u64,
    /// As written, with the space that parted it from its neighbour.
    pub text: String,
    /// How many sentences it holds, as sentences side by side go out together.
    pub sentences: u64,
}

/// What comes before an issue's number in an address, after the repository.
const ISSUE_PATHS: [&str; 1] = ["/issues/"];
/// What comes before a pull request's number in an address, after the repository.
pub const PULL_PATHS: [&str; 4] = ["/pull/", "/pulls/", "/pull-requests/", "/merge_requests/"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackedKind {
    Issue,
    Pull,
}

/// The issue or pull request a context item is, read from its address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracked {
    pub kind: TrackedKind,
    pub number: String,
    /// Its title, without the kind and number the context names it by.
    pub title: String,
}

/// The issue or pull request at `uri`, when it names one, and its number.
pub fn tracked_at(uri: &str) -> Option<(TrackedKind, &str)> {
    let kinds = ISSUE_PATHS
        .iter()
        .map(|path| (*path, TrackedKind::Issue))
        .chain(PULL_PATHS.iter().map(|path| (*path, TrackedKind::Pull)));
    kinds.into_iter().find_map(|(path, kind)| {
        let (_, after) = uri.split_once(path)?;
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let number = &after[..end];
        (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some((kind, number))
    })
}

/// An issue's or pull request's title without the kind and number its context names it by.
pub fn tracked_title<'a>(title: &'a str, number: &str) -> &'a str {
    let bare = title.trim_start_matches("Issue ");
    let bare = bare.trim_start_matches("Pull request ");
    bare.strip_prefix(&format!("#{number}"))
        .map_or(title, |rest| rest.trim_start_matches(':').trim_start())
}

/// A stretch of a sent message's text: its words, or a chip its draft wrote there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentSpan {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chip: Option<SentChip>,
}

/// What a draft's chip names in a sent message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentChip {
    pub kind: SentChipKind,
    /// What it says: `@Otter`, `+src/main.rs`, `Image #1`, `#12 Title`.
    pub label: String,
    /// The file or address the message carried for it, which needs no chip of its own; none
    /// for a command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SentChipKind {
    Agent,
    Chat,
    Command,
    File,
    Folder,
    Image,
    Issue,
    Pull,
    /// A kind from a newer core, which a client draws as plain words.
    #[serde(other)]
    Other,
}

/// Something read elsewhere and sent with a message, named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentContext {
    pub uri: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracked: Option<Tracked>,
    /// The words of the answer a reply quotes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

impl SentContext {
    pub fn new(uri: String, title: String) -> Self {
        let tracked = tracked_at(&uri).map(|(kind, number)| Tracked {
            kind,
            number: number.to_owned(),
            title: tracked_title(&title, number).to_owned(),
        });
        Self {
            uri,
            title,
            tracked,
            quote: None,
        }
    }

    /// The item as its text read, which for a reply is the words it quotes.
    pub fn with_text(mut self, text: &str) -> Self {
        if self.uri.starts_with(REPLY_URI) {
            let text = text.trim();
            self.quote = (!text.is_empty()).then(|| text.to_owned());
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LetterKind {
    /// A message another agent sent with agent_send.
    Message,
    /// The person's answer to a question an agent asked.
    Answer,
    /// The prompt of a trigger the chat set, sent when its time came.
    Trigger,
}

/// A message from another agent, or an answer to an agent's question, as the
/// core hands it to the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLetter {
    pub kind: LetterKind,
    /// Who sent it, or for an answer the agent that asked.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    pub cli: String,
    /// The question an answer answers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub question: String,
    pub text: String,
    /// The answer is a secret the agent reads from a file, so `text` only says where.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    /// The files sent with an answer, which `text` then leaves out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// The tree of a message's `text`, for a client that cannot read markdown itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<PartTree>,
    /// For a trigger's prompt, what the trigger is called and the request that fired it;
    /// `text` then holds the prompt alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerFired>,
}

/// A trigger that fired, as its chat is sent it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerFired {
    pub title: String,
    pub prompt: String,
    /// The webhook request that fired it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<FiredRequest>,
}

/// A request to a trigger's webhook, as its sender made it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiredRequest {
    pub method: String,
    /// When it arrived, as the host's clock read it, such as "at 14:40".
    pub received: String,
    /// The query, without its leading `?`.
    pub query: String,
    pub headers: Vec<RequestHeader>,
    /// As much of the body as the prompt carries.
    pub body: String,
    /// The body as a code block, laid out when it is JSON, for the apps to draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_tree: Option<PartTree>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHeader {
    pub name: String,
    pub value: String,
}

/// The core's markdown tree of a part's text and how much of the text it covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartTree {
    pub blocks: Value,
    pub n: u64,
}

/// Something besides text in a message, as the agent's protocol (ACP) gives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// `data` is the picture in base64; a picture too big to keep is kept without it.
    Image {
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
    Audio {
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    /// A file or page the block points at.
    ResourceLink {
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// A file handed over whole, such as an issue or a letter.
    Resource {
        resource: EmbeddedResource,
    },
    /// A block of a kind this build does not know, as it came.
    Other {
        block: Value,
    },
}

impl ContentBlock {
    /// A block as the agent sent it, kept whole under `Other` when it is not one this build reads.
    pub fn read(block: &Value) -> Self {
        serde_json::from_value(block.clone()).unwrap_or_else(|_| Self::Other {
            block: block.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedResource {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ChatPart {
    Text {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        md: Option<PartTree>,
        /// How many bytes of the text were left off the end, for a row too big to send whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cut_bytes: Option<u64>,
    },
    Thought {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        md: Option<PartTree>,
        /// How many bytes of the text were left off the end, for a row too big to send whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cut_bytes: Option<u64>,
    },
    /// A picture or a file the agent put in its reply, or the person sent.
    Content {
        id: String,
        content: ContentBlock,
    },
    Tool {
        id: String,
        tool: ToolCall,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<DiffSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<OutputSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<PageRef>,
        /// The page the call's MCP server gave it, shown in place of the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<McpAppRef>,
        /// A GIF the agent sent, which shows in place of the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gif: Option<GifRef>,
        /// What a failed call left behind, short enough to sit under it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ended_at: Option<u64>,
    },
    Subagent {
        id: String,
        subagent: SubagentCard,
    },
    Notice {
        id: String,
        notice: TaskNotice,
    },
    AgentNotice {
        id: String,
        notice: AgentNotice,
    },
    Account {
        id: String,
        #[serde(rename = "move")]
        moved: AccountMove,
    },
    /// The core started a fresh session under the chat, from a brief it built.
    Rotation {
        id: String,
        rotation: SessionRotation,
    },
}

impl ChatPart {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Thought { id, .. }
            | Self::Content { id, .. }
            | Self::Tool { id, .. }
            | Self::Subagent { id, .. }
            | Self::Notice { id, .. }
            | Self::AgentNotice { id, .. }
            | Self::Account { id, .. }
            | Self::Rotation { id, .. } => id,
        }
    }
}

/// A tool call as its row needs it: what it is, how it went, and the few
/// arguments a row names, not everything it was handed or handed back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolStatus>,
    /// Its arguments, each cut to one short line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    /// The file it touched, with its line when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What the agent said a command is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tool's own name, when the agent gives it, which the core reads the row by.
    #[serde(skip)]
    pub name: Option<String>,
    /// What its row shows, worked out from the rest.
    pub row: Box<ToolRow>,
}

/// What a tool call's row shows. Each client words, marks and colours the
/// action its own way; the rest is shown as it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRow {
    pub action: ToolAction,
    /// What the call was about: a file by its name, a command as typed, a page.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The MCP server it went to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The plugin whose tool it is, whose mark the row wears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The file the row opens, with its line when the call said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The call's own word where the tool gives one: our tools' verbs, or
    /// another tool's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    /// Our tools' own icon, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// What it is doing while it runs, for a call whose action does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// How many calls one after another this row stands for, when more than one:
    /// an agent waiting on the same thing again and again shows as one row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<u32>,
    /// The kind of tool it is, which a run of calls is read by.
    #[serde(default, skip_serializing_if = "is_default")]
    pub family: ToolFamily,
    /// What the call did, as its run counts it: a command by what it does.
    #[serde(default, skip_serializing_if = "is_default")]
    pub intent: ToolIntent,
    /// What a command read, searched or listed, when `target` is the command as typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The folder a search looked in, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
}

impl ToolRow {
    pub fn calls(&self) -> u64 {
        self.times.map_or(1, u64::from)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAction {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Mcp,
    #[default]
    Other,
}

/// The kind of tool a call is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    Shell,
    Browser,
    Files,
    Agents,
    Waiting,
    /// Commits and code hosts.
    Git,
    /// Logs, metrics, deploys and databases.
    Ops,
    Trackers,
    Simulator,
    Web,
    /// Calls that already show as themselves or say nothing to the reader, such as loading a
    /// tool or a skill. A run leaves them out and only counts them.
    Housekeeping,
    #[default]
    Other,
}

/// What a call did, as its run counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIntent {
    Read,
    /// Looked at a picture.
    View,
    Search,
    List,
    Edit,
    Write,
    Move,
    Delete,
    Run,
    Test,
    /// Built, checked, linted or type-checked.
    Build,
    /// Any other git command.
    Git,
    Commit,
    /// Opened a page or an app.
    Open,
    /// Clicked or tapped.
    Click,
    Type,
    /// Took a screenshot.
    Look,
    Message,
    /// Started an agent or a process.
    Start,
    Stop,
    Wait,
    /// Asked a service: logs, issues, pipelines.
    Query,
    /// Read a web page.
    Fetch,
    /// Switched the agent's mode.
    Mode,
    #[default]
    Use,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The change a call made, counted. `chatToolDetail` sends its lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSummary {
    pub path: String,
    pub adds: u64,
    pub dels: u64,
}

/// What a command or MCP call printed, counted. `chatToolDetail` sends it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSummary {
    pub lines: u64,
    /// It ran past what the core keeps, and the rest was let go.
    pub cut: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub image: bool,
}

/// What `chatToolDetail` answers: a call's change line by line and what it printed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ToolOutput>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiff {
    pub path: String,
    pub adds: u64,
    pub dels: u64,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// ` `, `+` or `-`.
    pub sign: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u64>,
    /// The span that changed, in UTF-16 units, when one deleted line pairs with one added line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<Vec<u64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub text: String,
    pub cut: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<OutputImage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputImage {
    pub data: String,
    pub mime_type: String,
}

/// A GIF an agent sent with `gif_send`, which clients load from `url`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GifRef {
    pub id: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub title: String,
}

/// An MCP app: the page an MCP server gave a call to its tool, kept by the core under the
/// call's id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppRef {
    pub id: String,
}

/// A page an agent showed with `page_show`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRef {
    pub id: String,
    pub title: String,
    /// Its height at the reply width, when the agent measured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Shown in a tab on the agent's desk, so the reply draws a card for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tab: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentState {
    Running,
    Completed,
    Failed,
    Cancelled,
    Disconnected,
}

/// A subagent the chat started. Its own rows come under its session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCard {
    pub session_id: String,
    pub name: String,
    pub task: String,
    pub state: SubagentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// How many tools it called.
    pub calls: u64,
    /// The last tool it called, which says what it is up to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<ToolCall>,
    /// The person can stop it alone, leaving the chat that started it at work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_stop: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeState {
    Completed,
    Failed,
    Stopped,
}

/// What a background task left behind once it ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNotice {
    pub name: String,
    pub state: NoticeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// A subagent's notice shows that subagent's own card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeSeverity {
    Info,
    Warning,
    Error,
}

/// Something the agent tells the person outside its reply, like a model fallback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNotice {
    pub severity: NoticeSeverity,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The goal it marks the start or the end of. A client that does not know it shows the
    /// notice's title like any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalMarker>,
    /// The rule it says was kept from the person's words, which Undo retires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<KeptRule>,
    /// The decision the person overruled, which Undo (`undoOverrule`) puts back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overruled: Option<OverruledDecision>,
    /// The chief's count of a week, as numbers the client words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<ChiefWeek>,
    /// A rule of the person's that left the list or was reworded by an agent, with Undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_change: Option<RuleChange>,
}

/// A rule of the person's that an agent took off the list or reworded: now a check when a
/// landed commit's check holds it, removed, or changed when `was` holds its words before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleChange {
    pub project: String,
    /// The rule, by id.
    pub rule: u64,
    /// The rule's words.
    pub text: String,
    /// The landed commit that made the check that holds it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// What it said before an agent reworded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    /// The agent that did it, by name.
    pub by: String,
    /// The rule is back on the list: the line offers no Undo.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restored: bool,
    /// The agent that brought it back, by name, as the person asked; none when the person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_by: Option<String>,
}

/// A week in a chief's project, counted by the core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChiefWeek {
    /// When the week began and ended, in Unix milliseconds.
    pub from: u64,
    pub to: u64,
    /// Decisions agents made in the project.
    pub decided: u32,
    /// Decisions the person overruled.
    pub overruled: u32,
    /// Calls of the project the person answered.
    pub answered: u32,
    /// Of those, the ones answered as the asking agent picked.
    pub as_picked: u32,
}

/// A decision the person overruled, with the rule that now brings its kind to them, when one
/// was kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverruledDecision {
    pub project: String,
    /// The goal whose log keeps it; none for the project's own log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<u64>,
    pub decision: u64,
    /// What was decided, as the agent wrote it: its first sentence, on one line. Shown as the
    /// decision, never as the person's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<u64>,
    /// The person undid the Overrule: the decision stands again, and the line offers no Undo.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undone: bool,
    /// The Overrule stopped Auto's run on a slice, which Undo cannot put back, so the line offers
    /// no Undo and says why.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped_auto: bool,
}

/// What happened while the person was away from their project's chief, as facts the client
/// words: what needs them, what was decided for them and what landed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChiefAway {
    /// When the person was last with the chief, in Unix milliseconds.
    pub since: u64,
    /// When they came back and the block began: how long they were away is `back - since`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<u64>,
    /// The project's calls open now, by id, oldest first, as the device view lists them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decided: Vec<AwayDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landed: Vec<AwayLanded>,
}

/// A decision agents made while the person was away, which they can overrule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwayDecision {
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<u64>,
    pub decision: u64,
    pub text: String,
    /// What it rested on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<AwayAgent>,
    pub at: u64,
    /// The person overruled it since the block began, so it is struck and offers no Overrule;
    /// Undo clears it again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overruled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwayAgent {
    pub agent_id: String,
    pub name: String,
}

/// Work an agent landed while the person was away.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwayLanded {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub title: String,
    pub at: u64,
    /// Its turn offers Try it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub try_it: bool,
}

/// A rule kept from what the person said in a chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptRule {
    pub id: u64,
    pub project: String,
}

/// Where a goal began, passed a step or ended in a chat, and how it stood then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalMarker {
    pub card: u64,
    #[serde(default)]
    pub moment: GoalMoment,
    pub state: SliceState,
    #[serde(default)]
    pub name: String,
    /// A step for each gate or slice, as `ChatGoal` counts them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<SliceState>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub of_gates: bool,
    /// How long the chat had worked on it, in milliseconds, past its start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took: Option<u64>,
}

/// What happened to a goal where its line sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoalMoment {
    #[default]
    Started,
    /// One more of its steps is done, and it goes on.
    Step,
    Done,
    /// The chat no longer works on it, though it is not done.
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveReason {
    Limit,
    Chosen,
}

/// Why the core started a fresh session under a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RotationReason {
    /// The old session's context had grown past what a call should carry.
    Full,
    /// The person came back after the prompt cache had died.
    Away,
    /// The chat moved to another model.
    Model,
    /// The chat moved to another account.
    Account,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// The core started a fresh session under the chat, from a brief it built from what it keeps.
/// The chat stays one conversation; this is the line that says so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRotation {
    /// Unix milliseconds.
    pub at: u64,
    pub reason: RotationReason,
    pub from_session: String,
    pub to_session: String,
    pub brief: RotationBrief,
}

/// What a fresh session started from, part by part.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationBrief {
    /// About how many tokens it holds in all.
    pub tokens: u64,
    pub sections: Vec<BriefSection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefSection {
    pub kind: BriefSectionKind,
    /// About how many tokens it holds.
    pub tokens: u64,
    #[serde(default)]
    pub counts: BriefCounts,
    /// Word for word, as the session was given it. A chat's line leaves it out; the core's
    /// `rotationBrief` answers it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BriefSectionKind {
    /// How the seat works and the project's rules, in the session's instructions.
    Seat,
    Decisions,
    /// Workers, waits and open calls.
    Work,
    /// What the old session said it was in the middle of.
    Handoff,
    /// The latest exchanges with the person, word for word.
    Exchanges,
    /// A value a newer core sends.
    #[serde(other)]
    Unknown,
}

/// What a section holds, counted, for a client to word.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefCounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<u32>,
    /// How many days back the decisions reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchanges: Option<u32>,
}

/// What a chat hands its next agent: its words with the person, chosen to fit a budget, not
/// written up by an agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    /// The agent the chat comes from, such as `claude`.
    pub from: String,
    /// The chat it comes from, when it is another one this chat was forked from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// About how many tokens it holds.
    pub tokens: u64,
    /// The chat's first request is in it.
    pub request: bool,
    /// Exchanges with the person it holds, and how many earlier ones it leaves out.
    pub exchanges: u32,
    pub left_out: u32,
    /// Commands it reports the outcome of.
    pub commands: u32,
}

/// The chat moved to another of the person's accounts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMove {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub reason: MoveReason,
    /// Why smart accounts started the chat on this account, as a person reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<String>,
}
