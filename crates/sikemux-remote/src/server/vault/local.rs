//! What this host remembers of the vault between runs: which entries it has put in its secret
//! store, as what, and whether its own sign-ins have moved in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_plugin_api::vault::SignIn;

use super::Scope;

const RECOVERY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-vault-recovery-dev"
} else {
    "sikemux-vault-recovery"
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Local {
    pub account: String,
    /// The vault this record is of, by its recovery key's public half. An account's vault
    /// made again starts this host over, rather than taking its old entries for revoked ones.
    #[serde(default)]
    pub vault: String,
    /// This host's sign-ins went into the vault once, as it made or joined it.
    pub moved_in: bool,
    pub applied: BTreeMap<String, Applied>,
    /// This host's key on the account, and who may use each entry it holds, so a core that
    /// has not reached the account since it started still keeps to them.
    pub host: String,
    pub rules: BTreeMap<String, Rule>,
}

/// Who may use an entry and who refreshes it, without its secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub label: String,
    /// The secrets it holds, by service and account.
    pub secrets: Vec<(String, String)>,
    pub refresher: String,
    pub access: BTreeMap<String, Scope>,
}

/// An entry as this host's secret store holds it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub revision: i64,
    pub plugin: String,
    pub identity: String,
    /// What the plugin's store held of it then, so a change made here can be told from one
    /// made elsewhere.
    pub digest: String,
}

pub fn digest(sign_in: &SignIn) -> String {
    let bytes = serde_json::to_vec(&(&sign_in.secrets, &sign_in.records)).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("vault.json")
}

pub fn read(data_dir: Option<&Path>) -> Option<Local> {
    let bytes = std::fs::read(path(data_dir?)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn load(data_dir: Option<&Path>, account: &str, vault: &str) -> Local {
    let read = read(data_dir).filter(|local| local.account == account && local.vault == vault);
    read.unwrap_or_else(|| Local {
        account: account.to_owned(),
        vault: vault.to_owned(),
        ..Local::default()
    })
}

pub fn save(data_dir: Option<&Path>, local: &Local) -> Result<(), String> {
    let Some(dir) = data_dir else { return Ok(()) };
    let bytes = serde_json::to_vec_pretty(local).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(&path(dir), &bytes, true)
        .map(drop)
        .map_err(|error| error.to_string())
}

/// The recovery key of a vault this host made, kept in its secret store until the person
/// says they saved it.
pub fn keep_recovery(account: &str, text: &str) -> Result<(), String> {
    sikemux_keychain::write(RECOVERY_SERVICE, account, text).map_err(|error| error.to_string())
}

pub fn recovery(account: &str) -> Option<String> {
    sikemux_keychain::read(RECOVERY_SERVICE, account)
        .ok()
        .flatten()
}

pub fn forget_recovery(account: &str) -> Result<(), String> {
    sikemux_keychain::delete(RECOVERY_SERVICE, account).map_err(|error| error.to_string())
}

fn uses_path(data_dir: &Path) -> PathBuf {
    data_dir.join("vault-uses.json")
}

/// The uses not yet sent to the account, kept so a core that stops before sending them does
/// not lose them.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unsent<T> {
    pub account: String,
    pub uses: Vec<T>,
}

pub fn keep_uses<T: Serialize>(data_dir: Option<&Path>, unsent: &Unsent<T>) -> Result<(), String> {
    let Some(dir) = data_dir else { return Ok(()) };
    let bytes = serde_json::to_vec(unsent).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(&uses_path(dir), &bytes, true)
        .map(drop)
        .map_err(|error| error.to_string())
}

pub fn kept_uses<T: serde::de::DeserializeOwned>(data_dir: Option<&Path>) -> Option<Unsent<T>> {
    let bytes = std::fs::read(uses_path(data_dir?)).ok()?;
    serde_json::from_slice(&bytes).ok()
}
