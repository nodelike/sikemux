//! A lead switching a chat's model, or the agent it runs on, between messages.

use crate::agent_catalog::AgentEntry;
use crate::server::chat_state::ChatParts;
use crate::server::workspace::launcher_of;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;

use serde_json::Value;

use super::handoff::{hand_off, HandOffParams};
use crate::server::{Core, CoreResult};

/// The most model names a refusal lists.
const NAMED: usize = 12;

/// Moves `agent_id`'s chat onto what `run_on` names: a model, an agent, or `agent/model`. Another
/// agent is handed what the chat said. Answers what it runs on now, as a lead reads it.
pub async fn switch(core: &Arc<Core>, agent_id: &str, run_on: &str) -> CoreResult<String> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    if agent.pty_id.is_some() || agent.terminal {
        return Err("a chat in its own terminal stays on its agent and model".into());
    }
    let (to, model) = named(run_on);
    let Some(to) = to.filter(|to| to.kind != agent.provider) else {
        let Some(model) = model else {
            return Ok(on(&agent.provider, agent.model.as_deref()));
        };
        let offered = (core.chats().readable(agent_id).ok())
            .and_then(|feed| feed.config_options())
            .or_else(|| core.workspaces().launcher_options(&launcher_of(&agent)));
        let value = model_named(offered, &agent.provider, model)?;
        super::set_config(core, agent_id, "model".into(), value.clone()).await?;
        return Ok(on(&agent.provider, Some(&value)));
    };
    if core.chats().at_work(agent_id) {
        return Err("it is mid-turn: switch it once its turn ends".into());
    }
    let model = model
        .map(|model| model_named(core.workspaces().launcher_options(to.kind), to.kind, model))
        .transpose()?;
    hand_off(
        core,
        HandOffParams {
            agent_id: agent_id.to_owned(),
            provider: to.kind.to_owned(),
            profile_id: None,
            model: model.clone(),
            model_provider: None,
            fork: false,
        },
    )
    .await?;
    Ok(on(to.kind, model.as_deref()))
}

fn on(cli: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{cli}, {model}"),
        None => format!("{cli}, its default model"),
    }
}

/// The agent and the model `run_on` names. A model's own slash, as in `openai/gpt-5.5`, stays
/// in the model unless what comes before it names an agent.
fn named(run_on: &str) -> (Option<&'static AgentEntry>, Option<&str>) {
    let run_on = run_on.trim();
    if let Some(agent) = chat_agent(run_on) {
        return (Some(agent), None);
    }
    match run_on.split_once('/') {
        Some((agent, model)) if chat_agent(agent).is_some() && !model.trim().is_empty() => {
            (chat_agent(agent), Some(model.trim()))
        }
        _ => (None, Some(run_on)),
    }
}

fn chat_agent(name: &str) -> Option<&'static AgentEntry> {
    let name = name.trim();
    (crate::agent_catalog::all().into_iter())
        .filter(|entry| entry.chat.is_some())
        .find(|entry| {
            entry.kind.eq_ignore_ascii_case(name) || entry.label.eq_ignore_ascii_case(name)
        })
}

/// The id of the model `wanted` names, by id or by the name its picker shows, among those
/// `offered`. A model of an agent that has not run here yet goes as it is named.
fn model_named(offered: Option<Value>, cli: &str, wanted: &str) -> CoreResult<String> {
    let wanted = wanted.trim();
    let choices = (offered.and_then(|options| sikemux_fold::pickers::pickers(&options).0))
        .map(|picker| picker.choices)
        .unwrap_or_default();
    if choices.is_empty() {
        return Ok(wanted.to_owned());
    }
    let named = |choice: &&sikemux_view::workspace::ConfigChoice| {
        choice.value.eq_ignore_ascii_case(wanted) || choice.label.eq_ignore_ascii_case(wanted)
    };
    if let Some(choice) = choices.iter().find(named) {
        return Ok(choice.value.clone());
    }
    let listed: Vec<&str> = (choices.iter())
        .filter(|choice| !choice.superseded)
        .take(NAMED)
        .map(|choice| choice.value.as_str())
        .collect();
    Err(format!(
        "{cli} offers no model {wanted}; it offers {}",
        listed.join(", ")
    )
    .into())
}
