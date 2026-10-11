// Avatars and pictures GitLab shows beside its text. The window only draws
// images the app holds itself, so each is fetched here and handed over as a
// `data:` address. A token only ever goes to its own account's server.

use std::path::Path;

use base64::Engine;
use reqwest::Url;
use serde::Deserialize;

use crate::client::{self, Session};
use crate::config::{self, Account};
use crate::error::{GitlabError, GitlabResult};

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_HOPS: usize = 3;

#[derive(Deserialize)]
pub struct ImageRef {
    pub url: String,
}

/// Whether an image may be fetched, and if so with which account's token: only
/// an account on the image's own server sends one. Gravatar and GitLab's own
/// image hosts need none.
fn access<'a>(url: &Url, accounts: &'a [Account]) -> Option<Option<&'a Account>> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let with_port = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    if let Some(account) = accounts.iter().find(|account| account.host == with_port) {
        return Some(Some(account));
    }
    let public = host == "gitlab.com"
        || host.ends_with(".gitlab-static.net")
        || host == "secure.gravatar.com"
        || host == "www.gravatar.com";
    public.then_some(None)
}

/// `image/png` and the like, and nothing that could break out of a `data:` address.
fn image_kind(content_type: &str) -> Option<String> {
    let kind = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let plain = kind
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '.' | '-'));
    (kind.starts_with("image/") && plain).then_some(kind)
}

pub async fn image(data_dir: &Path, input: ImageRef) -> GitlabResult<String> {
    let refused = || GitlabError::BadArg("that is not an image GitLab serves".into());
    let mut url = Url::parse(&input.url).map_err(|_| refused())?;
    let chosen = client::chosen();
    let mut accounts = config::load(data_dir).in_order();
    accounts.sort_by_key(|account| Some(&account.id) != chosen.as_ref());
    for _ in 0..MAX_HOPS {
        let mut request = client::http()?.get(url.clone());
        if let Some(owner) = access(&url, &accounts).ok_or_else(refused)? {
            let session =
                client::as_account(Some(owner.id.clone()), Session::current(data_dir)).await;
            if let Ok(session) = session {
                request = client::authorize(request, &session.token);
            }
        }
        let response = client::limited(request.send()).await?;
        if response.status().is_redirection() {
            url = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or_else(|| GitlabError::Response("the image moved to no address".into()))?;
            continue;
        }
        let status = response.status();
        if !status.is_success() {
            return Err(client::classify(status, &[]));
        }
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(image_kind)
            .ok_or_else(|| GitlabError::Response("that address is not an image".into()))?;
        let (bytes, _) = client::read_body(response, MAX_IMAGE_BYTES, false).await?;
        return Ok(format!(
            "data:{kind};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ));
    }
    Err(GitlabError::Response(
        "the image moved too many times".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(host: &str) -> Account {
        Account {
            id: Account::id_for(host, 1),
            host: host.into(),
            login: "someone".into(),
            display_name: None,
            avatar_url: None,
        }
    }

    fn token_for(raw: &str, accounts: &[Account]) -> Option<Option<String>> {
        access(&Url::parse(raw).expect("parses"), accounts)
            .map(|owner| owner.map(|account| account.id.clone()))
    }

    #[test]
    fn a_token_only_goes_to_its_own_accounts_server() {
        let accounts = [on("gitlab.com"), on("gitlab.acme.dev")];
        assert_eq!(
            token_for(
                "https://gitlab.acme.dev/uploads/-/system/user/avatar/3/a.png",
                &accounts
            ),
            Some(Some("gitlab.acme.dev#1".into()))
        );
        assert_eq!(
            token_for("https://gitlab.com/uploads/a.png", &accounts),
            Some(Some("gitlab.com#1".into()))
        );
        assert_eq!(
            token_for(
                "https://gitlab.com/uploads/-/system/user/avatar/3/a.png",
                &accounts[1..]
            ),
            Some(None)
        );
        assert_eq!(
            token_for("https://secure.gravatar.com/avatar/abc", &accounts),
            Some(None)
        );
        assert_eq!(
            token_for(
                "https://git.acme.dev:8443/a.png",
                &[on("git.acme.dev:8443")]
            ),
            Some(Some("git.acme.dev:8443#1".into()))
        );
    }

    #[test]
    fn anywhere_else_is_refused() {
        let accounts = [on("gitlab.acme.dev")];
        for raw in [
            "http://gitlab.com/a.png",
            "https://evil.example/a.png",
            "https://gitlab.acme.dev.evil.example/a.png",
            "https://gitlab.acme.dev:8443/a.png",
            "https://user:pw@gitlab.com/a.png",
        ] {
            assert_eq!(token_for(raw, &accounts), None, "{raw}");
        }
    }
}
