use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::FailureCategory;

/// A plugin's own method and what it takes, which only the plugin knows the shape of.
#[derive(Serialize, Deserialize)]
pub struct PluginCall {
    pub plugin: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize, Deserialize)]
pub struct Disabled {
    pub ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct StreamCall {
    /// The client's number for the stream its items arrive as.
    pub stream: u64,
    #[serde(flatten)]
    pub call: PluginCall,
}

/// How a plugin call failed, as the window shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginFailure {
    pub plugin: String,
    pub category: FailureCategory,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl PluginFailure {
    pub fn new(plugin: &str, category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            category,
            message: message.into(),
            status: None,
        }
    }
}

/// What the core answers the window's `plugin` call with.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginOutcome {
    Value(Value),
    Failed(PluginFailure),
}

impl PluginOutcome {
    /// The value inside the answer, read as a `T`, or what its failure said.
    pub fn read<T: serde::de::DeserializeOwned>(self) -> Result<T, String> {
        match self {
            PluginOutcome::Value(value) => {
                serde_json::from_value(value).map_err(|error| error.to_string())
            }
            PluginOutcome::Failed(failure) => Err(failure.message),
        }
    }
}

impl From<PluginFailure> for String {
    fn from(failure: PluginFailure) -> Self {
        failure.message
    }
}
