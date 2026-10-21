//! A session's model and effort options: built as the select options ACP
//! agents send, for the agents that list their models some other way, and
//! read for a picker the way `src/chat/sessionConfig.ts` reads them.

use serde_json::{json, Value};

use crate::protocol::{ConfigChoice, ConfigPicker};

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

/// The model and effort pickers among a session's `configOptions`.
pub fn pickers(options: &Value) -> (Option<ConfigPicker>, Option<ConfigPicker>) {
    let selects: Vec<(&Value, ConfigPicker)> = options
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["type"] == "select")
        .filter_map(|row| {
            Some((
                row,
                ConfigPicker {
                    id: row["id"].as_str()?.to_owned(),
                    current: row["currentValue"].as_str()?.to_owned(),
                    choices: choices(&row["options"], None),
                },
            ))
        })
        .collect();
    let find = |matches: &dyn Fn(&Value) -> bool| {
        selects
            .iter()
            .find(|(row, _)| matches(row))
            .map(|(_, picker)| picker.clone())
    };
    (
        find(&|row| row["id"] == "model"),
        find(&|row| row["category"] == "thought_level"),
    )
}

fn choices(options: &Value, group: Option<&str>) -> Vec<ConfigChoice> {
    options
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| {
            if row["options"].is_array() {
                return choices(&row["options"], row["name"].as_str());
            }
            let (Some(value), Some(name)) = (row["value"].as_str(), row["name"].as_str()) else {
                return Vec::new();
            };
            let description = row["description"].as_str().or(group);
            vec![ConfigChoice {
                value: value.to_owned(),
                label: versioned(name, description),
            }]
        })
        .collect()
}

/// The agent names a model without its release number ("Opus") and leaves
/// that number in the description ("Opus 5 with 1M context"), so put it back.
fn versioned(label: &str, description: Option<&str>) -> String {
    let Some((family, version)) = description.and_then(named_version) else {
        return label.to_owned();
    };
    let head = label.split(' ').next().unwrap_or_default();
    if head.to_lowercase() != family.to_lowercase() || label.contains(version) {
        return label.to_owned();
    }
    label.replacen(head, &format!("{head} {version}"), 1)
}

/// The leading "Name 1.2" of a description.
fn named_version(text: &str) -> Option<(&str, &str)> {
    let family_end = text
        .char_indices()
        .find(|(_, c)| !c.is_alphabetic())
        .map_or(text.len(), |(at, _)| at);
    let rest = &text[family_end..];
    let gap = rest.len() - rest.trim_start().len();
    if family_end == 0 || gap == 0 {
        return None;
    }
    let digits = |text: &str| text.chars().take_while(char::is_ascii_digit).count();
    let start = family_end + gap;
    let whole = digits(&text[start..]);
    if whole == 0 {
        return None;
    }
    let whole_end = start + whole;
    let fraction = text[whole_end..].strip_prefix('.').map_or(0, digits);
    let ends_word = |end: usize| {
        text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
    };
    [whole_end + 1 + fraction, whole_end]
        .into_iter()
        .filter(|end| *end == whole_end || fraction > 0)
        .find(|end| ends_word(*end))
        .map(|end| (&text[..family_end], &text[start..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn models_take_their_version_from_the_description_or_group() {
        let options = json!([
            { "type": "select", "id": "model", "currentValue": "opus", "options": [
                { "value": "opus", "name": "Opus", "description": "Opus 5.5 with 1M context" },
                { "name": "Sonnet 4", "options": [{ "value": "sonnet", "name": "Sonnet" }] },
            ] },
            { "type": "select", "id": "thinking", "category": "thought_level", "currentValue": "high",
              "options": [{ "value": "high", "name": "High" }] },
        ]);
        let (model, effort) = pickers(&options);
        let labels: Vec<String> = model
            .unwrap()
            .choices
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert_eq!(labels, ["Opus 5.5", "Sonnet 4"]);
        assert_eq!(effort.unwrap().id, "thinking");
    }
}
