//! Moving an agent between its chat and its CLI in a terminal, on the same
//! saved session. Whichever view it is in, what the person had typed comes
//! with it. An agent at work stays where it is.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::agent::AgentActivity;
use super::changed::Topic;
use super::{launching, session, Core, CoreError, CoreResult};

const AT_WORK: &str = "Wait for the agent to finish";

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
enum View {
    Chat,
    Terminal,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwitchParams {
    agent_id: String,
    to: View,
}

fn draft_key(agent_id: &str) -> String {
    format!("chat:{agent_id}")
}

/// Answers `{ptyId}` once the agent runs in a terminal, or null once it runs
/// as a chat.
pub(crate) async fn switch(core: &Arc<Core>, params: SwitchParams) -> CoreResult<Value> {
    let agent = core
        .workspaces
        .agent(&params.agent_id)
        .ok_or_else(|| CoreError::from("that agent is no longer on this host"))?;
    let terminal = agent
        .pty_id
        .and_then(|pty| core.session(pty))
        .filter(|session| session.is_running());
    match (params.to, terminal) {
        (View::Terminal, Some(terminal)) => Ok(json!({ "ptyId": terminal.id })),
        (View::Terminal, None) => to_terminal(core, &params.agent_id).await,
        (View::Chat, None) if core.chats.get(&params.agent_id).is_some() => Ok(Value::Null),
        (View::Chat, terminal) => to_chat(core, &params.agent_id, terminal).await,
    }
}

async fn to_terminal(core: &Arc<Core>, agent_id: &str) -> CoreResult<Value> {
    if let Some(chat) = core.chats.get(agent_id) {
        core.workspaces.note_stopping(&chat);
        if !core.chats.stop_if_idle(agent_id) {
            return Err(AT_WORK.into());
        }
        core.note_agent_gone(agent_id);
    }
    core.workspaces.set_asleep(agent_id, false);
    let pty =
        launching::start_terminal(core, agent_id, super::spare::opening_size(core), None).await?;
    let key = draft_key(agent_id);
    let draft = core.drafts.take_text(&key)?;
    core.changes.changed(Topic::Drafts, None, Some(vec![key]));
    if let (Some(terminal), Some(draft)) = (core.session(pty), session::typable(&draft)) {
        session::type_when_ready(&terminal, draft, session::Typist::Other);
    }
    core.view_changed();
    Ok(json!({ "ptyId": pty }))
}

async fn to_chat(
    core: &Arc<Core>,
    agent_id: &str,
    terminal: Option<Arc<session::Session>>,
) -> CoreResult<Value> {
    if let Some(terminal) = terminal {
        let state = terminal.agent.as_ref().and_then(AgentActivity::state_label);
        if matches!(state, Some("working" | "blocked")) {
            return Err(AT_WORK.into());
        }
        let draft = session::prompt_draft(&terminal);
        session::kill(core, &terminal);
        let patch = Map::from_iter([("ptyId".to_owned(), Value::Null)]);
        core.workspaces.update_agent(agent_id, patch)?;
        if !draft.trim().is_empty() {
            let key = draft_key(agent_id);
            core.drafts.set(&key, draft, None)?;
            core.changes.changed(Topic::Drafts, None, Some(vec![key]));
        }
        core.view_changed();
    }
    launching::start_saved(core, agent_id, false).await?;
    Ok(Value::Null)
}
