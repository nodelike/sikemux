//! What the chat's own methods take, and what they do with more than one part of a chat.

use crate::server::parts::Parts;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::server::{Core, CoreResult};
use sikemux_wire::protocol::ChatContext;

/// A page of a chat's rows: those before or after an event, or around the row holding one.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPageParams {
    pub agent_id: String,
    pub feed: String,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub around: Option<u64>,
    /// A subagent's rows rather than the chat's own.
    pub session: Option<String>,
    pub turns: Option<u32>,
}

impl ChatPageParams {
    /// The page of `feed` nothing narrows yet.
    pub fn new(agent_id: &str, feed: String) -> Self {
        Self {
            agent_id: agent_id.to_owned(),
            feed,
            before: None,
            after: None,
            around: None,
            session: None,
            turns: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailParams {
    pub agent_id: String,
    pub tool_call_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLeftOutParams {
    pub agent_id: String,
    pub message_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditParams {
    agent_id: String,
    message_id: String,
    text: String,
    #[serde(default)]
    paths: Vec<String>,
}

/// Takes the chat back to before the message and sends the new text in its place, as the
/// window's own edit does, starting the chat first when it was stopped.
pub(crate) async fn edit_message(core: &Arc<Core>, params: EditParams) -> CoreResult<()> {
    let EditParams {
        agent_id,
        message_id,
        text,
        paths,
    } = params;
    edit_with(core, agent_id, message_id, text, paths, Vec::new()).await
}

pub(crate) async fn edit_with(
    core: &Arc<Core>,
    agent_id: String,
    message_id: String,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<()> {
    core.workspaces().person_said(&agent_id);
    crate::server::chief::person_wrote(core, &agent_id);
    super::wake_for_message(core, &agent_id, &mut context).await?;
    super::edit(core, &agent_id, message_id, text, paths, context).await
}

/// What the person says to an agent running in a terminal goes in as a message.
pub(crate) fn steer_terminal(core: &Arc<Core>, agent_id: &str, text: String) -> CoreResult<String> {
    core.workspaces().person_said(agent_id);
    crate::server::chief::person_wrote(core, agent_id);
    crate::server::messages::from_person(core, agent_id, text)?;
    Ok("queued".to_owned())
}

pub(crate) async fn steer_chat(
    core: &Arc<Core>,
    agent_id: &str,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<String> {
    core.workspaces().person_said(agent_id);
    crate::server::chief::person_wrote(core, agent_id);
    super::wake_for_message(core, agent_id, &mut context).await?;
    let whose = super::Whose::typed_to(core, agent_id);
    let steered = super::steer(core, agent_id, text, paths, context, whose).await?;
    Ok(steered.word().to_owned())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptChatParams {
    pub(crate) agent_id: String,
    #[serde(default)]
    pub(crate) message_id: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) context: Vec<ChatContext>,
}

impl PromptChatParams {
    /// Plain words for `agent_id`, with nothing attached.
    pub fn new(agent_id: String, text: String) -> Self {
        Self {
            agent_id,
            message_id: None,
            text,
            paths: Vec::new(),
            context: Vec::new(),
        }
    }
}

/// Whether the message must wait for its chat, or a chat it names, to wake, rather than going
/// at once, in the order it came.
pub(crate) fn must_wake(core: &Core, params: &PromptChatParams) -> bool {
    !core.workspaces().in_terminal(&params.agent_id)
        && (super::stopped(core, &params.agent_id) || super::mentions::wakes(&params.context))
}

/// Sends the person's message to an agent nothing need wake: a chat, or one in a terminal, where
/// it goes in as a message.
pub(crate) fn prompt_awake(core: &Arc<Core>, params: PromptChatParams) -> CoreResult<()> {
    let PromptChatParams {
        agent_id,
        message_id,
        text,
        paths,
        context,
    } = params;
    core.workspaces().person_said(&agent_id);
    let typed = super::Whose::typed_to(core, &agent_id);
    let answered = crate::server::person_calls::written(core, &agent_id, &text).max(typed.answered);
    core.activity_log()
        .person_wrote(crate::server::chief::is_chief(core, &agent_id));
    crate::server::chief::person_wrote(core, &agent_id);
    if core.workspaces().in_terminal(&agent_id) {
        crate::server::messages::from_person(core, &agent_id, text)
    } else {
        let whose = super::Whose {
            person: true,
            answered,
        };
        super::prompt(core, &agent_id, message_id, text, paths, context, whose)
    }
}

/// Sends a mod's words to an agent, waking it first: never marked as the person's, since a
/// page wrote them.
pub(crate) async fn prompt_from_mod(
    core: &Arc<Core>,
    agent_id: &str,
    text: String,
) -> CoreResult<()> {
    if core.workspaces().in_terminal(agent_id) {
        return crate::server::messages::from_person(core, agent_id, text);
    }
    let mut context = Vec::new();
    super::wake_for_prompt(core, agent_id, &mut context).await?;
    super::prompt(
        core,
        agent_id,
        None,
        text,
        Vec::new(),
        context,
        super::Whose::NOT_PERSON,
    )
}

/// Sends the person's message, waking first what it needs: the agents, earlier chats and issues
/// it names.
pub(crate) async fn prompt_chat(core: &Arc<Core>, mut params: PromptChatParams) -> CoreResult<()> {
    if !core.workspaces().in_terminal(&params.agent_id) {
        super::wake_for_prompt(core, &params.agent_id, &mut params.context).await?;
    }
    prompt_awake(core, params)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedParams {
    pub agent_id: String,
    pub ids: Vec<String>,
    /// A message sent with them, after them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<super::queue::QueuedMessageParams>,
}
