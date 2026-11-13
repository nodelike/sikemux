use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct PixelsParams {
    pub name: String,
    /// The wallpaper's pixels as RGBA bytes, in base64.
    pub pixels: String,
}
