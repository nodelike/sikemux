use serde::{Deserialize, Serialize};

/// A change to the face style; a field left out stays as it is.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceStyleChange {
    pub on: Option<bool>,
    pub pack: Option<String>,
    pub shape: Option<String>,
    pub motion: Option<bool>,
}
