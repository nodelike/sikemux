// The service account signed in and the upload keystores kept here. The account's key
// and each keystore's password live in the Keychain; this file only names them.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Secrets};

use crate::error::PlayResult;

pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-googleplay"));
pub const ACCOUNT_KEY: &str = "service-account";

/// A Keychain account name allows no slashes, so a keystore is kept under its path's digest.
pub fn keystore_entry(path: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, path.as_bytes());
    let hex: String = digest
        .as_ref()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("keystore:{hex}")
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: String,
    pub project: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Keystore {
    pub path: String,
    pub alias: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlayConfig {
    #[serde(default)]
    pub account: Option<Account>,
    #[serde(default)]
    pub keystores: Vec<Keystore>,
}

pub fn load(data_dir: &Path) -> PlayResult<PlayConfig> {
    Ok(store::load(data_dir)?)
}
