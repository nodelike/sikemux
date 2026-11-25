//! Each org's vault (ADR 0026): what an org's owners and admins keep for every member, sealed
//! under the org's own key, which the accounts server never sees. A member's host gets the key
//! from a host that holds it, keeps its copy in the secret store apart from the account's
//! vault, and lets it go when the account leaves the org or the host leaves the account.

use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sikemux_agents::launch::providers::{catalogue, OrgKey, ProviderOrg};
use sikemux_calls::vault::{
    OrgEntryParams, OrgLetIn, OrgLetInAnswer, OrgModelKeyAdd, OrgParams, OrgVaultStatus,
    ShownOrgModelKey, ShownOrgOpener, ShownOrgVault,
};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::device::{AccountOrg, OrgRole};
use tokio::sync::Notify;

use crate::accounts::orgvault::{
    OrgVault as Remote, OrgVaultCreate, OrgVaultEntry, OrgVaultEntryPut, OrgVaultEntryRead,
    OrgVaultHolder, OrgVaultKeysAdd, OrgVaultRemake, OrgVaultRotate, OrgVaultState,
    OrgVaultVouchPut, OrgVaultVouches, OrgWrappedKey,
};
use crate::accounts::vault::VaultEntryReseal;

use super::super::account::api::{host_request, host_send_any};
use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::floor;
use super::seal::{self, Key};
use super::trust::{self, wrap_context, Me, Pins};

const TICK: Duration = Duration::from_secs(60);
/// How often a vault this host waits on is read while computers that could let it in are listed,
/// so the person sees soon when the last of them goes offline. Nothing else tells this host.
const WAITING_TICK: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(2);
const MODEL_KEY: &str = "model-key";
/// The most hosts one request may seal the key for.
const WRAPS: usize = 500;
/// The longest sealed text the server takes of one entry.
const SEALED: usize = 87_400;

const SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-org-vaults-dev"
} else {
    "sikemux-org-vaults"
};
const ITEM: &str = "org-vaults";
/// Where builds that keep the trusted hosts in the same item look for it.
const ITEM_BEFORE: &str = "org-keys";
/// Where builds that seal the key for hosts nobody let in look for it.
const ITEM_FIRST: &str = "orgs";
const ENTRIES_FOLDER: &str = "org-vault-entries";
/// Where the first builds keep the entries, and clear away what their key does not open.
const ENTRIES_FOLDER_BEFORE: &str = "org-vaults";

/// What a model-key entry holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ModelKey {
    /// The catalogue provider it is a key of.
    provider: String,
    key: String,
    /// The hosts it is kept to, as the account vault's entries mark it. A host left out does
    /// not use it.
    #[serde(default, rename = "access", skip_serializing_if = "Option::is_none")]
    limited_to: Option<BTreeMap<String, Value>>,
}

/// An entry as this host opened it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Opened {
    kind: String,
    revision: i64,
    body: Value,
}

/// One org's vault as this host holds it, which its secret store keeps between runs. The server
/// is refused anything older than this: an earlier generation of the key, or an earlier revision
/// of an entry held.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Held {
    generation: i64,
    key: Option<Key>,
    /// Every host this key was sealed for was let in. One kept from before hosts were let in is
    /// not, and goes to no other host.
    vetted: bool,
    /// This host made the key, or it opened an entry here at this generation, so it is known to be
    /// the vault's.
    proved: bool,
    entries: BTreeMap<String, Opened>,
    /// The last revision of each entry the vault removed, which a new entry under its id passes.
    ended: BTreeMap<String, i64>,
    /// When the vault these entries came from was last made again, as the accounts server said.
    /// An entry read before a different one began anew, under the same id.
    made: Option<String>,
    pins: Pins,
    /// This host's half of the code it shows while it waits for the key.
    code: Option<trust::Secret>,
}

impl Held {
    fn takes_generation(&self, generation: i64) -> bool {
        floor::takes_generation(self.generation, generation)
    }

    /// What this host holds once it cannot open the vault at `generation`: no key and no
    /// entries, only the hosts it trusts, the code that lets it in, and the generation, below
    /// which the server is refused anything it hands back.
    fn let_go(self, generation: i64) -> Held {
        Held {
            generation: generation.max(self.generation),
            key: None,
            vetted: false,
            proved: false,
            entries: BTreeMap::new(),
            ended: BTreeMap::new(),
            made: self.made,
            pins: self.pins,
            code: self.code.or_else(trust::new_secret),
        }
    }

    /// The marks of entries this host saw go once it reads `vault`: those the server lists, and
    /// in a full read those it leaves out, each at least one above the copy this host held.
    fn ended_after(&self, vault: &Remote, partial: bool) -> BTreeMap<String, i64> {
        let above_held = |id: &str| self.entries.get(id).map_or(0, |entry| entry.revision + 1);
        let listed =
            (vault.ended.iter()).map(|gone| (&gone.id, gone.revision.max(above_held(&gone.id))));
        let left_out = (self.entries.keys())
            .filter(|id| !partial && !vault.entries.iter().any(|entry| &entry.id == *id))
            .map(|id| (id, above_held(id)));
        let mut ended = self.ended.clone();
        floor::end(&mut ended, listed.chain(left_out));
        ended
    }

    /// The copy of `entry` this host opened before, unless the vault removed it since.
    fn copy_of(&self, ended: &BTreeMap<String, i64>, entry: &OrgVaultEntry) -> Option<&Opened> {
        (self.entries.get(&entry.id))
            .filter(|kept| (ended.get(&entry.id)).is_none_or(|mark| kept.revision > *mark))
    }

    fn takes_entry(&self, ended: &BTreeMap<String, i64>, id: &str, revision: i64) -> bool {
        floor::takes_entry(
            self.entries.get(id).map(|held| held.revision),
            ended.get(id).copied(),
            revision,
        )
    }
}

/// What this host keeps between runs: every org vault it holds, for one account. The secret
/// store keeps each org's key; each org's entries go in a file of their own, sealed under it.
#[derive(Clone, Default, PartialEq)]
struct Copy {
    account: String,
    orgs: BTreeMap<String, Held>,
}

/// The secret store's part of [`Copy`].
#[derive(Serialize, Deserialize)]
struct Keys {
    account: String,
    orgs: BTreeMap<String, KeptKey>,
}

/// An org's key at its generation. Once this host can no longer open the vault, only the
/// rest, as the entries file is sealed under the key let go.
#[derive(Serialize, Deserialize)]
struct KeptKey {
    generation: i64,
    key: Option<Key>,
    #[serde(default)]
    vetted: bool,
    #[serde(default)]
    proved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    made: Option<String>,
    /// Kept in an item of its own, and here only by earlier builds.
    #[serde(default, rename = "pins", skip_serializing)]
    moving: Option<Pins>,
    #[serde(skip)]
    pins: Pins,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<trust::Secret>,
}

impl Copy {
    fn keys(&self) -> Keys {
        Keys {
            account: self.account.clone(),
            orgs: (self.orgs.iter())
                .map(|(id, held)| {
                    let kept = KeptKey {
                        generation: held.generation,
                        key: held.key,
                        vetted: held.vetted,
                        proved: held.proved,
                        made: held.made.clone(),
                        moving: None,
                        pins: Pins::default(),
                        code: held.code,
                    };
                    (id.clone(), kept)
                })
                .collect(),
        }
    }
}

fn entries_file(data_dir: &Path, org: &str) -> PathBuf {
    data_dir
        .join(ENTRIES_FOLDER)
        .join(URL_SAFE_NO_PAD.encode(org))
}

/// Moves the entries kept where earlier builds look, unless this one kept its own since.
fn move_entries(data_dir: &Path) -> std::io::Result<()> {
    let before = data_dir.join(ENTRIES_FOLDER_BEFORE);
    let folder = data_dir.join(ENTRIES_FOLDER);
    match (before.exists(), folder.exists()) {
        (true, false) => std::fs::rename(before, folder),
        (true, true) => std::fs::remove_dir_all(before),
        (false, _) => Ok(()),
    }
}

fn entries_context(account: &str, org: &str, generation: i64) -> String {
    format!("sikemux org vault entries\n{account}\n{org}\n{generation}")
}

fn read_entries(data_dir: Option<&Path>, account: &str, org: &str, kept: &KeptKey) -> Held {
    let Some(key) = kept.key else {
        return Held {
            generation: kept.generation,
            made: kept.made.clone(),
            pins: kept.pins.clone(),
            code: kept.code,
            ..Held::default()
        };
    };
    let (entries, ended) = data_dir
        .and_then(|dir| std::fs::read(entries_file(dir, org)).ok())
        .and_then(|sealed| {
            let context = entries_context(account, org, kept.generation);
            seal::open_bytes(&key, &context, &sealed)
        })
        .and_then(|plain| serde_json::from_slice(&plain).ok())
        .unwrap_or_default();
    Held {
        generation: kept.generation,
        key: Some(key),
        vetted: kept.vetted,
        proved: kept.proved,
        entries,
        ended,
        made: kept.made.clone(),
        pins: kept.pins.clone(),
        code: kept.code,
    }
}

/// Writes the entries of each org in `now` that differ from `was`, and deletes the files of
/// orgs whose key is no longer held.
fn write_entries(data_dir: &Path, was: Option<&Copy>, now: Option<&Copy>) -> CoreResult<()> {
    let folder = data_dir.join(ENTRIES_FOLDER);
    let held = |copy: Option<&Copy>, org: &str| copy.and_then(|copy| copy.orgs.get(org)).cloned();
    if let Some(now) = now {
        std::fs::create_dir_all(&folder)?;
        for (org, kept) in &now.orgs {
            let Some(key) = kept.key else {
                continue;
            };
            let same_account = was.is_some_and(|was| was.account == now.account);
            if same_account && held(was, org).as_ref() == Some(kept) {
                continue;
            }
            let plain = serde_json::to_vec(&(&kept.entries, &kept.ended))?;
            let context = entries_context(&now.account, org, kept.generation);
            let sealed = seal::seal_bytes(&key, &context, &plain)
                .ok_or_else(|| CoreError::from("the org vault's entries could not be sealed"))?;
            let path = entries_file(data_dir, org);
            let fresh = path.with_extension("new");
            std::fs::write(&fresh, sealed)?;
            std::fs::rename(&fresh, &path)?;
        }
    }
    let Ok(files) = std::fs::read_dir(&folder) else {
        return Ok(());
    };
    for file in files.flatten() {
        let org = (file.file_name().to_str())
            .and_then(|name| URL_SAFE_NO_PAD.decode(name).ok())
            .and_then(|name| String::from_utf8(name).ok());
        if org.is_none_or(|org| held(now, &org).is_none_or(|held| held.key.is_none())) {
            std::fs::remove_file(file.path())?;
        }
    }
    Ok(())
}

#[derive(Clone, Default)]
struct Org {
    held: Held,
    status: OrgVaultStatus,
    holders: usize,
    failed: Option<String>,
    /// Why this host is past the most computers trusted, and what frees a place.
    full: Option<String>,
    /// The vault's count of changes when this host last took in its entries, so the next read
    /// asks only for what changed after it.
    changes: Option<i64>,
    /// The computers that hold the key and would let this host in, as the server tells an owner's
    /// or admin's host.
    openers: Option<Vec<OrgVaultHolder>>,
    remade_at: Option<String>,
    /// What the person pastes on a computer that holds the key to let this one in.
    code: Option<String>,
    /// The entries that do not open here and no copy of which is held, but those a host may take
    /// out itself when it replaces the key. All that do not open, when that stops it replacing it.
    unreadable: BTreeSet<String>,
    /// The account's orgs no longer name it, or the accounts server knows no such org for the
    /// account. The key is used for nothing meanwhile.
    left: bool,
}

impl Org {
    /// Why nothing may be sealed under the key held, as the end of a sentence: one that could not
    /// be replaced, or may have gone to hosts nobody let in, may be held by a host that left.
    fn unwritable(&self) -> Option<String> {
        match &self.failed {
            Some(why) => Some(format!(": {why}")),
            None => (self.held.key.is_some() && !self.held.vetted).then(String::new),
        }
    }
}

#[derive(Default)]
struct State {
    /// The account whose copy is loaded, once it is.
    account: Option<String>,
    orgs: BTreeMap<String, Org>,
    /// What the secret store holds, as last written or read; None until this run looked.
    kept: Option<Option<Copy>>,
    /// The orgs whose vault the accounts server has answered for since the copy was loaded.
    heard: BTreeSet<String>,
}

#[derive(Default)]
pub struct OrgVaults {
    state: Mutex<State>,
    wake: Arc<Notify>,
    /// Told when this host first hears an org's vault, and each time its key or entries change.
    pub changed: Arc<Notify>,
    /// One round or change at a time, so a write never races the loop's reading of the vault.
    busy: tokio::sync::Mutex<()>,
}

impl OrgVaults {
    /// Whether this host can open the org's vault, and the code it shows while it waits.
    #[cfg(any(test, feature = "testing"))]
    pub fn seen(&self, org: &str) -> (OrgVaultStatus, Option<String>) {
        let state = self.state.locked();
        (state.orgs.get(org)).map_or((OrgVaultStatus::Waiting, None), |org| {
            (org.status, org.code.clone())
        })
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    fn waits_on_openers(&self) -> bool {
        (self.state.locked().orgs.values()).any(|org| {
            org.status == OrgVaultStatus::Waiting
                && org
                    .openers
                    .as_ref()
                    .is_some_and(|openers| !openers.is_empty())
        })
    }
}

/// What a sealed entry is bound to, so the server cannot move it to another org, entry or kind,
/// or hand back another generation's copy.
fn entry_context(org: &str, id: &str, kind: &str, generation: i64, revision: i64) -> String {
    format!("sikemux-org-vault-entry|v1|{org}|{id}|{kind}|{generation}|{revision}")
}

fn manages(org: &AccountOrg) -> bool {
    matches!(org.role, OrgRole::Owner | OrgRole::Admin)
}

fn vault_path(org: &str) -> String {
    format!("/v1/orgs/{org}/vault")
}

pub fn start(core: &Arc<Core>) {
    tokio::spawn(run(Arc::downgrade(core)));
}

async fn run(core: Weak<Core>) {
    let Some(wake) = core.upgrade().map(|core| core.vault().orgs().wake.clone()) else {
        return;
    };
    let mut first = true;
    let mut tick = TICK;
    loop {
        if !first && tokio::time::timeout(tick, wake.notified()).await.is_ok() {
            tokio::time::sleep(SETTLE).await;
        }
        first = false;
        let Some(core) = core.upgrade() else { return };
        tick = match core.vault().orgs().waits_on_openers() {
            true => WAITING_TICK,
            false => TICK,
        };
        let _busy = core.vault().orgs().busy.lock().await;
        if let Err(error) = round(&core).await {
            eprintln!("sikemux core: the org vaults did not sync: {error}");
        }
    }
}

/// The account's orgs, or None while this host has not heard which.
fn account_orgs(core: &Core) -> Option<Vec<AccountOrg>> {
    core.account().orgs()
}

async fn round(core: &Arc<Core>) -> CoreResult<()> {
    if !core.remote().loaded() {
        return Ok(());
    }
    let (Some(account), Some(host), true) = (
        core.remote().owner(),
        core.remote().host_key(),
        core.remote().signed_in_api().is_some(),
    ) else {
        {
            let mut state = core.vault().orgs().state.locked();
            let kept = state.kept.take();
            *state = State {
                kept,
                ..State::default()
            };
        }
        hand_on(core, &[]);
        return keep(core, None).await;
    };
    load(core, &account).await?;
    let Some(orgs) = account_orgs(core) else {
        return Ok(());
    };
    let me = Me::of(&host);
    leave_unlisted(core, &orgs).await;
    for org in &orgs {
        if let Err(error) = sync_org(core, org, &me).await {
            core.vault()
                .orgs()
                .state
                .locked()
                .orgs
                .entry(org.id.clone())
                .or_default()
                .failed = Some(error.to_string());
            eprintln!("sikemux core: an org vault did not sync: {error}");
        }
    }
    hand_on(core, &orgs);
    keep(core, Some(&account)).await
}

/// Uses no vault of an org that `orgs` leaves out, and lets its key go once the accounts server
/// also refuses the vault, so neither one wrong answer nor a lost list takes it away.
pub(super) async fn leave_unlisted(core: &Core, orgs: &[AccountOrg]) {
    let listed: BTreeSet<&str> = orgs.iter().map(|org| org.id.as_str()).collect();
    let keyed: Vec<String> = {
        let mut state = core.vault().orgs().state.locked();
        state
            .orgs
            .retain(|id, org| listed.contains(id.as_str()) || org.held.generation > 0);
        state.heard.retain(|id| listed.contains(id.as_str()));
        let mut unlisted = Vec::new();
        for (id, org) in &mut state.orgs {
            if listed.contains(id.as_str()) {
                continue;
            }
            org.left = true;
            org.openers = None;
            org.changes = None;
            if org.held.key.is_some() {
                unlisted.push(id.clone());
            }
        }
        unlisted
    };
    for id in keyed {
        match fetch(core, &id, None).await {
            Ok(Fetched::Gone | Fetched::Refused(_)) => {
                let orgs = core.vault().orgs();
                if let Some(org) = orgs.state.locked().orgs.get_mut(&id) {
                    org.held = std::mem::take(&mut org.held).let_go(0);
                    org.status = OrgVaultStatus::Waiting;
                    org.unreadable.clear();
                }
                orgs.changed.notify_one();
            }
            Ok(Fetched::Vault(_)) => {}
            Err(error) => eprintln!(
                "sikemux core: the vault of an org the account is not in now was not read: {error}"
            ),
        }
    }
}

/// The secret store's item of the hosts `account` trusts in `org`.
fn pins_item(account: &str, org: &str) -> String {
    let of = json!([account, org]).to_string();
    format!("org-hosts-{}", hex::encode(Sha256::digest(of.as_bytes())))
}

/// The secret store's copy, moved first from where earlier builds kept it. The first builds' key
/// may have been sealed for any host, so it counts as not vetted whatever its mark says.
fn read_keys(service: &str) -> CoreResult<Option<Keys>> {
    let failed = |error: sikemux_keychain::KeychainError| CoreError::from(error.to_string());
    let read = |item| -> CoreResult<Option<Keys>> {
        Ok(sikemux_keychain::read(service, item)
            .map_err(failed)?
            .and_then(|text| URL_SAFE_NO_PAD.decode(text.trim()).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    };
    let mut keys = read(ITEM)?;
    let moved = keys.is_none();
    if moved {
        keys = read(ITEM_BEFORE)?;
    }
    if keys.is_none() {
        keys = read(ITEM_FIRST)?;
        for key in keys.iter_mut().flat_map(|keys| keys.orgs.values_mut()) {
            key.vetted = false;
        }
    }
    if let Some(keys) = &mut keys {
        for (org, key) in &mut keys.orgs {
            let item = pins_item(&keys.account, org);
            if moved {
                key.pins = key.moving.take().unwrap_or_default();
                sikemux_keychain::write(service, &item, &key.pins.text()?).map_err(failed)?;
                continue;
            }
            let pins = (sikemux_keychain::read(service, &item).map_err(failed)?)
                .and_then(|text| Pins::read(&text));
            // Without the hosts it trusted, the key is read again only from a sealed copy.
            if pins.is_none() {
                key.key = None;
            }
            key.pins = pins.unwrap_or_default();
        }
    }
    if let Some(keys) = keys.as_ref().filter(|_| moved) {
        let text = URL_SAFE_NO_PAD.encode(serde_json::to_vec(keys)?);
        sikemux_keychain::write(service, ITEM, &text).map_err(failed)?;
    }
    for item in [ITEM_BEFORE, ITEM_FIRST] {
        sikemux_keychain::delete(service, item).map_err(failed)?;
    }
    Ok(keys)
}

/// Reads the secret store's copy once per account, dropping one another account left.
async fn load(core: &Core, account: &str) -> CoreResult<()> {
    if core.vault().orgs().state.locked().account.as_deref() == Some(account) {
        return Ok(());
    }
    let data_dir = core.data_dir().clone();
    let copy = blocking(move || {
        let keys = read_keys(SERVICE)?;
        if let Some(data_dir) = &data_dir {
            move_entries(data_dir)?;
        }
        Ok(keys.map(|keys| Copy {
            orgs: (keys.orgs.iter())
                .map(|(org, key)| {
                    let held = read_entries(data_dir.as_deref(), &keys.account, org, key);
                    (org.clone(), held)
                })
                .collect(),
            account: keys.account,
        }))
    })
    .await?;
    let mut state = core.vault().orgs().state.locked();
    *state = State {
        account: Some(account.to_owned()),
        ..State::default()
    };
    if let Some(copy) = &copy {
        if copy.account == account {
            state.orgs = copy
                .orgs
                .iter()
                .map(|(id, held)| {
                    let status = match (held.key, held.generation) {
                        (Some(_), _) => OrgVaultStatus::Open,
                        (None, 0) => OrgVaultStatus::Unmade,
                        (None, _) => OrgVaultStatus::Waiting,
                    };
                    let org = Org {
                        held: held.clone(),
                        status,
                        ..Org::default()
                    };
                    (id.clone(), org)
                })
                .collect();
        }
    }
    state.kept = Some(copy);
    Ok(())
}

/// Writes what this host holds to the secret store when it changed, or deletes the copy when
/// it holds nothing.
async fn keep(core: &Core, account: Option<&str>) -> CoreResult<()> {
    let copy = {
        let state = core.vault().orgs().state.locked();
        account.map(|account| Copy {
            account: account.to_owned(),
            orgs: state
                .orgs
                .iter()
                .filter(|(_, org)| org.held.key.is_some() || org.held.generation > 0)
                .map(|(id, org)| (id.clone(), org.held.clone()))
                .collect(),
        })
    };
    let copy = copy.filter(|copy| !copy.orgs.is_empty());
    if core.vault().orgs().state.locked().kept.as_ref() == Some(&copy) {
        return Ok(());
    }
    let was = core.vault().orgs().state.locked().kept.clone().flatten();
    let text = |copy: Option<&Copy>| {
        copy.map(|copy| serde_json::to_vec(&copy.keys()).map(|bytes| URL_SAFE_NO_PAD.encode(bytes)))
            .transpose()
    };
    let (text, kept_text) = (text(copy.as_ref())?, text(was.as_ref())?);
    let pins = |copy: Option<&Copy>| -> serde_json::Result<BTreeMap<String, String>> {
        (copy.iter())
            .flat_map(|copy| copy.orgs.iter().map(move |org| (&copy.account, org)))
            .map(|(account, (org, held))| Ok((pins_item(account, org), held.pins.text()?)))
            .collect()
    };
    let (pins, kept_pins) = (pins(copy.as_ref())?, pins(was.as_ref())?);
    let data_dir = core.data_dir().clone();
    let now = copy.clone();
    blocking(move || {
        let failed = |error: sikemux_keychain::KeychainError| CoreError::from(error.to_string());
        for (item, text) in &pins {
            if kept_pins.get(item) != Some(text) {
                sikemux_keychain::write(SERVICE, item, text).map_err(failed)?;
            }
        }
        if text != kept_text {
            match &text {
                Some(text) => sikemux_keychain::write(SERVICE, ITEM, text),
                None => sikemux_keychain::delete(SERVICE, ITEM),
            }
            .map_err(failed)?;
        }
        for item in kept_pins.keys().filter(|item| !pins.contains_key(*item)) {
            sikemux_keychain::delete(SERVICE, item).map_err(failed)?;
        }
        let Some(data_dir) = data_dir else {
            return Ok(());
        };
        move_entries(&data_dir)?;
        write_entries(&data_dir, was.as_ref(), now.as_ref())
    })
    .await?;
    core.vault().orgs().state.locked().kept = Some(copy);
    Ok(())
}

/// Hands the model keys of the orgs whose plan is paid to the host's providers, which replace
/// what they held. A lapsed org's keys stay held here and go to no agent.
fn hand_on(core: &Arc<Core>, orgs: &[AccountOrg]) {
    let Some(providers) = core.workspaces().providers() else {
        return;
    };
    let me = core
        .remote()
        .host_key()
        .map(|host| host.public().to_string());
    let keys: Vec<OrgKey> = {
        let state = core.vault().orgs().state.locked();
        orgs.iter()
            .filter(|org| org.active)
            .filter_map(|org| Some((org, state.orgs.get(&org.id).filter(|held| !held.left)?)))
            .flat_map(|(org, held)| {
                model_keys(&held.held)
                    .filter(|(_, key)| {
                        key.limited_to.as_ref().is_none_or(|hosts| {
                            me.as_ref().is_some_and(|me| hosts.contains_key(me))
                        })
                    })
                    .map(|(_, key)| OrgKey {
                        org: ProviderOrg {
                            id: org.id.clone(),
                            name: org.name.clone(),
                        },
                        provider: key.provider,
                        key: key.key,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    providers.set_org_keys(&keys);
    (core.workspace_above().org_keys_changed)(core);
}

fn model_keys(held: &Held) -> impl Iterator<Item = (&String, ModelKey)> {
    held.entries
        .iter()
        .filter(|(_, entry)| entry.kind == MODEL_KEY)
        .filter_map(|(id, entry)| Some((id, serde_json::from_value(entry.body.clone()).ok()?)))
}

pub(super) async fn sync_org(core: &Core, org: &AccountOrg, me: &Me) -> CoreResult<()> {
    take_in(core, org, me).await?;
    let orgs = core.vault().orgs();
    if (orgs.state.locked().heard).insert(org.id.clone()) {
        orgs.changed.notify_one();
    }
    Ok(())
}

/// Why the accounts server refused, in its own words.
fn refusal(said: &[u8]) -> String {
    serde_json::from_slice::<Value>(said)
        .ok()
        .and_then(|said| Some(said.get("error")?.get("message")?.as_str()?.to_owned()))
        .unwrap_or_else(|| String::from_utf8_lossy(said).into_owned())
}

/// The accounts server's answer for one org's vault.
enum Fetched {
    Vault(Option<Box<Remote>>),
    /// The account is in no such org.
    Gone,
    /// The account may not read it, in the server's words.
    Refused(String),
}

/// One org's vault from the accounts server, only what changed after `after` changes when given.
async fn fetch(core: &Core, org: &str, after: Option<i64>) -> CoreResult<Fetched> {
    let path = match after {
        Some(after) => format!("{}?after={after}", vault_path(org)),
        None => vault_path(org),
    };
    let answer = host_send_any(core, Method::GET, &path, None).await?;
    if answer.status() == StatusCode::NOT_FOUND {
        return Ok(Fetched::Gone);
    }
    let status = answer.status();
    let bytes = answer.bytes().await.map_err(|error| {
        CoreError::unreachable(&format!("the answer to {path} broke off"), &error)
    })?;
    if status == StatusCode::FORBIDDEN {
        return Ok(Fetched::Refused(refusal(&bytes)));
    }
    if !status.is_success() {
        return Err(format!(
            "the accounts server answered {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    let read: OrgVaultState = serde_json::from_slice(&bytes)?;
    Ok(Fetched::Vault(read.vault.map(Box::new)))
}

/// All of one org's vault, or None when nobody made it yet.
async fn fetch_whole(core: &Core, org: &AccountOrg) -> CoreResult<Option<Remote>> {
    match fetch(core, &org.id, None).await? {
        Fetched::Vault(vault) => Ok(vault.map(|vault| *vault)),
        Fetched::Gone => Err(not_in(&org.name)),
        Fetched::Refused(why) => Err(why.into()),
    }
}

fn not_in(org: &str) -> CoreError {
    format!("The accounts server says the account is not in {org}").into()
}

/// Reads one org's vault and takes in what it holds: the hosts trusted with its key and the key
/// itself, sealed for each of them that waits and replaced when a host left, and every entry.
async fn take_in(core: &Core, org: &AccountOrg, me: &Me) -> CoreResult<()> {
    let before = (core.vault().orgs().state.locked().orgs.get(&org.id))
        .cloned()
        .unwrap_or_default();
    let mut held = before.held;
    let after = before.changes.filter(|_| held.key.is_some());
    let mut read = fetch(core, &org.id, after).await?;
    if let Fetched::Vault(Some(vault)) = &read {
        if vault.since.is_some() && replaces(org, vault, held.vetted) {
            read = fetch(core, &org.id, None).await?;
        }
    }
    let vault = match read {
        Fetched::Vault(vault) => vault,
        Fetched::Refused(why) => return Err(why.into()),
        Fetched::Gone => {
            if let Some(org) = core.vault().orgs().state.locked().orgs.get_mut(&org.id) {
                org.left = true;
                org.changes = None;
            }
            return Err(not_in(&org.name));
        }
    };
    let Some(vault) = vault else {
        if held.generation > 0 {
            return Err(
                "the accounts server says the org has no vault, yet this host has read it".into(),
            );
        }
        set(core, &org.id, Org::default());
        return Ok(());
    };
    if !held.takes_generation(vault.generation) {
        return Err(format!(
            "the accounts server handed back generation {} of the org's key, older than the {} this host holds",
            vault.generation, held.generation
        )
        .into());
    }
    held.pins.refresh(&org.id, &me.key, &vault, now());
    if let Some(secret) = &held.code {
        held.pins.introduced(&org.id, me, &vault, secret, now());
        held.pins.refresh(&org.id, &me.key, &vault, now());
    }
    let key = match (held.key, &vault.key) {
        (Some(key), _) if held.generation == vault.generation => {
            Some((key, held.vetted, held.proved))
        }
        (_, Some(wrapped)) => {
            let opened = open_wrap(wrapped, &org.id, me, &vault, &held.pins);
            if opened.is_none() {
                eprintln!(
                    "sikemux core: an org's key came from a computer this one was not let in by"
                );
            }
            opened.map(|key| (key, true, false))
        }
        (_, None) => None,
    };
    let openers = (vault.holding.clone()).map(|openers| {
        openers
            .into_iter()
            .filter(|opener| opener.host_key != me.key)
            .collect()
    });
    let remade_at = vault.remade_at.clone();
    let member = (trust::listed(&vault)).and_then(|listed| listed.get(me.key.as_str()).copied());
    let then = match (key.is_some(), manages(org)) {
        (true, true) => "this one can let others in and replace the key",
        (true, false) => "this one can let others in",
        (false, _) => "this one can be let in",
    };
    let full = member.and_then(|member| no_place(org, me, &vault, (&me.key, member), then));
    let Some((key, vetted, proved)) = key else {
        let held = held.let_go(vault.generation);
        let code = (held.code.as_ref())
            .filter(|_| full.is_none())
            .and_then(|secret| Some(trust::code(&org.id, &me.key, member?, secret)));
        set(
            core,
            &org.id,
            Org {
                held,
                status: OrgVaultStatus::Waiting,
                holders: vault.holders.len(),
                openers,
                remade_at,
                code,
                full,
                ..Org::default()
            },
        );
        return Ok(());
    };
    held.code = None;
    held.proved = proved || opens_any(&key, &org.id, &vault);
    keep_pins(core, &org.id, &held.pins);
    // Past the most computers trusted, no other host takes this one's word or a key from it.
    let hands_on = full.is_none();
    if hands_on {
        add_vouches(core, org, me, &vault, &held.pins).await;
    }
    let sealable = sealable(&vault, &held.pins);
    // A rotation reseals every entry, so an answer at a new generation lists them all.
    let partial = vault.since.is_some() && held.generation == vault.generation;
    let mut ended = held.ended_after(&vault, partial);
    let wants = replaces(org, &vault, vetted);
    let replaces = hands_on && wants;
    let mut failed = full.clone().filter(|_| wants);
    if replaces {
        let change = Change {
            org: &org.id,
            me,
            vault: &vault,
            key: &key,
            held: &held,
        };
        match rotate(core, &change, &sealable, ended.clone(), Vec::new()).await {
            Ok(next) => {
                set(
                    core,
                    &org.id,
                    Org {
                        held: Held {
                            made: remade_at.clone(),
                            ..next
                        },
                        status: OrgVaultStatus::Open,
                        holders: vault.holders.len(),
                        openers,
                        remade_at,
                        ..Org::default()
                    },
                );
                core.vault().orgs().wake();
                return Ok(());
            }
            Err(error) => {
                eprintln!("sikemux core: an org vault's key was not replaced: {error}");
                failed = Some(error.to_string());
            }
        }
    } else if vetted && hands_on {
        if let Err(error) = add_keys(core, org, me, &vault, &key, &sealable).await {
            eprintln!("sikemux core: an org vault's key was not handed on: {error}");
        }
    }
    let (mut entries, mut unreadable) = match partial {
        true => (held.entries.clone(), before.unreadable),
        false => Default::default(),
    };
    for gone in vault.ended.iter().filter(|_| partial) {
        entries.remove(&gone.id);
        unreadable.remove(&gone.id);
    }
    let shut = replaces && !(held.proved || vault.entries.is_empty());
    for entry in &vault.entries {
        match open_current(&key, &org.id, &vault, &held, &ended, entry) {
            Some((_, body)) => {
                unreadable.remove(&entry.id);
                entries.insert(
                    entry.id.clone(),
                    Opened {
                        kind: entry.kind.clone(),
                        revision: entry.revision,
                        body,
                    },
                );
            }
            None => {
                eprintln!(
                    "sikemux core: entry {} of an org's vault is older than this host's copy or does not open, so the copy stays",
                    entry.id
                );
                if shut {
                    unreadable.insert(entry.id.clone());
                }
                match held.copy_of(&ended, entry) {
                    Some(kept) => {
                        entries
                            .entry(entry.id.clone())
                            .or_insert_with(|| kept.clone());
                    }
                    None if !is_project(&entry.kind) => {
                        unreadable.insert(entry.id.clone());
                    }
                    None => {}
                }
            }
        }
    }
    ended.retain(|id, _| !entries.contains_key(id));
    set(
        core,
        &org.id,
        Org {
            held: Held {
                generation: vault.generation,
                key: Some(key),
                vetted,
                proved: held.proved,
                entries,
                ended,
                made: remade_at.clone(),
                pins: held.pins,
                code: None,
            },
            status: OrgVaultStatus::Open,
            holders: vault.holders.len(),
            changes: failed.is_none().then_some(vault.changes),
            failed,
            full,
            openers,
            remade_at,
            unreadable,
            ..Org::default()
        },
    );
    Ok(())
}

/// Whether this host replaces the key: a host that held it left, or it is not known to have gone
/// only to hosts let in.
fn replaces(org: &AccountOrg, vault: &Remote, vetted: bool) -> bool {
    manages(org) && (vault.rotate || (org.active && !vetted))
}

fn held(core: &Core, org: &str) -> Held {
    core.vault()
        .orgs()
        .state
        .locked()
        .orgs
        .get(org)
        .map(|org| org.held.clone())
        .unwrap_or_default()
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn keep_from_before(core: &Core, org: &str, key: Key, generation: i64) {
    let held = Held {
        generation,
        key: Some(key),
        ..Held::default()
    };
    let org_vault = Org {
        held,
        status: OrgVaultStatus::Open,
        ..Org::default()
    };
    set(core, org, org_vault);
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn opened(core: &Core, org: &str) -> BTreeMap<String, (i64, Value)> {
    (held(core, org).entries.into_iter())
        .map(|(id, entry)| (id, (entry.revision, entry.body)))
        .collect()
}

pub(super) fn full(core: &Core, org: &str) -> Option<String> {
    (core.vault().orgs().state.locked().orgs.get(org)).and_then(|org| org.full.clone())
}

/// Why `host` can't be let in, and what would change that, when it is past the most computers
/// trusted. Said on `host` itself, or on one its code was pasted on, with what is `then` next.
fn no_place(
    org: &AccountOrg,
    me: &Me,
    vault: &Remote,
    (host, member): (&str, &str),
    then: &str,
) -> Option<String> {
    let placed = trust::placed(vault)?;
    if placed.contains_key(host) {
        return None;
    }
    let earlier = (vault.hosts.iter().flatten())
        .take_while(|listed| listed.host_key != host)
        .filter(|listed| listed.member == member)
        .count();
    let over = (earlier + 1).saturating_sub(trust::COMPUTERS);
    let some = match over {
        0 => return None,
        1 => "one".to_owned(),
        over => over.to_string(),
    };
    let mine = trust::listed(vault)?.get(me.key.as_str()) == Some(&member);
    let (name, most) = (&org.name, trust::COMPUTERS);
    let why = match mine {
        true => format!("yours has {earlier} linked earlier. Once you remove {some} you don't use"),
        false => {
            format!("that member's has {earlier} linked earlier. Once they remove {some} they don't use")
        }
    };
    Some(format!(
        "{name}'s vault opens on at most {most} computers of one account, and {why} at app.sikemux.com, {then}"
    ))
}

fn now() -> i64 {
    epoch_ms().cast_signed()
}

/// Keeps the hosts this host trusts with the org's key, before anything is sealed for them.
fn keep_pins(core: &Core, org: &str, pins: &Pins) {
    if let Some(org) = core.vault().orgs().state.locked().orgs.get_mut(org) {
        org.held.pins = pins.clone();
    }
}

/// The same, in the secret store too, so a core that stops halfway still knows.
async fn store_pins(core: &Core, org: &str, pins: &Pins) -> CoreResult<()> {
    keep_pins(core, org, pins);
    let account = core.vault().orgs().state.locked().account.clone();
    match account {
        Some(account) => keep(core, Some(&account)).await,
        None => Ok(()),
    }
}

/// Opens the key wrapped for this host, taking it only from itself or a holder it trusts, so no
/// server can hand over a key of its own making.
fn open_wrap(wrapped: &str, org: &str, me: &Me, vault: &Remote, pins: &Pins) -> Option<Key> {
    let context = wrap_context(org, vault.generation, &me.key);
    let trusted = (vault.holders.iter().map(String::as_str)).filter(|host| pins.hold(host));
    let holders = std::iter::once(me.key.as_str()).chain(trusted);
    seal::unwrap_from_holder(wrapped, &me.secret, &context, holders)
}

fn set(core: &Core, org: &str, value: Org) {
    let orgs = core.vault().orgs();
    let seen = |org: &Org| (org.held.key.is_some(), org.held.entries.clone());
    let shown = |org: &Org| {
        (
            org.status,
            org.openers.clone(),
            org.remade_at.clone(),
            org.code.clone(),
            org.unreadable.clone(),
            org.failed.clone(),
            org.full.clone(),
            org.left,
        )
    };
    let (now, now_shown) = (seen(&value), shown(&value));
    let before = orgs.state.locked().orgs.insert(org.to_owned(), value);
    if before.as_ref().map(seen) != Some(now) {
        orgs.changed.notify_one();
    }
    if before.as_ref().map(shown) != Some(now_shown) {
        super::announce(core);
    }
}

fn is_project(kind: &str) -> bool {
    [PROJECT_MEMORY, PROJECT_MEMORY_WAITING, PROJECT_MEMORY_DROPS].contains(&kind)
}

/// What `entry` holds, as it was sealed and as read, when it opens under this generation's key
/// and is no older than this host's copy.
fn open_current(
    key: &Key,
    org: &str,
    vault: &Remote,
    held: &Held,
    ended: &BTreeMap<String, i64>,
    entry: &OrgVaultEntry,
) -> Option<(Vec<u8>, Value)> {
    let current =
        entry.generation == vault.generation && held.takes_entry(ended, &entry.id, entry.revision);
    current.then(|| open_entry(key, org, entry)).flatten()
}

fn opens_any(key: &Key, org: &str, vault: &Remote) -> bool {
    (vault.entries.iter())
        .any(|entry| entry.generation == vault.generation && open_entry(key, org, entry).is_some())
}

fn open_entry(key: &Key, org: &str, entry: &OrgVaultEntry) -> Option<(Vec<u8>, Value)> {
    let context = entry_context(
        org,
        &entry.id,
        &entry.kind,
        entry.generation,
        entry.revision,
    );
    let plain = seal::open_bound(key, &context, &entry.sealed)?;
    let body = serde_json::from_slice(&plain).ok()?;
    Some((plain, body))
}

/// Seals `plain` for the write made at `read_at`, which the server stores one revision on.
pub(super) fn seal_entry(
    key: &Key,
    org: &str,
    id: &str,
    kind: &str,
    generation: i64,
    read_at: i64,
    plain: &[u8],
) -> CoreResult<String> {
    let context = entry_context(org, id, kind, generation, read_at + 1);
    seal::seal_bound(key, &context, plain)
        .ok_or_else(|| CoreError::from("an org vault entry could not be sealed"))
}

fn wrap_for(key: &Key, from: &Me, host: &str, context: &str) -> Option<OrgWrappedKey> {
    let public = seal::host_public(host)?;
    Some(OrgWrappedKey {
        host_key: host.to_owned(),
        wrapped: seal::wrap_from(key, &from.secret, &public, context)?,
    })
}

/// The hosts this host may seal the key for besides itself: those it trusts that are listed.
fn sealable(vault: &Remote, pins: &Pins) -> BTreeSet<String> {
    (trust::listed(vault).iter().flatten())
        .map(|(host, _)| (*host).to_owned())
        .filter(|host| pins.hold(host))
        .collect()
}

/// Signs for the hosts this host trusts where the server holds no one's vouch that counts here.
async fn add_vouches(core: &Core, org: &AccountOrg, me: &Me, vault: &Remote, pins: &Pins) {
    if !org.active {
        return;
    }
    let missing = pins.unvouched(&org.id, me, vault, manages(org), now());
    let path = format!("{}/vouches", vault_path(&org.id));
    for vouches in missing.chunks(WRAPS) {
        let put = json!(OrgVaultVouches {
            vouches: vouches.to_vec(),
        });
        if let Err(error) = host_request(core, Method::PUT, &path, Some(put)).await {
            eprintln!(
                "sikemux core: what this host says of an org's other hosts was not kept: {error}"
            );
        }
    }
}

/// Seals the key for each waiting host in `sealable`. A member's host seals it only for its own
/// account's hosts.
async fn add_keys(
    core: &Core,
    org: &AccountOrg,
    me: &Me,
    vault: &Remote,
    key: &Key,
    sealable: &BTreeSet<String>,
) -> CoreResult<()> {
    let Some(listed) = trust::listed(vault) else {
        return Ok(());
    };
    let mine = listed.get(me.key.as_str());
    let keys: Vec<OrgWrappedKey> = (vault.waiting.iter())
        .filter(|host| sealable.contains(*host))
        .filter(|host| manages(org) || listed.get(host.as_str()) == mine)
        .filter_map(|host| {
            let context = wrap_context(&org.id, vault.generation, host);
            wrap_for(key, me, host, &context)
        })
        .collect();
    let path = format!("{}/keys", vault_path(&org.id));
    for keys in keys.chunks(WRAPS) {
        let add = OrgVaultKeysAdd {
            generation: vault.generation,
            keys: keys.to_vec(),
            vouches: None,
            approved_only: Some(true),
        };
        host_request(core, Method::PUT, &path, Some(json!(add))).await?;
    }
    Ok(())
}

/// An org's vault as this host read it and may change it.
struct Change<'a> {
    org: &'a str,
    me: &'a Me,
    vault: &'a Remote,
    key: &'a Key,
    held: &'a Held,
}

/// Replaces the key for this host, those in `sealable`, and one let in with it, sealing every
/// entry again under the new one. Past the most one request takes, the other hosts wait, and the
/// next round seals it for them. A project's entry that does not open goes back to the copy this
/// host holds, or out of the vault when it holds none, so nobody who may write one can keep the
/// key from being replaced. Where its key has opened nothing yet, it may not be the vault's, so it
/// replaces nothing.
async fn rotate(
    core: &Core,
    change: &Change<'_>,
    sealable: &BTreeSet<String>,
    mut ended: BTreeMap<String, i64>,
    let_in: Vec<OrgVaultVouchPut>,
) -> CoreResult<Held> {
    let Change {
        org,
        me,
        vault,
        key,
        held,
    } = *change;
    if !(held.proved || vault.entries.is_empty() || opens_any(key, org, vault)) {
        return Err(CoreError::from(
            "Nothing in the org's vault opens on this computer, so it can't replace the key. Another owner's or admin's computer can",
        ));
    }
    let next = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
    let generation = vault.generation + 1;
    let own = std::iter::once(&me.key);
    let first = let_in.iter().map(|vouch| &vouch.host_key);
    let holders = vault.holders.iter().filter(|host| sealable.contains(*host));
    let mut given = BTreeSet::new();
    let keys = (own.chain(first).chain(holders).chain(sealable))
        .filter(|host| given.insert(*host))
        .filter_map(|host| wrap_for(&next, me, host, &wrap_context(org, generation, host)))
        .take(WRAPS)
        .collect();
    let mut entries = Vec::new();
    let mut opened = BTreeMap::new();
    let mut drops = Vec::new();
    for entry in &vault.entries {
        let seal = |plain: &[u8]| {
            seal_entry(
                &next,
                org,
                &entry.id,
                &entry.kind,
                generation,
                entry.revision,
                plain,
            )
        };
        let resealed = match open_current(key, org, vault, held, &ended, entry) {
            Some((plain, body)) => Some((seal(&plain)?, body)),
            None if is_project(&entry.kind) => match held
                .copy_of(&ended, entry)
                .filter(|kept| kept.kind == entry.kind && held.made == vault.remade_at)
            {
                Some(kept) => Some((seal(&serde_json::to_vec(&kept.body)?)?, kept.body.clone()))
                    .filter(|(sealed, _)| sealed.len() <= SEALED),
                None => None,
            },
            None => {
                return Err(CoreError::from(
                    "One of the org's keys does not open on this computer, or is older than the copy here. The vault's own key is replaced once it is removed",
                ))
            }
        };
        let Some((sealed, body)) = resealed else {
            drops.push(OrgVaultEntryRead {
                id: entry.id.clone(),
                revision: entry.revision,
            });
            continue;
        };
        entries.push(VaultEntryReseal {
            id: entry.id.clone(),
            sealed,
            revision: entry.revision,
        });
        opened.insert(
            entry.id.clone(),
            Opened {
                kind: entry.kind.clone(),
                revision: entry.revision + 1,
                body,
            },
        );
    }
    let gone: Vec<(String, i64)> = (drops.iter())
        .map(|drop| (drop.id.clone(), drop.revision + 1))
        .collect();
    let rotate = OrgVaultRotate {
        from: vault.generation,
        keys,
        entries,
        drops: Some(drops).filter(|drops| !drops.is_empty()),
        approved_only: Some(true),
        vouches: Some(let_in).filter(|vouches| !vouches.is_empty()),
    };
    let path = format!("{}/rotate", vault_path(org));
    host_request(core, Method::POST, &path, Some(json!(rotate))).await?;
    floor::end(&mut ended, gone);
    ended.retain(|id, _| !opened.contains_key(id));
    Ok(Held {
        generation,
        key: Some(next),
        vetted: true,
        proved: true,
        entries: opened,
        ended,
        made: held.made.clone(),
        pins: held.pins.clone(),
        code: None,
    })
}

/// The org and this host.
fn member_of(core: &Core, org_id: &str) -> CoreResult<(AccountOrg, Me)> {
    let org = account_orgs(core)
        .unwrap_or_default()
        .into_iter()
        .find(|org| org.id == org_id)
        .ok_or_else(|| CoreError::from("The account is not in that org"))?;
    let host = core
        .remote()
        .host_key()
        .ok_or_else(|| CoreError::from("this host is not signed in"))?;
    Ok((org, Me::of(&host)))
}

/// The same, while the org's plan is paid.
fn joined(core: &Core, org_id: &str) -> CoreResult<(AccountOrg, Me)> {
    let (org, me) = member_of(core, org_id)?;
    match org.active {
        true => Ok((org, me)),
        false => Err(format!(
            "{}'s Team plan has lapsed, so its vault is frozen",
            org.name
        )
        .into()),
    }
}

/// The same, when this host's account is an owner or admin there.
fn managed(core: &Core, org_id: &str) -> CoreResult<(AccountOrg, Me)> {
    let (org, me) = joined(core, org_id)?;
    require_manager(&org)?;
    Ok((org, me))
}

fn require_manager(org: &AccountOrg) -> CoreResult<()> {
    match manages(org) {
        true => Ok(()),
        false => Err(format!("Only {}'s owners and admins change its vault", org.name).into()),
    }
}

/// Lets in the host that shows `code`: seals the key for it, and vouches for it to every other.
pub async fn let_in(core: &Arc<Core>, params: OrgLetIn) -> CoreResult<OrgLetInAnswer> {
    let _busy = core.vault().orgs().busy.lock().await;
    let (org, me) = joined(core, &params.org)?;
    let answer = let_in_to(core, &org, &me, &params.code).await?;
    if let Err(error) = settle(core).await {
        eprintln!("sikemux core: an org's vault was not read again: {error}");
    }
    Ok(answer)
}

pub(super) async fn let_in_to(
    core: &Core,
    org: &AccountOrg,
    me: &Me,
    code: &str,
) -> CoreResult<OrgLetInAnswer> {
    let code = trust::Code::read(code)
        .ok_or_else(|| CoreError::from("That is not a whole code. Copy it again"))?;
    sync_org(core, org, me).await?;
    let mut held = held(core, &org.id);
    let vault = fetch_whole(core, org).await?;
    let (Some(key), Some(vault)) = (
        held.key,
        vault.filter(|vault| vault.generation == held.generation),
    ) else {
        return Err(format!("This computer does not hold {}'s key", org.name).into());
    };
    if let Some(why) = full(core, &org.id) {
        return Err(why.into());
    }
    if !held.vetted {
        let why = (core.vault().orgs().state.locked().orgs.get(&org.id))
            .and_then(|org| org.failed.clone())
            .map_or_else(String::new, |why| format!(": {why}"));
        return Err(format!(
            "An owner's or admin's computer has to replace {}'s key first{why}",
            org.name
        )
        .into());
    }
    held.pins.refresh(&org.id, &me.key, &vault, now());
    let listed = trust::listed(&vault).unwrap_or_default();
    let (host, member) = listed
        .iter()
        .map(|(host, member)| (*host, *member))
        .find(|(host, member)| code.names(&org.id, host, member))
        .ok_or_else(|| {
            CoreError::from(format!(
                "No computer of {}'s members shows that code now. Copy it again",
                org.name
            ))
        })?;
    if host == me.key {
        return Err("That is this computer's own code".into());
    }
    let mine = listed.get(me.key.as_str()) == Some(&member);
    if !manages(org) && !mine {
        return Err(format!(
            "That computer is another member's. Ask one of {}'s owners or admins to let it in",
            org.name
        )
        .into());
    }
    if let Some(why) = no_place(org, me, &vault, (host, member), "paste the code again") {
        return Err(why.into());
    }
    let placed = trust::placed(&vault).unwrap_or_default();
    if placed.get(host) != Some(&member) || !held.pins.takes(host, member) {
        return Err(format!(
            "{}'s vault is open on as many computers as it can be, of that account or in all. Once some that aren't used are removed at app.sikemux.com, paste the code again",
            org.name
        )
        .into());
    }
    let answer = OrgLetInAnswer {
        computer: (vault.hosts.iter().flatten())
            .find(|listed| listed.host_key == host)
            .and_then(|listed| listed.computer.clone()),
        member: match mine {
            true => None,
            false => member_name(core, &org.id, member).await,
        },
    };
    let sealed = vault.holders.iter().any(|holder| holder == host);
    let before = held.pins.clone();
    let at = held.pins.let_in(host, member, now());
    let introduction = trust::introduce(
        &org.id,
        me,
        &vault,
        &held.pins,
        (host, member),
        at,
        &code.secret,
    )
    .ok_or_else(|| CoreError::from("this host's word could not be signed"))?;
    store_pins(core, &org.id, &held.pins).await?;
    let waits = !sealed;
    let sent = send_let_in(core, org, me, (&vault, &key, &held), introduction, waits).await;
    if let Err(error) = sent {
        store_pins(core, &org.id, &before).await?;
        return Err(error);
    }
    Ok(answer)
}

/// The org's member with that membership, by name.
async fn member_name(core: &Core, org: &str, member: &str) -> Option<String> {
    let registry = crate::server::account::shares::read(core).await?;
    let list = registry.orgs.into_iter().find(|list| list.org == org)?;
    let known = (list.members.into_iter()).find(|known| known.id == member)?;
    Some(known.name)
}

/// Hands the key to the host `introduction` lets in: sealed for it while it waits, else in a
/// replacement, which takes the key back from any host that got it otherwise.
async fn send_let_in(
    core: &Core,
    org: &AccountOrg,
    me: &Me,
    (vault, key, held): (&Remote, &Key, &Held),
    introduction: OrgVaultVouchPut,
    waits: bool,
) -> CoreResult<()> {
    if !waits && manages(org) {
        let sealable = sealable(vault, &held.pins);
        let change = Change {
            org: &org.id,
            me,
            vault,
            key,
            held,
        };
        let ended = held.ended_after(vault, false);
        let next = rotate(core, &change, &sealable, ended, vec![introduction]).await?;
        set(
            core,
            &org.id,
            Org {
                held: next,
                status: OrgVaultStatus::Open,
                holders: vault.holders.len(),
                remade_at: vault.remade_at.clone(),
                ..Org::default()
            },
        );
        return Ok(());
    }
    if !waits {
        let path = format!("{}/vouches", vault_path(&org.id));
        let put = json!(OrgVaultVouches {
            vouches: vec![introduction],
        });
        host_request(core, Method::PUT, &path, Some(put)).await?;
        return Ok(());
    }
    let host = introduction.host_key.as_str();
    let context = wrap_context(&org.id, vault.generation, host);
    let wrap = wrap_for(key, me, host, &context)
        .ok_or_else(|| CoreError::from("the org's key could not be sealed for that computer"))?;
    let add = OrgVaultKeysAdd {
        generation: vault.generation,
        keys: vec![wrap],
        vouches: Some(vec![introduction]),
        approved_only: Some(true),
    };
    let path = format!("{}/keys", vault_path(&org.id));
    host_request(core, Method::PUT, &path, Some(json!(add))).await?;
    Ok(())
}

/// The org's vault open on this host, made first when it has none, for an owner or admin to
/// write to.
async fn writable(core: &Core, org_id: &str) -> CoreResult<(AccountOrg, Held)> {
    let (org, me) = managed(core, org_id)?;
    sync_org(core, &org, &me).await?;
    if org_status(core, org_id) == OrgVaultStatus::Unmade {
        make_vault(core, org_id, &me).await?;
    }
    let held = open_to_writes(core, &org)?;
    Ok((org, held))
}

/// The org's vault while it is open here and may be written to.
pub(super) fn open_to_writes(core: &Core, org: &AccountOrg) -> CoreResult<Held> {
    let held = (core.vault().orgs().state.locked().orgs.get(&org.id))
        .filter(|held| held.status == OrgVaultStatus::Open)
        .cloned()
        .ok_or_else(|| {
            CoreError::from(format!(
                "This computer does not hold {}'s key yet. Paste its code on a computer that does",
                org.name
            ))
        })?;
    match held.unwritable() {
        Some(why) => Err(format!("{}'s vault key has to be replaced first{why}", org.name).into()),
        None => Ok(held.held),
    }
}

/// Makes the org's vault, its first key sealed for this host alone.
pub(super) async fn make_vault(core: &Core, org: &str, me: &Me) -> CoreResult<()> {
    let key = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
    let create = OrgVaultCreate {
        key: wrap_for(&key, me, &me.key, &wrap_context(org, 1, &me.key))
            .map(|wrap| wrap.wrapped)
            .ok_or_else(|| CoreError::from("the org's key could not be wrapped"))?,
        approved_only: Some(true),
    };
    host_request(core, Method::POST, &vault_path(org), Some(json!(create))).await?;
    set(
        core,
        org,
        Org {
            held: Held {
                generation: 1,
                key: Some(key),
                vetted: true,
                proved: true,
                ..Held::default()
            },
            status: OrgVaultStatus::Open,
            holders: 1,
            ..Org::default()
        },
    );
    Ok(())
}

fn org_status(core: &Core, org: &str) -> OrgVaultStatus {
    core.vault()
        .orgs()
        .state
        .locked()
        .orgs
        .get(org)
        .map(|held| held.status)
        .unwrap_or_default()
}

/// After a change: takes the vault in again and hands its keys on at once.
async fn settle(core: &Arc<Core>) -> CoreResult<()> {
    round(core).await?;
    super::announce(core);
    Ok(())
}

pub async fn add_model_key(core: &Arc<Core>, params: OrgModelKeyAdd) -> CoreResult<()> {
    let provider = catalogue()
        .iter()
        .find(|provider| provider.id == params.provider && provider.key.is_some())
        .ok_or_else(|| {
            CoreError::from(format!(
                "{} is not a provider Sikemux keeps keys of",
                params.provider
            ))
        })?;
    let key = params.key.trim();
    if key.is_empty() {
        return Err(format!("Enter {}'s key", provider.label).into());
    }
    let _busy = core.vault().orgs().busy.lock().await;
    let (org, held) = writable(core, &params.org).await?;
    let vault_key = held
        .key
        .ok_or_else(|| CoreError::from("the org's key is not open"))?;
    let found = model_keys(&held)
        .find(|(_, model)| model.provider == params.provider)
        .map(|(id, _)| {
            (
                id.clone(),
                held.entries.get(id).map_or(0, |entry| entry.revision),
            )
        });
    let (id, revision) = match found {
        Some(found) => found,
        None => (new_id()?, 0),
    };
    let body = ModelKey {
        provider: params.provider,
        key: key.to_owned(),
        limited_to: None,
    };
    let put = OrgVaultEntryPut {
        kind: MODEL_KEY.to_owned(),
        generation: held.generation,
        sealed: seal_entry(
            &vault_key,
            &org.id,
            &id,
            MODEL_KEY,
            held.generation,
            revision,
            &serde_json::to_vec(&body)?,
        )?,
        revision,
        checked: None,
    };
    let path = format!("{}/entries/{id}", vault_path(&org.id));
    host_request(core, Method::PUT, &path, Some(json!(put))).await?;
    settle(core).await
}

pub async fn remove_entry(core: &Arc<Core>, params: OrgEntryParams) -> CoreResult<()> {
    let _busy = core.vault().orgs().busy.lock().await;
    let (org, _) = member_of(core, &params.org)?;
    require_manager(&org)?;
    let vault = fetch_whole(core, &org).await?;
    let entry = (vault.iter().flat_map(|vault| &vault.entries))
        .find(|entry| entry.id == params.id)
        .ok_or_else(|| CoreError::from("The org's vault has no such entry"))?;
    let path = format!(
        "{}/entries/{}?revision={}",
        vault_path(&org.id),
        params.id,
        entry.revision
    );
    host_request(core, Method::DELETE, &path, None).await?;
    settle(core).await
}

/// Makes the org's vault again, empty, under a new key only this host holds, when no computer
/// can open it: none holds the key, or none that would let this one in is online.
pub async fn remake(core: &Arc<Core>, params: OrgParams) -> CoreResult<()> {
    let _busy = core.vault().orgs().busy.lock().await;
    let (org, me) = managed(core, &params.org)?;
    sync_org(core, &org, &me).await?;
    let held = core
        .vault()
        .orgs()
        .state
        .locked()
        .orgs
        .get(&org.id)
        .cloned()
        .unwrap_or_default();
    if held.status != OrgVaultStatus::Waiting {
        return Err(format!(
            "This computer can open {}'s vault, so there is nothing to make again",
            org.name
        )
        .into());
    }
    if let Some(why) = held.full {
        return Err(why.into());
    }
    let Some(openers) = &held.openers else {
        return Err(format!(
            "The accounts server does not say yet which computers hold {}'s key, so it cannot make the vault again",
            org.name
        )
        .into());
    };
    if let Some(opener) = openers.iter().find(|opener| opener.online) {
        return Err(format!(
            "{} holds {}'s key and is online now. Paste this computer's code there; make the vault again only once no computer that holds it is left",
            opener.computer, org.name
        )
        .into());
    }
    let key = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
    let generation = held.held.generation + 1;
    let remake = OrgVaultRemake {
        from: held.held.generation,
        key: wrap_for(
            &key,
            &me,
            &me.key,
            &wrap_context(&org.id, generation, &me.key),
        )
        .map(|wrap| wrap.wrapped)
        .ok_or_else(|| CoreError::from("the org's key could not be wrapped"))?,
        approved_only: Some(true),
    };
    let path = format!("{}/remake", vault_path(&org.id));
    let pins = held.held.pins.cleared(now());
    store_pins(core, &org.id, &pins).await?;
    let answer = host_request(core, Method::POST, &path, Some(json!(remake))).await?;
    let made = (answer.json::<OrgVaultState>().await.ok())
        .and_then(|state| state.vault)
        .and_then(|vault| vault.remade_at);
    if made.is_some() {
        set(
            core,
            &org.id,
            Org {
                held: Held {
                    generation,
                    key: Some(key),
                    vetted: true,
                    proved: true,
                    made,
                    pins,
                    ..Held::default()
                },
                status: OrgVaultStatus::Open,
                holders: 1,
                ..Org::default()
            },
        );
    }
    settle(core).await
}

pub fn shown(core: &Core) -> Vec<ShownOrgVault> {
    let orgs = account_orgs(core).unwrap_or_default();
    let state = core.vault().orgs().state.locked();
    orgs.iter()
        .filter(|org| !org.hidden)
        .map(|org| {
            let held = state.orgs.get(&org.id).cloned().unwrap_or_default();
            let model_keys = model_keys(&held.held)
                .filter(|_| !held.left)
                .map(|(id, key)| {
                    let label = catalogue()
                        .iter()
                        .find(|provider| provider.id == key.provider)
                        .map_or_else(|| key.provider.clone(), |provider| provider.label.clone());
                    let route = OrgKey {
                        org: ProviderOrg {
                            id: org.id.clone(),
                            name: org.name.clone(),
                        },
                        provider: key.provider.clone(),
                        key: String::new(),
                    }
                    .route();
                    ShownOrgModelKey {
                        id: id.clone(),
                        provider: key.provider,
                        label,
                        route,
                    }
                })
                .collect();
            let writes = manages(org) && org.active;
            let openers: Vec<ShownOrgOpener> = (held.openers.iter().flatten())
                .filter(|_| held.status == OrgVaultStatus::Waiting)
                .map(|opener| ShownOrgOpener {
                    host: opener.computer.clone(),
                    whose: opener.member.clone().filter(|_| !opener.yours),
                    yours: opener.yours,
                    owner: opener.role == crate::accounts::protocol::OrgRole::Owner,
                    online: opener.online,
                    last_online: (opener.last_seen_at.as_deref())
                        .and_then(crate::accounts::live::parse_rfc3339),
                })
                .collect();
            ShownOrgVault {
                org: org.id.clone(),
                name: org.name.clone(),
                status: held.status,
                holders: held.holders,
                active: org.active,
                writes,
                model_keys,
                unreadable: (held.unreadable.into_iter())
                    .filter(|_| manages(org))
                    .collect(),
                failed: held.failed,
                full: held.full.clone(),
                remakes: writes
                    && held.full.is_none()
                    && held.status == OrgVaultStatus::Waiting
                    && held.openers.is_some()
                    && !openers.iter().any(|opener| opener.online),
                openers,
                remade_at: (held.remade_at.as_deref())
                    .and_then(crate::accounts::live::parse_rfc3339),
                code: held.code,
            }
        })
        .collect()
}

/// A project's store of rules and knowledge in an org's vault, which agents read.
pub const PROJECT_MEMORY: &str = "project-memory";
/// One member's rules for a project, waiting for an owner or admin to keep or drop them; the
/// server lets only that member write them.
pub const PROJECT_MEMORY_WAITING: &str = "project-memory-waiting";
/// The waiting rules of a project that an owner or admin dropped; only they may write it.
pub const PROJECT_MEMORY_DROPS: &str = "project-memory-drops";

/// Where the org's vault keeps `kind` of the project the server knows as `project`, as the
/// server works it out.
pub fn project_entry_id(kind: &str, project: &str) -> String {
    entry_id(&format!("{kind}|{project}"))
}

/// Where the org's vault keeps the waiting rules of `member`, a membership in the org, for the
/// project the server knows as `project`.
pub fn waiting_entry_id(project: &str, member: &str) -> String {
    entry_id(&format!("{PROJECT_MEMORY_WAITING}|{project}|{member}"))
}

fn entry_id(of: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(of.as_bytes());
    hex::encode(digest.into_iter().take(16).collect::<Vec<u8>>())
}

/// An entry of an org's vault as this host last read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub revision: i64,
    pub body: Value,
}

/// What this host holds of one id in an org's vault.
pub struct Reading {
    /// When the vault was last made again, as the accounts server said; an entry read under
    /// another of these came from a vault that is gone.
    pub made: Option<String>,
    /// None when the vault has no such entry.
    pub entry: Option<Read>,
}

/// Entry `id` of `org`'s vault: None while this host holds no key to it.
pub fn read(core: &Core, org: &str, id: &str) -> Option<Reading> {
    let state = core.vault().orgs().state.locked();
    let org = (state.orgs.get(org)).filter(|org| org.held.key.is_some() && !org.left)?;
    Some(Reading {
        made: org.held.made.clone(),
        entry: org.held.entries.get(id).map(|entry| Read {
            revision: entry.revision,
            body: entry.body.clone(),
        }),
    })
}

/// Whether this host holds the key to `org`'s vault, so the org's entries can be read and
/// written; None while it holds one it may not use now, or holds none and the accounts server has
/// not answered for the vault yet.
pub fn holds_key(core: &Core, org: &str) -> Option<bool> {
    let state = core.vault().orgs().state.locked();
    match state.orgs.get(org).filter(|org| org.held.key.is_some()) {
        Some(held) => (!held.left).then_some(true),
        None => state.heard.contains(org).then_some(false),
    }
}

/// Makes `org`'s vault when nobody has yet, from an owner's or admin's host.
pub async fn make(core: &Arc<Core>, org: &str) -> CoreResult<()> {
    if org_status(core, org) != OrgVaultStatus::Unmade {
        return Ok(());
    }
    let _busy = core.vault().orgs().busy.lock().await;
    writable(core, org).await?;
    settle(core).await
}

/// Whether this host's account manages `org`.
pub fn manages_org(core: &Core, org: &str) -> bool {
    account_orgs(core)
        .unwrap_or_default()
        .iter()
        .any(|known| known.id == org && manages(known))
}

pub enum Put {
    /// Written at this revision.
    Written(i64),
    /// Another host wrote it after `revision`; read it again and redo the change.
    Stale,
    /// The server will not take it, saying why in words for the person.
    Refused(String),
}

/// Where an entry was read: its revision (0 for none), and when the vault it came from was last
/// made again.
#[derive(Clone, Debug, PartialEq)]
pub struct At {
    pub revision: i64,
    pub made: Option<String>,
}

/// Writes a project's entry of `kind` over the one read `at`, and reads the vault again. Stale
/// once the vault was made again since, or once any entry in `checked` has moved on from the
/// revision given beside it.
pub async fn put_project(
    core: &Core,
    org: &str,
    kind: &str,
    id: &str,
    at: &At,
    body: &Value,
    checked: &[(String, i64)],
) -> CoreResult<Put> {
    if !is_project(kind) {
        return Err(format!("a project's entry is never of kind {kind}").into());
    }
    let _busy = core.vault().orgs().busy.lock().await;
    let (generation, key, revision) = {
        let state = core.vault().orgs().state.locked();
        let org = (state.orgs.get(org)).filter(|org| !org.left);
        if let Some(why) = org.and_then(Org::unwritable) {
            return Err(format!("the org's key has to be replaced first{why}").into());
        }
        let held = org.map(|org| org.held.clone()).unwrap_or_default();
        if held.made != at.made {
            return Ok(Put::Stale);
        }
        let added_after = (at.revision == 0 && !held.entries.contains_key(id))
            .then(|| held.ended.get(id).copied())
            .flatten();
        (
            held.generation,
            held.key
                .ok_or_else(|| CoreError::from("this host holds no key to the org's vault"))?,
            added_after.unwrap_or(at.revision),
        )
    };
    let put = OrgVaultEntryPut {
        kind: kind.to_owned(),
        generation,
        sealed: seal_entry(
            &key,
            org,
            id,
            kind,
            generation,
            revision,
            &serde_json::to_vec(body)?,
        )?,
        revision,
        checked: (!checked.is_empty()).then(|| {
            (checked.iter())
                .map(|(id, revision)| OrgVaultEntryRead {
                    id: id.clone(),
                    revision: *revision,
                })
                .collect()
        }),
    };
    let path = format!("{}/entries/{id}", vault_path(org));
    let upload = super::super::account::api::Upload {
        bytes: serde_json::to_vec(&put)?,
        content_type: "application/json".into(),
        headers: Vec::new(),
    };
    let answer = host_send_any(core, Method::PUT, &path, Some(upload)).await?;
    let status = answer.status();
    let outcome = if status == StatusCode::CONFLICT {
        Put::Stale
    } else if status.is_success() {
        let written: OrgVaultEntry =
            serde_json::from_slice(&answer.bytes().await.map_err(|error| {
                CoreError::unreachable(&format!("the answer to {path} broke off"), &error)
            })?)?;
        let orgs = core.vault().orgs();
        if let Some(held) = orgs.state.locked().orgs.get_mut(org) {
            held.held.entries.insert(
                id.to_owned(),
                Opened {
                    kind: kind.to_owned(),
                    revision: written.revision,
                    body: body.clone(),
                },
            );
        }
        Put::Written(written.revision)
    } else if status.is_client_error() && !answers_later(status) {
        Put::Refused(refusal(&answer.bytes().await.unwrap_or_default()))
    } else {
        return Err(format!("the accounts server answered {status}").into());
    };
    if let Some(known) = account_orgs(core)
        .unwrap_or_default()
        .into_iter()
        .find(|known| known.id == org)
    {
        if let Some(host) = core.remote().host_key() {
            sync_org(core, &known, &Me::of(&host)).await?;
        }
    }
    Ok(outcome)
}

/// A refusal the server takes back by itself: an expired sign-in, or too many writes at once.
fn answers_later(status: StatusCode) -> bool {
    [
        StatusCode::UNAUTHORIZED,
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_MANY_REQUESTS,
    ]
    .contains(&status)
}

fn new_id() -> CoreResult<String> {
    seal::random_key()
        .map(|bytes| hex::encode(&bytes[..16]))
        .ok_or_else(|| CoreError::from("no randomness for an entry's id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_kept_where_earlier_builds_read_it_moves_with_the_hosts_trusted() {
        let service = format!("sikemux-org-vaults-test-{}", std::process::id());
        let keys = Keys {
            account: "account".to_owned(),
            orgs: BTreeMap::from([(
                "org".to_owned(),
                KeptKey {
                    generation: 2,
                    key: Some([7; 32]),
                    vetted: true,
                    proved: true,
                    made: None,
                    moving: None,
                    pins: Pins::default(),
                    code: None,
                },
            )]),
        };
        let mut pins = Pins::default();
        pins.let_in("host", "member", 1);
        let mut kept = serde_json::to_value(&keys).unwrap();
        kept["orgs"]["org"]["pins"] = serde_json::to_value(&pins).unwrap();
        let text = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&kept).unwrap());
        let first = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&keys).unwrap());
        for (item, text, vetted, pins) in [
            (ITEM_BEFORE, text, true, pins),
            (ITEM_FIRST, first, false, Pins::default()),
        ] {
            sikemux_keychain::write(&service, item, &text).unwrap();
            for _ in 0..2 {
                let moved = read_keys(&service).unwrap().unwrap();
                assert_eq!(moved.orgs["org"].key, Some([7; 32]));
                assert_eq!(moved.orgs["org"].vetted, vetted);
                assert!(moved.orgs["org"].pins == pins);
                assert!(sikemux_keychain::read(&service, item).unwrap().is_none());
            }
            sikemux_keychain::delete(&service, &pins_item("account", "org")).unwrap();
            let without = read_keys(&service).unwrap().unwrap();
            assert_eq!(without.orgs["org"].key, None);
            sikemux_keychain::delete(&service, ITEM).unwrap();
        }
    }
}
