//! What a driver and the chat it runs for say to each other: the driver is a [`Backend`], and
//! tells the chat what its agent does through a [`Sink`].

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use sikemux_view::chat::StopReason;
use sikemux_view::questions::QuestionChoice;
use sikemux_wire::protocol::{ChatContext, ChatLaunch};

use super::super::feed::{Earlier, Reach};
use super::super::{permission, Respond};
use super::process::{Lines, Process, Released, Writer};
use crate::acp::failure::{sounds_offline, Failure, FailureKind};
use crate::launch::PermissionMode;
use crate::server::activity_log::{Compaction, ModelCall};

/// What the person sent, for the agent to read in its own form.
#[derive(Clone)]
pub struct Prompt {
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
    /// A message from another agent rather than the person.
    pub letter: bool,
    /// Sent before, to this agent or one that ran the session earlier, so its id may be taken.
    pub resent: bool,
}

/// The most of any one text an update carries. The chat shows only the first
/// few hundred lines of what a tool printed, and a long chat loaded again
/// would otherwise send all of it.
const MAX_TEXT: usize = 128 * 1024;
/// The largest picture an update carries, as base64. The chat keeps none
/// bigger.
const MAX_IMAGE: usize = 2 * 1024 * 1024;

/// Cuts every text in `value` down to what the chat can show.
pub fn slim(value: &mut Value) {
    match value {
        Value::String(text) if text.len() > MAX_TEXT => {
            let mut end = MAX_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        Value::Array(items) => items.iter_mut().for_each(slim),
        Value::Object(fields) => {
            let picture = fields.get("data").and_then(Value::as_str).map(str::len);
            if picture.is_some_and(|len| len > MAX_IMAGE) {
                fields.remove("data");
            }
            for (key, field) in fields.iter_mut() {
                if key != "data" {
                    slim(field);
                }
            }
        }
        _ => {}
    }
}

pub enum TurnEnd {
    /// The `turn_completed` payload, such as `{"stopReason": "end_turn"}`.
    Completed(Value),
    /// The agent could not finish it, and said why.
    Failed {
        failure: Failure,
        response: Option<Value>,
    },
    /// The agent turned the prompt down with this error.
    Refused(String),
    /// The agent can take no more turns in this session, for this reason, and
    /// is started again.
    Broken(String),
}

impl TurnEnd {
    /// A failure or refusal whose words say the network was at fault reads as
    /// one, whichever agent it came from, so the prompt goes again once it is back.
    pub fn read_offline(self) -> Self {
        match self {
            Self::Failed { failure, response }
                if failure.kind == FailureKind::Other && sounds_offline(&failure.title) =>
            {
                Self::Failed {
                    failure: Failure::lost_connection(failure.title),
                    response,
                }
            }
            Self::Refused(message) if sounds_offline(&message) => Self::Failed {
                failure: Failure::lost_connection(message),
                response: None,
            },
            end => end,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steered {
    Injected,
    /// The turn ended first, so it goes as a prompt instead.
    PromptRequired,
}

impl Steered {
    /// As ACP agents say it and as the phone's call answers it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::PromptRequired => "promptRequired",
        }
    }
}

/// The session an agent opened, as the chat reads it.
pub struct Opened {
    pub session_id: String,
    pub capabilities: Value,
    pub setup: Value,
}

impl Opened {
    /// What the chat can do with an agent the core speaks to in that agent's own protocol.
    pub fn native_capabilities() -> Value {
        json!({
            "loadSession": true,
            "steering": true,
            "editing": true,
            "promptCapabilities": { "image": true, "embeddedContext": true },
        })
    }
}

/// One agent the core speaks to in its own protocol.
pub trait Backend: Sized + Send {
    /// What it needs kept with a question to answer the agent.
    type Reply: DeserializeOwned;

    /// The agent takes instructions of the core's own as it starts, so a
    /// chat's brief goes there rather than with its first message.
    const INSTRUCTIONS: bool = false;

    /// The agent takes a new effort while a turn runs, so the person need not stop it first.
    const EFFORT_MID_TURN: bool = false;

    /// Starts the agent and opens the chat's session, or loads the one it
    /// resumes, replaying its history through `sink`, with `brief` among its
    /// instructions when it takes them.
    fn start(
        launch: &ChatLaunch,
        brief: Option<&str>,
        sink: Sink,
    ) -> impl std::future::Future<Output = Result<(Self, Opened), String>> + Send;

    /// Sends a prompt that begins turn `turn`. How it ends is told to the
    /// sink.
    fn prompt(
        &mut self,
        turn: u64,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Puts a message into the running turn. The answer is waited on apart
    /// from the chat's other commands, so a stop sent right after a steer is
    /// never held up behind it.
    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<Steered, String>> + Send + 'static;

    fn cancel(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send;

    fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Changes a session option, answering with the options as they now are.
    fn set_config(
        &mut self,
        config_id: &str,
        value: &str,
    ) -> impl std::future::Future<Output = Result<Value, String>> + Send;

    fn stop_task(
        &mut self,
        task_id: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Compacts the session between turns, with no turn in the chat to show for it.
    fn compact(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send {
        async { Ok(()) }
    }

    /// Asks the session between turns what a fresh one would need to know, with no turn in
    /// the chat to show for it, and answers with what it wrote.
    fn handoff(&mut self) -> impl std::future::Future<Output = Result<String, String>> + Send {
        async { Err("This agent writes no handoff note".to_owned()) }
    }

    /// Passes a request an MCP app made on to its server, answering with what the server said.
    fn mcp_app(
        &mut self,
        _request: crate::server::mcp_apps::ServerRequest,
    ) -> impl std::future::Future<Output = Result<Value, String>> + Send + 'static {
        std::future::ready(Err("This agent cannot pass on an app's requests".to_owned()))
    }

    /// The person answered a dialog the agent is blocked on.
    fn answer_dialog(&mut self, _id: &str, _answer: String, _choice: Option<usize>) {}

    /// Asks the agent to sign in by itself, by `method_id`, one of the
    /// methods it offered.
    fn authenticate(
        &mut self,
        _method_id: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send {
        async { Err("This agent signs in only through its own sign-in".to_string()) }
    }

    /// Takes the conversation back to before the person's message `message_id`,
    /// answering with the session it goes on in when that is a new one.
    fn rewind(
        &mut self,
        message_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + Send;

    /// Resolves once the agent is gone, with why.
    fn closed(&self) -> impl std::future::Future<Output = String> + Send;

    /// The agent's process, which leads its own process group.
    fn pid(&self) -> u32;

    /// Whether a new agent process can load this session, so the chat can go
    /// on in one after a sign-in or on another account.
    fn reloadable(&self) -> bool {
        true
    }

    /// How far into its own record of the session the agent has written, all of which the
    /// chat has been told.
    fn reach(&self) -> Option<Reach> {
        None
    }

    /// Lets go of the agent without stopping it, answering with what a core
    /// that takes it up needs: this side's state as plain data, and the
    /// agent's process. On failure the agent is stopped.
    fn release(self)
        -> impl std::future::Future<Output = Result<(Value, Released), String>> + Send;

    /// Takes up an agent an earlier core released, from the state `release`
    /// answered.
    fn adopt(
        launch: &ChatLaunch,
        sink: Sink,
        state: Value,
        agent: (Process, Writer, Lines),
    ) -> Result<Self, String>;

    /// What answers the agent with the person's choice for a question whose
    /// backend data is `reply`.
    fn responder(&self, reply: Self::Reply) -> Respond;
}

/// How many subagents' histories a resumed chat reads.
const MAX_SUBAGENT_HISTORIES: usize = 64;

/// The histories of the subagents `first` names and of theirs, each read by
/// `read`, whose subagents `children` names.
pub async fn subagent_histories<F>(
    first: Vec<String>,
    read: impl Fn(String) -> F,
    children: impl Fn(&[Value]) -> Vec<String>,
) -> HashMap<String, Vec<Value>>
where
    F: std::future::Future<Output = Vec<Value>>,
{
    let mut histories = HashMap::new();
    let mut queue = first;
    while let Some(child) = queue.pop() {
        if histories.contains_key(&child) || histories.len() >= MAX_SUBAGENT_HISTORIES {
            continue;
        }
        let said = read(child.clone()).await;
        queue.extend(children(&said));
        histories.insert(child, said);
    }
    histories
}

/// What tells a chat a session's history, the one thing a chat's saved history needs shown
/// before its agent starts.
pub trait Told: Send + Sync {
    fn update(&self, session_id: &str, update: Value);

    /// Keeps the MCP app the call `id` showed, for the update that ends the call to name.
    fn keep_app(&self, id: &str, record: &crate::server::mcp_apps::Record) -> Result<(), String>;

    /// Whether this start loads the session again under a running chat.
    fn reloading(&self) -> bool;

    /// What the agent says until [`Telling::loaded`] is history the chat already shows.
    fn hush(&self);

    /// The session has loaded, so what the agent says from now on is new to
    /// the chat.
    fn loaded(&self);

    fn reach(&self, session: &str) -> Option<Reach>;

    fn leave_room(&self);

    fn tell_earlier(&self, earlier: Earlier);

    fn reached(&self, session: &str, reach: Reach);

    /// Whether the chat's session is kept small.
    fn capped(&self) -> bool;

    /// Whether the chat's session gives way to fresh ones, so it may be asked for a handoff note.
    fn rotates(&self) -> bool;

    /// The core's own compaction has ended, done or not.
    fn compaction_over(&self);

    /// What the session's compaction is told to keep.
    fn keep_list(&self) -> String;

    /// What the session is told after its compaction.
    fn after_compaction(&self) -> Option<String>;

    /// The session called the model with `context` tokens.
    fn called(&self, context: u64);

    /// The session was compacted down to `context` tokens.
    fn compacted(&self, context: u64);

    fn session_used(&self, calls: Vec<ModelCall>, compactions: Vec<Compaction>);
}

pub type Telling = dyn Told;

/// What the chat does for a driver's [`Sink`].
pub trait SinkPort: Send + Sync {
    /// Whether the provider has a saved session to load.
    fn has_answered(&self) -> bool;

    /// Whether a turn this side prompted is being answered.
    fn running(&self) -> bool;

    fn cli_version(&self, version: &str);

    /// Keeps the plan limits the agent reported as it worked as its account's reading.
    fn plan_limits(&self, live: Vec<crate::agent_accounts::usage::UsageWindow>);

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    fn work_started(&self);

    fn work_ended(&self, stop_reason: StopReason);

    /// Settles turn `turn`, or hands a failure another account may fix to
    /// the chat's loop to decide on, with the turn still running.
    fn end_turn(&self, turn: u64, end: TurnEnd);

    /// A message from another agent went into the running turn but the agent
    /// dropped it unread, so it waits for the agent's next turn instead.
    fn letter_unread(&self, text: String);

    /// The agent asked a question with its CLI's own tool. Answers what it
    /// is told in place of the answer, which comes later as a message.
    fn question(&self, input: &Value) -> String;

    /// Puts to the person a dialog the agent is blocked on, whose answer comes
    /// back through [`Backend::answer_dialog`].
    fn ask_dialog(
        &self,
        dialog: String,
        header: &str,
        text: &str,
        choices: Vec<QuestionChoice>,
    ) -> Result<(), String>;

    fn withdraw_dialogs(&self, only: Option<&str>);

    fn withdraw_permission(&self, tool_call_id: &str);

    /// `text` went into the running turn; `person` when the person sent it.
    fn prompted(&self, text: &str, paths: &[String], person: bool);

    /// Says `message` in the chat as an error.
    fn error_text(&self, message: String);

    /// Whether the chat answers what it may for the person, as under YOLO.
    fn approves(&self) -> bool;

    /// Puts `request` to the person. `kept` is the reply as plain data, for a core that takes
    /// the chat over, and `respond` is told the chosen option's id, or none when cancelled.
    fn hold(&self, request: Value, kept: Value, respond: Respond);

    /// What it tells the chat of a session's history.
    fn telling(&self) -> &Telling;
}

impl std::ops::Deref for dyn SinkPort {
    type Target = Telling;

    fn deref(&self) -> &Telling {
        self.telling()
    }
}

/// What an agent's reader tells the chat. Cheap to clone, and safe to use from any task.
#[derive(Clone)]
pub struct Sink(Arc<dyn SinkPort>);

impl std::ops::Deref for Sink {
    type Target = dyn SinkPort;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Sink {
    pub fn new(port: Arc<dyn SinkPort>) -> Self {
        Self(port)
    }

    pub fn error(&self, message: impl std::fmt::Display) {
        self.error_text(message.to_string());
    }

    /// Asks the person to choose, unless the chat approves it for them. `reply` is what the
    /// agent's backend needs to answer with the choice; `responder` makes from it what is told
    /// the chosen option's id, or none when it was cancelled.
    pub fn ask<R: Serialize>(
        &self,
        request: Value,
        reply: R,
        responder: impl FnOnce(R) -> Respond,
    ) {
        if self.approves() {
            if let Some(option_id) = permission::approval(&request) {
                responder(reply)(Some(option_id));
                return;
            }
        }
        self.ask_person(request, reply, responder);
    }

    /// Like `ask`, for what only the person may answer, even under YOLO.
    pub fn ask_person<R: Serialize>(
        &self,
        request: Value,
        reply: R,
        responder: impl FnOnce(R) -> Respond,
    ) {
        let kept = serde_json::to_value(&reply).unwrap_or(Value::Null);
        self.hold(request, kept, responder(reply));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_texts_are_cut_and_pictures_kept_whole() {
        let long = "é".repeat(MAX_TEXT);
        let picture = "A".repeat(MAX_TEXT * 2);
        let mut update = json!({
            "rawOutput": long,
            "content": [{ "type": "image", "data": picture, "mimeType": "image/png" }],
            "huge": [{ "type": "image", "source": { "type": "base64", "data": "A".repeat(MAX_IMAGE + 1) } }],
        });
        slim(&mut update);
        let cut = update["rawOutput"].as_str().unwrap();
        assert!(cut.len() <= MAX_TEXT + '…'.len_utf8());
        assert!(cut.ends_with('…'));
        assert_eq!(
            update["content"][0]["data"].as_str().map(str::len),
            Some(MAX_TEXT * 2)
        );
        assert!(update["huge"][0]["source"].get("data").is_none());
    }
}
