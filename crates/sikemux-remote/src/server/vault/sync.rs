//! Keeps this host's copy and the account's vault in step: makes the vault when the account
//! has none, wraps its key for hosts that join, replaces it when one leaves, moves this host's
//! sign-ins in, writes back what changed here and takes in what changed elsewhere.

use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde_json::json;
use sikemux_plugin_api::vault::{Book, SignIn};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::accounts::protocol::DeviceList;
use crate::accounts::vault::{
    Vault as Remote, VaultCreate, VaultEntry, VaultEntryPut, VaultEntryReseal, VaultKeysAdd,
    VaultRotate, VaultState, VaultUse, VaultUsesAdd, WrappedKey,
};

use super::super::account::api::{as_host, host_request};
use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::local::{self, Applied, Local, Mark, Marks};
use super::seal::{self, Key};
use super::{announce, Body, Entry, State, Status, Tally};

/// How often the vault is read again with nothing to wake it, its uses sent and the access
/// tokens this host hands on renewed.
const TICK: Duration = Duration::from_secs(60);
/// A burst of changes, such as a sign-in writing several secrets, is taken in one go.
const SETTLE: Duration = Duration::from_secs(2);
/// An access token handed on is renewed this long before it runs out.
const RENEW_AHEAD_MS: u64 = 10 * 60 * 1000;
const RENEW_RETRY_MS: u64 = 60 * 60 * 1000;
const RECOVERY_HOLDER: &str = "recovery";

pub async fn run(core: Weak<Core>) {
    let Some(wake) = core.upgrade().map(|core| core.vault().wake.clone()) else {
        return;
    };
    if let Some(core) = core.upgrade() {
        keep_to_rules(&core).await;
    }
    loop {
        if tokio::time::timeout(TICK, wake.notified()).await.is_ok() {
            tokio::time::sleep(SETTLE).await;
        }
        let Some(core) = core.upgrade() else { return };
        match round(&core).await {
            Ok(()) => core.vault().state.locked().failures = 0,
            Err(error) => failed(&core, &error),
        }
    }
}

/// Rounds that may fail before Settings says so: one lost to the network for a moment, as
/// when the Mac wakes, comes right on the next.
const FAILURES_SHOWN: u32 = 3;

fn failed(core: &Core, error: &CoreError) {
    eprintln!("sikemux core: the account vault did not sync: {error}");
    let changed = {
        let mut state = core.vault().state.locked();
        state.failures = state.failures.saturating_add(1);
        let said = if error.is_unreachable() {
            "Can't reach Sikemux's server. Trying again."
        } else {
            "The account vault could not sync. Trying again."
        };
        let failed = Status::Failed(said.to_owned());
        let shown = state.failures >= FAILURES_SHOWN
            && state.status != failed
            && state.status != Status::Waiting;
        if shown {
            state.status = failed;
        }
        shown
    };
    if changed {
        announce(core);
    }
}

fn books(core: &Core) -> Vec<(String, PathBuf, &'static dyn Book)> {
    (core.remote_above().vault_books)(core)
}

fn rfc3339(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|at| at.format(&Rfc3339).ok())
        .unwrap_or_default()
}

fn new_id() -> CoreResult<String> {
    seal::random_key()
        .map(|bytes| hex::encode(&bytes[..16]))
        .ok_or_else(|| CoreError::from("no randomness for a vault entry's id"))
}

async fn round(core: &Arc<Core>) -> CoreResult<()> {
    let (Some(account), Some(host), true) = (
        core.remote().owner(),
        core.remote().host_key(),
        core.remote().signed_in_api().is_some(),
    ) else {
        let mut state = core.vault().state.locked();
        if state.account.is_some() || state.status != Status::Off {
            *state = State::default();
            drop(state);
            announce(core);
        }
        return Ok(());
    };
    let me = host.public().to_string();
    let moved = core.vault().state.locked().account.as_deref() != Some(account.as_str());
    if moved {
        let reading = account.clone();
        let data_dir = core.data_dir().clone();
        let (recovery_text, kept, rules) = blocking(move || {
            let kept = local::kept_uses::<KeptUse>(data_dir.as_deref())
                .filter(|kept| kept.account == reading)
                .map(|kept| kept.uses)
                .unwrap_or_default();
            let rules = local::read(data_dir.as_deref())
                .filter(|local| local.account == reading)
                .map(|local| local.rules)
                .unwrap_or_default();
            Ok((local::recovery(&reading), kept, rules))
        })
        .await?;
        *core.vault().state.locked() = State {
            account: Some(account.clone()),
            me: Some(me.clone()),
            rules,
            recovery_text,
            opened_at: epoch_ms(),
            uses: kept
                .into_iter()
                .map(|kept| ((kept.entry, kept.actor), kept.tally))
                .collect(),
            ..State::default()
        };
    }
    let secret = seal::host_secret(&host);
    let Some(key) = open(core, &account, &me, &secret).await? else {
        return Ok(());
    };
    reconcile(core, &account, &me, key).await?;
    send_uses(core).await;
    renew_due(core, &me).await;
    Ok(())
}

/// Reads the vault and opens its key, making the vault first when the account has none.
/// None while this host holds no key, or after it replaced the key, which the next round reads.
async fn open(
    core: &Arc<Core>,
    account: &str,
    me: &str,
    secret: &Key,
) -> CoreResult<Option<(Key, Remote)>> {
    let read: VaultState = as_host(core, Method::GET, "/v1/vault", None).await?;
    let vault = match read.vault {
        Some(vault) => vault,
        None => {
            create(core, account, me, secret).await?;
            let read: VaultState = as_host(core, Method::GET, "/v1/vault", None).await?;
            read.vault
                .ok_or_else(|| CoreError::from("the account has no vault after making one"))?
        }
    };
    check_recovery_key(core, account, &vault.recovery_key).await?;
    check_floor(core, account, &vault).await?;
    let listed: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    let context = seal::wrap_context(account, vault.generation, me);
    let key = vault
        .key
        .as_deref()
        .and_then(|wrapped| seal::unwrap(wrapped, secret, &context));
    let changed = {
        let mut state = core.vault().state.locked();
        let before = (
            state.status.clone(),
            state.holders.clone(),
            state.waiting.clone(),
        );
        state.generation = vault.generation;
        state.recovery_key = Some(vault.recovery_key.clone());
        state.recovery_wrap = Some(vault.recovery_wrap.clone());
        state.holders = vault.holders.clone();
        state.waiting = vault.waiting.clone();
        state.hosts = listed
            .devices
            .into_iter()
            .map(|host| (host.key, host.name))
            .collect();
        state.key = key;
        state.status = if key.is_some() {
            Status::Open
        } else {
            Status::Waiting
        };
        before
            != (
                state.status.clone(),
                state.holders.clone(),
                state.waiting.clone(),
            )
    };
    if changed {
        announce(core);
    }
    let Some(key) = key else { return Ok(None) };
    if vault.rotate {
        rotate(core, account, me, &vault, &key).await?;
        core.vault().wake();
        return Ok(None);
    }
    if !vault.waiting.is_empty() {
        add_keys(core, account, &vault, &key).await?;
    }
    Ok(Some((key, vault)))
}

/// Holds the server to the recovery key this host first saw the vault with, since it wraps the
/// vault key for that key whenever it replaces it. Also drops a kept recovery key that is not
/// this vault's, as a host that lost the race to make the vault keeps.
async fn check_recovery_key(core: &Core, account: &str, recovery_key: &str) -> CoreResult<()> {
    let data_dir = core.data_dir().clone();
    let pinned = blocking(move || Ok(local::read(data_dir.as_deref())))
        .await?
        .filter(|local| local.account == account && !local.vault.is_empty())
        .map(|local| local.vault);
    if pinned.is_some_and(|pinned| pinned != recovery_key) {
        return Err(CoreError::from(
            "the accounts server sent a vault recovery key this host has not seen before, so it does not trust it",
        ));
    }
    let stale = core
        .vault()
        .state
        .locked()
        .recovery_text
        .as_deref()
        .is_some_and(|text| {
            seal::recovery_secret(text).is_none_or(|secret| {
                hex::encode(seal::public_of(&secret).as_bytes()) != recovery_key
            })
        });
    if stale {
        core.vault().state.locked().recovery_text = None;
        let account = account.to_owned();
        blocking(move || local::forget_recovery(&account).map_err(CoreError::from)).await?;
        super::inbox_changed(core);
    }
    Ok(())
}

/// Refuses a vault older than this host last read it: an earlier generation of its key, or an
/// entry at an earlier revision or that it saw removed, so nothing it holds is taken back.
async fn check_floor(core: &Core, account: &str, vault: &Remote) -> CoreResult<()> {
    let data_dir = core.data_dir().clone();
    let (account, recovery_key) = (account.to_owned(), vault.recovery_key.clone());
    let floor = blocking(move || Ok(local::load(data_dir.as_deref(), &account, &recovery_key)))
        .await?
        .floor;
    if !floor.takes_generation(vault.generation) {
        return Err(format!(
            "the accounts server handed back generation {} of the vault's key, older than the {} this host read",
            vault.generation, floor.generation
        )
        .into());
    }
    let stale = (vault.entries.iter())
        .filter(|entry| entry.generation == vault.generation)
        .find(|entry| !floor.takes_entry(&entry.id, entry.revision));
    if let Some(stale) = stale {
        return Err(format!(
            "the accounts server handed back vault entry {} at revision {}, older than this host's copy",
            stale.id, stale.revision
        )
        .into());
    }
    if let Some(missing) = floor.missing(&listed(vault), &ended(vault)) {
        return Err(format!(
            "the accounts server left vault entry {missing} out without saying it was removed"
        )
        .into());
    }
    Ok(())
}

fn listed(vault: &Remote) -> BTreeMap<&str, i64> {
    let mut listed = BTreeMap::new();
    for entry in &vault.entries {
        let revision = listed.entry(entry.id.as_str()).or_default();
        *revision = entry.revision.max(*revision);
    }
    listed
}

fn ended(vault: &Remote) -> BTreeMap<&str, i64> {
    (vault.ended.iter().flatten())
        .map(|gone| (gone.id.as_str(), gone.revision))
        .collect()
}

async fn create(core: &Arc<Core>, account: &str, me: &str, secret: &Key) -> CoreResult<()> {
    let no_random = || CoreError::from("no randomness for the vault's keys");
    let key = seal::random_key().ok_or_else(no_random)?;
    let recovery = seal::random_key().ok_or_else(no_random)?;
    let recovery_public = seal::public_of(&recovery);
    let wrap_failed = || CoreError::from("the vault's key could not be wrapped");
    let create = VaultCreate {
        recovery_key: hex::encode(recovery_public.as_bytes()),
        recovery_wrap: seal::wrap(
            &key,
            &recovery_public,
            &seal::wrap_context(account, 1, RECOVERY_HOLDER),
        )
        .ok_or_else(wrap_failed)?,
        key: seal::wrap(
            &key,
            &seal::public_of(secret),
            &seal::wrap_context(account, 1, me),
        )
        .ok_or_else(wrap_failed)?,
    };
    let text = seal::recovery_text(&recovery);
    let keeping = (account.to_owned(), text.clone());
    blocking(move || local::keep_recovery(&keeping.0, &keeping.1).map_err(CoreError::from)).await?;
    host_request(core, Method::POST, "/v1/vault", Some(json!(create))).await?;
    core.vault().state.locked().recovery_text = Some(text);
    super::inbox_changed(core);
    Ok(())
}

fn wrap_for(key: &Key, account: &str, generation: i64, host: &str) -> Option<WrappedKey> {
    let public = seal::host_public(host)?;
    Some(WrappedKey {
        host_key: host.to_owned(),
        wrapped: seal::wrap(key, &public, &seal::wrap_context(account, generation, host))?,
    })
}

/// Every host the account adds gets the key: signing a host in to the account is the person
/// letting it in (ADR 0018), and it uses every entry from then on.
async fn add_keys(core: &Arc<Core>, account: &str, vault: &Remote, key: &Key) -> CoreResult<()> {
    let keys: Vec<WrappedKey> = vault
        .waiting
        .iter()
        .filter_map(|host| wrap_for(key, account, vault.generation, host))
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let add = VaultKeysAdd {
        generation: vault.generation,
        keys,
    };
    host_request(core, Method::PUT, "/v1/vault/keys", Some(json!(add))).await?;
    super::inbox_changed(core);
    Ok(())
}

/// A host left, so the key it held is replaced. The entries it refreshed pass to this host.
async fn rotate(
    core: &Arc<Core>,
    account: &str,
    me: &str,
    vault: &Remote,
    key: &Key,
) -> CoreResult<()> {
    let next = seal::random_key().ok_or_else(|| CoreError::from("no randomness for the key"))?;
    let generation = vault.generation + 1;
    let hosts: BTreeSet<&String> = vault.holders.iter().chain(&vault.waiting).collect();
    let keys = hosts
        .iter()
        .filter_map(|host| wrap_for(&next, account, generation, host))
        .collect();
    let recovery = seal::recovery_public(&vault.recovery_key)
        .and_then(|public| {
            seal::wrap(
                &next,
                &public,
                &seal::wrap_context(account, generation, RECOVERY_HOLDER),
            )
        })
        .ok_or_else(|| CoreError::from("the vault's key could not be wrapped for recovery"))?;
    let mut entries = Vec::new();
    for entry in &vault.entries {
        let mut body = open_body(key, entry)
            .ok_or_else(|| CoreError::from(format!("vault entry {} does not open", entry.id)))?;
        if !hosts.contains(&body.refresher) {
            body.refresher = me.to_owned();
            body.shared = None;
        }
        entries.push(VaultEntryReseal {
            id: entry.id.clone(),
            sealed: seal_body(&next, &entry.id, generation, entry.revision, &body)?,
            revision: entry.revision,
        });
    }
    let rotate = VaultRotate {
        from: vault.generation,
        recovery_wrap: recovery,
        keys,
        entries,
    };
    host_request(core, Method::POST, "/v1/vault/rotate", Some(json!(rotate))).await?;
    Ok(())
}

fn open_body(key: &Key, entry: &VaultEntry) -> Option<Body> {
    let place = seal::Place {
        id: &entry.id,
        generation: entry.generation,
        revision: entry.revision,
    };
    let plain = seal::open_entry(key, place, &entry.sealed)?;
    serde_json::from_slice(&plain).ok()
}

/// Seals a body for the write made at `read_at`, which the server stores one revision on.
fn seal_body(
    key: &Key,
    id: &str,
    generation: i64,
    read_at: i64,
    body: &Body,
) -> CoreResult<String> {
    let plain = serde_json::to_vec(body)?;
    let place = seal::Place {
        id,
        generation,
        revision: read_at + 1,
    };
    seal::seal_entry(key, place, &plain)
        .ok_or_else(|| CoreError::from("a vault entry could not be sealed"))
}

/// A write to the vault this host decided on.
struct Write {
    id: String,
    body: Body,
    /// The revision it was read at; 0 makes the entry.
    revision: i64,
    /// What this host's store holds of it, when the write carries this host's own sign-in.
    holds: Option<String>,
}

struct Plan {
    writes: Vec<Write>,
    deletes: Vec<(String, i64)>,
    local: Local,
    /// What is decided for each sign-in this host holds.
    seen: Vec<Mark>,
}

fn newest_change(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|file| file.metadata().ok()?.modified().ok())
        .filter_map(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|at| u64::try_from(at.as_millis()).unwrap_or(u64::MAX))
        .max()
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |at| u64::try_from(at.as_millis()).unwrap_or(0))
        })
}

/// The entry with this host's copy of the sign-in. A new token makes this host the one that
/// refreshes it; a change to the plugin's settings alone does not.
fn owned_body(body: &Body, sign_in: SignIn, me: &str, written_at: u64) -> Body {
    let mut body = body.clone();
    body.limited_to = None;
    let new_token = body.sign_in.secrets != sign_in.secrets;
    body.sign_in = sign_in;
    if new_token && body.refresher != me {
        body.refresher = me.to_owned();
        body.shared = None;
    }
    body.written_at = written_at;
    body
}

/// What changed in this host's store since it last matched the vault, as writes to the vault.
fn plan(
    books: &[(String, PathBuf, &'static dyn Book)],
    entries: &BTreeMap<String, Entry>,
    mut local: Local,
    marks: &Marks,
    me: &str,
) -> CoreResult<Plan> {
    let moving_in = !local.moved_in;
    let now = epoch_ms();
    let mut writes = Vec::new();
    let mut deletes = Vec::new();
    let mut seen = Vec::new();
    for (plugin, dir, book) in books {
        let here = book.sign_ins(dir).map_err(CoreError::from)?;
        let made_at = newest_change(dir);
        for sign_in in &here {
            let digest = local::digest(sign_in);
            let found = entries.iter().find(|(_, entry)| {
                entry.body.plugin == *plugin && entry.body.sign_in.identity == sign_in.identity
            });
            let kept = match marks.of(plugin, &sign_in.identity) {
                Some(mark) => mark.kept,
                None => {
                    let new_here = found.is_none_or(|(id, _)| !local.applied.contains_key(id));
                    new_here && (sign_in.starts_kept || marks.keep_new)
                }
            };
            seen.push(Mark {
                plugin: plugin.clone(),
                identity: sign_in.identity.clone(),
                label: sign_in.label.clone(),
                kept,
            });
            if kept {
                local.applied.retain(|_, applied| {
                    applied.plugin != *plugin || applied.identity != sign_in.identity
                });
                if let Some((id, entry)) = found {
                    deletes.push((id.clone(), entry.revision));
                }
                continue;
            }
            match found {
                Some((id, entry)) => match local.applied.get(id) {
                    Some(applied) if applied.digest == digest => {}
                    Some(_) => writes.push(Write {
                        id: id.clone(),
                        body: owned_body(&entry.body, sign_in.clone(), me, now),
                        revision: entry.revision,
                        holds: Some(digest),
                    }),
                    None if !moving_in || made_at > entry.body.written_at => {
                        let written_at = if moving_in { made_at } else { now };
                        writes.push(Write {
                            id: id.clone(),
                            body: owned_body(&entry.body, sign_in.clone(), me, written_at),
                            revision: entry.revision,
                            holds: Some(digest),
                        });
                    }
                    None => {}
                },
                None => {
                    let removed_elsewhere = local.applied.iter().any(|(id, applied)| {
                        applied.plugin == *plugin
                            && applied.identity == sign_in.identity
                            && !entries.contains_key(id)
                    });
                    if removed_elsewhere {
                        continue;
                    }
                    writes.push(Write {
                        id: new_id()?,
                        body: Body {
                            plugin: plugin.clone(),
                            sign_in: sign_in.clone(),
                            origin: me.to_owned(),
                            refresher: me.to_owned(),
                            shared: None,
                            written_at: if moving_in { made_at } else { now },
                            limited_to: None,
                        },
                        revision: 0,
                        holds: Some(digest),
                    });
                }
            }
        }
        let signed_out: Vec<String> = local
            .applied
            .iter()
            .filter(|(_, applied)| {
                applied.plugin == *plugin
                    && !here
                        .iter()
                        .any(|sign_in| sign_in.identity == applied.identity)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in signed_out {
            local.applied.remove(&id);
            if let Some(entry) = entries.get(&id) {
                deletes.push((id, entry.revision));
            }
        }
    }
    for (id, entry) in entries {
        let touched = writes.iter().any(|write| write.id == *id)
            || deletes.iter().any(|(deleted, _)| deleted == id);
        if entry.body.limited_to.is_none() || touched {
            continue;
        }
        writes.push(Write {
            id: id.clone(),
            body: Body {
                limited_to: None,
                ..entry.body.clone()
            },
            revision: entry.revision,
            holds: local
                .applied
                .get(id)
                .filter(|applied| applied.revision == entry.revision)
                .map(|applied| applied.digest.clone()),
        });
    }
    let mut changed = Vec::new();
    for write in writes {
        let entry = entries
            .get(&write.id)
            .filter(|entry| entry.revision == write.revision);
        match entry {
            Some(entry) if same_contents(&entry.body, &write.body) => {
                if let Some(digest) = write.holds {
                    local.applied.insert(
                        write.id.clone(),
                        Applied {
                            revision: entry.revision,
                            plugin: entry.body.plugin.clone(),
                            identity: entry.body.sign_in.identity.clone(),
                            digest,
                        },
                    );
                }
            }
            _ => changed.push(write),
        }
    }
    local.moved_in = true;
    Ok(Plan {
        writes: changed,
        deletes,
        local,
        seen,
    })
}

/// Whether two bodies seal the same sign-in, whenever each was written.
fn same_contents(one: &Body, other: &Body) -> bool {
    let mut other = other.clone();
    other.written_at = one.written_at;
    *one == other
}

/// Takes in each entry that changed elsewhere, and lets go of each the vault no longer holds.
/// Takes in each entry of `entries`, and lets go of a sign-in only once `gone` says its entry
/// left this host at or after the revision it took in.
fn apply(
    books: &[(String, PathBuf, &'static dyn Book)],
    entries: &BTreeMap<String, Entry>,
    gone: &BTreeMap<String, i64>,
    local: &mut Local,
    marks: &Marks,
) -> bool {
    let mut moved = false;
    let book = |plugin: &str| {
        books
            .iter()
            .find(|(id, _, _)| id == plugin)
            .map(|(_, dir, book)| (dir, *book))
    };
    for (id, entry) in entries {
        if marks.kept(&entry.body.plugin, &entry.body.sign_in.identity) {
            continue;
        }
        if local
            .applied
            .get(id)
            .is_some_and(|applied| applied.revision == entry.revision)
        {
            continue;
        }
        let Some((dir, kept)) = book(&entry.body.plugin) else {
            continue;
        };
        let sign_in = &entry.body.sign_in;
        let held = kept.take_in(dir, sign_in).and_then(|()| kept.sign_ins(dir));
        match held {
            Ok(here) => {
                let digest = here
                    .iter()
                    .find(|held| held.identity == sign_in.identity)
                    .map(local::digest)
                    .unwrap_or_default();
                moved = true;
                local.applied.insert(
                    id.clone(),
                    Applied {
                        revision: entry.revision,
                        plugin: entry.body.plugin.clone(),
                        identity: sign_in.identity.clone(),
                        digest,
                    },
                );
            }
            Err(error) => eprintln!(
                "sikemux core: could not take in {} from the account vault: {error}",
                sign_in.label
            ),
        }
    }
    local
        .applied
        .retain(|_, applied| !marks.kept(&applied.plugin, &applied.identity));
    let mut letting_go = Vec::new();
    for (id, applied) in local.applied.iter() {
        if entries.contains_key(id) {
            continue;
        }
        match gone.get(id) {
            Some(revision) if *revision >= applied.revision => {
                letting_go.push((id.clone(), applied.clone()));
            }
            _ => eprintln!(
                "sikemux core: vault entry {id} did not open here and is not listed as removed, so its sign-in stays"
            ),
        }
    }
    for (id, applied) in letting_go {
        let Some((dir, kept)) = book(&applied.plugin) else {
            continue;
        };
        match kept.let_go(dir, &applied.identity) {
            Ok(()) => {
                moved = true;
                local.applied.remove(&id);
            }
            Err(error) => eprintln!(
                "sikemux core: could not let go of a sign-in the account vault took away: {error}"
            ),
        }
    }
    moved
}

async fn reconcile(
    core: &Arc<Core>,
    account: &str,
    me: &str,
    opened: (Key, Remote),
) -> CoreResult<()> {
    let (key, vault) = opened;
    let shown_before = super::shown::vault(core);
    let mut entries = BTreeMap::new();
    let mut gone: BTreeMap<String, i64> = (vault.ended.iter().flatten())
        .map(|ended| (ended.id.clone(), ended.revision))
        .collect();
    for entry in &vault.entries {
        if entry.generation != vault.generation {
            continue;
        }
        match open_body(&key, entry) {
            Some(body)
                if body
                    .limited_to
                    .as_ref()
                    .is_some_and(|hosts| !hosts.contains_key(me)) =>
            {
                gone.insert(entry.id.clone(), entry.revision);
            }
            Some(body) => {
                entries.insert(
                    entry.id.clone(),
                    Entry {
                        revision: entry.revision,
                        body,
                    },
                );
            }
            None => eprintln!("sikemux core: vault entry {} does not open", entry.id),
        }
    }
    let pending = {
        let mut state = core.vault().state.locked();
        let pending: Vec<(String, Body)> = std::mem::take(&mut state.pending)
            .into_iter()
            .filter_map(|id| {
                let shared = state.entries.get(&id)?.body.shared.clone();
                let mut body = entries.get(&id)?.body.clone();
                (body.refresher == me).then(|| {
                    body.shared = shared;
                    (id, body)
                })
            })
            .collect();
        state.entries = entries.clone();
        state.rules.clear();
        state.reindex();
        pending
    };
    let compare = {
        let mut state = core.vault().state.locked();
        let compare = state.dirty || !state.compared;
        state.dirty = false;
        state.compared = true;
        compare
    };
    let books = books(core);
    let data_dir = core.data_dir().clone();
    let planning = (
        books.clone(),
        entries.clone(),
        account.to_owned(),
        me.to_owned(),
        vault.recovery_key.clone(),
    );
    let plan = blocking(move || {
        let (books, entries, account, me, vault) = planning;
        let local = local::load(data_dir.as_deref(), &account, &vault);
        let marks = local::marks(data_dir.as_deref()).map_err(CoreError::from)?;
        let planned = compare || !local.moved_in;
        let plan = if planned {
            plan(&books, &entries, local, &marks, &me)?
        } else {
            Plan {
                writes: Vec::new(),
                deletes: Vec::new(),
                local,
                seen: Vec::new(),
            }
        };
        let marks = local::change_marks(data_dir.as_deref(), |marks| {
            for seen in &plan.seen {
                match marks
                    .sign_ins
                    .iter_mut()
                    .find(|mark| mark.plugin == seen.plugin && mark.identity == seen.identity)
                {
                    Some(mark) => mark.label.clone_from(&seen.label),
                    None => marks.sign_ins.push(seen.clone()),
                }
            }
        })
        .map_err(CoreError::from)?;
        Ok((plan, marks, planned))
    })
    .await?;
    let (mut plan, marks, planned) = plan;
    plan.local
        .floor
        .saw(vault.generation, &listed(&vault), &ended(&vault));
    {
        let mut state = core.vault().state.locked();
        state.marks = marks.clone();
        if planned {
            state.here.clone_from(&plan.seen);
        }
    }
    for (id, body) in pending {
        if let Some(write) = plan.writes.iter_mut().find(|write| write.id == id) {
            if write.body.refresher == me {
                write.body.shared = body.shared;
            }
            continue;
        }
        let revision = entries.get(&id).map_or(0, |entry| entry.revision);
        plan.writes.push(Write {
            id,
            body,
            revision,
            holds: None,
        });
    }
    plan.writes
        .retain(|write| !marks.kept(&write.body.plugin, &write.body.sign_in.identity));
    let wrote = !plan.writes.is_empty() || !plan.deletes.is_empty();
    for write in plan.writes {
        let put = VaultEntryPut {
            plugin: write.body.plugin.clone(),
            generation: vault.generation,
            sealed: seal_body(
                &key,
                &write.id,
                vault.generation,
                write.revision,
                &write.body,
            )?,
            revision: write.revision,
        };
        let path = format!("/v1/vault/entries/{}", write.id);
        let written: VaultEntry = match as_host(core, Method::PUT, &path, Some(json!(put))).await {
            Ok(written) => written,
            Err(error) => {
                core.vault().state.locked().dirty = true;
                return Err(error);
            }
        };
        plan.local.floor.wrote(&write.id, written.revision);
        if let Some(digest) = write.holds {
            plan.local.applied.insert(
                write.id.clone(),
                Applied {
                    revision: written.revision,
                    plugin: write.body.plugin.clone(),
                    identity: write.body.sign_in.identity.clone(),
                    digest,
                },
            );
        }
        entries.insert(
            write.id,
            Entry {
                revision: written.revision,
                body: write.body,
            },
        );
    }
    if !plan.deletes.is_empty() {
        let data_dir = core.data_dir().clone();
        let saving = plan.local.clone();
        blocking(move || local::save(data_dir.as_deref(), &saving).map_err(CoreError::from))
            .await?;
    }
    for (id, revision) in plan.deletes {
        let path = format!("/v1/vault/entries/{id}?revision={revision}");
        host_request(core, Method::DELETE, &path, None).await?;
        entries.remove(&id);
        gone.insert(id, revision);
    }
    {
        let mut state = core.vault().state.locked();
        state.entries = entries.clone();
        state.reindex();
    }
    let data_dir = core.data_dir().clone();
    let me = me.to_owned();
    let mut local = plan.local;
    let took_in_or_let_go = blocking(move || {
        let moved = apply(&books, &entries, &gone, &mut local, &marks);
        local.rules = rules_of(&entries, &local);
        local.host = me;
        local::save(data_dir.as_deref(), &local).map_err(CoreError::from)?;
        Ok(moved)
    })
    .await?;
    if wrote {
        super::inbox_changed(core);
    }
    if wrote || took_in_or_let_go || super::shown::vault(core) != shown_before {
        announce(core);
    }
    Ok(())
}

/// Until this host reaches the account, its keeper holds to the rules it kept when it last did.
async fn keep_to_rules(core: &Core) {
    let data_dir = core.data_dir().clone();
    let Ok(Some(local)) = blocking(move || Ok(local::read(data_dir.as_deref()))).await else {
        return;
    };
    if core.remote().owner().as_deref() != Some(local.account.as_str()) {
        return;
    }
    let mut state = core.vault().state.locked();
    if state.account.is_none() && !local.host.is_empty() {
        state.me = Some(local.host);
        state.rules = local.rules;
    }
}

fn rules_of(entries: &BTreeMap<String, Entry>, local: &Local) -> BTreeMap<String, local::Rule> {
    local
        .applied
        .keys()
        .filter_map(|id| {
            let body = &entries.get(id)?.body;
            let rule = local::Rule {
                label: body.sign_in.label.clone(),
                secrets: body
                    .sign_in
                    .secrets
                    .iter()
                    .map(|kept| (kept.service.clone(), kept.account.clone()))
                    .collect(),
                refresher: body.refresher.clone(),
            };
            Some((id.clone(), rule))
        })
        .collect()
}

/// One entry's unsent uses by one actor, as the disk keeps them.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptUse {
    entry: String,
    actor: String,
    tally: Tally,
}

/// Writes the unsent uses to disk a moment after each new one, and again once they are sent.
pub async fn keep_uses(core: Weak<Core>) {
    let Some(used) = core.upgrade().map(|core| core.vault().used.clone()) else {
        return;
    };
    loop {
        used.notified().await;
        tokio::time::sleep(SETTLE).await;
        let Some(core) = core.upgrade() else { return };
        save_uses(&core).await;
    }
}

async fn save_uses(core: &Core) {
    let unsent = {
        let state = core.vault().state.locked();
        let Some(account) = state.account.clone() else {
            return;
        };
        local::Unsent {
            account,
            uses: state
                .uses
                .iter()
                .map(|((entry, actor), tally)| KeptUse {
                    entry: entry.clone(),
                    actor: actor.clone(),
                    tally: *tally,
                })
                .collect(),
        }
    };
    let data_dir = core.data_dir().clone();
    let saved =
        blocking(move || local::keep_uses(data_dir.as_deref(), &unsent).map_err(CoreError::from))
            .await;
    if let Err(error) = saved {
        eprintln!("sikemux core: the account vault's uses were not kept: {error}");
    }
}

/// Sends the uses logged since the last time, keeping them for next time if it fails.
async fn send_uses(core: &Arc<Core>) {
    let (taken, uses) = {
        let mut state = core.vault().state.locked();
        let taken = std::mem::take(&mut state.uses);
        let uses: Vec<VaultUse> = taken
            .iter()
            .filter_map(|((id, actor), tally)| {
                Some(VaultUse {
                    entry_id: id.clone(),
                    plugin: state.entries.get(id)?.body.plugin.clone(),
                    actor: actor.clone(),
                    count: tally.count,
                    first_at: rfc3339(tally.first),
                    last_at: rfc3339(tally.last),
                })
            })
            .collect();
        (taken, uses)
    };
    if uses.is_empty() {
        return;
    }
    let mut taken = taken;
    let mut sent = true;
    for chunk in uses.chunks(200) {
        let add = VaultUsesAdd {
            uses: chunk.to_vec(),
        };
        if let Err(error) =
            host_request(core, Method::POST, "/v1/vault/uses", Some(json!(add))).await
        {
            eprintln!("sikemux core: the account vault's uses were not sent: {error}");
            sent = false;
            break;
        }
        for each in chunk {
            taken.remove(&(each.entry_id.clone(), each.actor.clone()));
        }
    }
    if !sent {
        let mut state = core.vault().state.locked();
        for (key, tally) in taken {
            let kept = state.uses.entry(key).or_default();
            kept.first = if kept.count == 0 {
                tally.first
            } else {
                kept.first.min(tally.first)
            };
            kept.count += tally.count;
            kept.last = kept.last.max(tally.last);
        }
        return;
    }
    save_uses(core).await;
}

/// Renews the access token of each sign-in this host refreshes, before it runs out, while the
/// account has another host to use it, so those hosts never have to refresh it themselves.
async fn renew_due(core: &Arc<Core>, me: &str) {
    let now = epoch_ms();
    let due: Vec<(String, String, String)> = {
        let state = core.vault().state.locked();
        let others = state
            .holders
            .iter()
            .chain(&state.waiting)
            .any(|host| host != me);
        state
            .entries
            .iter()
            .filter(|(id, entry)| {
                others
                    && entry.body.refresher == me
                    && entry
                        .body
                        .shared
                        .as_ref()
                        .is_none_or(|shared| shared.expires_at < now + RENEW_AHEAD_MS)
                    && state
                        .renew_failed
                        .get(*id)
                        .is_none_or(|at| now.saturating_sub(*at) > RENEW_RETRY_MS)
            })
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry.body.plugin.clone(),
                    entry.body.sign_in.identity.clone(),
                )
            })
            .collect()
    };
    let books = books(core);
    for (id, plugin, identity) in due {
        let Some(renewer) = books
            .iter()
            .find(|(each, _, _)| *each == plugin)
            .and_then(|(_, _, book)| book.renewer())
        else {
            continue;
        };
        if renewer.renew(&identity).await.is_err() {
            core.vault().state.locked().renew_failed.insert(id, now);
        }
    }
}

/// Opens the vault on a host that holds no key, with the recovery key the person kept.
pub async fn recover(core: &Arc<Core>, typed: &str) -> CoreResult<()> {
    let (account, me, generation, wrap) = {
        let state = core.vault().state.locked();
        (
            state.account.clone(),
            state.me.clone(),
            state.generation,
            state.recovery_wrap.clone(),
        )
    };
    let (Some(account), Some(me), Some(wrap)) = (account, me, wrap) else {
        return Err("this host has not reached the account's vault yet".into());
    };
    let secret = seal::recovery_secret(typed)
        .ok_or_else(|| CoreError::from("that is not a recovery key"))?;
    let key = seal::unwrap(
        &wrap,
        &secret,
        &seal::wrap_context(&account, generation, RECOVERY_HOLDER),
    )
    .ok_or_else(|| CoreError::from("that recovery key does not open this account's vault"))?;
    let mine = wrap_for(&key, &account, generation, &me)
        .ok_or_else(|| CoreError::from("the vault's key could not be wrapped for this host"))?;
    let add = VaultKeysAdd {
        generation,
        keys: vec![mine],
    };
    host_request(core, Method::PUT, "/v1/vault/keys", Some(json!(add))).await?;
    core.vault().wake();
    Ok(())
}
