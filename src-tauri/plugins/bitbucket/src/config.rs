// The Bitbucket accounts signed in here. Each account's secret, an OAuth
// refresh token or an API token, lives in the Keychain; the file beside it
// only says which accounts there are, how each signed in, and which one a
// project uses when it names none.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BitbucketError, BitbucketResult};

#[cfg(not(test))]
const TOKEN_SERVICE: &str = "sikemux-bitbucket-token";
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
const TOKEN_SERVICE: &str = "sikemux-bitbucket-token-test";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    /// Signed in through the browser; the Keychain holds a refresh token.
    Oauth,
    /// A pasted API token or access token; the Keychain holds the token.
    Token,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Bitbucket's id for the person, without its braces.
    pub id: String,
    pub login: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub method: Method,
    /// An Atlassian API token is sent with the account's email; an access
    /// token made for a repository or workspace is sent on its own.
    #[serde(default)]
    pub email: Option<String>,
}

impl Account {
    fn keychain_account(&self) -> String {
        let kind = match self.method {
            Method::Oauth => "oauth",
            Method::Token => "token",
        };
        format!("{kind}:{}", self.id)
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BitbucketConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// The account a project uses when it names none.
    #[serde(default)]
    pub default: Option<String>,
}

impl BitbucketConfig {
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
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load(data_dir: &Path) -> BitbucketConfig {
    std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> BitbucketError {
    BitbucketError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &BitbucketConfig) -> BitbucketResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> BitbucketError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => BitbucketError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => BitbucketError::Keychain(message),
    }
}

pub fn keychain_read(account: &Account) -> BitbucketResult<Option<String>> {
    sikemux_keychain::read(TOKEN_SERVICE, &account.keychain_account()).map_err(keychain_error)
}

pub fn keychain_write(account: &Account, secret: &str) -> BitbucketResult<()> {
    sikemux_keychain::write(TOKEN_SERVICE, &account.keychain_account(), secret)
        .map_err(keychain_error)
}

pub fn keychain_delete(account: &Account) -> BitbucketResult<()> {
    sikemux_keychain::delete(TOKEN_SERVICE, &account.keychain_account()).map_err(keychain_error)
}

/// Runs work that starts a process or touches the Keychain on a thread meant
/// for blocking, away from the few threads every plugin shares.
pub async fn blocking<T: Send + 'static>(
    work: Box<dyn FnOnce() -> BitbucketResult<T> + Send>,
) -> BitbucketResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| BitbucketError::Keychain(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str) -> Account {
        Account {
            id: id.into(),
            login: id.into(),
            display_name: None,
            avatar_url: None,
            method: Method::Oauth,
            email: None,
        }
    }

    #[test]
    fn the_first_account_becomes_the_default_and_a_second_does_not() {
        let mut config = BitbucketConfig::default();
        assert!(config.account(None).is_none());
        config.upsert(account("work"));
        config.upsert(account("home"));
        assert_eq!(config.account(None).map(|a| a.id.as_str()), Some("work"));
        assert_eq!(
            config.account(Some("home")).map(|a| a.id.as_str()),
            Some("home")
        );
        assert!(config.account(Some("gone")).is_none());
    }

    #[test]
    fn signing_in_again_replaces_the_account_rather_than_adding_one() {
        let mut config = BitbucketConfig::default();
        config.upsert(account("work"));
        let mut again = account("work");
        again.login = "renamed".into();
        config.upsert(again);
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].login, "renamed");
    }

    #[test]
    fn removing_the_default_hands_it_to_the_next_account() {
        let mut config = BitbucketConfig::default();
        config.upsert(account("work"));
        config.upsert(account("home"));
        config.remove("work");
        assert_eq!(config.default.as_deref(), Some("home"));
        config.remove("home");
        assert_eq!(config.default, None);
    }

    #[test]
    fn accounts_are_tried_default_first() {
        let mut config = BitbucketConfig::default();
        config.upsert(account("work"));
        config.upsert(account("home"));
        config.default = Some("home".into());
        let order: Vec<String> = config.in_order().into_iter().map(|a| a.id).collect();
        assert_eq!(order, ["home", "work"]);
    }

    #[test]
    fn nothing_saved_reads_as_no_accounts() {
        let dir = std::env::temp_dir().join(format!("sikemux-bb-config-{}", std::process::id()));
        assert_eq!(load(&dir), BitbucketConfig::default());
        let mut saved = BitbucketConfig::default();
        saved.upsert(account("work"));
        save(&dir, &saved).expect("saves");
        assert_eq!(load(&dir), saved);
        std::fs::remove_dir_all(dir).ok();
    }
}
