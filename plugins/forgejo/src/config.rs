// The Forgejo and Gitea accounts signed in here, each on its own server: Codeberg,
// gitea.com or a company's own. Each account's token lives in the Keychain; the
// file beside it only says which accounts there are, where, and which one a
// project uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Accounts, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::ForgejoResult;

/// Each account's access token, under `Account::keychain_account`.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-forgejo-token"));

pub const CODEBERG: &str = "codeberg.org";

/// The public servers a remote counts as Forgejo's on before anyone signs in to them.
pub const PUBLIC: [&str; 2] = [CODEBERG, "gitea.com"];

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The server and the person's id on it, `codeberg.org#123`, since ids repeat across servers.
    pub id: String,
    /// The server's host name, such as `codeberg.org` or `git.acme.dev`.
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

    /// The Keychain only takes token-like names, so the `#` between server and id is left out.
    pub fn keychain_account(&self) -> String {
        self.id.replace('#', ".")
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ForgejoConfig {
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

accounts!(ForgejoConfig, Account, accounts, default);

impl ForgejoConfig {
    /// The first account on the server, the default first.
    pub fn account_on(&self, host: &str) -> Option<Account> {
        self.in_order()
            .into_iter()
            .find(|account| same_server(&account.host, host))
    }

    /// The servers a remote may point at to count as Forgejo: the public ones, and every server signed in to.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = PUBLIC.iter().map(|host| host.to_string()).collect();
        for account in &self.accounts {
            if !hosts.iter().any(|host| same_server(host, &account.host)) {
                hosts.push(account.host.clone());
            }
        }
        hosts
    }
}

pub fn load(data_dir: &Path) -> ForgejoResult<ForgejoConfig> {
    Ok(store::load(data_dir)?)
}

impl sikemux_plugin_api::vault::Listing for Account {
    fn secret_account(&self) -> Option<String> {
        Some(self.keychain_account())
    }

    fn label(&self) -> String {
        format!("{} on {}", self.login, self.host)
    }
}

pub static BOOK: sikemux_plugin_api::vault::Listed<Account> =
    sikemux_plugin_api::vault::Listed::new(
        "config.json",
        "accounts",
        "id",
        Some("default"),
        KEYCHAIN,
    );

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
    fn the_public_servers_and_every_signed_in_one_count_as_forgejo() {
        let mut config = ForgejoConfig::default();
        assert_eq!(config.hosts(), ["codeberg.org", "gitea.com"]);
        config.upsert(account("git.acme.dev:8443", 1));
        config.upsert(account("codeberg.org", 2));
        assert_eq!(
            config.hosts(),
            ["codeberg.org", "gitea.com", "git.acme.dev:8443"]
        );
        assert_eq!(
            config.account_on("git.acme.dev").map(|a| a.id),
            Some("git.acme.dev:8443#1".to_string())
        );
    }
}
