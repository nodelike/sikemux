//! Each org's vault (ADR 0026): what an org's owners and admins keep for every member, sealed
//! under the org's own key, which the accounts server never sees. A member's host gets the key
//! from a host that holds it, keeps its copy in the secret store apart from the account's
//! vault, and lets it go when the account leaves the org or the host leaves the account.

use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeMap, BTreeSet};
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
use super::seal::{self, Key, Seen};

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

/// One org's vault as this host holds it, which its secret store keeps between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Held {
    seen: Seen,
    key: Option<Key>,
    entries: BTreeMap<String, Opened>,
}

/// The copy in the secret store: every org vault this host holds, for one account.
#[derive(Default, PartialEq, Serialize, Deserialize)]
struct Copy {
    account: String,
    orgs: BTreeMap<String, Held>,
}

#[derive(Clone, Default)]
struct Org {
    held: Held,
    status: OrgVaultStatus,
    holders: usize,
    failed: Option<String>,
}

#[derive(Default)]
struct State {
    /// The account whose copy is loaded, once it is.
    account: Option<String>,
    orgs: BTreeMap<String, Org>,
    /// What the secret store holds, as last written or read; None until this run looked.
    kept: Option<Option<Copy>>,
}

#[derive(Default)]
pub struct OrgVaults {
    state: Mutex<State>,
    wake: Arc<Notify>,
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
        if let Err(error) = sync_org(core, org, &me, &secret).await {
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
    let copy = blocking(|| {
        Ok(sikemux_keychain::read(SERVICE, ITEM)
            .map_err(|error| CoreError::from(error.to_string()))?
            .and_then(|text| URL_SAFE_NO_PAD.decode(text.trim()).ok())
            .and_then(|bytes| serde_json::from_slice::<Copy>(&bytes).ok()))
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
    let text = copy
        .as_ref()
        .map(|copy| serde_json::to_vec(copy).map(|bytes| URL_SAFE_NO_PAD.encode(bytes)))
        .transpose()?;
    blocking(move || {
        match text {
            Some(text) => sikemux_keychain::write(SERVICE, ITEM, &text),
            None => sikemux_keychain::delete(SERVICE, ITEM),
        }
        .map_err(|error| CoreError::from(error.to_string()))
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

/// Reads one org's vault and takes in what it holds: opens its key, lets waiting hosts in,
/// replaces the key when a host left, and opens every entry.
async fn sync_org(core: &Core, org: &AccountOrg, me: &str, secret: &Key) -> CoreResult<()> {
    let path = vault_path(&org.id);
    let answer = host_send_any(core, Method::GET, &path, None).await?;
    if answer.status() == StatusCode::NOT_FOUND {
        core.vault().orgs().state.locked().orgs.remove(&org.id);
        return Ok(());
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
    let read: OrgVaultState = serde_json::from_slice(&bytes)?;
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
    if !held.seen.takes_generation(vault.generation) {
        return Err(format!(
            "the accounts server handed back generation {} of the org's key, older than the {} this host holds",
            vault.generation, held.seen.generation
        )
        .into());
    }
    let (key, failed) = match (held.key, &vault.key) {
        (Some(key), _) if held.seen.generation == vault.generation => (Some(key), None),
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
            },
        );
        core.vault().orgs().wake();
        return Ok(());
    }
    if !vault.waiting.is_empty() {
        add_keys(core, &org.id, secret, &vault, &key).await?;
    }
    let mut entries = BTreeMap::new();
    for entry in &vault.entries {
        let opened = (entry.generation == vault.generation
            && held.seen.takes_entry(&entry.id, entry.revision))
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
    let mut seen = held.seen;
    seen.note(
        vault.generation,
        entries
            .iter()
            .map(|(id, entry)| (id.as_str(), entry.revision)),
    );
    set(
        core,
        &org.id,
        Org {
            held: Held {
                seen,
                key: Some(key),
                entries,
            },
            status: OrgVaultStatus::Open,
            holders: vault.holders.len(),
            failed: None,
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
    core.vault()
        .orgs()
        .state
        .locked()
        .orgs
        .insert(org.to_owned(), value);
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
/// letting their hosts in.
async fn add_keys(
    core: &Core,
    org: &str,
    secret: &Key,
    vault: &Remote,
    key: &Key,
) -> CoreResult<()> {
    let keys: Vec<OrgWrappedKey> = vault
        .waiting
        .iter()
        .filter_map(|host| wrap_for(key, secret, org, vault.generation, host))
        .collect();
    let path = format!("{}/keys", vault_path(org));
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
    let mut seen = held(core, org).seen;
    if let Some(stale) = vault.entries.iter().find(|entry| {
        entry.generation != vault.generation || !seen.takes_entry(&entry.id, entry.revision)
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
    seen.note(
        generation,
        opened
            .iter()
            .map(|(id, entry)| (id.as_str(), entry.revision)),
    );
    Ok(Held {
        seen,
        key: Some(next),
        entries: opened,
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
        let mut seen = Seen::default();
        seen.note(1, []);
        set(
            core,
            org_id,
            Org {
                held: Held {
                    seen,
                    key: Some(key),
                    entries: BTreeMap::new(),
                },
                status: OrgVaultStatus::Open,
                holders: 1,
                failed: None,
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
        generation: held.seen.generation,
        sealed: seal_entry(
            &vault_key,
            &org.id,
            &id,
            MODEL_KEY,
            held.seen.generation,
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

fn new_id() -> CoreResult<String> {
    seal::random_key()
        .map(|bytes| hex::encode(&bytes[..16]))
        .ok_or_else(|| CoreError::from("no randomness for an entry's id"))
}
