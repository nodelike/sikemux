// The Bitbucket accounts signed in here. Each account's secret, an OAuth
// refresh token or an API token, lives in the Keychain; the file beside it
// only says which accounts there are, how each signed in, and which one a
// project uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Accounts};

use crate::error::BitbucketResult;

pub const TOKEN_SERVICE: &str = keychain_service!("sikemux-bitbucket-token");

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
    pub fn keychain_account(&self) -> String {
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

impl store::Account for Account {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Accounts for BitbucketConfig {
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

pub fn load(data_dir: &Path) -> BitbucketResult<BitbucketConfig> {
    Ok(store::load(data_dir)?)
}

pub fn keychain_read(account: &Account) -> BitbucketResult<Option<String>> {
    Ok(sikemux_keychain::read(
        TOKEN_SERVICE,
        &account.keychain_account(),
    )?)
}

pub fn keychain_write(account: &Account, secret: &str) -> BitbucketResult<()> {
    Ok(sikemux_keychain::write(
        TOKEN_SERVICE,
        &account.keychain_account(),
        secret,
    )?)
}

pub fn keychain_delete(account: &Account) -> BitbucketResult<()> {
    Ok(sikemux_keychain::delete(
        TOKEN_SERVICE,
        &account.keychain_account(),
    )?)
}
