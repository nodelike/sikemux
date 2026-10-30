// The Slack workspaces signed in here. Each workspace's token lives in the
// Keychain; the file beside it only says which workspaces there are, who is
// signed in to each, and which one a call uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::SlackResult;

/// Each workspace's token, under the workspace's id.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-slack-token"));

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Slack's team id, `T…`.
    pub id: String,
    pub name: String,
    /// The workspace's address, `acme.slack.com`, which its message links use.
    pub domain: String,
    pub user_id: String,
    pub user: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SlackConfig {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub default: Option<String>,
}

impl store::Account for Workspace {
    fn id(&self) -> &str {
        &self.id
    }
}

accounts!(SlackConfig, Workspace, workspaces, default);

impl SlackConfig {
    /// The workspace a message link points into, by its address.
    pub fn by_domain(&self, domain: &str) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.domain.eq_ignore_ascii_case(domain))
    }
}

pub fn load(data_dir: &Path) -> SlackResult<SlackConfig> {
    Ok(store::load(data_dir)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, domain: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: id.into(),
            domain: domain.into(),
            user_id: "U1".into(),
            user: "ankit".into(),
        }
    }

    #[test]
    fn links_find_their_workspace_by_address() {
        let config = SlackConfig {
            workspaces: vec![
                workspace("T1", "acme.slack.com"),
                workspace("T2", "swishx.slack.com"),
            ],
            default: Some("T1".into()),
        };
        assert_eq!(
            config.by_domain("SwishX.slack.com").map(|w| w.id.as_str()),
            Some("T2")
        );
    }
}
