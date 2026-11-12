// Avatars and pictures the server shows beside its text. The window only draws
// images the app holds itself, so each is fetched here and handed over as a
// `data:` address. A token only ever goes to its own account's server.

use std::path::Path;

use reqwest::Url;
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::codehost::asks::ImageRef;
use sikemux_plugin_api::images;
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::Service;

use crate::client::{self, Session};
use crate::config::{self, Account, PUBLIC};
use crate::error::{Forgejo, ForgejoError, ForgejoResult};

/// Whether an image may be fetched, and if so with which account's token: only
/// an account on the image's own server sends one. The public servers and the
/// avatar services they use need none.
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
    let public = url.port().is_none()
        && (PUBLIC.contains(&host)
            || matches!(
                host,
                "secure.gravatar.com" | "www.gravatar.com" | "seccdn.libravatar.org"
            ));
    public.then_some(None)
}

pub async fn image(data_dir: &Path, input: ImageRef) -> ForgejoResult<String> {
    let refused = || ForgejoError::BadArg("that is not an image Forgejo serves".into());
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    let chosen = chosen();
    let mut accounts = config::load(data_dir)?.in_order();
    accounts.sort_by_key(|account| Some(&account.id) != chosen.as_ref());
    let accounts = &accounts;
    let response = images::follow(&client::HTTP, url, move |url, request| async move {
        let Some(owner) = access(&url, accounts).ok_or_else(refused)? else {
            return Ok(request);
        };
        let session = as_account(Some(owner.id.clone()), Session::current(data_dir)).await?;
        Ok(client::authorized(request, &session.token))
    })
    .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Forgejo::refused(status, &[], None));
    }
    images::read_image(response).await
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
        let accounts = [on("codeberg.org"), on("git.acme.dev:3000")];
        assert_eq!(
            token_for("https://codeberg.org/avatars/abc", &accounts),
            Some(Some("codeberg.org#1".into()))
        );
        assert_eq!(
            token_for("https://git.acme.dev:3000/avatars/abc", &accounts),
            Some(Some("git.acme.dev:3000#1".into()))
        );
        assert_eq!(
            token_for("https://gitea.com/avatars/abc", &accounts),
            Some(None)
        );
        for raw in [
            "http://codeberg.org/a.png",
            "https://evil.example/a.png",
            "https://codeberg.org.evil.example/a.png",
            "https://git.acme.dev/a.png",
            "https://user:pw@codeberg.org/a.png",
        ] {
            assert_eq!(token_for(raw, &accounts), None, "{raw}");
        }
    }
}
