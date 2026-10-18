//! Agents that speak ACP themselves. Each one shapes its session a little
//! differently, so the choices here are read from what the session offers
//! rather than assumed.

use agent_client_protocol::schema::v1::Meta;
use serde::Serialize;
use serde_json::{json, Value};

use crate::git_guard::shim;

/// The arguments that put each agent's own binary into ACP mode.
///
/// Grok shares one background process between its clients unless told not to,
/// and a chat's approval choices must not reach the other clients.
pub fn arguments(provider: &str) -> Option<&'static [&'static str]> {
    match provider {
        "opencode" | "omp" | "hermes" => Some(&["acp"]),
        "grok" => Some(&["agent", "--no-leader", "stdio"]),
        _ => None,
    }
}

/// What a session of `provider`'s opens with beside its folder and tools.
/// Grok loads the plugin whose hook checks its shell commands, since its
/// login profile's PATH puts the system's git ahead of Sikemux's.
pub fn session_meta(provider: &str) -> Option<Meta> {
    let plugin = shim::grok_plugin().filter(|_| provider == "grok")?;
    let mut meta = Meta::new();
    meta.insert("pluginDirs".into(), json!([plugin]));
    Some(meta)
}

/// Hermes lists its models beside the session instead of as a config option,
/// and changes them through `session/set_model`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModel {
    pub session_id: String,
    pub model_id: String,
}

impl SetSessionModel {
    pub const METHOD: &str = "session/set_model";
}

/// Whether the model picker has to be built from the session's model list.
pub fn models_outside_config(setup: &Value) -> bool {
    config_option(setup, |option| option["id"] == "model").is_none()
        && setup.pointer("/models/availableModels").is_some()
}

/// Presents the session's model list as the select option every other agent
/// sends, so the chat's model picker reads it the same way.
pub fn model_config(setup: &Value, current: &str) -> Option<Value> {
    let models = setup.pointer("/models/availableModels")?.as_array()?;
    let options = models
        .iter()
        .filter_map(|model| {
            let value = model.get("modelId")?.as_str()?;
            let name = model.get("name").and_then(Value::as_str).unwrap_or(value);
            let mut option = json!({ "value": value, "name": name });
            if let Some(description) = model.get("description").and_then(Value::as_str) {
                option["description"] = json!(description);
            }
            Some(option)
        })
        .collect::<Vec<_>>();
    Some(json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": options,
    }))
}

/// Folds the model list into the config options, so the setup reads the same
/// as an agent that sends one.
pub fn with_model_config(mut setup: Value) -> Value {
    if !models_outside_config(&setup) {
        return setup;
    }
    let current = setup
        .pointer("/models/currentModelId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let Some(model) = model_config(&setup, &current) else {
        return setup;
    };
    match setup.get_mut("configOptions").and_then(Value::as_array_mut) {
        Some(options) => options.insert(0, model),
        None => setup["configOptions"] = json!([model]),
    }
    setup
}

fn config_option(setup: &Value, matches: impl Fn(&Value) -> bool) -> Option<&Value> {
    setup
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|option| matches(option))
}

/// The id of the option that sets how hard the model thinks. Agents name it
/// differently (`effort`, `thinking`, `reasoning_effort`) but tag it alike.
pub fn effort_config_id(setup: &Value) -> Option<&str> {
    config_option(setup, |option| option["category"] == "thought_level")?
        .get("id")?
        .as_str()
}

/// Whether a select option offers the value. Options can arrive flat or in
/// named groups.
pub fn offers(setup: &Value, config_id: &str, value: &str) -> bool {
    fn contains(options: &Value, value: &str) -> bool {
        options.as_array().is_some_and(|options| {
            options.iter().any(|option| {
                option.get("value").and_then(Value::as_str) == Some(value)
                    || option
                        .get("options")
                        .is_some_and(|group| contains(group, value))
            })
        })
    }
    config_option(setup, |option| option["id"] == config_id)
        .and_then(|option| option.get("options"))
        .is_some_and(|options| contains(options, value))
}

/// Moves the model option's current value after a `session/set_model`, which
/// answers with nothing to read it from.
pub fn select_model(setup: &mut Value, model_id: &str) {
    if let Some(models) = setup.get_mut("models").and_then(Value::as_object_mut) {
        models.insert("currentModelId".into(), json!(model_id));
    }
    let Some(options) = setup.get_mut("configOptions").and_then(Value::as_array_mut) else {
        return;
    };
    for option in options {
        if option["id"] == "model" {
            option["currentValue"] = json!(model_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hermes_setup() -> Value {
        json!({
            "modes": { "availableModes": [{ "id": "default" }], "currentModeId": "default" },
            "models": {
                "currentModelId": "gemini:flash",
                "availableModels": [
                    { "modelId": "gemini:flash", "name": "Flash" },
                    { "modelId": "openai-codex:gpt-5.5", "name": "GPT-5.5", "description": "Codex" },
                ],
            },
        })
    }

    #[test]
    fn a_model_list_becomes_the_model_option() {
        let setup = with_model_config(hermes_setup());
        let option = &setup["configOptions"][0];
        assert_eq!(option["id"], "model");
        assert_eq!(option["type"], "select");
        assert_eq!(option["currentValue"], "gemini:flash");
        assert_eq!(option["options"][1]["description"], "Codex");
        assert!(offers(&setup, "model", "openai-codex:gpt-5.5"));
        assert!(!offers(&setup, "model", "anthropic:opus"));
    }

    #[test]
    fn an_agent_with_a_model_option_keeps_it() {
        let setup = json!({
            "configOptions": [{ "id": "model", "type": "select", "currentValue": "a", "options": [] }],
            "models": { "availableModels": [{ "modelId": "b" }] },
        });
        assert_eq!(with_model_config(setup.clone()), setup);
    }

    #[test]
    fn selecting_a_model_moves_both_current_values() {
        let mut setup = with_model_config(hermes_setup());
        select_model(&mut setup, "openai-codex:gpt-5.5");
        assert_eq!(
            setup["configOptions"][0]["currentValue"],
            "openai-codex:gpt-5.5"
        );
        assert_eq!(setup["models"]["currentModelId"], "openai-codex:gpt-5.5");
    }

    #[test]
    fn effort_is_found_by_category_whatever_its_id() {
        for id in ["effort", "thinking", "reasoning_effort"] {
            let setup = json!({ "configOptions": [
                { "id": "model", "category": "model" },
                { "id": id, "category": "thought_level", "options": [{ "value": "high" }] },
            ] });
            assert_eq!(effort_config_id(&setup), Some(id));
            assert!(offers(&setup, id, "high"));
        }
        assert_eq!(effort_config_id(&json!({})), None);
    }

    #[test]
    fn grouped_options_are_searched() {
        let setup = json!({ "configOptions": [{ "id": "model", "options": [
            { "name": "OpenAI", "options": [{ "value": "openai/gpt" }] },
        ] }] });
        assert!(offers(&setup, "model", "openai/gpt"));
    }
}
