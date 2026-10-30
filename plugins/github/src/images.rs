// Pictures GitHub shows beside its text: avatars, and images in pull
// requests, issues and release notes. The window only draws images the app
// holds itself, so each is fetched here and handed over as a `data:` address.

use std::path::Path;
use std::sync::OnceLock;

use reqwest::Url;
use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::codehost::asks::ImageRef;
use sikemux_plugin_api::images;

use crate::client::{self, Session};
use crate::config::{self, DEFAULT_HOST};
use crate::error::{GithubError, GithubResult};

fn authority(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// Whether an image may be fetched, and if so whether it gets the token. Only
/// GitHub's own image hosts are reached, and the token only goes to the GitHub
/// signed in to, which is where private attachments are kept.
fn access(url: &Url, signed_in: &str) -> Option<bool> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let at = authority(url)?;
    if at == DEFAULT_HOST {
        return url
            .path()
            .starts_with("/user-attachments/")
            .then_some(signed_in == DEFAULT_HOST);
    }
    if at == signed_in {
        return Some(true);
    }
    let company_media = signed_in != DEFAULT_HOST && at.ends_with(&format!(".{signed_in}"));
    (company_media || at.ends_with(".githubusercontent.com")).then_some(false)
}

pub async fn image(data_dir: &Path, input: ImageRef) -> GithubResult<String> {
    let refused = || GithubError::BadArg("that is not an image GitHub serves".into());
    let signed_in = &config::load(data_dir)?.host_of(chosen().as_deref());
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    let token_owner = &OnceLock::new();
    let response = images::follow(&client::HTTP, url, move |url, request| async move {
        if !access(&url, signed_in).ok_or_else(refused)? {
            return Ok(request);
        }
        // With nobody signed in, a public image is still fetched, only without a token.
        match Session::current(data_dir).await {
            Ok(session) => {
                token_owner.get_or_init(|| session.account.id);
                Ok(request.bearer_auth(session.token))
            }
            Err(GithubError::Unconfigured) => Ok(request),
            Err(error) => Err(error),
        }
    })
    .await?;
    if !response.status().is_success() {
        let account = token_owner.get().map_or("", String::as_str);
        return Err(client::failure(account, response).await);
    }
    images::read_image(response).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(raw: &str, signed_in: &str) -> Option<bool> {
        access(&Url::parse(raw).expect("parses"), signed_in)
    }

    #[test]
    fn github_image_hosts_are_reached_without_the_token() {
        for raw in [
            "https://avatars.githubusercontent.com/u/1?v=4",
            "https://private-user-images.githubusercontent.com/1/abc.png?jwt=x",
            "https://user-images.githubusercontent.com/1/abc.png",
            "https://camo.githubusercontent.com/abc/def",
            "https://raw.githubusercontent.com/a/b/main/logo.png",
        ] {
            assert_eq!(allowed(raw, "github.com"), Some(false), "{raw}");
        }
    }

    #[test]
    fn an_attachment_gets_the_token_only_from_the_github_signed_in_to() {
        let attachment = "https://github.com/user-attachments/assets/0f1e-2d3c";
        assert_eq!(allowed(attachment, "github.com"), Some(true));
        assert_eq!(allowed(attachment, "ghe.corp"), Some(false));
        assert_eq!(
            allowed("https://github.com/settings/tokens", "github.com"),
            None
        );
    }

    #[test]
    fn a_company_github_serves_its_own_images() {
        assert_eq!(
            allowed("https://ghe.corp/storage/user/1/files/a.png", "ghe.corp"),
            Some(true)
        );
        assert_eq!(
            allowed("https://ghe.corp:8443/a.png", "ghe.corp:8443"),
            Some(true)
        );
        assert_eq!(allowed("https://ghe.corp/a.png", "ghe.corp:8443"), None);
        assert_eq!(
            allowed("https://media.ghe.corp/user/1/a.png", "ghe.corp"),
            Some(false)
        );
        assert_eq!(allowed("https://ghe.corp/a.png", "github.com"), None);
    }

    #[test]
    fn anything_else_is_refused() {
        for raw in [
            "http://avatars.githubusercontent.com/u/1",
            "https://githubusercontent.com.evil.example/a.png",
            "https://evilgithubusercontent.com/a.png",
            "https://example.com/a.png",
            "https://user:pass@avatars.githubusercontent.com/u/1",
            "https://127.0.0.1/a.png",
            "file:///etc/passwd",
        ] {
            assert_eq!(allowed(raw, "github.com"), None, "{raw}");
        }
    }
}
