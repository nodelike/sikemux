// The Linear workspaces signed in here. Each one's API key, or the refresh token
// of a browser sign-in, lives in the Keychain; the file beside it only says which workspaces there are, who is
// signed in to each, and which one a call uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::LinearResult;

/// Each workspace's API key or refresh token, under the workspace's id.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-linear-key"));

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Linear's organization id.
    pub id: String,
    pub name: String,
    /// The workspace's part of its addresses, `linear.app/<url_key>/…`.
    pub url_key: String,
    pub user_id: String,
    pub user: String,
    #[serde(default)]
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    #[default]
    Key,
    Browser,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LinearConfig {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub default: Option<String>,
}

impl store::Account for Workspace {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_named(&self, name: &str) -> bool {
        self.id == name || self.url_key == name
    }
}

accounts!(LinearConfig, Workspace, workspaces, default);

pub fn load(data_dir: &Path) -> LinearResult<LinearConfig> {
    Ok(store::load(data_dir)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_plugin_api::store::Accounts;

    fn workspace(id: &str, url_key: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: id.into(),
            url_key: url_key.into(),
            user_id: "u1".into(),
            user: "ankit".into(),
            sign_in: SignIn::Key,
        }
    }

    #[test]
    fn a_url_key_finds_its_workspace() {
        let config = LinearConfig {
            workspaces: vec![workspace("o1", "acme"), workspace("o2", "side")],
            default: Some("o1".into()),
        };
        assert_eq!(
            config.account(Some("side")).map(|w| w.id.as_str()),
            Some("o2")
        );
        assert_eq!(
            config.account(Some("o2")).map(|w| w.id.as_str()),
            Some("o2")
        );
    }
}
