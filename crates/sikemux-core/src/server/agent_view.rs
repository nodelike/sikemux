//! Moving an agent between its chat and its CLI in a terminal, on the same
//! saved session. Whichever view it is in, what the person had typed comes
//! with it. An agent at work stays where it is.

use crate::server::activity::ActivityCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::agent_view::{AgentViewKind, AgentViewParams};
use std::sync::Arc;

use serde_json::{json, Value};

use super::agent::AgentActivity;
use super::{launching, session, Core, CoreResult};

const AT_WORK: &str = "Wait for the agent to finish";

/// Answers `{ptyId}` once the agent runs in a terminal, or null once it runs
/// as a chat.
pub(crate) async fn switch(core: &Arc<Core>, params: AgentViewParams) -> CoreResult<Value> {
    let agent = core.workspaces().agent_or_gone(&params.agent_id)?;
    let terminal = agent
        .pty_id
        .and_then(|pty| core.session(pty))
        .filter(|session| session.is_running());
    match (params.to, terminal) {
        (AgentViewKind::Terminal, Some(terminal)) => Ok(json!({ "ptyId": terminal.id })),
        (AgentViewKind::Terminal, None) => to_terminal(core, &params.agent_id).await,
        (AgentViewKind::Chat, None) if core.chats().get(&params.agent_id).is_some() => {
            Ok(Value::Null)
        }
        (AgentViewKind::Chat, terminal) => to_chat(core, &params.agent_id, terminal).await,
    }
}

async fn to_terminal(core: &Arc<Core>, agent_id: &str) -> CoreResult<Value> {
    if let Some(chat) = core.chats().get(agent_id) {
        core.workspaces()
            .note_stopping(chat.agent_id(), chat.current_now());
        if !core.chats().stop_if_idle(agent_id) {
            return Err(AT_WORK.into());
        }
        core.note_agent_gone(agent_id);
    }
    core.workspaces().set_asleep(agent_id, false);
    let pty =
        launching::start_terminal(core, agent_id, super::spare::opening_size(core), None).await?;
    let key = sikemux_view::chat::chat_draft_key(agent_id);
    let draft = core.drafts().take_text(&key)?;
    super::drafts::changed(core, vec![key]);
    if let (Some(terminal), Some(draft)) = (core.session(pty), session::typable(&draft)) {
        session::type_when_ready(&terminal, draft, session::Typist::Other);
    }
    core.changed(What::View);
    Ok(json!({ "ptyId": pty }))
}

async fn to_chat(
    core: &Arc<Core>,
    agent_id: &str,
    terminal: Option<Arc<session::Session>>,
) -> CoreResult<Value> {
    if let Some(terminal) = terminal {
        if terminal.agent.as_ref().is_some_and(AgentActivity::busy) {
            return Err(AT_WORK.into());
        }
        let draft = session::prompt_draft(&terminal);
        session::kill(core, &terminal);
        core.workspaces()
            .edit_agent(agent_id, |agent| agent.pty_id = None)?;
        if !draft.trim().is_empty() {
            let key = sikemux_view::chat::chat_draft_key(agent_id);
            core.drafts().set(&key, draft, None)?;
            super::drafts::changed(core, vec![key]);
        }
        core.changed(What::View);
    }
    launching::start_saved(core, agent_id, false).await?;
    Ok(Value::Null)
}
