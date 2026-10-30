// The Vercel account signed in here. Its access token, or the token of each team the
// Sikemux integration is installed on, lives in the Keychain; the file beside it only
// says whose they are and which team calls use when they name none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Secrets};

use crate::error::VercelResult;

/// The account's token and each team install's, under `entry`.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-vercel-token"));

pub use sikemux_plugin_vercel_views::{Account, SignIn};

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
