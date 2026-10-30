//! The session options a Claude chat offers: its model and how hard that
//! model thinks, read from the models the CLI lists when it starts.

use serde_json::{json, Value};

use super::super::json::text;
use crate::launch::PermissionMode;
use crate::server::chat::pickers;

pub(crate) const DEFAULT: &str = "default";

/// The effort Claude Code runs each model at when none is set, which its model list does not
/// say, by the model's full name.
const OWN_EFFORTS: &[(&str, &str)] = &[
    ("claude-fable-5-1", "medium"),
    ("claude-fable-5", "medium"),
    ("claude-opus-5-5", "medium"),
    ("claude-opus-5", "high"),
    ("claude-opus-4-8", "high"),
    ("claude-opus-4-7", "xhigh"),
    ("claude-opus-4-6", "high"),
    ("claude-opus-4-5", "high"),
    ("claude-sonnet-5-5", "high"),
    ("claude-sonnet-5", "high"),
    ("claude-sonnet-4-6", "high"),
    ("claude-haiku-5-5", "medium"),
];

/// The permission mode Claude Code runs in for each of the app's modes.
pub(crate) fn permission_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "bypassPermissions",
        PermissionMode::WorkspaceWrite => "acceptEdits",
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

/// Lists a model another provider serves, which Claude Code itself does not.
pub(crate) fn provider_model(models: &mut Vec<Value>, value: &str) {
    if find(models, value).is_none() {
        models.push(json!({ "value": value, "displayName": value }));
    }
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

/// The models to pick from, one row each: Claude Code's default row goes by the name of the
/// model it stands for and is marked as the default, in place of that model's own row, which
/// shows instead only while it is the one picked.
fn model_option(models: &[Value], current: &str) -> Value {
    let resolved = find(models, DEFAULT).and_then(|row| text(row, "resolvedModel"));
    let twin = |row: &Value| {
        resolved.is_some()
            && text(row, "value") != Some(DEFAULT)
            && text(row, "resolvedModel") == resolved
    };
    let twin_picked = models
        .iter()
        .any(|row| twin(row) && text(row, "value") == Some(current));
    let choices = models
        .iter()
        .filter_map(|model| {
            let value = text(model, "value")?;
            let is_twin = twin(model);
            if value == DEFAULT {
                if twin_picked {
                    return None;
                }
                let named = models.iter().find(|other| twin(other)).unwrap_or(model);
                let name = text(named, "displayName").unwrap_or(value);
                let choice = pickers::choice(value, name, text(named, "description"));
                return Some(pickers::as_default(choice));
            }
            if is_twin && !twin_picked {
                return None;
            }
            let name = text(model, "displayName").unwrap_or(value);
            let choice = pickers::choice(value, name, text(model, "description"));
            Some(match is_twin {
                true => pickers::as_default(choice),
                false => choice,
            })
        })
        .collect();
    pickers::model_select(Some("AI model to use"), current, choices)
}

/// The effort `model` runs at when none is set, when it is one this knows.
fn own_effort(models: &[Value], model: &str) -> Option<&'static str> {
    let named = find(models, model)
        .and_then(|row| text(row, "resolvedModel"))
        .unwrap_or(model);
    let named = named.strip_suffix("[1m]").unwrap_or(named);
    let named = match named.rsplit_once('-') {
        Some((name, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => named,
    };
    OWN_EFFORTS
        .iter()
        .find(|(known, _)| *known == named)
        .map(|(_, effort)| *effort)
}

/// The levels to pick from, the model's own marked as the default; a model whose own level is
/// not known offers Auto, which leaves it to Claude Code.
fn effort_option(levels: &[String], current: &str, own: Option<&str>) -> Value {
    let own = own.filter(|own| levels.iter().any(|level| level == own));
    let mut choices: Vec<Value> = levels
        .iter()
        .map(|level| {
            let choice = pickers::choice(level, &pickers::level_name(level), None);
            match Some(level.as_str()) == own {
                true => pickers::as_default(choice),
                false => choice,
            }
        })
        .collect();
    if own.is_none() {
        choices.insert(
            0,
            pickers::as_default(pickers::choice(DEFAULT, "Auto", None)),
        );
    }
    let current = if levels.iter().any(|level| level == current) {
        current
    } else {
        own.unwrap_or(DEFAULT)
    };
    pickers::effort_select(
        "effort",
        "Effort",
        "Available effort levels for this model",
        current,
        choices,
    )
}

/// The chat's options for `model` at `effort`. A model that takes no effort
/// setting has no effort option.
pub(crate) fn options(models: &[Value], model: &str, effort: &str) -> Value {
    let mut options = vec![model_option(models, model)];
    let levels = effort_levels(models, model);
    if !levels.is_empty() {
        options.push(effort_option(&levels, effort, own_effort(models, model)));
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
    fn the_default_model_goes_by_the_model_it_resolves_to_in_that_models_place() {
        let options = options(&models(), "default", "high");
        let rows = options[0]["options"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["value"], "default");
        assert_eq!(rows[0]["name"], "Opus 5.5");
        assert_eq!(rows[0]["isDefault"], true);
        assert_eq!(options[1]["category"], "thought_level");
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(options[1]["options"][1]["isDefault"], true);
        assert_eq!(options[1]["options"][3]["name"], "Extra high");
    }

    #[test]
    fn a_settings_model_with_a_larger_context_gets_its_own_row() {
        let mut models = models();
        assert_eq!(
            settings_model(&mut models, "opus[1m]").as_deref(),
            Some("opus[1m]")
        );
        let options = options(&models, "opus[1m]", "high");
        let row = &options[0]["options"][2];
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
    fn an_effort_the_model_does_not_take_falls_back_to_the_models_own() {
        assert_eq!(
            options(&models(), "opus", "ultra")[1]["currentValue"],
            "medium"
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
