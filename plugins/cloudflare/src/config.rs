use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::CloudflareResult;

pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-cloudflare-token"));

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareAccount {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    #[default]
    Token,
    Browser,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account: Option<String>,
    #[serde(default)]
    pub cf_accounts: Vec<CloudflareAccount>,
    #[serde(default)]
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub default: Option<String>,
}

impl store::Account for Account {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_named(&self, name: &str) -> bool {
        self.id == name || self.name.eq_ignore_ascii_case(name)
    }
}

accounts!(CloudflareConfig, Account, accounts, default);

pub fn load(data_dir: &Path) -> CloudflareResult<CloudflareConfig> {
    Ok(store::load(data_dir)?)
}

impl sikemux_plugin_api::vault::Listing for Account {
    fn secret_account(&self) -> Option<String> {
        Some(self.id.clone())
    }

    fn label(&self) -> String {
        self.name.clone()
    }
}
