// The Bitbucket accounts signed in here. Each account's secret, an OAuth
// refresh token or an API token, lives in the Keychain; the file beside it
// only says which accounts there are, how each signed in, and which one a
// project uses when it names none.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::BitbucketResult;

/// Each account's refresh token or API token, under `Account::keychain_account`.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-bitbucket-token"));

pub use sikemux_plugin_api::codehost::SignedInWith as Method;

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

accounts!(BitbucketConfig, Account, accounts, default);

pub fn load(data_dir: &Path) -> BitbucketResult<BitbucketConfig> {
    Ok(store::load(data_dir)?)
}
