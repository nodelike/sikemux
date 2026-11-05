//! What the chat's own methods take, and what they do with more than one part of a chat.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::server::{Core, CoreResult};

/// A page of a chat's rows: those before or after an event, or around the row holding one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatPageParams {
    pub agent_id: String,
    pub feed: String,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub around: Option<u64>,
    /// A subagent's rows rather than the chat's own.
    pub session: Option<String>,
    pub turns: Option<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailParams {
    pub agent_id: String,
    pub tool_call_id: String,
}

/// A person's message `message_id` written again as `text`, sent with the files at `paths`.
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
    super::wake_for_message(core, &params.agent_id, &mut []).await?;
    let EditParams {
        agent_id,
        message_id,
        text,
        paths,
    } = params;
    super::edit(core, &agent_id, message_id, text, paths, Vec::new()).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptChatParams {
    agent_id: String,
    #[serde(default)]
    message_id: Option<String>,
    text: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<sikemux_wire::protocol::ChatContext>,
}

/// Sends the person's message as `AcpPrompt` does, for a client whose own request
/// carries no context: the agents, earlier chats and issues the message names.
pub(crate) async fn prompt_chat(core: &Arc<Core>, params: PromptChatParams) -> CoreResult<()> {
    let PromptChatParams {
        agent_id,
        message_id,
        text,
        paths,
        mut context,
    } = params;
    if core.workspaces.in_terminal(&agent_id) {
        crate::server::messages::from_person(core, &agent_id, text)?;
    } else {
        super::wake_for_prompt(core, &agent_id, &mut context).await?;
        super::prompt(core, &agent_id, message_id, text, paths, context)?;
    }
    Ok(())
}

/// Messages waiting behind an agent's turn, by id.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedParams {
    pub agent_id: String,
    pub ids: Vec<String>,
    /// A message sent with them, after them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<super::queue::Message>,
}
