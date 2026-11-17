use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFileParams {
    pub path: PathBuf,
    /// For a picture shown small: the longest side, in pixels, to send it scaled to.
    #[serde(default)]
    pub max_edge: Option<u32>,
}

#[derive(Serialize)]
pub struct ChatFile {
    /// Set when the file was sent as a scaled copy, which is a JPEG.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<&'static str>,
    pub data: String,
}

#[derive(Deserialize, Serialize)]
pub struct KeepPastedParams {
    pub name: String,
    pub mime: String,
    pub data: String,
}

#[derive(Serialize, Deserialize)]
pub struct Kept {
    pub path: PathBuf,
}
