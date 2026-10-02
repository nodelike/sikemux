// Avatars and pictures Bitbucket shows beside its text. The window only draws
// images the app holds itself, so each is fetched here and handed over as a
// `data:` address. The credential only ever goes to Bitbucket itself.

use std::path::Path;

use base64::Engine;
use reqwest::Url;
use serde::Deserialize;

use crate::client::{self, Session};
use crate::error::{BitbucketError, BitbucketResult};

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Deserialize)]
pub struct ImageRef {
    pub url: String,
}

/// Whether an image may be fetched, and if so whether it gets the credential.
/// Avatars live on Atlassian's and Gravatar's hosts, which need none.
fn access(url: &Url) -> Option<bool> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?;
    if host == "bitbucket.org" || host == "api.bitbucket.org" {
        return Some(true);
    }
    let public = host.ends_with(".atl-paas.net")
        || host == "secure.gravatar.com"
        || host.ends_with(".bitbucket.org")
        || host.ends_with(".atlassian.com");
    public.then_some(false)
}

/// `image/png` and the like, and nothing that could break out of a `data:` address.
fn image_kind(content_type: &str) -> Option<String> {
    let kind = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let plain = kind
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '.' | '-'));
    (kind.starts_with("image/") && plain).then_some(kind)
}

pub async fn image(data_dir: &Path, input: ImageRef) -> BitbucketResult<String> {
    let refused = || BitbucketError::BadArg("that is not an image Bitbucket serves".into());
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    let mut request = client::http()?.get(url.clone());
    if access(&url).ok_or_else(refused)? {
        if let Ok(session) = Session::current(data_dir).await {
            request = session.credential.apply(request);
        }
    }
    let response = client::limited(request.send()).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(client::classify(status, &[]));
    }
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(image_kind)
        .ok_or_else(|| BitbucketError::Response("that address is not an image".into()))?;
    let (bytes, _) = client::read_body(response, MAX_IMAGE_BYTES, false).await?;
    Ok(format!(
        "data:{kind};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(raw: &str) -> Option<bool> {
        access(&Url::parse(raw).expect("parses"))
    }

    #[test]
    fn the_credential_only_goes_to_bitbucket() {
        assert_eq!(
            allowed("https://bitbucket.org/account/x/avatar/32/"),
            Some(true)
        );
        assert_eq!(
            allowed(
                "https://avatar-management--avatars.us-west-2.prod.public.atl-paas.net/1/a.png"
            ),
            Some(false)
        );
        assert_eq!(
            allowed("https://secure.gravatar.com/avatar/abc"),
            Some(false)
        );
    }

    #[test]
    fn anywhere_else_is_refused() {
        for raw in [
            "http://bitbucket.org/a.png",
            "https://evil.example/a.png",
            "https://bitbucket.org.evil.example/a.png",
            "https://user:pw@bitbucket.org/a.png",
        ] {
            assert_eq!(allowed(raw), None, "{raw}");
        }
    }
}
