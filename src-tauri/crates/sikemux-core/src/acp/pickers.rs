//! A session's model and effort options, built as the select options ACP
//! agents send, for the agents that list their models some other way. The
//! fold reads them back into the chat head's pickers.

use serde_json::{json, Value};

pub use sikemux_fold::pickers::pickers;

/// One choice of a select option.
pub fn choice(value: &str, name: &str, description: Option<&str>) -> Value {
    let mut choice = json!({ "value": value, "name": name });
    if let Some(description) = description {
        choice["description"] = json!(description);
    }
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

/// An effort level as a picker names it: `xhigh` as "Xhigh", `max_tokens`
/// as "Max Tokens".
pub fn level_name(level: &str) -> String {
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

/// The first of a session's config options `matches` picks.
pub fn config_option(setup: &Value, matches: impl Fn(&Value) -> bool) -> Option<&Value> {
    setup
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|option| matches(option))
}

/// The value a select option currently holds.
pub fn current_choice(setup: &Value, config_id: &str) -> Option<String> {
    config_option(setup, |option| option["id"] == config_id)?
        .get("currentValue")?
        .as_str()
        .map(str::to_owned)
}
