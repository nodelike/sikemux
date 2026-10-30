// Signing in and out, by installing the Sikemux integration from the browser or
// with an access token, choosing the team calls are for, and saying who is signed in.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

use sikemux_plugin_api::store;
use sikemux_plugin_api::{SignInStatus, StreamSink};
use sikemux_plugin_vercel_views::{Signed, Team};

use crate::client::{self, at, text, Session};
use crate::config::{self, Account, Install, SignIn, VercelConfig};
use crate::error::{VercelError, VercelResult};
use crate::oauth;

/// Who a token belongs to, as `/v2/user` says.
pub fn account_of(data: &Value) -> VercelResult<Account> {
    let user = at(data, "/user");
    let id = text(user, "/id");
    if id.is_empty() {
        return Err(VercelError::Response(
            "Vercel did not say whose token this is".into(),
        ));
    }
    Ok(Account {
        id,
        username: text(user, "/username"),
        name: text(user, "/name"),
        email: text(user, "/email"),
        sign_in: SignIn::Token,
    })
}

pub fn teams_of(data: &Value) -> Vec<Team> {
    at(data, "/teams")
        .as_array()
        .into_iter()
        .flatten()
        .map(|team| Team {
            id: text(team, "/id"),
            slug: text(team, "/slug"),
            name: text(team, "/name"),
        })
        .filter(|team| !team.id.is_empty())
        .collect()
}

pub type Status = SignInStatus<Signed>;

/// Who is signed in, whether its token still works, and the teams it can see.
pub async fn status(data_dir: &Path) -> Status {
    let config = match config::load(data_dir) {
        Ok(config) => config,
        Err(error) => {
            let signed = Signed {
                personal: true,
                browser_sign_in: oauth::APP.available(),
                ..Signed::default()
            };
            return Status::checked(false, signed, &Err::<(), _>(error));
        }
    };
    let mut signed = Signed {
        account: config.account.clone(),
        teams: Vec::new(),
        team: config.team.clone(),
        personal: true,
        browser_sign_in: oauth::APP.available(),
    };
    let browser = config
        .account
        .as_ref()
        .is_some_and(|account| account.sign_in == SignIn::Browser);
    let check = if browser {
        installs_status(&config, data_dir, &mut signed).await
    } else {
        token_status(data_dir, &mut signed).await
    };
    Status::checked(config.account.is_some(), signed, &check)
}

async fn token_status(data_dir: &Path, signed: &mut Signed) -> VercelResult<()> {
    let session = Session::of(data_dir, Some("")).await?;
    let (user, teams) = futures::future::try_join(
        session.get("/v2/user", &[]),
        session.get("/v2/teams", &[("limit", "100".into())]),
    )
    .await?;
    let teams = teams_of(&teams);
    let default_team = Some(text(&user, "/user/defaultTeamId"))
        .filter(|id| teams.iter().any(|team| &team.id == id));
    signed.personal = default_team.is_none();
    signed.team = signed.team.take().or(default_team);
    signed.teams = teams;
    Ok(())
}

/// A browser sign-in sees only the teams the integration is installed on.
async fn installs_status(
    config: &VercelConfig,
    data_dir: &Path,
    signed: &mut Signed,
) -> VercelResult<()> {
    let teams: Vec<Team> = config
        .installs
        .iter()
        .filter_map(|install| {
            Some(Team {
                id: install.team.clone()?,
                slug: install.slug.clone(),
                name: install.name.clone(),
            })
        })
        .collect();
    let personal = config.install(None).is_some();
    let chosen = config
        .team
        .clone()
        .filter(|team| config.install(Some(team)).is_some())
        .or_else(|| {
            (!personal)
                .then(|| teams.first().map(|team| team.id.clone()))
                .flatten()
        });
    signed.personal = personal;
    signed.team = chosen.clone();
    Session::of(data_dir, Some(chosen.as_deref().unwrap_or("")))
        .await?
        .get("/v2/user", &[])
        .await?;
    signed.teams = teams;
    Ok(())
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
}

/// Signs in with an access token, replacing any account signed in before.
pub async fn sign_in(data_dir: &Path, input: TokenSignIn) -> VercelResult<()> {
    let token = input.token.trim().to_string();
    if token.is_empty() {
        return Err(VercelError::BadArg("no access token was given".into()));
    }
    if token.chars().any(char::is_whitespace) {
        return Err(VercelError::BadArg(
            "that is not a Vercel access token; make one at vercel.com/account/tokens".into(),
        ));
    }
    let account = account_of(&client::request(&token, Method::GET, "/v2/user", &[], None).await?)?;
    let removed = store::update(data_dir, move |config: &mut VercelConfig| {
        let removed = replace_account(config, &account)?;
        config::KEYCHAIN.write(&config::entry(&account, None), &token)?;
        config.account = Some(account);
        Ok(removed)
    })
    .await?;
    finish(removed).await;
    Ok(())
}

/// Opens Vercel's page for installing the Sikemux integration, through the URL sent
/// down the stream. The person picks a team there; signing in again adds another.
/// Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> VercelResult<()> {
    let installed: oauth::Installed = oauth::APP.sign_in(&client::HTTP, sink).await?;
    if !is_id(&installed.installation_id) {
        return Err(VercelError::Response(
            "Vercel named the installation strangely".into(),
        ));
    }
    let team = installed.team_id.filter(|team| !team.is_empty());
    let token = installed.access_token;
    let in_team: Vec<(&str, String)> = team
        .as_deref()
        .map(client::team_param)
        .into_iter()
        .collect();
    let account = Account {
        sign_in: SignIn::Browser,
        ..account_of(&client::request(&token, Method::GET, "/v2/user", &in_team, None).await?)?
    };
    let install = match &team {
        Some(id) => {
            let about = client::request(
                &token,
                Method::GET,
                &format!("/v2/teams/{id}"),
                &in_team,
                None,
            )
            .await?;
            Install {
                team: Some(id.clone()),
                slug: text(&about, "/slug"),
                name: text(&about, "/name"),
                configuration: installed.installation_id,
            }
        }
        None => Install {
            team: None,
            slug: account.username.clone(),
            name: account.name.clone(),
            configuration: installed.installation_id,
        },
    };
    let removed = store::update(data_dir, move |config: &mut VercelConfig| {
        let removed = replace_account(config, &account)?;
        config::KEYCHAIN.write(&config::entry(&account, install.team.as_deref()), &token)?;
        config.team = install.team.clone();
        config.upsert(install);
        config.account = Some(account);
        Ok(removed)
    })
    .await?;
    finish(removed).await;
    Ok(())
}

fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// An install taken out of the config, to be removed from Vercel too.
struct Removed {
    token: Option<String>,
    team: Option<String>,
    configuration: String,
}

/// Clears the account signed in before when `account` is someone else, or signs in
/// the other way, so its tokens are not left behind.
fn replace_account(config: &mut VercelConfig, account: &Account) -> VercelResult<Vec<Removed>> {
    let same = config
        .account
        .as_ref()
        .is_some_and(|previous| previous.id == account.id && previous.sign_in == account.sign_in);
    if same {
        return Ok(Vec::new());
    }
    clear_account(config)
}

fn clear_account(config: &mut VercelConfig) -> VercelResult<Vec<Removed>> {
    let mut removed = Vec::new();
    if let Some(previous) = config.account.take() {
        config::KEYCHAIN.delete(&config::entry(&previous, None))?;
        for install in config.installs.drain(..) {
            let entry = config::entry(&previous, install.team.as_deref());
            let token = config::KEYCHAIN.read(&entry)?;
            config::KEYCHAIN.delete(&entry)?;
            removed.push(Removed {
                token,
                team: install.team,
                configuration: install.configuration,
            });
        }
    }
    config.installs.clear();
    config.team = None;
    Ok(removed)
}

async fn finish(removed: Vec<Removed>) {
    client::forget();
    for install in removed {
        if let Some(token) = install.token {
            oauth::uninstall(&token, install.team.as_deref(), &install.configuration).await;
        }
    }
}

/// Signs out, and removes the integration from every team it was installed on.
pub async fn sign_out(data_dir: &Path) -> VercelResult<()> {
    let removed = store::update(data_dir, clear_account).await?;
    finish(removed).await;
    Ok(())
}

#[derive(Deserialize)]
pub struct TeamChoice {
    /// A team's id, or empty for the account's own projects.
    #[serde(default)]
    pub team: Option<String>,
}

/// Makes the team named the one calls use when they name none.
pub async fn set_team(data_dir: &Path, input: TeamChoice) -> VercelResult<()> {
    let team = input
        .team
        .map(|team| team.trim().to_string())
        .filter(|team| !team.is_empty());
    store::update(data_dir, move |config: &mut VercelConfig| {
        if config.account.is_none() {
            return Err(VercelError::Unconfigured);
        }
        config.team = team;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_belongs_to_its_user_and_sees_their_teams() {
        let user = json!({ "user": { "id": "u1", "username": "ankit", "name": "Ankit", "email": "a@example.com" } });
        assert_eq!(
            account_of(&user).expect("reads"),
            Account {
                id: "u1".into(),
                username: "ankit".into(),
                name: "Ankit".into(),
                email: "a@example.com".into(),
                sign_in: SignIn::Token,
            }
        );
        assert!(account_of(&json!({ "user": {} })).is_err());
        let teams = json!({ "teams": [{ "id": "team_1", "slug": "acme", "name": "Acme" }, { "slug": "no-id" }] });
        assert_eq!(
            teams_of(&teams),
            [Team {
                id: "team_1".into(),
                slug: "acme".into(),
                name: "Acme".into()
            }]
        );
    }
}
