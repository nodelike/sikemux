//! Keeps each mod's folder the same on every host of the account. A folder that changed here goes
//! up once it is whole and no longer being written: its text files as documents, the rest as
//! files sealed under a key of the mod's, and a listing of it all. A listing from another host
//! is built beside the folder and swapped in. When both changed, the newer folder wins and the
//! other goes to the Trash.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use ring::hmac;
use serde_json::{json, Map};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::super::super::account::api::{host_send_any, retry_after, Upload};
use super::super::super::connection::blocking;
use super::super::super::parts::Parts;
use super::super::super::vault::{open_bytes, seal_bytes, Key};
use super::super::super::{Core, CoreError, CoreResult};
use super::super::data::{host_key, OWN};
use super::super::folder::{self, Local, Scan, Shared};
use super::super::QUIET;
use super::folder_rows::{text_fields, Matched, FILES, FOLDER};
use super::{ask, Answer, ACCOUNT_FULL, TICK};
use crate::accounts::vault::ModFiles;

/// A file no copy of the folder names is removed from the account this long after, by when
/// every host that changed the folder at the same time has been heard.
const DROP_AFTER_MS: u64 = 10 * 60 * 1000;
/// How soon to look again at a folder waiting on files from another host.
const AWAITING: Duration = Duration::from_secs(30);
const LOST: &str =
    "Changed on another host at the same time; this host's older copy went to the Trash.";
const FILES_FULL: &str =
    "Not syncing: this account's mod files are full. Remove a mod to make room.";

enum Outcome {
    Done,
    Wait(Duration),
}

fn derived(naming: &[u8], what: &str, hash: &str) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, naming);
    let tag = hmac::sign(&key, format!("{what}\n{hash}").as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// A file's id on the account, which names its content without showing it.
fn file_id(naming: &[u8], hash: &str) -> String {
    hex::encode(derived(naming, "sikemux-mod-file", hash))
}

/// The key a file is sealed under, the same on every host for the same content, so it outlasts a
/// new vault key as the naming key does.
fn file_key(naming: &[u8], hash: &str) -> Key {
    derived(naming, "sikemux-mod-file-key", hash)
}

fn file_context(module: &str, id: &str) -> String {
    format!("sikemux-mod-file|v1|{module}|{id}")
}

fn mods_folder(core: &Core) -> Option<PathBuf> {
    core.mods().folder.clone()
}

/// Looks at every folder waiting to be, and answers how soon to look again, if any must be.
pub(super) async fn step(core: &Arc<Core>) -> CoreResult<Option<Duration>> {
    let Some(mods) = mods_folder(core) else {
        return Ok(None);
    };
    let sync = &core.mods().sync;
    if sync.every_folder.swap(false, Ordering::AcqRel) {
        let (worker, root) = (core.clone(), mods.clone());
        let every = blocking(move || {
            clear_leftovers(&root);
            let mut every: BTreeSet<String> = folder::ids(&root).into_iter().collect();
            every.extend(worker.mods().data.shared_mods()?);
            Ok(every)
        })
        .await?;
        sync.folders.locked().extend(every);
    }
    let waiting = std::mem::take(&mut *sync.folders.locked());
    let mut later: Option<Duration> = None;
    for id in waiting {
        let wait = match one(core, &mods, &id).await {
            Ok(Outcome::Done) => continue,
            Ok(Outcome::Wait(wait)) => wait,
            Err(error) => {
                eprintln!("sikemux core: mod {id}'s folder did not sync: {error}");
                TICK
            }
        };
        sync.folders.locked().insert(id);
        later = Some(later.map_or(wait, |later| later.min(wait)));
    }
    Ok(later)
}

/// Removes folders a swap left half done when the core stopped.
fn clear_leftovers(mods: &Path) {
    let Ok(entries) = std::fs::read_dir(mods) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let left =
            name.starts_with('.') && (name.contains(".incoming-") || name.ends_with(".outgoing"));
        if left {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

async fn one(core: &Arc<Core>, mods: &Path, id: &str) -> CoreResult<Outcome> {
    let (worker, folder, module) = (core.clone(), mods.join(id), id.to_owned());
    let read = blocking(move || {
        let before = folder::stats(&folder);
        std::thread::sleep(QUIET);
        let local = folder::scan(&folder);
        if folder::stats(&folder) != before {
            return Ok(None);
        }
        let data = &worker.mods().data;
        Ok(Some((
            local,
            before,
            data.shared(&module)?,
            data.matched(&module)?,
        )))
    })
    .await?;
    let Some((local, stats, shared, matched)) = read else {
        return Ok(Outcome::Wait(super::SETTLE));
    };
    let decided = decide(
        &local,
        shared.as_ref().map(|(shared, at)| (shared, *at)),
        &matched,
    );
    match (decided, local, shared) {
        (Action::Agree { own }, Local::Whole(scan), _) => {
            agreed(core, id, &scan, own).await?;
            Ok(Outcome::Done)
        }
        (Action::Publish, Local::Whole(scan), shared) => {
            let before = shared.map(|(shared, _)| shared.files);
            publish(core, id, scan, matched.digest, before).await?;
            Ok(Outcome::Done)
        }
        (Action::Apply { lost }, local, Some((shared, _))) => {
            let scan = match &local {
                Local::Whole(scan) => Some(scan),
                _ => None,
            };
            apply(core, mods, id, shared, (scan, stats), lost).await
        }
        (Action::Unsyncable(problem), ..) => {
            note(core, id, Some(format!("Not syncing: {problem}."))).await?;
            Ok(Outcome::Done)
        }
        _ => Ok(Outcome::Done),
    }
}

/// What to do with a mod's folder, from how it is here, the account's copy and when that was
/// written, and the copy the two last agreed on.
#[derive(Debug, PartialEq)]
enum Action {
    /// The two agree; `own` keeps whether this host made that copy.
    Agree {
        own: bool,
    },
    Publish,
    /// Put the account's copy in place, and when `lost`, this host's copy in the Trash.
    Apply {
        lost: bool,
    },
    Unsyncable(String),
    Nothing,
}

fn decide(local: &Local, shared: Option<(&Shared, u64)>, matched: &Matched) -> Action {
    let shared_digest = shared.map(|(shared, _)| folder::digest(&shared.files));
    let account_moved = shared_digest.is_some() && shared_digest != matched.digest;
    let newer_there = |newest: u64| account_moved && shared.is_some_and(|(_, at)| at > newest);
    match local {
        Local::Whole(scan) => {
            let here = folder::digest(&scan.files);
            if shared_digest.as_deref() == Some(here.as_str()) {
                return Action::Agree {
                    own: matched.own && matched.digest.as_deref() == Some(here.as_str()),
                };
            }
            let changed_here = matched.digest.as_deref() != Some(here.as_str());
            match shared {
                Some((shared, at)) if account_moved && (!changed_here || at >= scan.newest) => {
                    Action::Apply {
                        lost: changed_here || (matched.own && shared.base != matched.digest),
                    }
                }
                _ => Action::Publish,
            }
        }
        Local::Unsyncable { newest, .. } | Local::Unfit { newest } if newer_there(*newest) => {
            Action::Apply { lost: true }
        }
        Local::Unsyncable { problem, .. } => Action::Unsyncable(problem.clone()),
        Local::Unfit { .. } => Action::Nothing,
        Local::Absent if shared.is_some() => Action::Apply { lost: false },
        Local::Absent => Action::Nothing,
    }
}

async fn note(core: &Arc<Core>, id: &str, note: Option<String>) -> CoreResult<()> {
    let (worker, module) = (core.clone(), id.to_owned());
    blocking(move || worker.mods().data.set_note(&module, note.as_deref())).await?;
    super::super::settled_changed(core);
    Ok(())
}

/// This host's folder is the account's copy: keeps that, and makes sure the account holds every
/// file it names.
async fn agreed(core: &Arc<Core>, id: &str, scan: &Scan, own: bool) -> CoreResult<()> {
    let (worker, module, digest) = (core.clone(), id.to_owned(), folder::digest(&scan.files));
    let sealed: Vec<(String, String)> = (scan.files.iter())
        .filter(|(_, entry)| !entry.text)
        .map(|(path, entry)| (entry.hash.clone(), path.clone()))
        .collect();
    let noted = blocking(move || {
        let data = &worker.mods().data;
        let matched = data.matched(&module)?;
        data.queue_uploads(&module, &sealed)?;
        let kept = data.folder_notes()?.get(&module).cloned();
        let note = kept.filter(|note| note == LOST);
        data.set_matched(&module, &digest, own, note.as_deref())?;
        Ok(matched.digest.as_deref() != Some(digest.as_str()))
    })
    .await?;
    if noted {
        super::super::settled_changed(core);
    }
    Ok(())
}

/// Sends this host's folder to the account: its text files as documents, its listing, and its
/// other files to go up once the mod has a naming key.
async fn publish(
    core: &Arc<Core>,
    id: &str,
    scan: Scan,
    base: Option<String>,
    before: Option<folder::Listing>,
) -> CoreResult<()> {
    let host = host_key(core)?;
    let (worker, module) = (core.clone(), id.to_owned());
    blocking(move || {
        let data = &worker.mods().data;
        for (hash, text) in &scan.texts {
            if data.text_of(&module, hash)?.is_none() {
                data.set_own(&module, (FILES, hash), Some(text_fields(text)), &host)?;
            }
        }
        let digest = folder::digest(&scan.files);
        let sealed: Vec<(String, String)> = (scan.files.iter())
            .filter(|(_, entry)| !entry.text)
            .map(|(path, entry)| (entry.hash.clone(), path.clone()))
            .collect();
        let named: BTreeSet<&str> = scan
            .files
            .values()
            .map(|entry| entry.hash.as_str())
            .collect();
        let unnamed: Vec<(String, bool)> = (before.iter().flat_map(BTreeMap::values))
            .filter(|entry| !named.contains(entry.hash.as_str()))
            .map(|entry| (entry.hash.clone(), entry.text))
            .collect();
        let shared = Shared {
            files: scan.files,
            base,
        };
        let fields = Map::from_iter([(FOLDER.to_owned(), json!(shared))]);
        data.set_own(&module, (OWN, FOLDER), Some(fields), &host)?;
        data.queue_uploads(&module, &sealed)?;
        data.queue_drops(&module, &unnamed, epoch_ms() + DROP_AFTER_MS)?;
        data.set_matched(&module, &digest, true, None)
    })
    .await?;
    core.mods().sync.changed();
    super::super::settled_changed(core);
    Ok(())
}

/// Builds the account's copy of the folder beside this host's, from documents, files this host
/// holds already and files fetched from the account, then puts it in place. `lost` sends this
/// host's copy to the Trash rather than replacing it.
async fn apply(
    core: &Arc<Core>,
    mods: &Path,
    id: &str,
    shared: Shared,
    (here, stats): (Option<&Scan>, folder::Stats),
    lost: bool,
) -> CoreResult<Outcome> {
    if !folder::required()
        .iter()
        .all(|required| shared.files.contains_key(*required))
        || !shared.files.keys().all(|path| folder::fits(path))
    {
        return Ok(Outcome::Done);
    }
    let held: BTreeMap<String, PathBuf> = here
        .map(|scan| {
            (scan.files.iter())
                .map(|(path, entry)| (entry.hash.clone(), mods.join(id).join(path)))
                .collect()
        })
        .unwrap_or_default();
    let (worker, root, module, files) = (
        core.clone(),
        mods.to_path_buf(),
        id.to_owned(),
        shared.files.clone(),
    );
    let built = blocking(move || {
        let data = &worker.mods().data;
        let staged = folder::staging(&root, &module)?;
        let mut fetch = Vec::new();
        for (path, entry) in &files {
            let bytes = match held.get(&entry.hash) {
                Some(real) => std::fs::read(real).ok(),
                None if entry.text => data.text_of(&module, &entry.hash)?.map(String::into_bytes),
                None => None,
            };
            match bytes.filter(|bytes| folder::hash_of(bytes) == entry.hash) {
                Some(bytes) => folder::put(&staged, path, &bytes)?,
                None if entry.text => {
                    let _ = std::fs::remove_dir_all(&staged);
                    return Ok(None);
                }
                None => fetch.push((path.clone(), entry.hash.clone())),
            }
        }
        Ok(Some((staged, fetch)))
    })
    .await?;
    let Some((staged, fetch)) = built else {
        return Ok(Outcome::Wait(AWAITING));
    };
    let fetched = fetch_into(core, id, &staged, fetch).await;
    if !matches!(fetched, Ok(true)) {
        let _ = std::fs::remove_dir_all(&staged);
        return fetched.map(|_| Outcome::Wait(AWAITING));
    }
    let (worker, root, module) = (core.clone(), mods.to_path_buf(), id.to_owned());
    let digest = folder::digest(&shared.files);
    let swapped = blocking(move || {
        let folder = root.join(&module);
        if folder::stats(&folder) != stats {
            let _ = std::fs::remove_dir_all(&staged);
            return Ok(false);
        }
        if let Err(error) = folder::swap(&root, &module, &staged, lost) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(CoreError::from(format!(
                "mod {module}'s folder could not be replaced: {error}"
            )));
        }
        let note = lost.then_some(LOST);
        worker
            .mods()
            .data
            .set_matched(&module, &digest, false, note)?;
        Ok(true)
    })
    .await?;
    if !swapped {
        return Ok(Outcome::Wait(super::SETTLE));
    }
    super::super::settled_changed(core);
    Ok(Outcome::Done)
}

/// Fetches each file the account holds of the mod into `staged`, answering whether all came.
async fn fetch_into(
    core: &Arc<Core>,
    id: &str,
    staged: &Path,
    fetch: Vec<(String, String)>,
) -> CoreResult<bool> {
    if fetch.is_empty() {
        return Ok(true);
    }
    let (worker, module) = (core.clone(), id.to_owned());
    let Some(naming) = blocking(move || worker.mods().data.naming(&module)).await? else {
        return Ok(false);
    };
    for (path, hash) in fetch {
        let fid = file_id(&naming, &hash);
        let place = format!("/v1/mods/{id}/files/{fid}");
        super::same_account(core)?;
        let response = host_send_any(core, Method::GET, &place, None).await?;
        if !response.status().is_success() {
            return Ok(false);
        }
        let sealed = response.bytes().await.map_err(|error| {
            CoreError::unreachable("a mod's file broke off on its way down", &error)
        })?;
        let Some(bytes) = open_bytes(&file_key(&naming, &hash), &file_context(id, &fid), &sealed)
            .filter(|bytes| folder::hash_of(bytes) == hash)
        else {
            eprintln!("sikemux core: mod {id}'s file {fid} does not open; skipped");
            return Ok(false);
        };
        let (staged, path) = (staged.to_path_buf(), path.clone());
        blocking(move || Ok(folder::put(&staged, &path, &bytes)?)).await?;
    }
    Ok(true)
}

/// Sends up the files waiting to go, and removes from the account those no copy names any more.
/// Answers how soon to try again, if anything is left.
pub(super) async fn files(core: &Arc<Core>) -> CoreResult<Option<Duration>> {
    let Some(mods) = mods_folder(core) else {
        return Ok(None);
    };
    let worker = core.clone();
    let waiting = blocking(move || worker.mods().data.uploads()).await?;
    let mut by_mod: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (module, hash, path) in waiting {
        by_mod.entry(module).or_default().push((hash, path));
    }
    let mut later = None;
    for (module, files) in by_mod {
        if let Some(wait) = upload(core, &mods, &module, files).await? {
            later = Some(wait);
            break;
        }
    }
    drop_unnamed(core).await?;
    Ok(later)
}

async fn upload(
    core: &Arc<Core>,
    mods: &Path,
    id: &str,
    files: Vec<(String, String)>,
) -> CoreResult<Option<Duration>> {
    let (worker, module) = (core.clone(), id.to_owned());
    let Some(naming) = blocking(move || worker.mods().data.naming(&module)).await? else {
        return Ok(Some(super::SETTLE));
    };
    let held: BTreeSet<String> =
        match ask::<ModFiles>(core, Method::GET, &format!("/v1/mods/{id}/files"), None).await? {
            Answer::Got(held) => held.files.into_iter().collect(),
            Answer::Wait(wait) => return Ok(Some(wait)),
            Answer::Full => return Ok(Some(ACCOUNT_FULL)),
        };
    for (hash, path) in files {
        let fid = file_id(&naming, &hash);
        let real = mods.join(id).join(&path);
        let read = blocking(move || Ok(std::fs::read(real).ok())).await?;
        let current = read.filter(|bytes| folder::hash_of(bytes) == hash);
        if let (false, Some(bytes)) = (held.contains(&fid), current) {
            let sealed = seal_bytes(&file_key(&naming, &hash), &file_context(id, &fid), &bytes)
                .ok_or_else(|| CoreError::from("a mod's file could not be sealed"))?;
            let place = format!("/v1/mods/{id}/files/{fid}");
            let body = Upload {
                bytes: sealed,
                content_type: "application/octet-stream".into(),
                headers: Vec::new(),
            };
            super::same_account(core)?;
            let response = host_send_any(core, Method::PUT, &place, Some(body)).await?;
            match response.status().as_u16() {
                200 => {}
                413 => {
                    note(core, id, Some(FILES_FULL.to_owned())).await?;
                    return Ok(Some(ACCOUNT_FULL));
                }
                429 => return Ok(Some(retry_after(&response).unwrap_or(TICK))),
                status => {
                    let said = response.text().await.unwrap_or_default();
                    return Err(format!("the accounts server answered {status}: {said}").into());
                }
            }
        }
        let (worker, module) = (core.clone(), id.to_owned());
        blocking(move || worker.mods().data.uploaded(&module, &hash)).await?;
    }
    let (worker, module) = (core.clone(), id.to_owned());
    let full = blocking(move || {
        let data = &worker.mods().data;
        let note = data.folder_notes()?.remove(&module);
        Ok(note.as_deref() == Some(FILES_FULL))
    })
    .await?;
    if full {
        note(core, id, None).await?;
    }
    Ok(None)
}

/// Removes the files whose time to go has come and that no copy of their folder names again.
async fn drop_unnamed(core: &Arc<Core>) -> CoreResult<()> {
    let worker = core.clone();
    let due = blocking(move || worker.mods().data.due_drops(epoch_ms())).await?;
    if due.is_empty() {
        return Ok(());
    }
    let host = host_key(core)?;
    for (id, hash, text) in due {
        let (worker, module, host, kept) = (core.clone(), id.clone(), host.clone(), hash.clone());
        let naming = blocking(move || {
            let data = &worker.mods().data;
            let named = (data.shared(&module)?)
                .is_some_and(|(shared, _)| shared.files.values().any(|entry| entry.hash == kept));
            if named {
                data.dropped(&module, &kept)?;
                return Ok(None);
            }
            if text {
                data.set_own(&module, (FILES, &kept), None, &host)?;
                data.dropped(&module, &kept)?;
                return Ok(None);
            }
            data.naming(&module)
        })
        .await?;
        let Some(naming) = naming else { continue };
        let place = format!("/v1/mods/{id}/files/{}", file_id(&naming, &hash));
        super::same_account(core)?;
        let response = host_send_any(core, Method::DELETE, &place, None).await?;
        if response.status().is_success() {
            let (worker, module) = (core.clone(), id.clone());
            blocking(move || worker.mods().data.dropped(&module, &hash)).await?;
        }
    }
    core.mods().sync.changed();
    Ok(())
}

/// Another host removed these mods: their folders here go to the Trash.
pub(super) async fn purged(core: &Arc<Core>, modules: Vec<String>) -> CoreResult<()> {
    let Some(mods) = mods_folder(core) else {
        return Ok(());
    };
    if modules.is_empty() {
        return Ok(());
    }
    let shown = modules.clone();
    blocking(move || {
        for module in modules {
            let folder = mods.join(&module);
            if folder.exists() {
                if let Err(error) = trash::delete(&folder) {
                    eprintln!("sikemux core: mod {module} was removed on another host, and its folder here could not go to the Trash: {error}");
                }
            }
        }
        Ok(())
    })
    .await?;
    core.mods().sync.folders.locked().extend(shown);
    super::super::settled_changed(core);
    Ok(())
}
