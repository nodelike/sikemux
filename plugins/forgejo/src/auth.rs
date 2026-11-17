// Signing accounts in and out with an access token, on Codeberg, gitea.com or a
// company's own Forgejo or Gitea, and saying who the app is talking to it as.
// Signing in to a second account adds it beside the first.

use std::path::Path;

use serde::Deserialize;

use sikemux_plugin_api::account::chosen;
use sikemux_plugin_api::codehost::answers;
use sikemux_plugin_api::codehost::asks::AccountQuery;
use sikemux_plugin_api::codehost::Signed;
use sikemux_plugin_api::http::{self, Read};
use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::SignInStatus;

use crate::client::{self, Session};
use crate::config::{self, Account, ForgejoConfig, CODEBERG};
use crate::error::{ForgejoError, ForgejoResult};
use crate::repo::{ApiPath, User};

/// The server's host name from whatever was typed: `git.acme.dev`, a whole
/// address, or nothing for Codeberg. Only https servers are taken, since the
/// token travels with every request.
pub fn host_of(typed: &str) -> ForgejoResult<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(CODEBERG.into());
    }
    if typed.starts_with("http://") {
        return Err(ForgejoError::BadArg(
            "Forgejo is reached over https only, since the token goes with every request".into(),
        ));
    }
    http::bare_host(typed)
        .filter(|host| host.contains('.') && !typed.contains('@'))
        .ok_or_else(|| {
            ForgejoError::BadArg(format!(
                "`{typed}` is not a Forgejo or Gitea server address"
            ))
        })
}

async fn identify(host: &str, token: &str) -> ForgejoResult<User> {
    let request = client::authorized(
        client::HTTP
            .client()?
            .get(format!("{}/user", client::api_base(host))),
        token,
    );
    client::HTTP
        .ask(request, Read::IDENTITY)
        .await?
        .succeeded()?
        .parsed()
}

pub type Status = SignInStatus<Signed>;

pub async fn status(data_dir: &Path) -> Status {
    let config = match config::load(data_dir) {
        Ok(config) => config,
        Err(error) => return Status::checked(false, Signed::default(), &Err::<(), _>(error)),
    };
    let account = config.account(chosen().as_deref()).cloned();
    let mut signed = Signed {
        account: account.as_ref().map(|account| account.id.clone()),
        host: account.as_ref().map(|account| account.host.clone()),
        login: account
            .as_ref()
            .map(|account| account.login.clone())
            .unwrap_or_default(),
        display_name: account
            .as_ref()
            .and_then(|account| account.display_name.clone()),
        avatar_url: account
            .as_ref()
            .and_then(|account| account.avatar_url.clone()),
        ..Signed::default()
    };
    let check = match Session::current(data_dir).await {
        Ok(session) => identify(&session.account.host, &session.token).await,
        Err(error) => Err(error),
    };
    match &check {
        Ok(user) => {
            if let Some(login) = user.login.clone() {
                signed.login = login;
            }
            signed.display_name = user.display_name();
            signed.avatar_url = user.avatar_url.clone();
            signed.can_write_ci = true;
        }
        Err(ForgejoError::RateLimited { .. }) => signed.can_write_ci = true,
        Err(_) => {}
    }
    Status::checked(account.is_some(), signed, &check)
}

fn new_account(host: &str, user: &User) -> ForgejoResult<Account> {
    let user_id = user.id.ok_or_else(|| {
        ForgejoError::Response("the server did not say who the token belongs to".into())
    })?;
    Ok(Account {
        id: Account::id_for(host, user_id),
        host: host.to_string(),
        login: user
            .login
            .clone()
            .unwrap_or_else(|| format!("user {user_id}")),
        display_name: user.display_name(),
        avatar_url: user.avatar_url.clone(),
    })
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
    /// The server; left out or empty for Codeberg.
    #[serde(default)]
    pub host: String,
}

pub async fn sign_in_with_token(data_dir: &Path, input: TokenSignIn) -> ForgejoResult<String> {
    let token = input.token.trim().to_string();
    if token.is_empty() {
        return Err(ForgejoError::BadArg("no token was given".into()));
    }
    let host = host_of(&input.host)?;
    let user = identify(&host, &token).await?;
    let account = new_account(&host, &user)?;
    let id = account.id.clone();
    store::update(data_dir, move |config: &mut ForgejoConfig| {
        config::KEYCHAIN.write(&account.keychain_account(), &token)?;
        config.upsert(account);
        Ok(())
    })
    .await?;
    client::forget(Some(&id));
    Ok(id)
}

pub async fn sign_out(data_dir: &Path) -> ForgejoResult<()> {
    let removed = store::sign_out::<ForgejoConfig, _, _>(data_dir, chosen(), |account| {
        Ok(config::KEYCHAIN.delete(&account.keychain_account())?)
    })
    .await?;
    if let Some((account, ())) = removed {
        client::forget(Some(&account.id));
    }
    Ok(())
}

/// The first account, default first, on the repository's server that can see it.
/// When none can, the first on that server still answers, so the repository is
/// never looked for on another server.
pub async fn account_for(data_dir: &Path, input: AccountQuery) -> ForgejoResult<Option<String>> {
    let path = input.repo.path("")?;
    let on_server: Vec<Account> = config::load(data_dir)?
        .in_order()
        .into_iter()
        .filter(|account| {
            input
                .host
                .as_deref()
                .is_none_or(|host| config::same_server(host, &account.host))
        })
        .collect();
    let ids = on_server.iter().map(|account| account.id.clone()).collect();
    let seen = answers::first_that_sees(&client::API, data_dir, ids, &path).await?;
    if seen.is_some() {
        return Ok(seen);
    }
    Ok(input
        .host
        .and(on_server.into_iter().next())
        .map(|account| account.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_read_from_whatever_was_typed() -> ForgejoResult<()> {
        assert_eq!(host_of("")?, "codeberg.org");
        assert_eq!(host_of("gitea.com")?, "gitea.com");
        assert_eq!(host_of("https://Git.Acme.dev/user/login")?, "git.acme.dev");
        assert_eq!(host_of("git.acme.dev:3000")?, "git.acme.dev:3000");
        for typed in ["http://git.acme.dev", "localhost", "user@codeberg.org"] {
            assert!(host_of(typed).is_err(), "{typed}");
        }
        Ok(())
    }
}
