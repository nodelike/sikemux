// Signing accounts in and out, and saying who the app is talking to GitHub as.
// A token typed here is saved in the Keychain; one already in the environment
// or held by the `gh` CLI is used where it is and never copied. Signing in to
// a second account adds it beside the first rather than replacing it.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::client::{self, Session};
use crate::config::{self, Account, TokenSource};
use crate::error::{GithubError, GithubResult};
use crate::workflows::RepoRef;

#[derive(Deserialize)]
struct Viewer {
    login: String,
}

/// Who a token belongs to, and what it is allowed to do. Fine-grained tokens
/// list no scopes at all, which is why an empty list is not a problem.
pub struct Identity {
    pub login: String,
    pub scopes: Vec<String>,
}

pub async fn identify(session: &Session) -> GithubResult<Identity> {
    let (status, headers, bytes) = client::send(session, Method::GET, "/user", &[], None).await?;
    if !status.is_success() {
        return Err(client::classify(status, &headers, &bytes));
    }
    let viewer: Viewer = serde_json::from_slice(&bytes)?;
    let scopes = headers
        .get("x-oauth-scopes")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(Identity {
        login: viewer.login,
        scopes,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    /// Which account this is about; absent while a token the shell or `gh`
    /// holds is used before anybody has signed in.
    pub account: Option<String>,
    pub host: String,
    pub login: String,
    pub token_source: Option<TokenSource>,
    pub token_variable: Option<&'static str>,
    pub scopes: Vec<String>,
    /// Whether the token may read and start workflow runs, not only read code.
    pub can_write_workflows: bool,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
}

/// A classic token needs the `workflow` scope to re-run or dispatch. A
/// fine-grained one lists no scopes, so it is taken at its word and the API
/// says no later if it may not.
fn can_write(scopes: &[String]) -> bool {
    scopes.is_empty() || scopes.iter().any(|scope| scope == "workflow")
}

fn variable_for(source: Option<TokenSource>, host: &str) -> Option<&'static str> {
    (source == Some(TokenSource::Environment))
        .then(|| config::env_variable(host))
        .flatten()
}

/// The account the call named, or the default one.
pub async fn status(data_dir: &Path) -> Status {
    let config = config::load(data_dir);
    let chosen = client::chosen();
    let saved = config.account(chosen.as_deref()).cloned();
    let host = saved
        .as_ref()
        .map_or_else(|| config.host_hint(), |account| account.host.clone());
    let base = |ok: bool,
                auth_failed: bool,
                login: String,
                source: Option<TokenSource>,
                scopes: Vec<String>,
                message: Option<String>| Status {
        configured: source.is_some(),
        account: saved.as_ref().map(|account| account.id.clone()),
        host: host.clone(),
        can_write_workflows: ok && can_write(&scopes),
        token_variable: variable_for(source, &host),
        login,
        token_source: source,
        scopes,
        ok,
        auth_failed,
        message,
    };
    let saved_login = saved
        .as_ref()
        .map(|account| account.login.clone())
        .unwrap_or_default();
    let saved_source = saved.as_ref().map(|account| account.source);
    let session = match Session::current(data_dir).await {
        Ok(session) => session,
        // Not signed in, but a token the shell or `gh` holds can be offered.
        Err(GithubError::Unconfigured) => {
            let offer_host = host.clone();
            let waiting =
                config::blocking(Box::new(move || Ok(config::offered_token(&offer_host))))
                    .await
                    .ok()
                    .flatten()
                    .map(|(_, source)| source);
            let mut status = base(false, false, String::new(), waiting, Vec::new(), None);
            status.configured = false;
            return status;
        }
        // A locked or unanswering Keychain is not the same as being signed out.
        Err(error) => {
            return base(
                false,
                false,
                saved_login,
                saved_source,
                Vec::new(),
                Some(error.to_string()),
            )
        }
    };
    let source = Some(session.account.source);
    match identify(&session).await {
        Ok(identity) => base(true, false, identity.login, source, identity.scopes, None),
        // Still signed in; GitHub is only asking to wait.
        Err(error @ GithubError::RateLimited { .. }) => base(
            true,
            false,
            session.account.login.clone(),
            source,
            Vec::new(),
            Some(error.to_string()),
        ),
        Err(error) => {
            let auth_failed = matches!(
                error,
                GithubError::Auth(_) | GithubError::Unconfigured | GithubError::Forbidden(_)
            );
            base(
                false,
                auth_failed,
                session.account.login.clone(),
                source,
                Vec::new(),
                Some(error.to_string()),
            )
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub host: String,
    pub login: String,
    pub source: TokenSource,
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
            source: account.source,
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub host: Option<String>,
    /// Left out when the token already in the environment, or held by `gh`, is the one to use.
    pub token: Option<String>,
}

/// Adds the account the token belongs to, and says which one it is.
pub async fn sign_in(data_dir: &Path, input: SignIn) -> GithubResult<String> {
    let host = match input.host.as_deref() {
        Some(host) => config::validate_host(host)?,
        None => config::load(data_dir).host_hint(),
    };
    let token = input.token.map(|token| token.trim().to_string());
    let (token, source) = match token.filter(|token| !token.is_empty()) {
        Some(token) => (token, TokenSource::Keychain),
        None => {
            let host = host.clone();
            config::blocking(Box::new(move || Ok(config::offered_token(&host))))
                .await?
                .ok_or_else(|| {
                    GithubError::Auth(
                        "no token was given, and none is in the environment or the gh CLI".into(),
                    )
                })?
        }
    };
    let owns_token = source == TokenSource::Keychain;
    let probe = Session {
        account: Account {
            id: config::account_id(&host, ""),
            host: host.clone(),
            login: String::new(),
            source,
            owns_token,
        },
        token: token.clone(),
    };
    let identity = identify(&probe).await?;
    let account = Account {
        id: config::account_id(&host, &identity.login),
        host,
        login: identity.login,
        source,
        owns_token,
    };
    let id = account.id.clone();
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let leaves_a_token_behind = config
            .accounts
            .iter()
            .any(|kept| kept.id == account.id && kept.owns_token && !account.owns_token);
        if account.owns_token {
            config::keychain_write(&account.id, &token)?;
        }
        if leaves_a_token_behind {
            config::keychain_delete(&account.id)?;
        }
        config.upsert(account);
        config::save(&data_dir, &config)
    }))
    .await?;
    config::forget_token(Some(&id));
    Ok(id)
}

/// Signs out the account the call named, or the default one. Only a token
/// Sikemux saved is deleted; one the shell or `gh` provides is left where it is.
pub async fn sign_out(data_dir: &Path) -> GithubResult<()> {
    let data_dir = data_dir.to_path_buf();
    let chosen = client::chosen();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let Some(id) = config
            .account(chosen.as_deref())
            .map(|account| account.id.clone())
        else {
            config.signed_out = true;
            config::save(&data_dir, &config)?;
            return Ok(None);
        };
        let removed = config.remove(&id);
        if let Some(account) = removed.as_ref().filter(|account| account.owns_token) {
            config::keychain_delete(&account.id)?;
        }
        config::save(&data_dir, &config)?;
        Ok(removed)
    }))
    .await?;
    config::forget_token(removed.as_ref().map(|account| account.id.as_str()));
    Ok(())
}

#[derive(Deserialize)]
pub struct DefaultChoice {
    pub id: String,
}

pub async fn set_default(data_dir: &Path, input: DefaultChoice) -> GithubResult<()> {
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        if !config.accounts.iter().any(|account| account.id == input.id) {
            return Err(GithubError::NotFound(
                "no account signed in by that id".into(),
            ));
        }
        config.default = Some(input.id);
        config::save(&data_dir, &config)
    }))
    .await
}

/// The first account, default first, that can see the repository, so a
/// project owned by a work organisation opens as the work account by itself.
pub async fn account_for(data_dir: &Path, repo: RepoRef) -> GithubResult<Option<String>> {
    let path = repo.path("")?;
    for account in config::load(data_dir).in_order() {
        let id = account.id.clone();
        let seen: GithubResult<serde_json::Value> =
            client::as_account(Some(id.clone()), client::get(data_dir, &path, &[])).await;
        match seen {
            Ok(_) => return Ok(Some(id)),
            Err(GithubError::Unconfigured) => return Ok(None),
            Err(_) => continue,
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_classic_token_needs_the_workflow_scope_and_a_fine_grained_one_lists_none() {
        assert!(can_write(&[]));
        assert!(can_write(&["repo".into(), "workflow".into()]));
        assert!(!can_write(&["repo".into()]));
        assert!(!can_write(&["read:org".into()]));
    }

    #[test]
    fn only_a_token_from_the_shell_names_its_variable() {
        assert_eq!(
            variable_for(Some(TokenSource::Keychain), "github.com"),
            None
        );
        assert_eq!(variable_for(None, "github.com"), None);
    }
}
