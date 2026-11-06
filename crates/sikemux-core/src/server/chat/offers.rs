//! What a chat offers to pick before its agent starts.

use sikemux_view::workspace::ConfigPicker;

use super::*;

/// The options a chat that waits for the person offers: those its launcher's last session
/// offered, on the model and effort it will start with. The model it starts with is
/// always among them, as one a provider serves is not among its agent's own.
pub(super) fn waiting_options(core: &Core, chat: &Chat) -> Option<Value> {
    let launch = chat.current_launch();
    let mut options = core
        .workspaces
        .launcher_options(chat.origin.launcher.as_deref()?)
        .filter(Value::is_array)
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if let Some(model) = &launch.model {
        offer_model(core, &launch, &mut options, model);
    }
    if options.as_array().is_none_or(Vec::is_empty) {
        return None;
    }
    for row in options.as_array_mut().into_iter().flatten() {
        let chosen = if row["id"] == "model" {
            launch.model.clone()
        } else if sikemux_fold::pickers::is_effort(row) {
            launch.effort.clone()
        } else {
            None
        };
        if let Some(chosen) = chosen {
            row["currentValue"] = Value::String(chosen);
        }
    }
    Some(options)
}

/// Puts `model` among the models `options` offer, named as its provider's API names it.
pub(super) fn offer_model(core: &Core, launch: &ChatLaunch, options: &mut Value, model: &str) {
    let Some(rows) = options.as_array_mut() else {
        return;
    };
    let name = launch
        .model_provider
        .as_deref()
        .and_then(|id| core.workspaces.providers()?.find(id))
        .map_or(model, |provider| {
            provider.api_model(&launch.provider, model)
        })
        .to_owned();
    let choice = serde_json::json!({ "value": model, "name": name });
    let Some(row) = rows.iter_mut().find(|row| row["id"] == "model") else {
        rows.push(serde_json::json!({
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": [choice],
        }));
        return;
    };
    if !pickers::choices_offer(&row["options"], model) {
        match row["options"].as_array_mut() {
            Some(choices) => choices.insert(0, choice),
            None => row["options"] = Value::Array(vec![choice]),
        }
    }
}

/// Tells a chat that waits for the person what it offers to pick: what the launcher offered
/// last, while its agent's CLI is asked again, with no session, once each time the core starts,
/// so a newer CLI's models and this core's reading of them show before a session starts.
pub(super) fn offer_options(core: &Arc<Core>, chat: &Arc<Chat>) {
    let Some(launcher) = chat.origin.launcher.clone() else {
        return;
    };
    let offered = core.workspaces.launcher_options(&launcher).is_some();
    if let Some(options) = waiting_options(core, chat) {
        tell_options(chat, options);
    }
    if !core.workspaces.first_offer_read(&launcher) {
        return;
    }
    if !offered {
        core.workspaces
            .note_config_options(&launcher, &Value::Array(Vec::new()));
    }
    let (core, chat) = (core.clone(), chat.clone());
    tokio::spawn(async move {
        match native::read_options(&chat.current_launch()).await {
            Ok(options) => core.workspaces.note_config_options(&launcher, &options),
            Err(error) => {
                return eprintln!("sikemux core: {launcher} offered nothing to pick: {error}")
            }
        }
        if let Some(options) = waiting_options(&core, &chat).filter(|_| chat.unstarted()) {
            tell_options(&chat, options);
        }
    });
}

pub(super) fn tell_options(chat: &Chat, options: Value) {
    let session = chat
        .launch
        .resume_id
        .clone()
        .unwrap_or_else(|| chat.agent_id().to_owned());
    chat.emit(
        ChatEventKind::SessionUpdate,
        serde_json::json!({
            "sessionId": session,
            "update": { "sessionUpdate": UpdateKind::ConfigOptionUpdate, "configOptions": options },
        }),
    );
}

/// Keeps a model or effort chosen before the chat's agent starts, which starts on it.
pub(super) fn choose_before_start(
    core: &Core,
    chat: &Chat,
    config_id: &str,
    value: String,
) -> CoreResult<Value> {
    let options = waiting_options(core, chat).unwrap_or(Value::Null);
    let row = options
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == config_id);
    let mut launch = chat.current_launch();
    match row {
        Some(row) if row["id"] == "model" => launch.model = Some(value),
        Some(row) if sikemux_fold::pickers::is_effort(row) => launch.effort = Some(value),
        _ => return Err(format!("{config_id} is chosen once the chat starts").into()),
    }
    chat.set_current(launch);
    let options = waiting_options(core, chat).unwrap_or(Value::Null);
    keep_model_choice(core, chat.agent_id(), &options);
    tell_options(chat, options.clone());
    Ok(serde_json::json!({ "configOptions": options }))
}

/// Keeps the model and effort a chat now runs on with its agent, so its next
/// session starts on them. An effort its CLI does not take is left out.
pub(super) fn keep_model_choice(core: &Core, agent_id: &str, options: &Value) {
    let Some(agent) = core.workspaces.agent(agent_id) else {
        return;
    };
    let (model, effort) = crate::acp::pickers::pickers(options);
    let model = model
        .map(|model| model.current)
        .filter(|model| !model.is_empty());
    let takes = |effort: &str| {
        crate::agent_catalog::agent(&agent.provider)
            .is_some_and(|entry| entry.efforts.contains(&effort))
    };
    let effort = effort
        .map(|effort| effort.current)
        .filter(|effort| takes(effort));
    if model.is_none() && effort.is_none() {
        return;
    }
    let kept = core.workspaces.edit_agent(agent_id, |agent| {
        if model.is_some() {
            agent.model = model;
        }
        if effort.is_some() {
            agent.effort = effort;
        }
    });
    if let Err(error) = kept {
        eprintln!("sikemux core: the agent's model choice was not kept: {error}");
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct AgentModelsParams {
    pub(crate) agent: String,
}

/// The model choice `agent` last offered, from any launcher of it.
pub(crate) fn models_of(core: &Core, agent: &str) -> Option<ConfigPicker> {
    let options = core.workspaces.agent_options(agent)?;
    pickers::pickers(&options).0
}
