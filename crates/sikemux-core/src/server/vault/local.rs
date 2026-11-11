//! What this host remembers of the vault between runs: which entries it has put in its secret
//! store, as what, and whether its own sign-ins have moved in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_plugin_api::vault::SignIn;

const RECOVERY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-vault-recovery-dev"
} else {
    "sikemux-vault-recovery"
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Local {
    pub account: String,
    /// The vault this record is of, by its recovery key's public half. An account's vault
    /// made again starts this host over, rather than taking its old entries for revoked ones.
    #[serde(default)]
    pub vault: String,
    /// This host's sign-ins went into the vault once, as it made or joined it.
    pub moved_in: bool,
    pub applied: BTreeMap<String, Applied>,
}

/// An entry as this host's secret store holds it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Applied {
    pub revision: i64,
    pub plugin: String,
    pub identity: String,
    /// What the plugin's store held of it then, so a change made here can be told from one
    /// made elsewhere.
    pub digest: String,
}

pub(super) fn digest(sign_in: &SignIn) -> String {
    let bytes = serde_json::to_vec(&(&sign_in.secrets, &sign_in.records)).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("vault.json")
}

pub(super) fn load(data_dir: Option<&Path>, account: &str, vault: &str) -> Local {
    let read = data_dir
        .and_then(|dir| std::fs::read(path(dir)).ok())
        .and_then(|bytes| serde_json::from_slice::<Local>(&bytes).ok())
        .filter(|local| local.account == account && local.vault == vault);
    read.unwrap_or_else(|| Local {
        account: account.to_owned(),
        vault: vault.to_owned(),
        ..Local::default()
    })
}

pub(super) fn save(data_dir: Option<&Path>, local: &Local) -> Result<(), String> {
    let Some(dir) = data_dir else { return Ok(()) };
    let bytes = serde_json::to_vec_pretty(local).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(&path(dir), &bytes, true)
        .map(drop)
        .map_err(|error| error.to_string())
}

/// The recovery key of a vault this host made, kept in its secret store until the person
/// says they saved it.
pub(super) fn keep_recovery(account: &str, text: &str) -> Result<(), String> {
    sikemux_keychain::write(RECOVERY_SERVICE, account, text).map_err(|error| error.to_string())
}

pub(super) fn recovery(account: &str) -> Option<String> {
    sikemux_keychain::read(RECOVERY_SERVICE, account)
        .ok()
        .flatten()
}

pub(super) fn forget_recovery(account: &str) -> Result<(), String> {
    sikemux_keychain::delete(RECOVERY_SERVICE, account).map_err(|error| error.to_string())
}
