// Signing accounts in and out with an access token, on gitlab.com or a
// company's own server, and saying who the app is talking to GitLab as.
// Signing in to a second account adds it beside the first.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client::{self, Session};
use crate::config::{self, Account, GITLAB_COM};
use crate::error::{GitlabError, GitlabResult};
use crate::repo::{RepoRef, User};

/// The server's host name from whatever was typed: `gitlab.acme.dev`, a whole
/// address, or nothing for gitlab.com. Only https servers are taken, since the
/// token travels with every request.
pub fn host_of(typed: &str) -> GitlabResult<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(GITLAB_COM.into());
    }
    if typed.starts_with("http://") {
        return Err(GitlabError::BadArg(
            "GitLab is reached over https only, since the token goes with every request".into(),
        ));
    }
    let rest = typed.strip_prefix("https://").unwrap_or(typed);
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (name, port) = match host.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host.as_str(), None),
    };
    let name_ok = !name.is_empty()
        && name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    let port_ok = port.is_none_or(|port| port.parse::<u16>().is_ok());
    if !name_ok || !port_ok || host.contains('@') {
        return Err(GitlabError::BadArg(format!(
            "`{typed}` is not a GitLab server address"
        )));
    }
    Ok(host)
}

async fn get_with(host: &str, token: &str, path: &str) -> GitlabResult<serde_json::Value> {
    let request = client::authorize(
        client::http()?.get(format!("{}{path}", client::api_base(host))),
        token,
    );
    let response = client::limited(request.send()).await?;
    let status = response.status();
    let (bytes, _) = client::read_body(response, 1024 * 1024, false).await?;
    if !status.is_success() {
        return Err(client::classify(status, &bytes));
    }
    client::parse(&bytes)
}

async fn identify(host: &str, token: &str) -> GitlabResult<User> {
    Ok(serde_json::from_value(
        get_with(host, token, "/user").await?,
    )?)
}

/// What a personal access token may do. A group or project token cannot say,
/// and is taken at its word: GitLab says no later if it may not.
async fn scopes(host: &str, token: &str) -> Vec<String> {
    get_with(host, token, "/personal_access_tokens/self")
        .await
        .ok()
        .and_then(|token| token.get("scopes").cloned())
        .and_then(|scopes| serde_json::from_value(scopes).ok())
        .unwrap_or_default()
}

/// Only the `api` scope may start, retry and cancel pipelines; `read_api` only reads.
pub fn can_write(scopes: &[String]) -> bool {
    scopes.is_empty() || scopes.iter().any(|scope| scope == "api")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub account: Option<String>,
    pub host: Option<String>,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub can_write_ci: bool,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
}

/// The account the call named, or the default one.
pub async fn status(data_dir: &Path) -> Status {
    let config = config::load(data_dir);
    let account = config.account(client::chosen().as_deref()).cloned();
    let base = |ok: bool, auth_failed: bool, message: Option<String>| Status {
        configured: account.is_some(),
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
        can_write_ci: false,
        ok,
        auth_failed,
        message,
    };
    let session = match Session::current(data_dir).await {
        Ok(session) => session,
        Err(GitlabError::Unconfigured) => return base(false, false, None),
        Err(error) => return base(false, false, Some(error.to_string())),
    };
    match identify(&session.account.host, &session.token).await {
        Ok(user) => Status {
            login: user
                .username
                .clone()
                .unwrap_or_else(|| session.account.login.clone()),
            display_name: user.name.clone(),
            avatar_url: user.avatar_url.clone(),
            can_write_ci: can_write(&scopes(&session.account.host, &session.token).await),
            ..base(true, false, None)
        },
        Err(error @ GitlabError::RateLimited { .. }) => Status {
            can_write_ci: true,
            ..base(true, false, Some(error.to_string()))
        },
        Err(error) => {
            let auth_failed = matches!(error, GitlabError::Auth(_));
            base(false, auth_failed, Some(error.to_string()))
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub host: String,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_default: bool,
}

/// Every account signed in, the default first.
pub fn accounts(data_dir: &Path) -> Vec<Listed> {
    let config = config::load(data_dir);
    config
        .in_order()
        .into_iter()
        .map(|account| Listed {
            is_default: config.default.as_deref() == Some(account.id.as_str()),
            id: account.id,
            host: account.host,
            login: account.login,
            display_name: account.display_name,
            avatar_url: account.avatar_url,
        })
        .collect()
}

fn new_account(host: &str, user: &User) -> GitlabResult<Account> {
    let user_id = user.id.ok_or_else(|| {
        GitlabError::Response("GitLab did not say who the token belongs to".into())
    })?;
    Ok(Account {
        id: Account::id_for(host, user_id),
        host: host.to_string(),
        login: user
            .username
            .clone()
            .unwrap_or_else(|| format!("user {user_id}")),
        display_name: user.name.clone(),
        avatar_url: user.avatar_url.clone(),
    })
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
    /// The server; left out or empty for gitlab.com.
    #[serde(default)]
    pub host: String,
}

/// Adds the account the token belongs to, and says which one it is.
pub async fn sign_in_with_token(data_dir: &Path, input: TokenSignIn) -> GitlabResult<String> {
    let token = input.token.trim().to_string();
    if token.is_empty() {
        return Err(GitlabError::BadArg("no token was given".into()));
    }
    let host = host_of(&input.host)?;
    let user = identify(&host, &token).await?;
    let account = new_account(&host, &user)?;
    let id = account.id.clone();
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        config::keychain_write(&account, &token)?;
        let mut config = config::load(&data_dir);
        config.upsert(account);
        config::save(&data_dir, &config)
    }))
    .await?;
    client::forget(Some(&id));
    Ok(id)
}

/// Signs out the account the call named, or the default one.
pub async fn sign_out(data_dir: &Path) -> GitlabResult<()> {
    let data_dir = data_dir.to_path_buf();
    let chosen = client::chosen();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let Some(id) = config
            .account(chosen.as_deref())
            .map(|account| account.id.clone())
        else {
            return Ok(None);
        };
        let removed = config.remove(&id);
        if let Some(account) = &removed {
            config::keychain_delete(account)?;
        }
        config::save(&data_dir, &config)?;
        Ok(removed)
    }))
    .await?;
    if let Some(account) = removed {
        client::forget(Some(&account.id));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct DefaultChoice {
    pub id: String,
}

pub async fn set_default(data_dir: &Path, input: DefaultChoice) -> GitlabResult<()> {
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        if !config.accounts.iter().any(|account| account.id == input.id) {
            return Err(GitlabError::NotFound(
                "no account signed in by that id".into(),
            ));
        }
        config.default = Some(input.id);
        config::save(&data_dir, &config)
    }))
    .await
}

#[derive(Deserialize)]
pub struct ProjectOnHost {
    #[serde(flatten)]
    pub repo: RepoRef,
    /// The remote's server, so only accounts there are tried.
    #[serde(default)]
    pub host: Option<String>,
}

/// The first account, default first, on the project's server that can see it,
/// so a project on a work server opens as the work account by itself. When none
/// can, the first on that server still answers, so the project is never looked
/// for on another server.
pub async fn account_for(data_dir: &Path, input: ProjectOnHost) -> GitlabResult<Option<String>> {
    let path = input.repo.path("")?;
    let on_server: Vec<Account> = config::load(data_dir)
        .in_order()
        .into_iter()
        .filter(|account| {
            input
                .host
                .as_deref()
                .is_none_or(|host| config::same_server(host, &account.host))
        })
        .collect();
    for account in &on_server {
        let id = account.id.clone();
        let seen: GitlabResult<serde_json::Value> =
            client::as_account(Some(id.clone()), client::get(data_dir, &path, &[])).await;
        match seen {
            Ok(_) => return Ok(Some(id)),
            Err(GitlabError::Unconfigured) => return Ok(None),
            Err(_) => continue,
        }
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
    fn a_server_is_read_from_whatever_was_typed() -> GitlabResult<()> {
        assert_eq!(host_of("")?, "gitlab.com");
        assert_eq!(host_of("gitlab.acme.dev")?, "gitlab.acme.dev");
        assert_eq!(
            host_of("https://GitLab.Acme.dev/users/sign_in")?,
            "gitlab.acme.dev"
        );
        assert_eq!(host_of("git.acme.dev:8443")?, "git.acme.dev:8443");
        Ok(())
    }

    #[test]
    fn a_plain_http_or_odd_server_is_refused() {
        for typed in [
            "http://gitlab.acme.dev",
            "localhost",
            "user@gitlab.com",
            "gitlab..com",
            "a.b:99999",
        ] {
            assert!(host_of(typed).is_err(), "{typed}");
        }
    }

    #[test]
    fn only_the_api_scope_may_start_pipelines() {
        assert!(can_write(&["api".into(), "read_user".into()]));
        assert!(!can_write(&["read_api".into()]));
        assert!(can_write(&[]));
    }

    #[test]
    fn an_account_is_named_by_its_server_and_gitlabs_id() -> GitlabResult<()> {
        let user: User = serde_json::from_value(serde_json::json!({
            "id": 4211, "username": "ankit", "name": "Ankit Patidar", "avatar_url": "https://gitlab.com/uploads/a.png"
        }))?;
        let account = new_account("gitlab.com", &user)?;
        assert_eq!(account.id, "gitlab.com#4211");
        assert_eq!(account.login, "ankit");
        assert!(new_account("gitlab.com", &User::default()).is_err());
        Ok(())
    }
}
