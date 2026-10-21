// The Vercel account signed in here. Its access token, or the token of each team the
// Sikemux integration is installed on, lives in the Keychain; the file beside it only
// says whose they are and which team calls use when they name none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store;

use crate::error::VercelResult;

const KEY_SERVICE: &str = keychain_service!("sikemux-vercel-token");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Vercel's user id.
    pub id: String,
    pub username: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    #[default]
    Token,
    /// The Sikemux integration, installed from the browser on one team at a time.
    Browser,
}

/// Where the Sikemux integration is installed, each with a token of its own.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Install {
    /// The team's id; none for the account's own projects.
    pub team: Option<String>,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub name: String,
    /// Vercel's id for the installation, `icfg_…`.
    pub configuration: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VercelConfig {
    #[serde(default)]
    pub account: Option<Account>,
    /// The team calls use when they name none, by id; none means the account's own projects.
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub installs: Vec<Install>,
}

impl VercelConfig {
    /// The install for the team named by id or slug, or with none named, the personal one.
    pub fn install(&self, team: Option<&str>) -> Option<&Install> {
        self.installs.iter().find(|install| match team {
            Some(team) => install.team.as_deref() == Some(team) || install.slug == team,
            None => install.team.is_none(),
        })
    }

    /// Adds the install, or replaces the one on the same team.
    pub fn upsert(&mut self, install: Install) {
        match self
            .installs
            .iter_mut()
            .find(|kept| kept.team == install.team)
        {
            Some(kept) => *kept = install,
            None => self.installs.push(install),
        }
    }
}

pub fn load(data_dir: &Path) -> VercelResult<VercelConfig> {
    Ok(store::load(data_dir)?)
}

/// A token pasted in, or a personal install, is kept under the account; a team's install under the team too.
pub fn entry(account: &Account, team: Option<&str>) -> String {
    match team {
        Some(team) => format!("{}:{team}", account.id),
        None => account.id.clone(),
    }
}

pub fn keychain_read(account: &Account, team: Option<&str>) -> VercelResult<Option<String>> {
    Ok(sikemux_keychain::read(KEY_SERVICE, &entry(account, team))?)
}

pub fn keychain_write(account: &Account, team: Option<&str>, token: &str) -> VercelResult<()> {
    Ok(sikemux_keychain::write(
        KEY_SERVICE,
        &entry(account, team),
        token,
    )?)
}

pub fn keychain_delete(account: &Account, team: Option<&str>) -> VercelResult<()> {
    Ok(sikemux_keychain::delete(
        KEY_SERVICE,
        &entry(account, team),
    )?)
}
