// Signing accounts in and out, and saying who the app is talking to Bitbucket
// as. Signing in through the browser is the usual way; a pasted token is there
// for workspaces that do not allow outside apps. Signing in to a second
// account adds it beside the first rather than replacing it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::account::{as_account, chosen};
use sikemux_plugin_api::http;
use sikemux_plugin_api::oauth::Tokens;
use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::{SignInStatus, StreamSink};

use crate::client::{self, Credential, Session};
use crate::config::{self, Account, BitbucketConfig, Method};
use crate::error::{BitbucketError, BitbucketResult};
use crate::oauth;
use crate::repo::{RepoRef, User};

/// Long enough to sign in to Atlassian from scratch, two-step login included.
const PIPELINE_WRITE: &str = "pipeline:write";

async fn identify(credential: &Credential) -> BitbucketResult<User> {
    let request = credential.apply(client::HTTP.client()?.get(format!("{}/user", client::API)));
    let response = client::HTTP.send(request).await?;
    let status = response.status();
    let bytes = http::read_body(response, 1024 * 1024).await?;
    if !status.is_success() {
        return Err(client::classify(status, &bytes));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Signed {
    /// Which account this is about.
    pub account: Option<String>,
    pub method: Option<Method>,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// Whether this sign-in may start and stop pipelines, not only read them.
    pub can_write_ci: bool,
    /// A build without the OAuth secret can only take a pasted token.
    pub browser_sign_in: bool,
}

pub type Status = SignInStatus<Signed>;

/// An OAuth grant says what it allows; a pasted token is taken at its word,
/// and Bitbucket says no later if it may not.
fn can_write(session: &Session) -> bool {
    session.account.method == Method::Token
        || session.scopes.is_empty()
        || session.scopes.iter().any(|scope| scope == PIPELINE_WRITE)
}

/// The account the call named, or the default one.
pub async fn status(data_dir: &Path) -> Status {
    let config = match config::load(data_dir) {
        Ok(config) => config,
        Err(error) => {
            let signed = Signed {
                browser_sign_in: oauth::APP.available(),
                ..Signed::default()
            };
            return Status::checked(false, signed, &Err::<(), _>(error));
        }
    };
    let account = config.account(chosen().as_deref()).cloned();
    let mut signed = Signed {
        account: account.as_ref().map(|account| account.id.clone()),
        method: account.as_ref().map(|account| account.method),
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
        browser_sign_in: oauth::APP.available(),
    };
    let check = match Session::current(data_dir).await {
        Ok(session) => {
            let identified = identify(&session.credential).await;
            match &identified {
                Ok(user) => {
                    signed.login = user
                        .login()
                        .unwrap_or_else(|| session.account.login.clone());
                    signed.display_name = user.display_name.clone();
                    signed.avatar_url = user.avatar();
                    signed.can_write_ci = can_write(&session);
                }
                Err(BitbucketError::RateLimited { .. }) => {
                    signed.can_write_ci = can_write(&session);
                }
                Err(_) => {}
            }
            identified
        }
        Err(error) => Err(error),
    };
    Status::checked(account.is_some(), signed, &check)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_default: bool,
}

/// Every account signed in, the default first, as they were when they signed in.
pub fn accounts(data_dir: &Path) -> BitbucketResult<Vec<Listed>> {
    let config = config::load(data_dir)?;
    Ok(config
        .in_order()
        .into_iter()
        .map(|account| Listed {
            is_default: config.default.as_deref() == Some(account.id.as_str()),
            id: account.id,
            login: account.login,
            display_name: account.display_name,
            avatar_url: account.avatar_url,
        })
        .collect())
}

fn new_account(
    user: Option<&User>,
    method: Method,
    email: Option<String>,
) -> BitbucketResult<Account> {
    let id = match user.and_then(|user| user.uuid.as_deref()) {
        Some(uuid) => uuid.trim_matches(['{', '}']).to_string(),
        None => format!(
            "token-{}",
            sikemux_loopback::new_state().map_err(|error| BitbucketError::Auth(error.0))?
        ),
    };
    Ok(Account {
        id,
        login: user
            .and_then(User::login)
            .unwrap_or_else(|| "Access token".into()),
        display_name: user.and_then(|user| user.display_name.clone()),
        avatar_url: user.and_then(User::avatar),
        method,
        email,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSignIn {
    pub token: String,
    /// Given with an Atlassian API token; left out for a repository or workspace access token.
    pub email: Option<String>,
}

/// Adds the account the token belongs to, and says which one it is.
pub async fn sign_in_with_token(data_dir: &Path, input: TokenSignIn) -> BitbucketResult<String> {
    let token = input.token.trim().to_string();
    let email = input
        .email
        .map(|email| email.trim().to_string())
        .filter(|email| !email.is_empty());
    if token.is_empty() {
        return Err(BitbucketError::BadArg("no token was given".into()));
    }
    let credential = match &email {
        Some(email) => Credential::Basic {
            email: email.clone(),
            token: token.clone(),
        },
        None => Credential::Bearer(token.clone()),
    };
    let user = match identify(&credential).await {
        Ok(user) => Some(user),
        // An access token belongs to a repository or workspace rather than a
        // person, and may not be allowed to say who it is.
        Err(BitbucketError::Forbidden(_)) if email.is_none() => None,
        Err(error) => return Err(error),
    };
    let account = new_account(user.as_ref(), Method::Token, email)?;
    save_sign_in(data_dir, account, token).await
}

async fn save_sign_in(
    data_dir: &Path,
    account: Account,
    secret: String,
) -> BitbucketResult<String> {
    let id = account.id.clone();
    store::update(data_dir, move |config: &mut BitbucketConfig| {
        let replaced = config
            .accounts
            .iter()
            .find(|kept| kept.id == account.id && kept.method != account.method)
            .cloned();
        config::keychain_write(&account, &secret)?;
        if let Some(replaced) = replaced {
            config::keychain_delete(&replaced)?;
        }
        config.upsert(account);
        Ok(())
    })
    .await?;
    client::forget(Some(&id)).await;
    Ok(id)
}

/// Opens the sign-in page through the window, which is handed the address as
/// the stream's first item, then waits for the browser to come back. Closing
/// the stream stops the wait and frees the port. Says which account signed in.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> BitbucketResult<String> {
    let tokens: Tokens = oauth::APP.sign_in(&client::HTTP, sink).await?;
    let user = identify(&Credential::Bearer(tokens.access_token.clone())).await?;
    let account = new_account(Some(&user), Method::Oauth, None)?;
    let id = save_sign_in(data_dir, account, tokens.refresh_token.clone()).await?;
    oauth::ACCESS.remember(&id, &tokens).await;
    Ok(id)
}

/// Signs out the account the call named, or the default one.
pub async fn sign_out(data_dir: &Path) -> BitbucketResult<()> {
    let chosen = chosen();
    let removed = store::update(data_dir, move |config: &mut BitbucketConfig| {
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
        Ok(removed)
    })
    .await?;
    if let Some(account) = removed {
        client::forget(Some(&account.id)).await;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct DefaultChoice {
    pub id: String,
}

pub async fn set_default(data_dir: &Path, input: DefaultChoice) -> BitbucketResult<()> {
    store::update(data_dir, move |config: &mut BitbucketConfig| {
        config.set_default(&input.id)
    })
    .await
}

/// The first account, default first, that can see the repository, so a
/// project on a work workspace opens as the work account by itself.
pub async fn account_for(data_dir: &Path, repo: RepoRef) -> BitbucketResult<Option<String>> {
    let path = repo.path("")?;
    for account in config::load(data_dir)?.in_order() {
        let id = account.id.clone();
        let seen: BitbucketResult<serde_json::Value> =
            as_account(Some(id.clone()), client::get(data_dir, &path, &[])).await;
        match seen {
            Ok(_) => return Ok(Some(id)),
            Err(BitbucketError::Unconfigured) => return Ok(None),
            Err(_) => continue,
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session(method: Method, scopes: &[&str]) -> Session {
        Session {
            account: Account {
                id: "a".into(),
                login: "a".into(),
                display_name: None,
                avatar_url: None,
                method,
                email: None,
            },
            credential: Credential::Bearer(String::new()),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        }
    }

    #[test]
    fn only_a_grant_with_pipeline_write_may_start_pipelines() {
        assert!(can_write(&session(
            Method::Oauth,
            &["pipeline:write", "pullrequest"]
        )));
        assert!(!can_write(&session(
            Method::Oauth,
            &["pipeline", "pullrequest"]
        )));
        assert!(can_write(&session(Method::Token, &[])));
    }

    #[test]
    fn an_account_is_named_by_bitbuckets_id_without_its_braces() -> BitbucketResult<()> {
        let user: User = serde_json::from_value(json!({
            "uuid": "{19fa18a7-991d-4d69-9e7c-c837ff810a72}",
            "nickname": "Krisnasw",
            "display_name": "Krisnasw",
            "links": { "avatar": { "href": "https://a/k.png" } }
        }))?;
        let account = new_account(Some(&user), Method::Oauth, None)?;
        assert_eq!(account.id, "19fa18a7-991d-4d69-9e7c-c837ff810a72");
        assert_eq!(account.login, "Krisnasw");
        assert_eq!(account.avatar_url.as_deref(), Some("https://a/k.png"));
        Ok(())
    }

    #[test]
    fn an_access_token_that_cannot_say_who_it_is_gets_an_id_of_its_own() -> BitbucketResult<()> {
        let first = new_account(None, Method::Token, None)?;
        let second = new_account(None, Method::Token, None)?;
        assert!(first.id.starts_with("token-"));
        assert_ne!(first.id, second.id);
        assert_eq!(first.login, "Access token");
        Ok(())
    }
}
