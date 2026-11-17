//! Carries what belongs to the person, not to a host, through the account as the documents of
//! one mod of the core's own, a collection for each kind: the settings that are theirs, and each
//! folder of theirs in [`folders`]. Each round compares this host's copy, the account's and the
//! last copy the two agreed on, so a side that changed since is the one that moved. When both
//! moved, a setting keeps this host's and a file keeps the newer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use sikemux_view::prefs::PreferenceChange;

use super::super::data::{host_key, Change, Write};
use crate::server::connection::blocking;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{preferences, themes, Core, CoreResult};
use crate::skills;

/// The mod id the account keeps it all under, which no mod of the person's may take.
pub(crate) const MODULE: &str = "sikemux-personal";
const SETTINGS: &str = "settings";
const AGREED: &str = "personal-sync.json";
/// A setting's value, as JSON text, so a null one is kept as one.
const VALUE: &str = "json";
/// A file whose content, as JSON, fits in one document of the account; a larger one stays here.
const MOST_FILE_JSON: usize = 44 * 1024;

/// The last copy this host and the account agreed on, for the account it was agreed with.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Agreed {
    account: String,
    /// By the preference's key.
    settings: BTreeMap<String, Setting>,
    /// The SHA-256 of each file, by its folder's collection and its path in the folder.
    files: BTreeMap<String, BTreeMap<String, String>>,
}

/// A folder of the person's whose files travel, as the collection `collection`.
struct Folder {
    collection: &'static str,
    path: PathBuf,
    /// Whether a top-level entry of the folder stays with each host instead.
    stays: fn(&str) -> bool,
}

fn folders(data_dir: &Path) -> Vec<Folder> {
    let skills = Folder {
        collection: "skills",
        path: skills::library(data_dir),
        stays: skills::is_built_in,
    };
    let themes = themes::folder().map(|path| Folder {
        collection: "themes",
        path,
        stays: |_| false,
    });
    std::iter::once(skills).chain(themes).collect()
}

/// What the account said of a setting, and what this host made of it, which differs when this
/// build cannot hold the account's value.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Setting {
    account: Value,
    here: Value,
}

/// Makes this host's copy and the account's the same, while the person keeps sync on.
pub(super) async fn step(core: &Arc<Core>) -> CoreResult<()> {
    if !core.preferences().get().sync_across_hosts {
        return Ok(());
    }
    let Some(data_dir) = core.config().data_dir.clone() else {
        return Ok(());
    };
    let Some(account) = core.remote().owner() else {
        return Ok(());
    };
    let worker = core.clone();
    blocking(move || {
        let path = data_dir.join(AGREED);
        let mut agreed: Agreed = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .filter(|agreed: &Agreed| agreed.account == account)
            .unwrap_or_else(|| Agreed {
                account,
                ..Agreed::default()
            });
        let before = serde_json::to_vec(&agreed)?;
        let mut writes = settings(&worker, &mut agreed.settings)?;
        for folder in folders(&data_dir) {
            let known = agreed
                .files
                .entry(folder.collection.to_owned())
                .or_default();
            writes.extend(files(&worker, &folder, known)?);
        }
        if !writes.is_empty() {
            let host = host_key(&worker)?;
            (worker.mods().data).write(MODULE, writes, &BTreeMap::new(), (&host, "person"))?;
        }
        let after = serde_json::to_vec(&agreed)?;
        if after != before {
            write_whole(&path, &after)?;
        }
        Ok(())
    })
    .await
}

fn listed(core: &Core, collection: &str) -> CoreResult<BTreeMap<String, (Value, u64)>> {
    let listed = (core.mods().data).list(MODULE, collection, None, None, &|_| None)?;
    Ok(listed
        .into_iter()
        .map(|listed| (listed.id, (listed.doc, listed.at)))
        .collect())
}

fn settings(core: &Core, agreed: &mut BTreeMap<String, Setting>) -> CoreResult<Vec<Write>> {
    let account: BTreeMap<String, Value> = listed(core, SETTINGS)?
        .into_iter()
        .filter_map(|(key, (doc, _))| {
            let text = doc.get(VALUE)?.as_str()?;
            Some((key, serde_json::from_str(text).ok()?))
        })
        .collect();
    let mut writes = Vec::new();
    for (key, here) in of_person(core) {
        let theirs = account.get(&key);
        let known = agreed.get(&key);
        let ours_moved = known.is_none_or(|known| known.here != here);
        let theirs_moved = theirs != known.map(|known| &known.account);
        let take_theirs = match (theirs, known) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(_), Some(_)) => theirs_moved && !ours_moved,
        };
        if let (true, Some(theirs)) = (take_theirs, theirs) {
            let wanted = json!({ "key": key, "value": theirs });
            if let Ok(change) = serde_json::from_value::<PreferenceChange>(wanted) {
                preferences::set(core, change)?;
            }
            let here = of_person(core).remove(&key).unwrap_or(here);
            let account = theirs.clone();
            agreed.insert(key, Setting { account, here });
        } else if ours_moved || theirs.is_none() {
            writes.push(Write {
                collection: SETTINGS.to_owned(),
                id: key.clone(),
                change: Change::Set(Map::from_iter([(
                    VALUE.to_owned(),
                    here.to_string().into(),
                )])),
            });
            let account = here.clone();
            agreed.insert(key, Setting { account, here });
        }
    }
    Ok(writes)
}

/// The value of each preference of the person's this host holds, by its key.
fn of_person(core: &Core) -> BTreeMap<String, Value> {
    PreferenceChange::of_person(&core.preferences().get())
        .into_iter()
        .filter_map(|change| match serde_json::to_value(change).ok()? {
            Value::Object(mut pair) => match (pair.remove("key")?, pair.remove("value")?) {
                (Value::String(key), value) => Some((key, value)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// One file as this host keeps it.
struct Local {
    hash: String,
    bytes: Vec<u8>,
    /// When it last changed, in ms since 1970.
    changed: u64,
}

fn hash_of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Every file in `folder` but dot-files, symbolic links and what stays with this host, by its
/// path in it.
fn scan(folder: &Folder) -> BTreeMap<String, Local> {
    let mut found = BTreeMap::new();
    let mut waiting = vec![PathBuf::new()];
    while let Some(within) = waiting.pop() {
        let Ok(entries) = std::fs::read_dir(folder.path.join(&within)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let stays = within.as_os_str().is_empty() && (folder.stays)(&name);
            if name.starts_with('.') || kind.is_symlink() || stays {
                continue;
            }
            let path = within.join(&name);
            if kind.is_dir() {
                waiting.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let changed = (entry.metadata().ok())
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_millis() as u64);
            let path = path.to_string_lossy().replace('\\', "/");
            let hash = hash_of(&bytes);
            found.insert(
                path,
                Local {
                    hash,
                    bytes,
                    changed,
                },
            );
        }
    }
    found
}

/// A file as the account keeps it: its path and its content, as text when it is text.
fn document_of(path: &str, bytes: &[u8]) -> Map<String, Value> {
    let content = match std::str::from_utf8(bytes) {
        Ok(text) => ("text", text.to_owned()),
        Err(_) => (
            "base64",
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ),
    };
    Map::from_iter([
        ("path".to_owned(), Value::from(path)),
        (content.0.to_owned(), Value::from(content.1)),
    ])
}

fn content_of(doc: &Value) -> Option<(String, Vec<u8>)> {
    let path = doc.get("path")?.as_str()?.to_owned();
    let bytes = match (doc.get("text"), doc.get("base64")) {
        (Some(Value::String(text)), _) => text.clone().into_bytes(),
        (_, Some(Value::String(encoded))) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?,
        _ => return None,
    };
    Some((path, bytes))
}

/// A path from another host that stays inside the folder.
fn inside(folder: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    let plain = relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(name) if !name.to_string_lossy().starts_with('.')));
    (plain && !path.is_empty()).then(|| folder.join(relative))
}

fn files(
    core: &Core,
    folder: &Folder,
    agreed: &mut BTreeMap<String, String>,
) -> CoreResult<Vec<Write>> {
    let collection = folder.collection;
    let here = scan(folder);
    let mut account: BTreeMap<String, (String, Vec<u8>, u64)> = BTreeMap::new();
    for (_, (doc, at)) in listed(core, collection)? {
        if let Some((path, bytes)) = content_of(&doc) {
            account.insert(path, (hash_of(&bytes), bytes, at));
        }
    }
    let paths: BTreeSet<String> = (here.keys())
        .chain(account.keys())
        .chain(agreed.keys())
        .cloned()
        .collect();
    let mut writes = Vec::new();
    for path in paths {
        let ours = here.get(&path);
        let theirs = account.get(&path);
        let ours_hash = ours.map(|local| &local.hash);
        let theirs_hash = theirs.map(|(hash, _, _)| hash);
        if ours_hash == theirs_hash {
            match ours_hash {
                Some(hash) => agreed.insert(path, hash.clone()),
                None => agreed.remove(&path),
            };
            continue;
        }
        let known = agreed.get(&path);
        let ours_moved = ours_hash != known;
        let theirs_moved = theirs_hash != known;
        let ours_newer = match (ours, theirs) {
            (Some(local), Some((_, _, at))) => local.changed > *at,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let send = ours_moved && (!theirs_moved || ours_newer);
        if send {
            let id = hash_of(path.as_bytes());
            let change = match ours {
                Some(local) => {
                    let doc = document_of(&path, &local.bytes);
                    if Value::Object(doc.clone()).to_string().len() > MOST_FILE_JSON {
                        too_large(core, collection, &path);
                        continue;
                    }
                    Change::Set(doc)
                }
                None => Change::Remove,
            };
            writes.push(Write {
                collection: collection.to_owned(),
                id,
                change,
            });
            match ours_hash {
                Some(hash) => agreed.insert(path, hash.clone()),
                None => agreed.remove(&path),
            };
            continue;
        }
        let Some(target) = inside(&folder.path, &path) else {
            continue;
        };
        match theirs {
            Some((hash, bytes, _)) => {
                write_whole(&target, bytes)?;
                agreed.insert(path, hash.clone());
            }
            None => {
                remove_up_to(&folder.path, &target);
                agreed.remove(&path);
            }
        }
    }
    Ok(writes)
}

fn too_large(core: &Core, collection: &str, path: &str) {
    let first = (core.mods().sync.too_large.locked()).insert(format!("{collection}/{path}"));
    if first {
        eprintln!("sikemux core: {collection}/{path} is too large to sync and stays on this host");
    }
}

/// Writes `bytes` beside `target` and moves it into place, so a reader never sees half of it.
fn write_whole(target: &Path, bytes: &[u8]) -> CoreResult<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let writing = target.with_file_name(format!(".{name}.incoming"));
    std::fs::write(&writing, bytes)?;
    std::fs::rename(&writing, target)?;
    Ok(())
}

/// Removes the file `target` and each folder it leaves empty, up to `folder`.
fn remove_up_to(folder: &Path, target: &Path) {
    let _ = std::fs::remove_file(target);
    let mut parent = target.parent();
    while let Some(dir) = parent.filter(|dir| *dir != folder && dir.starts_with(folder)) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        parent = dir.parent();
    }
}
