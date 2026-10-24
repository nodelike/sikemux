//! The model and effort pickers a chat head shows, read from the select
//! options in a session's `configOptions`.

use serde_json::Value;
use sikemux_wire::protocol::{ConfigChoice, ConfigPicker};

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
                description: description.map(str::to_owned),
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

    fn labels(picker: Option<ConfigPicker>) -> Vec<String> {
        picker
            .map(|picker| {
                picker
                    .choices
                    .into_iter()
                    .map(|choice| choice.label)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn flattens_provider_model_groups_without_changing_their_values() {
        let options = json!([{ "id": "model", "type": "select", "currentValue": "custom/model", "options": [
            { "group": "custom", "name": "Custom provider", "options": [{ "value": "custom/model", "name": "My model" }] },
        ] }]);
        let (model, _) = pickers(&options);
        assert_eq!(
            model.unwrap().choices,
            [ConfigChoice {
                value: "custom/model".into(),
                label: "My model".into(),
                description: Some("Custom provider".into()),
            }]
        );
    }

    #[test]
    fn names_a_model_with_the_release_number_its_description_carries() {
        let options = json!([{ "id": "model", "type": "select", "currentValue": "opus[1m]", "options": [
            { "value": "default", "name": "Default (recommended)", "description": "Opus (1M context)" },
            { "value": "opus[1m]", "name": "Opus (1M context)", "description": "Opus 5 with 1M context · Best for everyday, complex tasks" },
            { "value": "sonnet", "name": "Sonnet", "description": "Sonnet 5 · Efficient for routine tasks" },
            { "value": "haiku", "name": "Haiku", "description": "Haiku 4.5 · Fastest for quick answers" },
        ] }]);
        assert_eq!(
            labels(pickers(&options).0),
            [
                "Default (recommended)",
                "Opus 5 (1M context)",
                "Sonnet 5",
                "Haiku 4.5"
            ]
        );
    }

    #[test]
    fn leaves_a_name_alone_when_its_description_names_another_family_or_has_the_number() {
        let options = json!([{ "id": "model", "type": "select", "currentValue": "a", "options": [
            { "value": "a", "name": "Sonnet", "description": "Opus 5 behind the scenes" },
            { "value": "b", "name": "Opus 5", "description": "Opus 5 with 1M context" },
            { "value": "c", "name": "Haiku", "description": "fast and cheap" },
        ] }]);
        assert_eq!(labels(pickers(&options).0), ["Sonnet", "Opus 5", "Haiku"]);
    }

    #[test]
    fn reads_only_well_formed_select_options() {
        assert_eq!(pickers(&json!(null)), (None, None));
        assert_eq!(pickers(&json!("model")), (None, None));
        let options = json!([
            null,
            ["nested"],
            { "id": "mode", "type": "toggle", "currentValue": "on" },
            { "type": "select", "currentValue": "x" },
            { "id": "model", "type": "select", "currentValue": 3 },
            { "id": "effort", "type": "select", "category": "thought_level", "currentValue": "high",
              "options": [null, { "value": "high", "name": "High" }, { "value": 1, "name": "Bad" }, { "value": "low" }] },
            { "id": "model", "type": "select", "currentValue": "m", "options": [{ "options": [{ "value": "m", "name": "M" }] }] },
        ]);
        let (model, effort) = pickers(&options);
        assert_eq!(labels(model), ["M"]);
        assert_eq!(labels(effort), ["High"]);
    }

    #[test]
    fn finds_no_effort_unless_one_is_tagged_as_thought_level() {
        let options =
            json!([{ "id": "effort", "type": "select", "currentValue": "high", "options": [] }]);
        assert!(pickers(&options).1.is_none());
    }
}
