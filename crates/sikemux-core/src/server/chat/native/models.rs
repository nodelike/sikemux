//! The models an agent's `model/list` offered and the effort levels each
//! takes, read into one shape whatever the agent named its fields, and the
//! session pickers built from them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::chat::pickers;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Model {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub efforts: Vec<Effort>,
    /// What the model runs at when no effort is asked for, one of `efforts`.
    pub default_effort: Option<String>,
    pub takes_images: bool,
    /// The one the agent picks when none is named.
    pub default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Effort {
    pub value: String,
    pub description: Option<String>,
}

/// The first of `preferred` the model offers, else its first effort.
pub(super) fn default_effort(efforts: &[Effort], preferred: &[Option<&str>]) -> Option<String> {
    preferred
        .iter()
        .flatten()
        .find(|wanted| efforts.iter().any(|effort| effort.value == **wanted))
        .map(|wanted| (*wanted).to_owned())
        .or_else(|| efforts.first().map(|effort| effort.value.clone()))
}

/// Reads each entry of a model listing, leaving out with a warning any the
/// agent sent in a shape it should not have.
pub(super) fn read_listing<Listed: serde::de::DeserializeOwned>(
    agent: &str,
    entries: Vec<Value>,
) -> Vec<Listed> {
    entries
        .into_iter()
        .filter_map(|entry| match serde_json::from_value(entry) {
            Ok(listed) => Some(listed),
            Err(error) => {
                eprintln!("sikemux core: {agent} listed a model Sikemux cannot read: {error}");
                None
            }
        })
        .collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Models {
    pub listed: Vec<Model>,
    /// Whether a model missing from `listed` still goes, for an agent that
    /// lists no models when it runs on someone else's.
    pub takes_unlisted: bool,
}

impl Models {
    fn find(&self, model: &str) -> Option<&Model> {
        self.listed.iter().find(|entry| entry.id == model)
    }

    pub fn offers(&self, model: &str) -> bool {
        self.takes_unlisted || self.find(model).is_some()
    }

    pub fn default_model(&self) -> Option<String> {
        self.listed
            .iter()
            .find(|entry| entry.default)
            .or(self.listed.first())
            .map(|entry| entry.id.clone())
    }

    fn efforts(&self, model: &str) -> &[Effort] {
        self.find(model).map_or(&[], |entry| &entry.efforts)
    }

    pub fn reasons(&self, model: &str) -> bool {
        !self.efforts(model).is_empty()
    }

    pub fn supports_effort(&self, model: &str, effort: &str) -> bool {
        self.efforts(model)
            .iter()
            .any(|option| option.value == effort)
    }

    /// Whether the model reads images. A model missing from the list is given
    /// the benefit of the doubt and the agent says if it cannot.
    pub fn takes_images(&self, model: &str) -> bool {
        self.find(model).is_none_or(|entry| entry.takes_images)
    }

    /// The effort to run `model` at: `wanted` when the model offers it,
    /// otherwise the model's own default.
    pub fn effort_for(&self, model: &str, wanted: Option<&str>) -> Option<String> {
        if let Some(wanted) = wanted.filter(|wanted| self.supports_effort(model, wanted)) {
            return Some(wanted.to_owned());
        }
        self.find(model)?.default_effort.clone()
    }

    /// The session's `configOptions`: the model picker, and the effort picker
    /// when the model reasons.
    pub fn options(&self, agent: &str, model: &str, effort: Option<&str>) -> Value {
        let models = self
            .listed
            .iter()
            .map(|entry| pickers::choice(&entry.id, &entry.name, entry.description.as_deref()))
            .collect();
        let mut options = vec![pickers::model_select(
            Some(&format!("Model {agent} uses for the session")),
            model,
            models,
        )];
        if let Some(effort) = effort.filter(|_| self.reasons(model)) {
            let efforts = self
                .efforts(model)
                .iter()
                .map(|option| {
                    pickers::choice(
                        &option.value,
                        &pickers::level_name(&option.value),
                        option.description.as_deref(),
                    )
                })
                .collect();
            options.push(pickers::effort_select(
                "reasoning_effort",
                "Reasoning effort",
                "How much reasoning effort the model should use",
                effort,
                efforts,
            ));
        }
        Value::Array(options)
    }
}
