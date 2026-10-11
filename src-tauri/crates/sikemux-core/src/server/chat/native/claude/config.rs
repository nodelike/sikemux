//! The session options a Claude chat offers: its model and how hard that
//! model thinks, read from the models the CLI lists when it starts.

use serde_json::{json, Value};

pub(crate) const DEFAULT: &str = "default";

/// The permission mode Claude Code runs in for each of the app's modes.
pub(crate) fn permission_mode(mode: &str) -> Result<&'static str, String> {
    match mode {
        "bypass" => Ok("bypassPermissions"),
        "workspace-write" => Ok("acceptEdits"),
        _ => Err(format!("Unsupported permission mode: {mode}")),
    }
}

/// The modes as an ACP session lists them, for the setup the chat keeps.
pub(crate) fn modes(current: &str) -> Value {
    json!({
        "currentModeId": current,
        "availableModes": [
            { "id": "default", "name": "Manual", "description": "Always ask before making changes" },
            { "id": "acceptEdits", "name": "Accept edits", "description": "Automatically accept all file edits" },
            { "id": "plan", "name": "Plan", "description": "Create a plan before making changes" },
            { "id": "bypassPermissions", "name": "Bypass permissions", "description": "Accepts all permissions" },
        ],
    })
}

fn text<'a>(model: &'a Value, key: &str) -> Option<&'a str> {
    model
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn find<'a>(models: &'a [Value], value: &str) -> Option<&'a Value> {
    models
        .iter()
        .find(|model| text(model, "value") == Some(value))
}

/// The row the picker shows for a model the transcript names by its full id,
/// such as `claude-opus-5-5`: the default when it resolves to that model.
pub(crate) fn row_for_resolved(models: &[Value], resolved: &str) -> Option<String> {
    models
        .iter()
        .find(|model| {
            text(model, "value") == Some(DEFAULT) && text(model, "resolvedModel") == Some(resolved)
        })
        .or_else(|| {
            models
                .iter()
                .find(|model| text(model, "resolvedModel") == Some(resolved))
        })
        .or_else(|| find(models, resolved))
        .and_then(|model| text(model, "value"))
        .map(str::to_owned)
}

/// Makes room in the list for the model the person's settings name, such as
/// `opus[1m]`, which Claude Code runs but does not list: a row like the one it
/// extends, named for its larger context. Answers with the row's value.
pub(crate) fn settings_model(models: &mut Vec<Value>, named: &str) -> Option<String> {
    if find(models, named).is_some() {
        return Some(named.to_owned());
    }
    if let Some(row) = row_for_resolved(models, named) {
        return Some(row);
    }
    let base = named.strip_suffix("[1m]")?;
    let mut row = find(models, base).cloned().or_else(|| {
        row_for_resolved(models, base).and_then(|value| find(models, &value).cloned())
    })?;
    let name = text(&row, "displayName").unwrap_or(base).to_owned();
    row["value"] = json!(named);
    row["displayName"] = json!(format!("{name} (1M context)"));
    if let Some(resolved) = text(&row, "resolvedModel").map(str::to_owned) {
        row["resolvedModel"] = json!(format!("{resolved}[1m]"));
    }
    models.push(row);
    Some(named.to_owned())
}

pub(crate) fn offers_model(models: &[Value], value: &str) -> bool {
    find(models, value).is_some()
}

/// The effort levels `model` takes, without the default.
pub(crate) fn effort_levels(models: &[Value], model: &str) -> Vec<String> {
    let Some(row) = find(models, model) else {
        return Vec::new();
    };
    if row.get("supportsEffort").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    row.get("supportedEffortLevels")
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn level_name(level: &str) -> String {
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

fn model_option(models: &[Value], current: &str) -> Value {
    let options: Vec<Value> = models
        .iter()
        .filter_map(|model| {
            let value = text(model, "value")?;
            let name = text(model, "displayName").unwrap_or(value);
            let description = if value == DEFAULT {
                let resolved = text(model, "resolvedModel");
                models
                    .iter()
                    .filter(|other| text(other, "value") != Some(DEFAULT))
                    .find(|other| resolved.is_some() && text(other, "resolvedModel") == resolved)
                    .and_then(|other| text(other, "displayName"))
                    .or(resolved)
                    .or_else(|| text(model, "description"))
            } else {
                text(model, "description")
            };
            let mut option = json!({ "value": value, "name": name });
            if let Some(description) = description {
                option["description"] = json!(description);
            }
            Some(option)
        })
        .collect();
    json!({
        "id": "model",
        "name": "Model",
        "description": "AI model to use",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": options,
    })
}

fn effort_option(levels: &[String], current: &str) -> Value {
    let mut options = vec![json!({ "value": DEFAULT, "name": "Default" })];
    options.extend(
        levels
            .iter()
            .map(|level| json!({ "value": level, "name": level_name(level) })),
    );
    let current = if current == DEFAULT || levels.iter().any(|level| level == current) {
        current
    } else {
        DEFAULT
    };
    json!({
        "id": "effort",
        "name": "Effort",
        "description": "Available effort levels for this model",
        "category": "thought_level",
        "type": "select",
        "currentValue": current,
        "options": options,
    })
}

/// The chat's options for `model` at `effort`. A model that takes no effort
/// setting has no effort option.
pub(crate) fn options(models: &[Value], model: &str, effort: &str) -> Value {
    let mut options = vec![model_option(models, model)];
    let levels = effort_levels(models, model);
    if !levels.is_empty() {
        options.push(effort_option(&levels, effort));
    }
    Value::Array(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<Value> {
        vec![
            json!({ "value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)",
                    "description": "Opus 5.5 · Best for everyday, complex tasks", "supportsEffort": true,
                    "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"] }),
            json!({ "value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5",
                    "description": "For complex work and everyday tasks", "supportsEffort": true,
                    "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"] }),
            json!({ "value": "haiku", "resolvedModel": "claude-haiku-4-5-20251001", "displayName": "Haiku 4.5",
                    "description": "Fastest for quick answers" }),
        ]
    }

    #[test]
    fn the_default_model_is_described_by_the_model_it_resolves_to() {
        let options = options(&models(), "default", "high");
        assert_eq!(options[0]["options"][0]["description"], "Opus 5.5");
        assert_eq!(options[0]["options"][1]["name"], "Opus 5.5");
        assert_eq!(options[1]["category"], "thought_level");
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(options[1]["options"][4]["name"], "Xhigh");
    }

    #[test]
    fn a_settings_model_with_a_larger_context_gets_its_own_row() {
        let mut models = models();
        assert_eq!(
            settings_model(&mut models, "opus[1m]").as_deref(),
            Some("opus[1m]")
        );
        let options = options(&models, "opus[1m]", "high");
        let row = &options[0]["options"][3];
        assert_eq!(row["value"], "opus[1m]");
        assert_eq!(row["name"], "Opus 5.5 (1M context)");
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(
            settings_model(&mut models, "haiku").as_deref(),
            Some("haiku")
        );
        assert_eq!(
            settings_model(&mut models, "claude-opus-5-5").as_deref(),
            Some("default")
        );
        assert_eq!(settings_model(&mut models, "gpt-9"), None);
    }

    #[test]
    fn a_model_without_effort_has_no_effort_option() {
        let options = options(&models(), "haiku", "high");
        assert_eq!(options.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn an_effort_the_model_does_not_take_falls_back_to_the_default() {
        assert_eq!(
            options(&models(), "opus", "ultra")[1]["currentValue"],
            "default"
        );
    }

    #[test]
    fn a_resolved_model_reads_as_the_default_row_first() {
        assert_eq!(
            row_for_resolved(&models(), "claude-opus-5-5").as_deref(),
            Some("default")
        );
        assert_eq!(
            row_for_resolved(&models(), "claude-haiku-4-5-20251001").as_deref(),
            Some("haiku")
        );
        assert_eq!(row_for_resolved(&models(), "gpt"), None);
    }
}
