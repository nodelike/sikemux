// The GitLab accounts signed in here, each on its own server: gitlab.com or a
// company's own. Each account's access token lives in the Keychain; the file
// beside it only says which accounts there are, where, and which one a project
// uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Accounts};

use crate::error::GitlabResult;

const TOKEN_SERVICE: &str = keychain_service!("sikemux-gitlab-token");

pub const GITLAB_COM: &str = "gitlab.com";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The server and the person's id on it, `gitlab.com#123`, since ids repeat across servers.
    pub id: String,
    /// The server's host name, such as `gitlab.com` or `gitlab.acme.dev`.
    pub host: String,
    pub login: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Whether two hosts are the same server. A remote may name its ssh port and an
/// account its https port, so the port is left out.
pub fn same_server(one: &str, other: &str) -> bool {
    let name = |host: &str| {
        host.split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    name(one) == name(other)
}

impl Account {
    pub fn id_for(host: &str, user_id: u64) -> String {
        format!("{host}#{user_id}")
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitlabConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// The account a project uses when it names none.
    #[serde(default)]
    pub default: Option<String>,
}

impl store::Account for Account {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Accounts for GitlabConfig {
    type Account = Account;

    fn accounts(&self) -> &[Account] {
        &self.accounts
    }
    fn accounts_mut(&mut self) -> &mut Vec<Account> {
        &mut self.accounts
    }
    fn default_id(&self) -> Option<&str> {
        self.default.as_deref()
    }
    fn default_id_mut(&mut self) -> &mut Option<String> {
        &mut self.default
    }
}

impl GitlabConfig {
    /// The first account on the server, the default first.
    pub fn account_on(&self, host: &str) -> Option<Account> {
        self.in_order()
            .into_iter()
            .find(|account| same_server(&account.host, host))
    }

    /// The servers a remote may point at to count as GitLab: gitlab.com, and every server signed in to.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts = vec![GITLAB_COM.to_string()];
        for account in &self.accounts {
            if !hosts.contains(&account.host) {
                hosts.push(account.host.clone());
            }
        }
        hosts
    }
}

pub fn load(data_dir: &Path) -> GitlabResult<GitlabConfig> {
    Ok(store::load(data_dir)?)
}

/// The Keychain only takes token-like names, so the `#` between server and id is left out.
fn keychain_account(account: &Account) -> String {
    account.id.replace('#', ".")
}

pub fn keychain_read(account: &Account) -> GitlabResult<Option<String>> {
    Ok(sikemux_keychain::read(
        TOKEN_SERVICE,
        &keychain_account(account),
    )?)
}

pub fn keychain_write(account: &Account, secret: &str) -> GitlabResult<()> {
    Ok(sikemux_keychain::write(
        TOKEN_SERVICE,
        &keychain_account(account),
        secret,
    )?)
}

pub fn keychain_delete(account: &Account) -> GitlabResult<()> {
    Ok(sikemux_keychain::delete(
        TOKEN_SERVICE,
        &keychain_account(account),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(host: &str, id: u64) -> Account {
        Account {
            id: Account::id_for(host, id),
            host: host.into(),
            login: format!("user{id}"),
            display_name: None,
            avatar_url: None,
        }
    }

    #[test]
    fn gitlab_com_and_every_signed_in_server_count_as_gitlab() {
        let mut config = GitlabConfig::default();
        assert_eq!(config.hosts(), ["gitlab.com"]);
        config.upsert(account("gitlab.acme.dev", 1));
        config.upsert(account("gitlab.com", 2));
        assert_eq!(config.hosts(), ["gitlab.com", "gitlab.acme.dev"]);
    }

    #[test]
    fn a_server_is_the_same_whichever_port_is_named() {
        assert!(same_server("git.acme.dev:8443", "git.acme.dev"));
        assert!(same_server("GitLab.com", "gitlab.com"));
        assert!(!same_server("gitlab.acme.dev", "gitlab.com"));
        let mut config = GitlabConfig::default();
        config.upsert(account("gitlab.com", 1));
        config.upsert(account("git.acme.dev:8443", 2));
        assert_eq!(
            config.account_on("git.acme.dev").map(|a| a.id),
            Some("git.acme.dev:8443#2".to_string())
        );
        assert!(config.account_on("gitlab.acme.dev").is_none());
    }

    #[test]
    fn the_keychain_name_holds_no_hash() {
        assert_eq!(
            keychain_account(&account("gitlab.acme.dev", 9)),
            "gitlab.acme.dev.9"
        );
    }
}
