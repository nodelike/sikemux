use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct DiagnosisParams {
    /// The linked host to diagnose by its core key; none for this host.
    #[serde(default)]
    pub host: Option<String>,
}
