// Avatars and pictures Bitbucket shows beside its text. The window only draws
// images the app holds itself, so each is fetched here and handed over as a
// `data:` address. The credential only ever goes to Bitbucket itself.

use std::path::Path;

use reqwest::Url;
use sikemux_plugin_api::codehost::asks::ImageRef;
use sikemux_plugin_api::images;

use crate::client::{self, Session};
use crate::error::{BitbucketError, BitbucketResult};

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

pub async fn image(data_dir: &Path, input: ImageRef) -> BitbucketResult<String> {
    let refused = || BitbucketError::BadArg("that is not an image Bitbucket serves".into());
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    let response = images::follow(&client::HTTP, url, move |url, request| async move {
        if !access(&url).ok_or_else(refused)? {
            return Ok(request);
        }
        let session = Session::current(data_dir).await?;
        Ok(session.credential.apply(request))
    })
    .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(client::classify(status, &[]));
    }
    images::read_image(response).await
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
