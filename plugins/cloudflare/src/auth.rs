use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::oauth::Tokens;
use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::{SignInStatus, SignedInAs, StreamSink};

use crate::client::{self, accounts_of, text, Session};
use crate::config::{self, Account, CloudflareAccount, CloudflareConfig, SignIn};
use crate::error::{CloudflareError, CloudflareResult};
use crate::oauth;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Signed {
    pub accounts: Vec<Listed>,
    pub browser_sign_in: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_account: Option<String>,
    pub cf_accounts: Vec<CloudflareAccount>,
    pub is_default: bool,
}

pub type Status = SignInStatus<Signed>;

impl SignedInAs for Signed {
    fn signed_in_as(&self) -> Option<String> {
        let names: Vec<&str> = self
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .filter(|name| !name.is_empty())
            .collect();
        (!names.is_empty()).then(|| names.join(", "))
    }
}

pub async fn status(data_dir: &Path) -> Status {
    let browser_sign_in = oauth::APP.available();
    let config = match config::load(data_dir) {
        Ok(config) => config,
        Err(error) => {
            return Status::checked(
                false,
                Signed {
                    accounts: Vec::new(),
                    browser_sign_in,
                },
                &Err::<(), _>(error),
            );
        }
    };
    let accounts = config
        .accounts
        .iter()
        .map(|account| Listed {
            id: account.id.clone(),
            name: account.name.clone(),
            default_account: account.default_account.clone(),
            cf_accounts: account.cf_accounts.clone(),
            is_default: config.default.as_deref() == Some(account.id.as_str()),
        })
        .collect();
    let check = match Session::of(data_dir).await {
        Ok(session) => verify_session(&session).await,
        Err(error) => Err(error),
    };
    Status::checked(
        !config.accounts.is_empty(),
        Signed {
            accounts,
            browser_sign_in,
        },
        &check,
    )
}

async fn verify_session(session: &Session) -> CloudflareResult<()> {
    match session.account.sign_in {
        SignIn::Browser => {
            session
                .get("/accounts", &[("per_page", "1".into())])
                .await?;
            Ok(())
        }
        SignIn::Token => match session.get("/user/tokens/verify", &[]).await {
            Ok(_) => Ok(()),
            Err(CloudflareError::Forbidden(_) | CloudflareError::NotFound(_)) => {
                session
                    .get("/accounts", &[("per_page", "1".into())])
                    .await?;
                Ok(())
            }
            Err(error) => Err(error),
        },
    }
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
}

pub async fn sign_in(data_dir: &Path, input: TokenSignIn) -> CloudflareResult<String> {
    let entered = input.token.trim().to_string();
    if entered.is_empty() {
        return Err(CloudflareError::BadArg("no API token was given".into()));
    }
    if entered.chars().any(char::is_whitespace) {
        return Err(CloudflareError::BadArg(
            "that is not a Cloudflare API token; create one at dash.cloudflare.com/profile/api-tokens".into(),
        ));
    }
    let resolved = store::entered_secret(entered.clone()).await?;
    let account = identify(&resolved, SignIn::Token).await?;
    let id = account.id.clone();
    store::update(data_dir, move |config: &mut CloudflareConfig| {
        config::KEYCHAIN.write(&account.id, &entered)?;
        config.upsert(account);
        Ok(())
    })
    .await?;
    client::forget(Some(&id));
    Ok(id)
}

pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> CloudflareResult<()> {
    let keep = |tokens| keep_browser_sign_in(data_dir, tokens);
    oauth::APP.sign_in(&client::HTTP, sink, keep).await
}

async fn keep_browser_sign_in(data_dir: &Path, tokens: Tokens) -> CloudflareResult<()> {
    let account = identify(&tokens.access_token, SignIn::Browser).await?;
    let id = account.id.clone();
    let refresh_token = tokens.refresh_token.clone();
    store::update(data_dir, move |config: &mut CloudflareConfig| {
        config::KEYCHAIN.write(&account.id, &refresh_token)?;
        config.upsert(account);
        Ok(())
    })
    .await?;
    oauth::ACCESS.remember(&id, &tokens).await;
    client::forget(Some(&id));
    Ok(())
}

async fn identify(token: &str, sign_in: SignIn) -> CloudflareResult<Account> {
    if sign_in == SignIn::Token {
        match client::request_json(
            "sign-in",
            token,
            Method::GET,
            "/user/tokens/verify",
            &[],
            None,
        )
        .await
        {
            Ok(verified) => {
                let status = text(&verified, "/result/status");
                if !status.is_empty() && !status.eq_ignore_ascii_case("active") {
                    return Err(CloudflareError::Auth(format!(
                        "token status is '{status}', expected active"
                    )));
                }
            }
            Err(CloudflareError::Forbidden(_) | CloudflareError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }

    let accounts_body = client::request_json(
        "sign-in",
        token,
        Method::GET,
        "/accounts",
        &[("per_page", "50".into())],
        None,
    )
    .await;
    let user_body = client::request_json("sign-in", token, Method::GET, "/user", &[], None).await;

    let cf_accounts = match accounts_body {
        Ok(body) => accounts_of(&body),
        Err(CloudflareError::Forbidden(_)) => Vec::new(),
        Err(error) => {
            if user_body.is_err() {
                return Err(error);
            }
            Vec::new()
        }
    };

    let (id, name) = match user_body {
        Ok(user) => user_identity(&user, &cf_accounts)?,
        Err(_) => account_identity(&cf_accounts)?,
    };

    let default_account = (cf_accounts.len() == 1)
        .then(|| cf_accounts.first().map(|account| account.id.clone()))
        .flatten();

    Ok(Account {
        id,
        name,
        default_account,
        cf_accounts,
        sign_in,
    })
}

fn user_identity(
    user: &Value,
    cf_accounts: &[CloudflareAccount],
) -> CloudflareResult<(String, String)> {
    let id = text(user, "/result/id");
    if id.is_empty() {
        return account_identity(cf_accounts);
    }
    let first = text(user, "/result/first_name");
    let last = text(user, "/result/last_name");
    let full = format!("{first} {last}").trim().to_owned();
    let username = text(user, "/result/username");
    let account_name = cf_accounts
        .first()
        .map(|account| account.name.clone())
        .unwrap_or_default();
    let name = [full, username, account_name]
        .into_iter()
        .find(|candidate| !candidate.is_empty())
        .unwrap_or_else(|| "Cloudflare".to_owned());
    Ok((id, name))
}

fn account_identity(cf_accounts: &[CloudflareAccount]) -> CloudflareResult<(String, String)> {
    let Some(first) = cf_accounts.first() else {
        return Ok(("cloudflare".to_owned(), "Cloudflare".to_owned()));
    };
    let name = if first.name.is_empty() {
        "Cloudflare".to_owned()
    } else {
        first.name.clone()
    };
    Ok((first.id.clone(), name))
}

#[derive(Deserialize, Default)]
pub struct AccountRef {
    #[serde(default)]
    pub account: Option<String>,
}

pub async fn sign_out(data_dir: &Path, input: AccountRef) -> CloudflareResult<()> {
    let removed = store::sign_out::<CloudflareConfig, _, _>(data_dir, input.account, |account| {
        let refresh_token = match account.sign_in {
            SignIn::Browser => config::KEYCHAIN.read(&account.id)?,
            SignIn::Token => None,
        };
        config::KEYCHAIN.delete(&account.id)?;
        Ok(refresh_token)
    })
    .await?;
    if let Some((account, refresh_token)) = removed {
        client::forget(Some(&account.id));
        oauth::ACCESS.forget(Some(&account.id)).await;
        if let Some(refresh_token) = refresh_token {
            oauth::revoke(&refresh_token).await;
        }
    }
    Ok(())
}
