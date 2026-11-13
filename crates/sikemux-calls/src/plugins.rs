use serde::{Deserialize, Serialize};
use serde_json::Value;

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
