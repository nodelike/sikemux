use serde::{Deserialize, Serialize};
use sikemux_agents::launch::providers::Helper;
use std::collections::BTreeMap;

/// The models a provider serves, with what it starts chats on.
#[derive(Serialize, Deserialize)]
pub struct ServedModels {
    pub models: Vec<String>,
    #[serde(flatten)]
    pub choices: Choices,
}

#[derive(Serialize, Deserialize)]
pub struct Choices {
    /// The coding model it starts helpers and checks on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model each agent's chat starts on.
    pub defaults: BTreeMap<String, String>,
    /// What models.dev says of these models, by API name.
    pub facts: BTreeMap<String, ModelFacts>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFacts {
    pub context: u64,
    /// USD per million tokens; a plan's models cost nothing more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    /// Only a hint: a check is the final word.
    pub tool_call: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ProviderParams {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct CheckParams {
    pub id: String,
    pub model: String,
}

#[derive(Serialize, Deserialize)]
pub struct HelperParams {
    pub helper: Option<Helper>,
}

#[derive(Serialize, Deserialize)]
pub struct ConnectParams {
    pub id: String,
    pub key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entered {
    pub base_url: String,
    #[serde(default)]
    pub key: Option<String>,
    /// The name the person gave it, in place of the one read from its URL.
    #[serde(default)]
    pub label: Option<String>,
}

/// The provider at a base URL, checked with the key entered or saved. Never the key.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// Its name: the catalogue's, the one entered, or one from its domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The base URL as Sikemux keeps it, without a trailing slash or endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub has_key: bool,
    /// The catalogue's provider at that URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_id: Option<String>,
    pub speaks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// Why it cannot be added as it stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub works: bool,
    /// When it was checked, in Unix milliseconds.
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
