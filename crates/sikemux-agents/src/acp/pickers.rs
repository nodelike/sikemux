//! A session's model and effort options, built as the select options ACP
//! agents send, for the agents that list their models some other way. The
//! fold reads them back into the chat head's pickers.

use serde_json::{json, Value};

pub use sikemux_fold::pickers::pickers;

pub fn choice(value: &str, name: &str, description: Option<&str>) -> Value {
    let mut choice = json!({ "value": value, "name": name });
    if let Some(description) = description {
        choice["description"] = json!(description);
    }
    choice
}

/// `choice` marked as the one the agent takes when none is made.
pub fn as_default(mut choice: Value) -> Value {
    choice["isDefault"] = json!(true);
    choice
}

/// The model select option, `current` offered first when it is not among
/// the `choices`.
pub fn model_select(about: Option<&str>, current: &str, mut choices: Vec<Value>) -> Value {
    if !current.is_empty() && !choices.iter().any(|choice| choice["value"] == current) {
        choices.insert(0, choice(current, current, None));
    }
    let mut select = json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": choices,
    });
    if let Some(about) = about {
        select["description"] = json!(about);
    }
    select
}

/// The select option that sets how hard the model thinks, under the `id`
/// the agent changes it by.
pub fn effort_select(
    id: &str,
    name: &str,
    about: &str,
    current: &str,
    choices: Vec<Value>,
) -> Value {
    json!({
        "id": id,
        "name": name,
        "description": about,
        "category": "thought_level",
        "type": "select",
        "currentValue": current,
        "options": choices,
    })
}

/// The choice of the speed select that runs the model at its usual speed.
pub const STANDARD: &str = "standard";

/// The select option that picks how fast the model answers: at its usual speed, or on one
/// of the faster `tiers` the agent offers for it.
pub fn speed_select(current: &str, tiers: Vec<Value>) -> Value {
    let mut choices = vec![as_default(choice(STANDARD, "Standard", None))];
    choices.extend(tiers);
    json!({
        "id": "speed",
        "name": "Speed",
        "category": sikemux_fold::pickers::SPEED,
        "type": "select",
        "currentValue": current,
        "options": choices,
    })
}

/// An effort level as a picker names it: `xhigh` as "Extra high", `max_tokens`
/// as "Max Tokens".
pub fn level_name(level: &str) -> String {
    if level == "xhigh" {
        return "Extra high".to_owned();
    }
    level
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn config_option(setup: &Value, matches: impl Fn(&Value) -> bool) -> Option<&Value> {
    setup
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|option| matches(option))
}

pub fn effort_config_id(setup: &Value) -> Option<&str> {
    config_option(setup, sikemux_fold::pickers::is_effort)?
        .get("id")?
        .as_str()
}

/// Whether choices, which can arrive flat or in named groups, offer the value.
pub fn choices_offer(choices: &Value, value: &str) -> bool {
    choices.as_array().is_some_and(|choices| {
        choices
            .iter()
            .any(|choice| choice["value"] == value || choices_offer(&choice["options"], value))
    })
}

pub fn offers(setup: &Value, config_id: &str, value: &str) -> bool {
    config_option(setup, |option| option["id"] == config_id)
        .is_some_and(|option| choices_offer(&option["options"], value))
}

pub fn current_choice(setup: &Value, config_id: &str) -> Option<String> {
    config_option(setup, |option| option["id"] == config_id)?
        .get("currentValue")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

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
