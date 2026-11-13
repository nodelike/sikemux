//! Keeps every host's copy of the mods' documents in step through the account (ADR 0016). A
//! write lands in this host's store and answers at once; a moment later this host sends the
//! documents it changed, each sealed under the vault key, and merges in what the account's other
//! hosts sent. The server sees each mod's id and the sizes and times of its documents, never
//! their names or contents.
//!
//! A document's id on the server is an HMAC of its mod, collection and id under the mod's naming
//! key: a random key kept as one more sealed document of the mod, at an id of all zeros, so ids
//! stay the same when the vault key is replaced.

mod rows;

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use reqwest::Method;
use ring::hmac;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_corekit::backoff::Backoff;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use super::super::account::api::{host_send_any, retry_after, Upload};
use super::super::changed::{ChangedCore, Topic, What};
use super::super::connection::blocking;
use super::super::parts::Parts;
use super::super::remote_state::RemoteParts;
use super::super::triggers::{mod_written, ModWritten};
use super::super::vault::{open_bound, random_key, seal_bound, Key};
use super::super::{Core, CoreError, CoreResult};
use super::data::Document;
use crate::accounts::protocol::{AccountEvent, AccountEventType};
use crate::accounts::vault::{
    ModDocument, ModDocumentPut, ModsPulled, ModsPurged, ModsPush, ModsPushed,
};
use rows::{Arrival, MergedIn, Outgoing, SealedCopy, Sent};

/// A burst of writes, such as an agent's list of them, goes in one push.
const SETTLE: Duration = Duration::from_secs(2);
/// How often the store is looked at with nothing to wake it, for keys that changed and
/// documents that could not be opened before.
const TICK: Duration = Duration::from_secs(60);
/// While signed in with no vault key yet, how soon to look again.
const KEYLESS: Duration = Duration::from_secs(10);
/// The server takes 30 pushes a minute from a host.
const PUSH_GAP: Duration = Duration::from_secs(2);
const MOST_PUSHED: usize = 200;
/// The server takes 4 MiB a push; this leaves room for what wraps the documents.
const MOST_PUSH_BYTES: usize = 4 * 1024 * 1024 - 64 * 1024;
/// The account holds all the sealed text it may, so pushing waits this long to try again.
const ACCOUNT_FULL: Duration = Duration::from_secs(15 * 60);
const FAILED: Backoff = Backoff {
    first: Duration::from_secs(5),
    cap: Duration::from_secs(10 * 60),
};
const NAMING_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// A stamp further ahead of this host's clock than this is refused on merge.
const FUTURE_MS: u64 = 24 * 60 * 60 * 1000;
/// Removal markers are kept this long, as the server keeps them.
const REMOVALS_KEPT_MS: u64 = 90 * 24 * 60 * 60 * 1000;
const SWEEP_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Default)]
pub(crate) struct Sync {
    wake: Arc<Notify>,
    /// The account's copy may hold what this host has not taken in yet.
    pull: AtomicBool,
    /// The seqs of purges this host sent, which it does not apply again when it reads them back.
    purged: Mutex<BTreeSet<i64>>,
    last_push: Mutex<Option<Instant>>,
    swept_at: Mutex<u64>,
    /// The account this host last synced with; nothing is sent once it is signed in to another.
    account: Mutex<Option<String>>,
    /// The mods whose changes the account had no room for, as their problem says.
    full: Mutex<BTreeSet<String>>,
}

impl Sync {
    /// A document changed here, so the account hears of it in a moment.
    pub(super) fn changed(&self) {
        self.wake.notify_one();
    }

    /// Why the mod `id`'s changes are not reaching the account's other hosts, if they are not.
    pub(super) fn problem(&self, id: &str) -> Option<String> {
        self.full.locked().contains(id).then(|| {
            "Not syncing: this account's mod data is full. Remove a mod's data to make room."
                .to_owned()
        })
    }

    fn pull_soon(&self) {
        self.pull.store(true, Ordering::Release);
        self.wake.notify_one();
    }
}

/// What the live connection heard: another host changed the account's mod documents.
pub(crate) fn heard(core: &Arc<Core>, events: &[AccountEvent]) {
    if events
        .iter()
        .any(|event| matches!(event.r#type, AccountEventType::ModsChanged))
    {
        core.mods().sync.pull_soon();
    }
}

/// The live connection is up again, so what changed while this host was away is read.
pub(crate) fn catch_up(core: &Arc<Core>) {
    core.mods().sync.pull_soon();
}

/// What the plain text of a sealed document holds.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Sealed {
    Document {
        collection: String,
        id: String,
        document: Document,
    },
    Naming {
        key: String,
    },
}

/// What a sealed document is bound to, so the server cannot pass one off as another, or as
/// another generation or revision of itself.
fn context(module: &str, sid: &str, generation: i64, revision: i64) -> String {
    format!("sikemux-mod-document|v1|{module}|{sid}|{generation}|{revision}")
}

fn sid_of(naming: &[u8], module: &str, collection: &str, id: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, naming);
    hex::encode(hmac::sign(
        &key,
        format!("{module}\n{collection}\n{id}").as_bytes(),
    ))
}

/// `sealed` for the write built on `revision`, which the server stores one revision on.
fn seal(
    (key, generation): (&Key, i64),
    module: &str,
    sid: &str,
    revision: i64,
    sealed: &Sealed,
) -> CoreResult<String> {
    let plain = serde_json::to_vec(sealed)?;
    seal_bound(key, &context(module, sid, generation, revision + 1), &plain)
        .ok_or_else(|| CoreError::from("a mod document could not be sealed"))
}

fn copy_of(document: ModDocument) -> SealedCopy {
    SealedCopy {
        module: document.r#mod,
        sid: document.id,
        revision: document.revision,
        generation: document.generation,
        sealed: document.sealed,
    }
}

fn open(key: &Key, copy: &SealedCopy) -> Option<Sealed> {
    let place = context(&copy.module, &copy.sid, copy.generation, copy.revision);
    serde_json::from_slice(&open_bound(key, &place, &copy.sealed)?).ok()
}

/// What a document from the account comes to here: opened when this host holds the key it was
/// sealed under, kept sealed to try again when it does not.
fn arrival((key, generation): (&Key, i64), copy: SealedCopy) -> Arrival {
    if copy.generation != generation {
        return Arrival::Unopened(copy);
    }
    let place = (copy.revision, copy.generation);
    match (open(key, &copy), copy.sid == NAMING_ID) {
        (Some(Sealed::Naming { key }), true) => Arrival::Naming {
            module: copy.module,
            key,
            place,
        },
        (
            Some(Sealed::Document {
                collection,
                id,
                document,
            }),
            false,
        ) => Arrival::Document {
            module: copy.module,
            sid: copy.sid,
            collection,
            id,
            document,
            place,
        },
        _ => {
            eprintln!(
                "sikemux core: mod {} document {} does not open with the vault key; skipped",
                copy.module, copy.sid
            );
            Arrival::Unreadable(copy.module, copy.sid)
        }
    }
}

enum Next {
    /// Nothing waits: look again when woken.
    Idle,
    /// More waits to be pushed now.
    More,
    Later(Duration),
}

pub(super) async fn run(core: Weak<Core>) {
    let Some(wake) = core.upgrade().map(|core| {
        core.mods().sync.pull.store(true, Ordering::Release);
        core.mods().sync.wake.clone()
    }) else {
        return;
    };
    let mut failures = 0;
    loop {
        let Some(strong) = core.upgrade() else { return };
        let next = round(&strong).await;
        drop(strong);
        match next {
            Ok(next) => {
                failures = 0;
                match next {
                    Next::Idle => {
                        if tokio::time::timeout(TICK, wake.notified()).await.is_ok() {
                            tokio::time::sleep(SETTLE).await;
                        }
                    }
                    Next::More => {}
                    Next::Later(wait) => tokio::time::sleep(wait).await,
                }
            }
            Err(error) => {
                failures += 1;
                if failures == 1 || !error.is_unreachable() {
                    eprintln!("sikemux core: the mods' data did not sync: {error}");
                }
                if let Some(core) = core.upgrade() {
                    core.mods().sync.pull.store(true, Ordering::Release);
                }
                tokio::time::sleep(FAILED.after(failures)).await;
            }
        }
    }
}

async fn round(core: &Arc<Core>) -> CoreResult<Next> {
    if core.remote().signed_in_api().is_none() {
        return Ok(Next::Idle);
    }
    let Some((account, generation, key)) = core.vault().key() else {
        return Ok(Next::Later(KEYLESS));
    };
    let sync = &core.mods().sync;
    if core.remote().owner().as_deref() != Some(account.as_str()) {
        return Ok(Next::Later(KEYLESS));
    }
    let moved = sync.account.locked().as_deref() != Some(account.as_str());
    if moved {
        *sync.account.locked() = Some(account.clone());
        sync.purged.locked().clear();
        sync.pull.store(true, Ordering::Release);
        if !std::mem::take(&mut *sync.full.locked()).is_empty() {
            super::settled_changed(core);
        }
    }
    let sealing = (&key, generation);
    let worker = core.clone();
    let Some(since) = blocking(move || worker.mods().data.cursor(&account)).await? else {
        return Ok(Next::Later(KEYLESS));
    };
    if sync.pull.swap(false, Ordering::AcqRel) {
        if let Err(error) = pull(core, sealing, since).await {
            sync.pull.store(true, Ordering::Release);
            return Err(error);
        }
    }
    let worker = core.clone();
    let retried = blocking(move || worker.mods().data.unopened(generation)).await?;
    let arrivals: Vec<Arrival> = (retried.into_iter())
        .map(|copy| arrival(sealing, copy))
        .collect();
    take_in(core, arrivals).await?;
    sweep(core).await?;
    if let Some(wait) = purge(core).await? {
        return Ok(Next::Later(wait));
    }
    if let Some(wait) = name(core, sealing).await? {
        return Ok(Next::Later(wait));
    }
    push(core, sealing).await
}

/// Takes in what the account sent, in the order it gives, and tells open panes what changed.
async fn take_in(core: &Arc<Core>, arrivals: Vec<Arrival>) -> CoreResult<()> {
    if arrivals.is_empty() {
        return Ok(());
    }
    let worker = core.clone();
    let (paths, merged) =
        blocking(move || worker.mods().data.take_in(arrivals, epoch_ms())).await?;
    for MergedIn {
        module,
        collection,
        id,
        by,
        doc,
    } in &merged
    {
        if let Some(doc) = doc {
            let written = ModWritten {
                collection,
                id,
                doc,
            };
            mod_written(core, module, by, &[written]);
        }
    }
    announce(core, paths);
    Ok(())
}

/// Tells open panes which collections changed, and every client when a mod's badge did.
fn announce(core: &Core, paths: BTreeSet<String>) {
    let own = format!("/{}", super::data::OWN);
    if paths.iter().any(|path| path.ends_with(&own)) {
        super::settled_changed(core);
    }
    if !paths.is_empty() {
        core.changed(What::ReadIn(
            Topic::ModData,
            None,
            Some(paths.into_iter().collect()),
        ));
    }
}

/// Reads what changed since `since`, a page at a time. When the server starts over because this
/// host was away longer than it keeps removals, every document this host keeps that the pages
/// did not list goes, unless it holds a change of this host's not sent yet. When it starts over
/// because it holds less than this host already read, it lost its record, so nothing goes and
/// this host sends everything it keeps back.
async fn pull(core: &Arc<Core>, sealing: (&Key, i64), mut since: i64) -> CoreResult<()> {
    let read_to = since;
    let mut listed: Option<HashSet<(String, String)>> = None;
    loop {
        let path = format!("/v1/mods/documents?since={since}");
        let pulled: ModsPulled = match ask(core, Method::GET, &path, None).await? {
            Answer::Got(pulled) => pulled,
            Answer::Wait(_) | Answer::Full => {
                return Err(
                    "the accounts server asked to wait before reading the mods' data".into(),
                )
            }
        };
        if pulled.reset && listed.is_none() {
            listed = Some(HashSet::new());
        }
        if let Some(listed) = &mut listed {
            listed.extend(
                (pulled.documents.iter())
                    .map(|document| (document.r#mod.clone(), document.id.clone())),
            );
        }
        let own: BTreeSet<i64> = {
            let mut purged = core.mods().sync.purged.locked();
            let own = (pulled.purges.iter())
                .filter(|purge| purged.remove(&purge.seq))
                .map(|purge| purge.seq)
                .collect();
            own
        };
        let mut arrivals: Vec<(i64, Arrival)> = (pulled.purges.into_iter())
            .filter(|purge| !own.contains(&purge.seq))
            .map(|purge| (purge.seq, Arrival::Purge(purge.r#mod)))
            .collect();
        arrivals.extend(
            (pulled.documents.into_iter())
                .map(|document| (document.seq, arrival(sealing, copy_of(document)))),
        );
        arrivals.sort_by_key(|(seq, _)| *seq);
        take_in(
            core,
            arrivals.into_iter().map(|(_, arrival)| arrival).collect(),
        )
        .await?;
        since = pulled.next;
        if listed.is_none() {
            let worker = core.clone();
            blocking(move || worker.mods().data.pulled_to(since)).await?;
        }
        if !pulled.more {
            break;
        }
    }
    if let Some(listed) = listed {
        let lost = read_to > since;
        if lost {
            eprintln!("sikemux core: the account's mod data is behind what this host read; sending this host's copy back");
        }
        let worker = core.clone();
        let paths = blocking(move || {
            let data = &worker.mods().data;
            let paths = match lost {
                true => data.send_all_again(&listed).map(|()| BTreeSet::new())?,
                false => data.drop_unlisted(&listed)?,
            };
            data.pulled_to(since)?;
            Ok(paths)
        })
        .await?;
        announce(core, paths);
    }
    Ok(())
}

/// Clears removal markers this host has kept longer than the server keeps them, once a day.
async fn sweep(core: &Arc<Core>) -> CoreResult<()> {
    let now = epoch_ms();
    {
        let mut swept_at = core.mods().sync.swept_at.locked();
        if now.saturating_sub(*swept_at) < SWEEP_MS {
            return Ok(());
        }
        *swept_at = now;
    }
    let worker = core.clone();
    blocking(move || (worker.mods().data).clear_markers(now.saturating_sub(REMOVALS_KEPT_MS))).await
}

/// Takes every removed mod's documents off the account, so its other hosts drop them too.
async fn purge(core: &Arc<Core>) -> CoreResult<Option<Duration>> {
    let worker = core.clone();
    let waiting = blocking(move || worker.mods().data.purging()).await?;
    for module in waiting {
        let path = format!("/v1/mods/{module}/documents");
        match ask::<ModsPurged>(core, Method::DELETE, &path, None).await? {
            Answer::Got(purged) => {
                core.mods().sync.purged.locked().insert(purged.seq);
            }
            Answer::Wait(wait) => return Ok(Some(wait)),
            Answer::Full => return Ok(Some(ACCOUNT_FULL)),
        }
        let worker = core.clone();
        blocking(move || worker.mods().data.purged(&module)).await?;
    }
    Ok(None)
}

/// Gives each mod with documents to send and no naming key on the account one, and seals each
/// naming key under the vault key it is kept under now.
async fn name(core: &Arc<Core>, sealing: (&Key, i64)) -> CoreResult<Option<Duration>> {
    let generation = sealing.1;
    let worker = core.clone();
    let (unnamed, stale) = blocking(move || {
        let data = &worker.mods().data;
        Ok((data.unnamed()?, data.stale_namings(generation)?))
    })
    .await?;
    let mut keys = Vec::new();
    let mut documents = Vec::new();
    let mut named = stale;
    for module in unnamed {
        let naming =
            random_key().ok_or_else(|| CoreError::from("no randomness for a mod's naming key"))?;
        named.push((module, hex::encode(naming), 0));
    }
    for (module, naming, revision) in named {
        let sealed = seal(
            sealing,
            &module,
            NAMING_ID,
            revision,
            &Sealed::Naming {
                key: naming.clone(),
            },
        )?;
        documents.push(ModDocumentPut {
            r#mod: module.clone(),
            id: NAMING_ID.to_owned(),
            sealed,
            removed: false,
            revision,
        });
        keys.push((module, naming));
    }
    if documents.is_empty() {
        return Ok(None);
    }
    let pushed = match send_push(core, generation, documents).await? {
        Answer::Got(pushed) => pushed,
        Answer::Wait(wait) => return Ok(Some(wait)),
        Answer::Full => return Ok(Some(ACCOUNT_FULL)),
    };
    let mut arrivals: Vec<Arrival> = (pushed.stored.iter())
        .filter_map(|stored| {
            let (_, naming) = keys.iter().find(|(module, _)| *module == stored.r#mod)?;
            Some(Arrival::Naming {
                module: stored.r#mod.clone(),
                key: naming.clone(),
                place: (stored.revision, generation),
            })
        })
        .collect();
    for conflict in pushed.conflicts {
        arrivals.push(match conflict.current {
            Some(current) => arrival(sealing, copy_of(current)),
            None => Arrival::Unnamed(conflict.r#mod),
        });
    }
    take_in(core, arrivals).await?;
    Ok(None)
}

/// Sends the documents that changed here, and those sealed under a vault key since replaced,
/// one push of at most 200 documents and 4 MiB at a time. A document another host wrote first
/// is merged with the server's copy and goes again at its revision.
async fn push(core: &Arc<Core>, sealing: (&Key, i64)) -> CoreResult<Next> {
    let generation = sealing.1;
    let worker = core.clone();
    let outgoing = blocking(move || worker.mods().data.outgoing(generation, MOST_PUSHED)).await?;
    if outgoing.is_empty() {
        return Ok(Next::Idle);
    }
    let mut cut = false;
    let mut bytes = 0;
    let mut documents = Vec::new();
    let mut sent = Vec::new();
    for Outgoing {
        module,
        sid,
        collection,
        id,
        document,
        revision,
        changed,
    } in outgoing
    {
        let removed = super::data::shown(&document).is_none();
        let sealed = Sealed::Document {
            collection: collection.clone(),
            id: id.clone(),
            document,
        };
        let put = ModDocumentPut {
            sealed: seal(sealing, &module, &sid, revision, &sealed)?,
            r#mod: module.clone(),
            id: sid.clone(),
            removed,
            revision,
        };
        bytes += serde_json::to_vec(&put)?.len() + 1;
        if bytes > MOST_PUSH_BYTES && !documents.is_empty() {
            cut = true;
            break;
        }
        documents.push(put);
        sent.push(Sent {
            module,
            sid,
            collection,
            id,
            changed,
        });
    }
    cut |= documents.len() == MOST_PUSHED;
    let pushed = match send_push(core, generation, documents).await? {
        Answer::Got(pushed) => pushed,
        Answer::Wait(wait) => return Ok(Next::Later(wait)),
        Answer::Full => return Ok(Next::Later(ACCOUNT_FULL)),
    };
    let conflicted = !pushed.conflicts.is_empty();
    let mut arrivals = Vec::new();
    let mut missing = Vec::new();
    for conflict in pushed.conflicts {
        match conflict.current {
            Some(current) => arrivals.push(arrival(sealing, copy_of(current))),
            None => missing.push((conflict.r#mod, conflict.id)),
        }
    }
    let worker = core.clone();
    blocking(move || {
        let data = &worker.mods().data;
        data.stored(&sent, &pushed.stored, generation)?;
        data.missing(&sent, &missing)
    })
    .await?;
    take_in(core, arrivals).await?;
    Ok(match cut || conflicted {
        true => Next::More,
        false => Next::Idle,
    })
}

async fn send_push(
    core: &Core,
    generation: i64,
    documents: Vec<ModDocumentPut>,
) -> CoreResult<Answer<ModsPushed>> {
    let wait = {
        let last = core.mods().sync.last_push.locked();
        last.map(|last| PUSH_GAP.saturating_sub(last.elapsed()))
    };
    if let Some(wait) = wait.filter(|wait| !wait.is_zero()) {
        tokio::time::sleep(wait).await;
    }
    *core.mods().sync.last_push.locked() = Some(Instant::now());
    let modules: BTreeSet<String> = (documents.iter())
        .map(|document| document.r#mod.clone())
        .collect();
    let push = ModsPush {
        generation,
        documents,
    };
    let path = "/v1/mods/documents";
    let answer = ask(core, Method::POST, path, Some(serde_json::to_value(push)?)).await?;
    let shown = {
        let mut full = core.mods().sync.full.locked();
        let before = full.len();
        match answer {
            Answer::Full => full.extend(modules),
            Answer::Got(_) => full.retain(|module| !modules.contains(module)),
            Answer::Wait(_) => {}
        }
        full.len() != before
    };
    if shown {
        super::settled_changed(core);
    }
    Ok(answer)
}

enum Answer<T> {
    Got(T),
    /// The server asked this host to wait this long before trying again.
    Wait(Duration),
    /// The account holds all the sealed mod data it may.
    Full,
}

async fn ask<T: DeserializeOwned>(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<Answer<T>> {
    let body = body
        .map(|body| serde_json::to_vec(&body))
        .transpose()?
        .map(|bytes| Upload {
            bytes,
            content_type: "application/json".into(),
            headers: Vec::new(),
        });
    let synced = core.mods().sync.account.locked().clone();
    if synced.is_none() || core.remote().owner() != synced {
        return Err("this host's account changed; the mods' data waits for the next round".into());
    }
    let response = host_send_any(core, method, path, body).await?;
    let status = response.status();
    if status.is_success() {
        let body = response.bytes().await.map_err(|error| {
            CoreError::unreachable(
                &format!("the accounts server's answer to {path} broke off"),
                &error,
            )
        })?;
        return Ok(Answer::Got(serde_json::from_slice(&body).map_err(
            |error| {
                CoreError::from(format!(
                    "the accounts server's answer to {path} is not what this build reads: {error}"
                ))
            },
        )?));
    }
    let wait = retry_after(&response);
    let said = response.text().await.unwrap_or_default();
    match status.as_u16() {
        429 => Ok(Answer::Wait(wait.unwrap_or(TICK))),
        403 | 409 => {
            core.vault().wake();
            Ok(Answer::Wait(TICK))
        }
        413 => {
            eprintln!("sikemux core: the account holds all the mod data it may: {said}");
            Ok(Answer::Full)
        }
        _ => Err(format!("the accounts server answered {status}: {said}").into()),
    }
}
