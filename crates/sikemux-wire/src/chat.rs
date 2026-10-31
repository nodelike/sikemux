//! A chat as the core folds it from what its agent says, and the changes to
//! it clients apply. Clients draw this; none of them reads the agent's own
//! messages. Events, attachments and pages carry it as JSON, so a field added
//! here reaches the phone with an over-the-air update.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Why the agent ended its turn.
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

/// One step of the plan an agent keeps for its turn.
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

/// How a tool call stands.
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
    /// The call has not finished yet.
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
    pub permission_mode: crate::protocol::PermissionMode,
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
    /// The steps the agent plans for the turn, once it has said them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<PlanEntry>>,
    pub commands: Vec<ChatCommandInfo>,
    pub capabilities: ChatCapabilities,
    pub setup: Value,
    /// The model the session runs on, and the ones it can switch to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::protocol::ConfigPicker>,
    /// How hard the model thinks. An agent that always offers it has one
    /// with no choices until its session says them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::protocol::ConfigPicker>,
    pub permissions: Vec<PermissionAsk>,
    pub tasks: Vec<AsyncTask>,
    /// The chat's own subagents still at work.
    pub running_subagents: Vec<SubagentCard>,
    /// What the person sent mid-turn, waiting to go out once the turn ends.
    pub queued: Vec<QueuedMessage>,
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
    /// What the person has written and not sent, as the host keeps it for every app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<ChatDraft>,
}

/// A chat's unsent draft: its text, what the apps keep beside it, such as its files as
/// `attachments`, and its revision, which a `setDraft` names as the one it was written over.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDraft {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
    pub rev: u64,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    SignIn,
    Limit,
    Network,
    Other,
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
}

/// How full the session's context window is, as the agent last reported it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used: f64,
    pub size: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<UsageCost>,
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

/// A slash command the agent offers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCommandInfo {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A tool call waiting on the person's permission.
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTrigger {
    pub id: u64,
    pub title: String,
    pub when: TriggerWhen,
    /// The next time it fires, in Unix milliseconds; none for a webhook, which fires on each
    /// request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
}

/// When a trigger fires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TriggerWhen {
    /// Once, at `at` in Unix milliseconds.
    Once { at: u64 },
    /// Every `seconds`.
    Every { seconds: u64 },
    /// Each day at `minute` past midnight on the host's clock, on `weekdays`
    /// counted from Sunday as 0, or on every day when there are none.
    Daily { minute: u32, weekdays: Vec<u8> },
    /// On each request to `url`, the address senders post to.
    Webhook { url: String },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    User,
    Assistant,
}

/// One message of a chat.
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
    /// What its row says beneath it: on a prompt, and on the last message of an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RowMeta>,
    /// The finished turn it belongs to, when that turn folds its work away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold: Option<WorkFold>,
    /// Emoji the agent reacted to it with, on a message to the agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<String>,
    /// The index of the chat event that opened it, where a client pages from: `chatPage`'s
    /// `before`, `after` and `around` take it. Clients read it here, never out of the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_event: Option<u64>,
}

/// The line under a prompt, or under the last message of an answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowMeta {
    /// The first message of the answer it closes, or its own id on a prompt.
    pub from: String,
    /// Tokens a second the answer was written at, estimated from its characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// When a prompt was sent, or when an answer's turn finished.
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
    /// Where the reply starts among the last message's parts.
    pub from: u64,
    pub calls: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took: Option<u64>,
    /// Everything folded is thinking.
    pub thought: bool,
}

/// Something read elsewhere and sent with a message, named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentContext {
    pub uri: String,
    pub title: String,
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
            | Self::Account { id, .. } => id,
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
    /// What its row shows, worked out from the rest.
    #[serde(default)]
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

/// A page an agent showed with `page_show`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRef {
    pub id: String,
    pub title: String,
    /// Its height at the reply width, when the agent measured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveReason {
    Limit,
    Chosen,
}

/// The chat moved to another of the person's accounts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMove {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub reason: MoveReason,
}
