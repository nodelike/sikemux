//! What this host remembers of the vault between runs: which entries it has put in its secret
//! store, as what, and whether its own sign-ins have moved in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_plugin_api::vault::SignIn;
use sikemux_process::sync::Locked;

use super::floor::Floor;

const RECOVERY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-vault-recovery-dev"
} else {
    "sikemux-vault-recovery"
};

const KEY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-vault-key-dev"
} else {
    "sikemux-vault-key"
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
    /// This host's key on the account, and who refreshes each entry it holds, so a core that
    /// has not reached the account since it started still knows.
    pub host: String,
    pub rules: BTreeMap<String, Rule>,
    #[serde(default)]
    pub floor: Floor,
    /// This host has opened or written an authenticated v2 key wrap for this vault, so
    /// unauthenticated v1 wraps are never accepted again.
    #[serde(default)]
    pub authenticated_wraps: bool,
}

/// What an entry holds and who refreshes it, without its secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub label: String,
    /// The secrets it holds, by service and account.
    pub secrets: Vec<(String, String)>,
    pub refresher: String,
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
    sikemux_process::files::read_or_set_aside(&path(data_dir?), |bytes| {
        serde_json::from_slice(bytes)
    })
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptKey {
    vault: String,
    generation: i64,
    key: super::seal::Key,
}

pub fn keep_key(
    data_dir: Option<&Path>,
    account: &str,
    vault: &str,
    generation: i64,
    key: &super::seal::Key,
) -> Result<(), String> {
    if data_dir.is_none() {
        return Ok(());
    }
    let text = serde_json::to_string(&KeptKey {
        vault: vault.to_owned(),
        generation,
        key: *key,
    })
    .map_err(|error| error.to_string())?;
    sikemux_keychain::write(KEY_SERVICE, account, &text).map_err(|error| error.to_string())
}

pub fn kept_key(
    data_dir: Option<&Path>,
    account: &str,
    vault: &str,
    generation: i64,
) -> Option<super::seal::Key> {
    data_dir?;
    let text = sikemux_keychain::read(KEY_SERVICE, account).ok()??;
    let kept: KeptKey = serde_json::from_str(&text).ok()?;
    (kept.vault == vault && kept.generation == generation).then_some(kept.key)
}

fn marks_path(data_dir: &Path) -> PathBuf {
    data_dir.join("vault-kept.json")
}

/// Which sign-ins stay on this host and out of the vault. Kept apart from [`Local`], which
/// starts over with a new account or vault, so a mark is never lost to one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marks {
    /// A sign-in first seen here that the vault does not hold stays here.
    pub keep_new: bool,
    pub sign_ins: Vec<Mark>,
}

/// What the person or the host's rule decided for one sign-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub plugin: String,
    pub identity: String,
    pub label: String,
    pub kept: bool,
}

impl Marks {
    pub fn of(&self, plugin: &str, identity: &str) -> Option<&Mark> {
        self.sign_ins
            .iter()
            .find(|mark| mark.plugin == plugin && mark.identity == identity)
    }

    pub fn kept(&self, plugin: &str, identity: &str) -> bool {
        self.of(plugin, identity).is_some_and(|mark| mark.kept)
    }

    pub fn set(&mut self, mark: Mark) {
        match self
            .sign_ins
            .iter_mut()
            .find(|kept| kept.plugin == mark.plugin && kept.identity == mark.identity)
        {
            Some(kept) => *kept = mark,
            None => self.sign_ins.push(mark),
        }
    }
}

/// The marks on disk, or none where there is no file yet. A file that does not read is an
/// error, since reading it as no marks would send every kept sign-in to the vault.
pub fn marks(data_dir: Option<&Path>) -> Result<Marks, String> {
    let Some(dir) = data_dir else {
        return Ok(Marks::default());
    };
    let unread = |error: &dyn std::fmt::Display| {
        format!("the sign-ins kept on this host (vault-kept.json) do not read: {error}")
    };
    match std::fs::read(marks_path(dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| unread(&error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Marks::default()),
        Err(error) => Err(unread(&error)),
    }
}

static CHANGING_MARKS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Changes the marks on disk, one change at a time, and returns them as they now are.
pub fn change_marks(
    data_dir: Option<&Path>,
    change: impl FnOnce(&mut Marks),
) -> Result<Marks, String> {
    let _turn = CHANGING_MARKS.locked();
    let mut kept = marks(data_dir)?;
    let before = kept.clone();
    change(&mut kept);
    if kept != before {
        save_marks(data_dir, &kept)?;
    }
    Ok(kept)
}

fn save_marks(data_dir: Option<&Path>, marks: &Marks) -> Result<(), String> {
    let Some(dir) = data_dir else { return Ok(()) };
    let bytes = serde_json::to_vec_pretty(marks).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(&marks_path(dir), &bytes, true)
        .map(drop)
        .map_err(|error| error.to_string())
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
    sikemux_process::files::read_or_set_aside(&uses_path(data_dir?), |bytes| {
        serde_json::from_slice(bytes)
    })
}
