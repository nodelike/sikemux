//! Pictures a service shows beside its text. The window only draws images the
//! app holds itself, so each is fetched by the plugin and handed over as a
//! `data:` address.

use base64::Engine;
use reqwest::{Response, Url};

use crate::http::read_body;
use crate::service::{Service, ServiceError};

pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// How many times an image may move before it is given up on.
pub const MAX_HOPS: usize = 3;

/// `image/png` and the like, and nothing that could break out of a `data:` address.
pub fn image_kind(content_type: &str) -> Option<String> {
    let kind = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let plain = kind
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '.' | '-'));
    (kind.starts_with("image/") && plain).then_some(kind)
}

pub fn data_address(kind: &str, bytes: &[u8]) -> String {
    format!(
        "data:{kind};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Where a redirect sends the image, which the caller checks before following.
pub fn moved_to<S: Service>(response: &Response, from: &Url) -> Result<Url, ServiceError<S>> {
    response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .and_then(|location| from.join(location).ok())
        .ok_or_else(|| ServiceError::Response("the image moved to no address".into()))
}

pub fn moved_too_often<S>() -> ServiceError<S> {
    ServiceError::Response("the image moved too many times".into())
}

/// A successful answer read as an image.
pub async fn read_image<S: Service>(response: Response) -> Result<String, ServiceError<S>> {
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(image_kind)
        .ok_or_else(|| ServiceError::Response("that address is not an image".into()))?;
    let bytes = read_body(response, MAX_IMAGE_BYTES).await?;
    Ok(data_address(&kind, &bytes))
}
