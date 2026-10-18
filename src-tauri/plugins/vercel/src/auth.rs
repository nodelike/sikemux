// Signing in and out, by installing the Sikemux integration from the browser or
// with an access token, choosing the team calls are for, and saying who is signed in.

use std::path::Path;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sikemux_plugin_api::StreamSink;

use crate::client::{self, at, text, Session};
use crate::config::{self, Account, Install, SignIn, VercelConfig};
use crate::error::{VercelError, VercelResult};
use crate::oauth;

const BROWSER_WAIT: Duration = Duration::from_secs(300);

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

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub slug: String,
    pub name: String,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
    pub account: Option<Account>,
    pub teams: Vec<Team>,
    /// The team calls use when they name none, by id; none for the account's own projects.
    pub team: Option<String>,
    /// Whether the account has projects of its own beside its teams. Newer accounts
    /// keep theirs in a default team instead, which is then the team chosen.
    pub personal: bool,
    /// Whether this build can sign in through the browser.
    pub browser_sign_in: bool,
}

/// Who is signed in, whether its token still works, and the teams it can see.
pub async fn status(data_dir: &Path) -> Status {
    let config = config::load(data_dir);
    let browser = config
        .account
        .as_ref()
        .is_some_and(|account| account.sign_in == SignIn::Browser);
    let base = |ok: bool,
                auth_failed: bool,
                message: Option<String>,
                teams: Vec<Team>,
                default_team: Option<String>| Status {
        configured: config.account.is_some(),
        ok,
        auth_failed,
        message,
        account: config.account.clone(),
        personal: default_team.is_none(),
        team: config.team.clone().or(default_team),
        teams,
        browser_sign_in: oauth::available(),
    };
    if browser {
        return installs_status(&config, data_dir, base).await;
    }
    let session = match Session::of(data_dir, Some("")).await {
        Ok(session) => session,
        Err(VercelError::Unconfigured) => return base(false, false, None, Vec::new(), None),
        Err(error) => return base(false, false, Some(error.to_string()), Vec::new(), None),
    };
    let answered = futures::future::try_join(
        session.get("/v2/user", &[]),
        session.get("/v2/teams", &[("limit", "100".into())]),
    )
    .await;
    match answered {
        Ok((user, teams)) => {
            let teams = teams_of(&teams);
            let default_team = Some(text(&user, "/user/defaultTeamId"))
                .filter(|id| teams.iter().any(|team| &team.id == id));
            base(true, false, None, teams, default_team)
        }
        Err(error) => failed(error, base),
    }
}

fn failed(
    error: VercelError,
    base: impl Fn(bool, bool, Option<String>, Vec<Team>, Option<String>) -> Status,
) -> Status {
    match error {
        VercelError::Unconfigured => base(false, false, None, Vec::new(), None),
        VercelError::Auth(_) => base(false, true, Some(error.to_string()), Vec::new(), None),
        VercelError::RateLimited => base(true, false, Some(error.to_string()), Vec::new(), None),
        error => base(false, false, Some(error.to_string()), Vec::new(), None),
    }
}

/// A browser sign-in sees only the teams the integration is installed on.
async fn installs_status(
    config: &VercelConfig,
    data_dir: &Path,
    base: impl Fn(bool, bool, Option<String>, Vec<Team>, Option<String>) -> Status,
) -> Status {
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
    let answered = match Session::of(data_dir, Some(chosen.as_deref().unwrap_or(""))).await {
        Ok(session) => session.get("/v2/user", &[]).await,
        Err(error) => Err(error),
    };
    let status = match answered {
        Ok(_) => base(true, false, None, teams, chosen.clone()),
        Err(error) => failed(error, base),
    };
    Status {
        personal,
        team: chosen,
        ..status
    }
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
    let data_dir = data_dir.to_path_buf();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let removed = replace_account(&mut config, &account)?;
        config::keychain_write(&account, None, &token)?;
        config.account = Some(account);
        config::save(&data_dir, &config)?;
        Ok(removed)
    }))
    .await?;
    finish(removed).await;
    Ok(())
}

/// Opens Vercel's page for installing the Sikemux integration, through the URL sent
/// down the stream. The person picks a team there; signing in again adds another.
/// Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> VercelResult<()> {
    let callback = oauth::listen().await?;
    let state = oauth::new_state()?;
    sink.send(json!({ "url": oauth::install_url(&state) }))
        .map_err(|_| VercelError::Auth("the sign-in was closed".into()))?;
    let code = tokio::time::timeout(BROWSER_WAIT, oauth::code(&callback, &state))
        .await
        .map_err(|_| VercelError::Auth("nobody finished signing in".into()))??;
    drop(callback);
    let installed = oauth::exchange(&code).await?;
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
    let data_dir = data_dir.to_path_buf();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let removed = replace_account(&mut config, &account)?;
        config::keychain_write(&account, install.team.as_deref(), &token)?;
        config.team = install.team.clone();
        config.upsert(install);
        config.account = Some(account);
        config::save(&data_dir, &config)?;
        Ok(removed)
    }))
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
        config::keychain_delete(&previous, None)?;
        for install in config.installs.drain(..) {
            let token = config::keychain_read(&previous, install.team.as_deref())?;
            config::keychain_delete(&previous, install.team.as_deref())?;
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
    let data_dir = data_dir.to_path_buf();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let removed = clear_account(&mut config)?;
        config::save(&data_dir, &config)?;
        Ok(removed)
    }))
    .await?;
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
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        if config.account.is_none() {
            return Err(VercelError::Unconfigured);
        }
        config.team = team;
        config::save(&data_dir, &config)
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

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
