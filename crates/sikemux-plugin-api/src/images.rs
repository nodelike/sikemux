//! Pictures a service shows beside its text. The window only draws images the
//! app holds itself, so each is fetched by the plugin and handed over as a
//! `data:` address.

use std::future::Future;

use base64::Engine;
use reqwest::{RequestBuilder, Response, Url};

use crate::http::{read_body, Http};
use crate::service::{Service, ServiceError};

pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// How many addresses are asked for one image before it is given up on.
const MAX_HOPS: usize = 3;

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

/// Asks for an image, following it wherever it moves. `prepare` sees each
/// address before it is asked: it refuses one the plugin does not fetch, and
/// adds the plugin's credential to the request where that address should get
/// it. The first answer that is not a move comes back, for the plugin to read.
pub async fn follow<S, F, Fut>(
    http: &Http<S>,
    mut url: Url,
    mut prepare: F,
) -> Result<Response, ServiceError<S>>
where
    S: Service,
    F: FnMut(Url, RequestBuilder) -> Fut,
    Fut: Future<Output = Result<RequestBuilder, ServiceError<S>>>,
{
    for _ in 0..MAX_HOPS {
        let request = prepare(url.clone(), http.client()?.get(url.clone())).await?;
        let response = http.send(request).await?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        url = moved_to(&response, &url)?;
    }
    Err(ServiceError::Response(
        "the image moved too many times".into(),
    ))
}

fn moved_to<S: Service>(response: &Response, from: &Url) -> Result<Url, ServiceError<S>> {
    response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .and_then(|location| from.join(location).ok())
        .ok_or_else(|| ServiceError::Response("the image moved to no address".into()))
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
