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
use sikemux_agents::launch::providers::{catalogue, OrgKey, ProviderOrg};
use sikemux_calls::vault::{
    OrgEntryParams, OrgModelKeyAdd, OrgVaultStatus, ShownOrgModelKey, ShownOrgVault,
};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::device::{AccountOrg, OrgRole};
use tokio::sync::Notify;

use crate::accounts::orgvault::{
    OrgVault as Remote, OrgVaultCreate, OrgVaultEntry, OrgVaultEntryPut, OrgVaultKeysAdd,
    OrgVaultRotate, OrgVaultState, OrgWrappedKey,
};
use crate::accounts::vault::VaultEntryReseal;

use super::super::account::api::{host_request, host_send_any};
use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::seal::{self, Key};

const TICK: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(2);
const MODEL_KEY: &str = "model-key";
/// The most hosts one request may seal the key for.
const WRAPS: usize = 500;

const SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-org-vaults-dev"
} else {
    "sikemux-org-vaults"
};
const ITEM: &str = "orgs";
const ENTRIES_FOLDER: &str = "org-vaults";

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
struct Held {
    generation: i64,
    key: Option<Key>,
    entries: BTreeMap<String, Opened>,
    /// The last revision of each entry the vault removed, which a new entry under its id passes.
    ended: BTreeMap<String, i64>,
}

impl Held {
    fn takes_generation(&self, generation: i64) -> bool {
        self.key.is_none() || generation >= self.generation
    }

    fn takes_entry(&self, id: &str, revision: i64) -> bool {
        self.entries
            .get(id)
            .is_none_or(|held| revision >= held.revision)
            && self.ended.get(id).is_none_or(|&ended| revision > ended)
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

#[derive(Serialize, Deserialize)]
struct KeptKey {
    generation: i64,
    key: Key,
}

impl Copy {
    fn keys(&self) -> Keys {
        Keys {
            account: self.account.clone(),
            orgs: (self.orgs.iter())
                .filter_map(|(id, held)| {
                    let key = held.key?;
                    let generation = held.generation;
                    Some((id.clone(), KeptKey { generation, key }))
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

fn entries_context(account: &str, org: &str, generation: i64) -> String {
    format!("sikemux org vault entries\n{account}\n{org}\n{generation}")
}

fn read_entries(data_dir: Option<&Path>, account: &str, org: &str, key: &KeptKey) -> Held {
    let (entries, ended) = data_dir
        .and_then(|dir| std::fs::read(entries_file(dir, org)).ok())
        .and_then(|sealed| {
            let context = entries_context(account, org, key.generation);
            seal::open_bytes(&key.key, &context, &sealed)
        })
        .and_then(|plain| serde_json::from_slice(&plain).ok())
        .unwrap_or_default();
    Held {
        generation: key.generation,
        key: Some(key.key),
        entries,
        ended,
    }
}

/// Writes the entries of each org in `now` that differ from `was`, and deletes the files of
/// orgs no longer held.
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
        if org.is_none_or(|org| held(now, &org).is_none()) {
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
    /// The vault's count of changes when this host last took in its entries, so the next read
    /// asks only for what changed after it.
    changes: Option<i64>,
    /// When the round that last read the org's vault in full began, in ms since the epoch.
    synced: Option<u64>,
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
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

fn wrap_context(org: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-org-vault-key|v1|{org}|{generation}|{holder}")
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
    loop {
        if !first && tokio::time::timeout(TICK, wake.notified()).await.is_ok() {
            tokio::time::sleep(SETTLE).await;
        }
        first = false;
        let Some(core) = core.upgrade() else { return };
        let _busy = core.vault().orgs().busy.lock().await;
        if let Err(error) = round(&core).await {
            eprintln!("sikemux core: the org vaults did not sync: {error}");
        }
    }
}

/// The account's orgs, or None while the core has not read who is signed in.
fn account_orgs(core: &Core) -> Option<Vec<AccountOrg>> {
    let shown = core.account().shown()?;
    Some(sikemux_corekit::accounts::orgs(Some(&shown)).to_vec())
}

async fn round(core: &Arc<Core>) -> CoreResult<()> {
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
    let me = host.public().to_string();
    let secret = seal::host_secret(&host);
    {
        let listed: BTreeSet<&str> = orgs.iter().map(|org| org.id.as_str()).collect();
        let mut state = core.vault().orgs().state.locked();
        state.orgs.retain(|id, _| listed.contains(id.as_str()));
    }
    for org in &orgs {
        let started = epoch_ms();
        let synced = sync_org(core, org, &me, &secret).await;
        if synced.is_ok() {
            let mut state = core.vault().orgs().state.locked();
            if let Some(held) = state.orgs.get_mut(&org.id) {
                held.synced = Some(started);
            }
        }
        if let Err(error) = synced {
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

/// Reads the secret store's copy once per account, dropping one another account left.
async fn load(core: &Core, account: &str) -> CoreResult<()> {
    if core.vault().orgs().state.locked().account.as_deref() == Some(account) {
        return Ok(());
    }
    let data_dir = core.data_dir().clone();
    let copy = blocking(move || {
        let keys = sikemux_keychain::read(SERVICE, ITEM)
            .map_err(|error| CoreError::from(error.to_string()))?
            .and_then(|text| URL_SAFE_NO_PAD.decode(text.trim()).ok())
            .and_then(|bytes| serde_json::from_slice::<Keys>(&bytes).ok());
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
                    let status = if held.key.is_some() {
                        OrgVaultStatus::Open
                    } else {
                        OrgVaultStatus::Unmade
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
                .filter(|(_, org)| org.held.key.is_some())
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
    let data_dir = core.data_dir().clone();
    let now = copy.clone();
    blocking(move || {
        if text != kept_text {
            match &text {
                Some(text) => sikemux_keychain::write(SERVICE, ITEM, text),
                None => sikemux_keychain::delete(SERVICE, ITEM),
            }
            .map_err(|error| CoreError::from(error.to_string()))?;
        }
        match data_dir {
            Some(data_dir) => write_entries(&data_dir, was.as_ref(), now.as_ref()),
            None => Ok(()),
        }
    })
    .await?;
    core.vault().orgs().state.locked().kept = Some(copy);
    Ok(())
}

/// Hands the model keys of the orgs whose plan is paid to the host's providers, which replace
/// what they held. A lapsed org's keys stay held here and go to no agent.
fn hand_on(core: &Core, orgs: &[AccountOrg]) {
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
            .filter_map(|org| Some((org, state.orgs.get(&org.id)?)))
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
}

fn model_keys(held: &Held) -> impl Iterator<Item = (&String, ModelKey)> {
    held.entries
        .iter()
        .filter(|(_, entry)| entry.kind == MODEL_KEY)
        .filter_map(|(id, entry)| Some((id, serde_json::from_value(entry.body.clone()).ok()?)))
}

async fn sync_org(core: &Core, org: &AccountOrg, me: &str, secret: &Key) -> CoreResult<()> {
    take_in(core, org, me, secret).await?;
    let orgs = core.vault().orgs();
    if (orgs.state.locked().heard).insert(org.id.clone()) {
        orgs.changed.notify_one();
    }
    Ok(())
}

/// One org's vault from the accounts server, only what changed after `after` changes when given;
/// None when the account is in no such org.
async fn fetch(core: &Core, org: &str, after: Option<i64>) -> CoreResult<Option<OrgVaultState>> {
    let path = match after {
        Some(after) => format!("{}?after={after}", vault_path(org)),
        None => vault_path(org),
    };
    let answer = host_send_any(core, Method::GET, &path, None).await?;
    if answer.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = answer.status();
    let bytes = answer.bytes().await.map_err(|error| {
        CoreError::unreachable(&format!("the answer to {path} broke off"), &error)
    })?;
    if !status.is_success() {
        return Err(format!(
            "the accounts server answered {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Reads one org's vault and takes in what it holds: opens its key, lets waiting hosts in,
/// replaces the key when a host left, and opens every entry.
async fn take_in(core: &Core, org: &AccountOrg, me: &str, secret: &Key) -> CoreResult<()> {
    let after = (core.vault().orgs().state.locked().orgs.get(&org.id))
        .filter(|held| held.held.key.is_some())
        .and_then(|held| held.changes);
    let mut read = fetch(core, &org.id, after).await?;
    let partial_rotation = (read.as_ref().and_then(|read| read.vault.as_ref()))
        .is_some_and(|vault| vault.since.is_some() && vault.rotate && manages(org));
    if partial_rotation {
        read = fetch(core, &org.id, None).await?;
    }
    let Some(read) = read else {
        core.vault().orgs().state.locked().orgs.remove(&org.id);
        return Ok(());
    };
    let held = held(core, &org.id);
    let Some(vault) = read.vault else {
        if held.key.is_some() {
            return Err(
                "the accounts server says the org has no vault, yet this host holds its key".into(),
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
    let (key, failed) = match (held.key, &vault.key) {
        (Some(key), _) if held.generation == vault.generation => (Some(key), None),
        (_, Some(wrapped)) => match open_wrap(wrapped, secret, &org.id, &vault, me) {
            Some(key) => (Some(key), None),
            None => (
                None,
                Some(
                    "the org's key handed to this host was not sealed by one of its holders"
                        .to_owned(),
                ),
            ),
        },
        (_, None) => (None, None),
    };
    let Some(key) = key else {
        set(
            core,
            &org.id,
            Org {
                held,
                status: OrgVaultStatus::Waiting,
                holders: vault.holders.len(),
                failed,
                changes: None,
                synced: None,
            },
        );
        return Ok(());
    };
    if vault.rotate && manages(org) {
        let next = rotate(core, &org.id, me, secret, &vault, &key).await?;
        set(
            core,
            &org.id,
            Org {
                held: next,
                status: OrgVaultStatus::Open,
                holders: vault.holders.len(),
                failed: None,
                changes: None,
                synced: None,
            },
        );
        core.vault().orgs().wake();
        return Ok(());
    }
    if !vault.waiting.is_empty() {
        add_keys(core, org, secret, &vault, &key).await?;
    }
    // A rotation reseals every entry, so an answer at a new generation lists them all.
    let partial = vault.since.is_some() && held.generation == vault.generation;
    let mut entries = match partial {
        true => held.entries.clone(),
        false => BTreeMap::new(),
    };
    for gone in vault.ended.iter().filter(|_| partial) {
        entries.remove(&gone.id);
    }
    for entry in &vault.entries {
        let opened = (entry.generation == vault.generation
            && held.takes_entry(&entry.id, entry.revision))
        .then(|| open_entry(&key, &org.id, entry))
        .flatten();
        match opened {
            Some(body) => {
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
                if let Some(kept) = held.entries.get(&entry.id) {
                    entries.insert(entry.id.clone(), kept.clone());
                }
            }
        }
    }
    let mut ended = held.ended;
    let unlisted = (held.entries.iter())
        .filter(|(id, _)| !partial && !entries.contains_key(*id))
        .map(|(id, entry)| (id, entry.revision));
    let listed = (vault.ended.iter()).map(|gone| (&gone.id, gone.revision));
    for (id, revision) in unlisted.chain(listed) {
        let floor = ended.entry(id.clone()).or_default();
        *floor = revision.max(*floor);
    }
    ended.retain(|id, _| !entries.contains_key(id));
    set(
        core,
        &org.id,
        Org {
            held: Held {
                generation: vault.generation,
                key: Some(key),
                entries,
                ended,
            },
            status: OrgVaultStatus::Open,
            holders: vault.holders.len(),
            failed: None,
            changes: Some(vault.changes),
            synced: None,
        },
    );
    Ok(())
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

/// Opens the key wrapped for this host, taking it only from itself or a host the vault lists as
/// holding it, so the server cannot hand over a key of its own making.
fn open_wrap(wrapped: &str, secret: &Key, org: &str, vault: &Remote, me: &str) -> Option<Key> {
    let context = wrap_context(org, vault.generation, me);
    let (key, sender) = seal::unwrap_from(wrapped, secret, &context)?;
    std::iter::once(me)
        .chain(vault.holders.iter().map(String::as_str))
        .filter_map(seal::host_public)
        .any(|holder| holder == sender)
        .then_some(key)
}

fn set(core: &Core, org: &str, value: Org) {
    let orgs = core.vault().orgs();
    let seen = |org: &Org| (org.held.key.is_some(), org.held.entries.clone());
    let now = seen(&value);
    let before = orgs.state.locked().orgs.insert(org.to_owned(), value);
    if before.as_ref().map(seen) != Some(now) {
        orgs.changed.notify_one();
    }
}

fn open_entry(key: &Key, org: &str, entry: &OrgVaultEntry) -> Option<Value> {
    let context = entry_context(
        org,
        &entry.id,
        &entry.kind,
        entry.generation,
        entry.revision,
    );
    let plain = seal::open_bound(key, &context, &entry.sealed)?;
    serde_json::from_slice(&plain).ok()
}

/// Seals `body` for the write made at `read_at`, which the server stores one revision on.
fn seal_entry(
    key: &Key,
    org: &str,
    id: &str,
    kind: &str,
    generation: i64,
    read_at: i64,
    body: &Value,
) -> CoreResult<String> {
    let plain = serde_json::to_vec(body)?;
    let context = entry_context(org, id, kind, generation, read_at + 1);
    seal::seal_bound(key, &context, &plain)
        .ok_or_else(|| CoreError::from("an org vault entry could not be sealed"))
}

fn wrap_for(
    key: &Key,
    secret: &Key,
    org: &str,
    generation: i64,
    host: &str,
) -> Option<OrgWrappedKey> {
    let public = seal::host_public(host)?;
    Some(OrgWrappedKey {
        host_key: host.to_owned(),
        wrapped: seal::wrap_from(key, secret, &public, &wrap_context(org, generation, host))?,
    })
}

/// Every host of every member gets the key: the org taking the person in is its admins
/// letting their hosts in. A member's host lets in only its own account's hosts.
async fn add_keys(
    core: &Core,
    org: &AccountOrg,
    secret: &Key,
    vault: &Remote,
    key: &Key,
) -> CoreResult<()> {
    let own = (!manages(org)).then(|| core.vault().state.locked().hosts.clone());
    let keys: Vec<OrgWrappedKey> = vault
        .waiting
        .iter()
        .filter(|host| own.as_ref().is_none_or(|own| own.contains_key(*host)))
        .filter_map(|host| wrap_for(key, secret, &org.id, vault.generation, host))
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let path = format!("{}/keys", vault_path(&org.id));
    for keys in keys.chunks(WRAPS) {
        let add = OrgVaultKeysAdd {
            generation: vault.generation,
            keys: keys.to_vec(),
        };
        host_request(core, Method::PUT, &path, Some(json!(add))).await?;
    }
    Ok(())
}

/// A host that held the key left the org, so the key is replaced for the members' hosts that
/// are left, and every entry sealed again under the new one. Past the most one request takes,
/// the other hosts wait, and the next round lets them in.
async fn rotate(
    core: &Core,
    org: &str,
    me: &str,
    secret: &Key,
    vault: &Remote,
    key: &Key,
) -> CoreResult<Held> {
    let held = held(core, org);
    if let Some(stale) = vault.entries.iter().find(|entry| {
        entry.generation != vault.generation || !held.takes_entry(&entry.id, entry.revision)
    }) {
        return Err(format!(
            "entry {} of the org's vault is older than this host's copy, so its key is not replaced",
            stale.id
        )
        .into());
    }
    let next = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
    let generation = vault.generation + 1;
    let mut hosts: Vec<&str> = Vec::new();
    for host in std::iter::once(me).chain(
        vault
            .holders
            .iter()
            .chain(&vault.waiting)
            .map(String::as_str),
    ) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    let keys = hosts
        .iter()
        .filter_map(|host| wrap_for(&next, secret, org, generation, host))
        .take(WRAPS)
        .collect();
    let mut entries = Vec::new();
    let mut opened = BTreeMap::new();
    for entry in &vault.entries {
        let body = open_entry(key, org, entry).ok_or_else(|| {
            CoreError::from(format!("org vault entry {} does not open", entry.id))
        })?;
        entries.push(VaultEntryReseal {
            id: entry.id.clone(),
            sealed: seal_entry(
                &next,
                org,
                &entry.id,
                &entry.kind,
                generation,
                entry.revision,
                &body,
            )?,
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
    let rotate = OrgVaultRotate {
        from: vault.generation,
        keys,
        entries,
    };
    let path = format!("{}/rotate", vault_path(org));
    host_request(core, Method::POST, &path, Some(json!(rotate))).await?;
    Ok(Held {
        generation,
        key: Some(next),
        entries: opened,
        ended: held.ended,
    })
}

/// The org's vault open on this host, made first when it has none, for an owner or admin to
/// write to.
async fn writable(core: &Core, org_id: &str) -> CoreResult<(AccountOrg, Held)> {
    let org = account_orgs(core)
        .unwrap_or_default()
        .into_iter()
        .find(|org| org.id == org_id)
        .ok_or_else(|| CoreError::from("The account is not in that org"))?;
    if !manages(&org) {
        return Err(format!("Only {}'s owners and admins change its vault", org.name).into());
    }
    if !org.active {
        return Err(format!(
            "{}'s Team plan has lapsed, so its vault is frozen",
            org.name
        )
        .into());
    }
    let host = core
        .remote()
        .host_key()
        .ok_or_else(|| CoreError::from("this host is not signed in"))?;
    let me = host.public().to_string();
    let secret = seal::host_secret(&host);
    sync_org(core, &org, &me, &secret).await?;
    let status = org_status(core, org_id);
    if status == OrgVaultStatus::Unmade {
        let key = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
        let create = OrgVaultCreate {
            key: seal::wrap_from(
                &key,
                &secret,
                &seal::public_of(&secret),
                &wrap_context(org_id, 1, &me),
            )
            .ok_or_else(|| CoreError::from("the org's key could not be wrapped"))?,
        };
        host_request(core, Method::POST, &vault_path(org_id), Some(json!(create))).await?;
        set(
            core,
            org_id,
            Org {
                held: Held {
                    generation: 1,
                    key: Some(key),
                    entries: BTreeMap::new(),
                    ended: BTreeMap::new(),
                },
                status: OrgVaultStatus::Open,
                holders: 1,
                failed: None,
                changes: None,
                synced: None,
            },
        );
    }
    let held = core
        .vault()
        .orgs()
        .state
        .locked()
        .orgs
        .get(org_id)
        .filter(|held| held.status == OrgVaultStatus::Open)
        .map(|held| held.held.clone())
        .ok_or_else(|| {
            CoreError::from(format!(
                "This computer does not hold {}'s key yet; another of its admins' computers lets it in when it next runs",
                org.name
            ))
        })?;
    Ok((org, held))
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
    let body = serde_json::to_value(ModelKey {
        provider: params.provider,
        key: key.to_owned(),
        limited_to: None,
    })?;
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
            &body,
        )?,
        revision,
    };
    let path = format!("{}/entries/{id}", vault_path(&org.id));
    host_request(core, Method::PUT, &path, Some(json!(put))).await?;
    settle(core).await
}

pub async fn remove_entry(core: &Arc<Core>, params: OrgEntryParams) -> CoreResult<()> {
    let _busy = core.vault().orgs().busy.lock().await;
    let (org, held) = writable(core, &params.org).await?;
    let entry = held
        .entries
        .get(&params.id)
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

pub fn shown(core: &Core) -> Vec<ShownOrgVault> {
    let orgs = account_orgs(core).unwrap_or_default();
    let state = core.vault().orgs().state.locked();
    orgs.iter()
        .map(|org| {
            let held = state.orgs.get(&org.id).cloned().unwrap_or_default();
            let model_keys = model_keys(&held.held)
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
            ShownOrgVault {
                org: org.id.clone(),
                name: org.name.clone(),
                status: held.status,
                holders: held.holders,
                active: org.active,
                writes: manages(org) && org.active,
                model_keys,
                failed: held.failed,
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

/// Entry `id` of `org`'s vault: None while this host holds no key to it, Some(None) when the
/// vault has no such entry.
pub fn read(core: &Core, org: &str, id: &str) -> Option<Option<Read>> {
    let state = core.vault().orgs().state.locked();
    let org = state.orgs.get(org).filter(|org| org.held.key.is_some())?;
    Some(org.held.entries.get(id).map(|entry| Read {
        revision: entry.revision,
        body: entry.body.clone(),
    }))
}

/// When the round that last read `org`'s vault began, in ms since the epoch: what this host
/// reads of the vault is no older than that.
pub fn synced(core: &Core, org: &str) -> Option<u64> {
    let state = core.vault().orgs().state.locked();
    state.orgs.get(org)?.synced
}

/// Whether this host holds the key to `org`'s vault, so the org's entries can be read and
/// written; None while it holds none and the accounts server has not answered for the vault yet.
pub fn holds_key(core: &Core, org: &str) -> Option<bool> {
    let state = core.vault().orgs().state.locked();
    match (state.orgs.get(org)).is_some_and(|org| org.held.key.is_some()) {
        true => Some(true),
        false => state.heard.contains(org).then_some(false),
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

/// Writes a project's entry of `kind` over the one at `revision` (0 for none), and reads the
/// vault again.
pub async fn put_project(
    core: &Core,
    org: &str,
    kind: &str,
    id: &str,
    revision: i64,
    body: &Value,
) -> CoreResult<Put> {
    if ![PROJECT_MEMORY, PROJECT_MEMORY_WAITING, PROJECT_MEMORY_DROPS].contains(&kind) {
        return Err(format!("a project's entry is never of kind {kind}").into());
    }
    let _busy = core.vault().orgs().busy.lock().await;
    let (generation, key, revision) = {
        let state = core.vault().orgs().state.locked();
        let held = state
            .orgs
            .get(org)
            .map(|org| org.held.clone())
            .unwrap_or_default();
        let added_after = (revision == 0 && !held.entries.contains_key(id))
            .then(|| held.ended.get(id).copied())
            .flatten();
        (
            held.generation,
            held.key
                .ok_or_else(|| CoreError::from("this host holds no key to the org's vault"))?,
            added_after.unwrap_or(revision),
        )
    };
    let put = OrgVaultEntryPut {
        kind: kind.to_owned(),
        generation,
        sealed: seal_entry(&key, org, id, kind, generation, revision, body)?,
        revision,
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
        let said = answer.bytes().await.unwrap_or_default();
        let why = serde_json::from_slice::<Value>(&said)
            .ok()
            .and_then(|said| Some(said.get("error")?.get("message")?.as_str()?.to_owned()))
            .unwrap_or_else(|| String::from_utf8_lossy(&said).into_owned());
        Put::Refused(why)
    } else {
        return Err(format!("the accounts server answered {status}").into());
    };
    if let Some(known) = account_orgs(core)
        .unwrap_or_default()
        .into_iter()
        .find(|known| known.id == org)
    {
        if let Some(host) = core.remote().host_key() {
            let me = host.public().to_string();
            sync_org(core, &known, &me, &seal::host_secret(&host)).await?;
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
