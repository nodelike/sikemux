//! A session's model and effort options read for a picker, the way
//! `src/chat/sessionConfig.ts` reads them for the app's own.

use serde_json::Value;

use crate::protocol::{ConfigChoice, ConfigPicker};

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
