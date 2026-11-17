use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ImageParams {
    pub url: String,
}

/// A picture for a client to show, its bytes in base64.
#[derive(Serialize, Deserialize)]
pub struct PictureData {
    pub mime: String,
    pub data: String,
}
