//! A session's model and effort options, built as the select options ACP
//! agents send, for the agents that list their models some other way. The
//! fold reads them back into the chat head's pickers.

use serde_json::{json, Value};

pub use sikemux_fold::pickers::pickers;

fn set(object: &mut Value, key: &str, value: Value) {
    if let Some(object) = object.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

pub fn choice(value: &str, name: &str, description: Option<&str>) -> Value {
    let mut choice = json!({ "value": value, "name": name });
    if let Some(description) = description {
        set(&mut choice, "description", json!(description));
    }
    choice
}

/// `choice` marked as the one the agent takes when none is made.
pub fn as_default(mut choice: Value) -> Value {
    set(&mut choice, "isDefault", json!(true));
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
        set(&mut select, "description", json!(about));
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

/// `choice` with the efforts its model takes, `default` the one it runs at when none is
/// picked, so the effort row can be built for it before a session is on it.
pub fn with_efforts(mut choice: Value, efforts: &[&str], default: Option<&str>) -> Value {
    set(&mut choice, "efforts", json!(efforts));
    if let Some(default) = default {
        set(&mut choice, "defaultEffort", json!(default));
    }
    choice
}

/// Names the effort row a model select's choices build, by the `id` the agent changes it by.
pub fn with_effort_row(mut select: Value, id: &str, name: &str, about: &str) -> Value {
    set(
        &mut select,
        "effortRow",
        json!({ "id": id, "name": name, "description": about }),
    );
    select
}

/// Builds the effort row again for the model `options` are on, from the efforts that model's
/// choice names, keeping the effort picked when the model takes it. A model with no efforts
/// gets no row; one whose choice names none leaves the row as it is.
pub fn fit_effort(options: &mut Value) {
    let Some(rows) = options.as_array_mut() else {
        return;
    };
    let Some(model) = rows.iter().find(|row| row["id"] == "model") else {
        return;
    };
    let Some(choice) = model["currentValue"]
        .as_str()
        .and_then(|current| find_choice(&model["options"], current))
    else {
        return;
    };
    let Some(levels) = choice["efforts"].as_array() else {
        return;
    };
    let levels: Vec<String> = levels
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let default = choice["defaultEffort"].as_str().map(str::to_owned);
    let template = model["effortRow"].clone();
    let at = rows.iter().position(sikemux_fold::pickers::is_effort);
    let old = at.map(|at| rows.remove(at));
    if levels.is_empty() {
        return;
    }
    let meta = old.as_ref().unwrap_or(&template);
    let text = |key: &str, fallback: &str| meta[key].as_str().unwrap_or(fallback).to_owned();
    let named = |level: &str| {
        old.as_ref()
            .and_then(|row| find_choice(&row["options"], level))
            .and_then(|choice| choice["name"].as_str())
            .map_or_else(|| level_name(level), str::to_owned)
    };
    let choices = levels
        .iter()
        .map(|level| {
            let choice = self::choice(level, &named(level), None);
            match default.as_ref() == Some(level) {
                true => as_default(choice),
                false => choice,
            }
        })
        .collect();
    let picked = old.as_ref().and_then(|row| row["currentValue"].as_str());
    let Some(current) = [picked, default.as_deref()]
        .into_iter()
        .flatten()
        .find(|level| levels.iter().any(|offered| offered == level))
        .or_else(|| levels.first().map(String::as_str))
    else {
        return;
    };
    let row = effort_select(
        &text("id", "effort"),
        &text("name", "Effort"),
        &text("description", "Available effort levels for this model"),
        current,
        choices,
    );
    let model_at = rows
        .iter()
        .position(|row| row["id"] == "model")
        .unwrap_or(0);
    rows.insert(at.unwrap_or(model_at + 1), row);
}

/// Marks `value` current on the row `matches` finds, when that row offers it.
pub fn pick(options: &mut Value, matches: impl Fn(&Value) -> bool, value: &str) {
    let row = options
        .as_array_mut()
        .and_then(|rows| rows.iter_mut().find(|row| matches(row)));
    if let Some(row) = row.filter(|row| choices_offer(&row["options"], value)) {
        row["currentValue"] = json!(value);
    }
}

/// Copies onto the model choices of `options` the efforts `before` named for the same models
/// where `options` name none, as an agent's session names only its current model's.
pub fn carry_efforts(options: &mut Value, before: &Value) {
    let Some(old) = before
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == "model"))
    else {
        return;
    };
    let Some(model) = options
        .as_array_mut()
        .and_then(|rows| rows.iter_mut().find(|row| row["id"] == "model"))
    else {
        return;
    };
    if model.get("effortRow").is_none() && old.get("effortRow").is_some() {
        model["effortRow"] = old["effortRow"].clone();
    }
    carry_choice_efforts(&mut model["options"], &old["options"]);
}

fn carry_choice_efforts(choices: &mut Value, before: &Value) {
    for choice in choices.as_array_mut().into_iter().flatten() {
        if choice["options"].is_array() {
            carry_choice_efforts(&mut choice["options"], before);
            continue;
        }
        if choice.get("efforts").is_some() {
            continue;
        }
        let Some(old) = choice["value"]
            .as_str()
            .and_then(|value| find_choice(before, value))
        else {
            continue;
        };
        for key in ["efforts", "defaultEffort"] {
            if let Some(kept) = old.get(key) {
                choice[key] = kept.clone();
            }
        }
    }
}

/// The choice of `value` among choices that can arrive flat or in named groups.
fn find_choice<'a>(choices: &'a Value, value: &str) -> Option<&'a Value> {
    choices.as_array()?.iter().find_map(|choice| {
        if choice["value"] == value {
            return Some(choice);
        }
        find_choice(&choice["options"], value)
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
