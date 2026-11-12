//! Side chats: a chat opened beside another to ask aside, with the same agent, model and
//! project. Its first message tells it what the other chat has been saying, and the other
//! chat is never told.

use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_view::chat::SIDE_URI;
use sikemux_wire::protocol::ChatContext;

use crate::server::workspace::ChatChoice;
use crate::server::{Core, CoreError, CoreResult};

/// The newest exchanges of the main chat its first message carries, and how long each side
/// of one may be.
const EXCHANGES: usize = 8;
const LONGEST_SAID: usize = 1_500;
const DIGEST_CHARS: usize = 12_000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideChatParams {
    /// The chat to open one beside.
    pub agent_id: String,
    /// The first message, sent at once when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideChat {
    pub agent_id: String,
}

/// Starts a side chat of `params.agent_id`, waiting for the person's first message.
pub(crate) async fn start(core: &Arc<Core>, params: SideChatParams) -> CoreResult<SideChat> {
    let main = (core.workspaces().agent(&params.agent_id))
        .ok_or_else(|| CoreError::from("That chat is no longer open"))?;
    if main.terminal {
        return Err("A side chat opens beside a chat, not an agent in a terminal".into());
    }
    let own = (main.profile_id.as_ref()).map(|profile| format!("{}:{profile}", main.provider));
    let launcher = own
        .filter(|own| core.workspaces().launcher(own).is_ok())
        .unwrap_or_else(|| main.provider.clone());
    let choice = ChatChoice {
        launcher,
        project: main.project.clone(),
        permission_mode: Some(main.permission_mode),
        model: main.model.clone(),
        effort: main.effort.clone(),
        resume_id: None,
        cwd: Some(main.cwd.clone()),
        title: None,
        terminal: false,
    };
    let (agent_id, _) = crate::server::launching::start_new(core, None, choice).await?;
    core.workspaces()
        .edit_agent(&agent_id, |agent| agent.side_of = Some(main.id.clone()))?;
    core.changed(crate::server::changed::What::View);
    if let Some(text) = params.text.filter(|text| !text.trim().is_empty()) {
        let prompt = super::calls::PromptChatParams::new(agent_id.clone(), text);
        super::calls::prompt_chat(core, prompt).await?;
    }
    Ok(SideChat { agent_id })
}

/// Adds to the first message of a side chat what its main chat has been saying.
pub(crate) fn tell(core: &Core, agent_id: &str, context: &mut Vec<ChatContext>) {
    let Some(main) = (core.workspaces().agent(agent_id)).and_then(|agent| agent.side_of) else {
        return;
    };
    let first = (core.chats().running(agent_id)).is_ok_and(|chat| chat.feed.turn_start().is_none());
    if !first || context.iter().any(|item| item.uri.starts_with(SIDE_URI)) {
        return;
    }
    let name = core.names().name(&main).unwrap_or_else(|| main.clone());
    let title = (core.workspaces().agent(&main)).map(|agent| agent.title);
    let exchanges = (core.chats().running(&main))
        .map(|chat| chat.feed.exchanges(EXCHANGES, LONGEST_SAID))
        .unwrap_or_default();
    context.push(ChatContext {
        uri: format!("{SIDE_URI}{main}"),
        title: format!("@{name}"),
        text: digest(&name, &main, title.as_deref(), &exchanges),
    });
}

fn digest(name: &str, id: &str, title: Option<&str>, exchanges: &[(String, String)]) -> String {
    let mut lines = vec![
        format!(
            "This is a side chat the person opened beside @{name}'s chat to ask something aside. \
             @{name} may still be working in the same folder. Answer the person here; do not carry \
             on or redo @{name}'s task, and do not message @{name} unless the person asks."
        ),
        format!("Agent id: {id}"),
    ];
    lines.extend(title.map(|title| format!("Working on: {title}")));
    lines.push(format!(
        "To read more of what it said, call agent_read with agent \"{id}\"."
    ));
    let mut kept = Vec::new();
    let mut room = DIGEST_CHARS;
    for (asked, answered) in exchanges.iter().rev() {
        let exchange = format!("The person: {asked}\n\n@{name}: {answered}");
        let Some(left) = room.checked_sub(exchange.chars().count()) else {
            break;
        };
        room = left;
        kept.push(exchange);
    }
    if !kept.is_empty() {
        kept.reverse();
        lines.push(String::new());
        lines.push("Its newest exchanges with the person, oldest first:".to_owned());
        lines.push(String::new());
        lines.push(kept.join("\n\n---\n\n"));
    }
    lines.join("\n").replace("</context", "&lt;/context")
}
