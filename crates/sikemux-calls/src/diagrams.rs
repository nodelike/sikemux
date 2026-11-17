use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
pub struct DiagramParams {
    pub key: String,
    /// The arrangement to keep; without one the kept arrangement is forgotten.
    #[serde(default)]
    pub layout: Option<Value>,
}

/// The arrangement kept for a diagram, whose shape only the app that drew it knows.
#[derive(Serialize, Deserialize)]
pub struct DiagramLayout {
    pub layout: Option<Value>,
}
