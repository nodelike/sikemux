// The GitLab accounts signed in here, each on its own server: gitlab.com or a
// company's own. Each account's access token lives in the Keychain; the file
// beside it only says which accounts there are, where, and which one a project
// uses when it names none.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{GitlabError, GitlabResult};

#[cfg(not(test))]
const TOKEN_SERVICE: &str = "sikemux-gitlab-token";
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
const TOKEN_SERVICE: &str = "sikemux-gitlab-token-test";

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

impl GitlabConfig {
    /// The account named, or with none named, the default one.
    pub fn account(&self, id: Option<&str>) -> Option<&Account> {
        let wanted = id.or(self.default.as_deref());
        match wanted {
            Some(wanted) => self.accounts.iter().find(|account| account.id == wanted),
            None => self.accounts.first(),
        }
    }

    /// Adds the account, or replaces the one with the same id. The first account becomes the default.
    pub fn upsert(&mut self, account: Account) {
        match self.accounts.iter_mut().find(|kept| kept.id == account.id) {
            Some(kept) => *kept = account,
            None => self.accounts.push(account),
        }
        let default_is_signed_in = self
            .default
            .as_ref()
            .is_some_and(|id| self.accounts.iter().any(|account| &account.id == id));
        if !default_is_signed_in {
            self.default = self.accounts.first().map(|account| account.id.clone());
        }
    }

    /// Takes the account out; a removed default passes to the first account left.
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let index = self.accounts.iter().position(|account| account.id == id)?;
        let removed = self.accounts.remove(index);
        if self.default.as_deref() == Some(id) {
            self.default = self.accounts.first().map(|account| account.id.clone());
        }
        Some(removed)
    }

    /// Every account, the default first, which is the order to try them in.
    pub fn in_order(&self) -> Vec<Account> {
        let mut ordered = self.accounts.clone();
        ordered.sort_by_key(|account| Some(&account.id) != self.default.as_ref());
        ordered
    }

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

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load(data_dir: &Path) -> GitlabConfig {
    std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> GitlabError {
    GitlabError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &GitlabConfig) -> GitlabResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> GitlabError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => GitlabError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => GitlabError::Keychain(message),
    }
}

/// The Keychain only takes token-like names, so the `#` between server and id is left out.
fn keychain_account(account: &Account) -> String {
    account.id.replace('#', ".")
}

pub fn keychain_read(account: &Account) -> GitlabResult<Option<String>> {
    sikemux_keychain::read(TOKEN_SERVICE, &keychain_account(account)).map_err(keychain_error)
}

pub fn keychain_write(account: &Account, secret: &str) -> GitlabResult<()> {
    sikemux_keychain::write(TOKEN_SERVICE, &keychain_account(account), secret)
        .map_err(keychain_error)
}

pub fn keychain_delete(account: &Account) -> GitlabResult<()> {
    sikemux_keychain::delete(TOKEN_SERVICE, &keychain_account(account)).map_err(keychain_error)
}

/// Runs work that starts a process or touches the Keychain on a thread meant
/// for blocking, away from the few threads every plugin shares.
pub async fn blocking<T: Send + 'static>(
    work: Box<dyn FnOnce() -> GitlabResult<T> + Send>,
) -> GitlabResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| GitlabError::Keychain(error.to_string()))?
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
    fn the_first_account_becomes_the_default_and_a_second_does_not() {
        let mut config = GitlabConfig::default();
        assert!(config.account(None).is_none());
        config.upsert(account("gitlab.com", 1));
        config.upsert(account("gitlab.acme.dev", 1));
        assert_eq!(
            config.account(None).map(|a| a.id.as_str()),
            Some("gitlab.com#1")
        );
        assert_eq!(
            config
                .account(Some("gitlab.acme.dev#1"))
                .map(|a| a.host.as_str()),
            Some("gitlab.acme.dev")
        );
    }

    #[test]
    fn the_same_person_id_on_two_servers_is_two_accounts() {
        let mut config = GitlabConfig::default();
        config.upsert(account("gitlab.com", 7));
        config.upsert(account("gitlab.acme.dev", 7));
        config.upsert(account("gitlab.com", 7));
        assert_eq!(config.accounts.len(), 2);
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
    fn removing_the_default_hands_it_to_the_next_account_and_order_starts_with_it() {
        let mut config = GitlabConfig::default();
        config.upsert(account("gitlab.com", 1));
        config.upsert(account("gitlab.com", 2));
        config.default = Some("gitlab.com#2".into());
        let order: Vec<String> = config.in_order().into_iter().map(|a| a.id).collect();
        assert_eq!(order, ["gitlab.com#2", "gitlab.com#1"]);
        config.remove("gitlab.com#2");
        assert_eq!(config.default.as_deref(), Some("gitlab.com#1"));
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
