//! How a Pi chat starts, and the models Pi offers read into the session pickers.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::acp::pickers;
use crate::launch::PermissionMode;
use sikemux_wire::protocol::ChatLaunch;

use super::super::models::{Effort, Model, Models};

/// Pi's thinking levels from least to most. `xhigh` and `max` exist only on a
/// model that maps them, and any level a model maps to null is switched off.
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub(super) fn permissions(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "bypass",
        _ => "normal",
    }
}

pub(super) fn arguments(launch: &ChatLaunch, extension: Option<&str>) -> Vec<String> {
    let mut args = vec!["--mode".to_owned(), "rpc".to_owned()];
    if let Some(extension) = extension {
        args.extend(["--extension".to_owned(), extension.to_owned()]);
    }
    if let Some(session) = &launch.resume_id {
        args.extend(["--session".to_owned(), session.clone()]);
    }
    args
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Listed {
    pub id: String,
    pub provider: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    thinking_level_map: Option<serde_json::Map<String, Value>>,
}

impl Listed {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    fn levels(&self) -> Vec<&'static str> {
        if !self.reasoning {
            return Vec::new();
        }
        let mapped = |level: &str| {
            self.thinking_level_map
                .as_ref()
                .and_then(|map| map.get(level))
        };
        LEVELS
            .into_iter()
            .filter(|level| match (*level, mapped(level)) {
                (_, Some(Value::Null)) => false,
                ("xhigh" | "max", found) => found.is_some(),
                _ => true,
            })
            .collect()
    }
}

/// The level a model runs `wanted` at: that level if it has it, otherwise the
/// nearest one above, otherwise the nearest below.
pub(super) fn nearest_level(levels: &[&str], wanted: &str) -> Option<String> {
    let at = LEVELS.iter().position(|level| *level == wanted)?;
    let above = LEVELS[at..].iter().find(|level| levels.contains(level));
    above
        .or_else(|| {
            LEVELS[..at]
                .iter()
                .rev()
                .find(|level| levels.contains(level))
        })
        .map(|level| (*level).to_owned())
}

/// The models Pi can use, `current` first among equals with the level it runs at.
pub(super) fn models(listed: &[Listed], current: Option<&str>, level: Option<&str>) -> Models {
    let listed = listed
        .iter()
        .map(|model| {
            let slug = model.slug();
            let levels = model.levels();
            let is_current = current == Some(slug.as_str());
            Model {
                name: model.name.clone().unwrap_or_else(|| model.id.clone()),
                description: Some(model.provider.clone()),
                efforts: levels
                    .iter()
                    .map(|value| Effort {
                        value: (*value).to_owned(),
                        description: None,
                    })
                    .collect(),
                default_effort: level
                    .filter(|_| is_current)
                    .and_then(|level| nearest_level(&levels, level)),
                takes_images: model.input.iter().any(|kind| kind == "image"),
                default: is_current,
                id: slug,
            }
        })
        .collect();
    Models {
        listed,
        takes_unlisted: false,
    }
}

pub(super) fn split_slug(slug: &str) -> Option<(&str, &str)> {
    slug.split_once('/')
        .filter(|(provider, id)| !provider.is_empty() && !id.is_empty())
}

pub(super) fn set_model(provider: &str, id: &str) -> Value {
    json!({ "type": "set_model", "provider": provider, "modelId": id })
}

pub(super) fn set_level(level: &str) -> Value {
    json!({ "type": "set_thinking_level", "level": level })
}

/// The pickers a chat with no usable model shows: none to choose from.
pub(super) fn no_models() -> Value {
    Value::Array(vec![pickers::model_select(
        Some("Model Pi uses for the session"),
        "",
        Vec::new(),
    )])
}
