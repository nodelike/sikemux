//! Each mod's own documents, which its page reads and writes through `sikemux.data` with no
//! declaration and no approval, since a mod reaches only its own. They live in `mods.sqlite3` in
//! the core's data folder, apart from the mod's folder, so a write never reloads its page.
//!
//! Every top-level field of a document carries a stamp saying when, on which host and by whom it
//! was written, and a removed document keeps a stamped marker. Two copies of a document merge
//! field by field, the newest stamp winning, so hosts that wrote apart agree once they meet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::super::changed::{ChangedCore, Topic, What};
use super::super::connection::blocking;
use super::super::parts::Parts;
use super::super::remote_state::RemoteParts;
use super::super::{Core, CoreError, CoreResult};

const DATABASE: &str = "mods.sqlite3";
const MAX_DOCUMENT_BYTES: usize = 60 * 1024;
const MAX_MOD_BYTES: i64 = 16 * 1024 * 1024;
const MAX_DOCUMENTS: i64 = 10_000;
const MAX_NAME: usize = 64;
/// Who a write from the mod's own page is stamped with.
const PERSON: &str = "person";

/// When, where and by whom a field was written. Stamps order by time, then by host, so every
/// host picks the same one; `by` only records who it was.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Stamp {
    /// Milliseconds since 1970 on the writing host's clock, never going backwards there.
    pub ms: u64,
    /// Counts the writes made within one millisecond.
    pub n: u32,
    /// The writing host's key.
    pub host: String,
    /// "person", or the id of the agent that wrote it.
    pub by: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// None once the field was taken out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    pub stamp: Stamp,
}

/// A document as it is kept and merged: its fields, each stamped, and when it was last removed
/// or replaced whole. A field older than that removal no longer counts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub fields: BTreeMap<String, Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<Stamp>,
}

/// Merges two copies of a document: each field takes the newer of its two stamps, and the later
/// removal stands. The answer is the same whichever copy comes first.
pub fn merge(mut ours: Document, theirs: Document) -> Document {
    for (name, field) in theirs.fields {
        match ours.fields.get(&name) {
            Some(kept) if kept.stamp >= field.stamp => {}
            _ => {
                ours.fields.insert(name, field);
            }
        }
    }
    ours.removed = ours.removed.max(theirs.removed);
    if let Some(removed) = &ours.removed {
        ours.fields.retain(|_, field| field.stamp >= *removed);
    }
    ours
}

/// What a document reads as: its fields that hold a value, or none when it has none.
pub fn shown(document: &Document) -> Option<Map<String, Value>> {
    let fields: Map<String, Value> = document
        .fields
        .iter()
        .filter_map(|(name, field)| Some((name.clone(), field.value.clone()?)))
        .collect();
    (!fields.is_empty()).then_some(fields)
}

/// Applies a JSON Merge Patch (RFC 7386) to `target`.
fn merge_patch(target: Option<Value>, patch: &Value) -> Option<Value> {
    let Value::Object(patch) = patch else {
        return (!patch.is_null()).then(|| patch.clone());
    };
    let mut target = match target {
        Some(Value::Object(target)) => target,
        _ => Map::new(),
    };
    for (name, value) in patch {
        if let Some(merged) = merge_patch(target.remove(name), value) {
            target.insert(name.clone(), merged);
        }
    }
    Some(Value::Object(target))
}

enum Change {
    Set(Map<String, Value>),
    Patch(Map<String, Value>),
    Remove,
}

impl Change {
    /// The fields this change writes, stamped `stamp`, onto `stored`.
    fn stamped(self, stored: &Document, stamp: &Stamp) -> Document {
        let field = |value: Option<Value>| Field {
            value: value.filter(|value| !value.is_null()),
            stamp: stamp.clone(),
        };
        match self {
            Change::Set(fields) => Document {
                fields: fields
                    .into_iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(name, value)| (name, field(Some(value))))
                    .collect(),
                removed: Some(stamp.clone()),
            },
            Change::Patch(patch) => Document {
                fields: patch
                    .into_iter()
                    .map(|(name, patch)| {
                        let now = stored.fields.get(&name).and_then(|kept| kept.value.clone());
                        let merged = field(merge_patch(now, &patch));
                        (name, merged)
                    })
                    .collect(),
                removed: None,
            },
            Change::Remove => Document {
                fields: BTreeMap::new(),
                removed: Some(stamp.clone()),
            },
        }
    }
}

/// The time and count of the last stamp this host gave.
#[derive(Clone, Copy, Default)]
struct Clock {
    ms: u64,
    n: u32,
}

impl Clock {
    fn tick(&mut self, now: u64) -> Clock {
        *self = match now > self.ms {
            true => Clock { ms: now, n: 0 },
            false => Clock {
                ms: self.ms,
                n: self.n + 1,
            },
        };
        *self
    }
}

struct Open {
    connection: Connection,
    clock: Clock,
}

pub(crate) struct Store {
    path: Option<PathBuf>,
    open: Mutex<Option<Open>>,
}

fn failed(error: rusqlite::Error) -> CoreError {
    CoreError::from(format!("the mods' data could not be reached: {error}"))
}

fn open(path: &Path) -> rusqlite::Result<Open> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS documents (
             mod TEXT NOT NULL,
             collection TEXT NOT NULL,
             id TEXT NOT NULL,
             stamped TEXT NOT NULL,
             value TEXT,
             changed INTEGER NOT NULL,
             PRIMARY KEY (mod, collection, id)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS mods (
             mod TEXT PRIMARY KEY,
             changes INTEGER NOT NULL,
             documents INTEGER NOT NULL,
             bytes INTEGER NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS clock (
             one INTEGER PRIMARY KEY CHECK (one = 0),
             ms INTEGER NOT NULL,
             n INTEGER NOT NULL
         );",
    )?;
    let clock = connection
        .query_row("SELECT ms, n FROM clock", [], |row| {
            Ok(Clock {
                ms: row.get::<_, i64>(0)? as u64,
                n: row.get(1)?,
            })
        })
        .optional()?
        .unwrap_or_default();
    Ok(Open { connection, clock })
}

impl Store {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join(DATABASE)),
            open: Mutex::new(None),
        }
    }

    fn with<T>(&self, work: impl FnOnce(&mut Open) -> CoreResult<T>) -> CoreResult<T> {
        let path = (self.path.as_ref())
            .ok_or_else(|| CoreError::from("this host keeps no data for mods"))?;
        let mut cache = self.open.locked();
        if cache.is_none() {
            *cache = Some(open(path).map_err(failed)?);
        }
        match cache.as_mut() {
            Some(open) => work(open),
            None => Err("the mods' data could not be opened".into()),
        }
    }

    fn get(&self, id: &str, collection: &str, document: &str) -> CoreResult<Value> {
        self.with(|open| {
            let value: Option<Option<String>> = open
                .connection
                .query_row(
                    "SELECT value FROM documents WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                    params![id, collection, document],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            Ok(match value.flatten() {
                Some(text) => serde_json::from_str(&text)?,
                None => Value::Null,
            })
        })
    }

    fn list(&self, id: &str, collection: &str) -> CoreResult<Value> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached(
                    "SELECT id, value FROM documents
                     WHERE mod = ?1 AND collection = ?2 AND value IS NOT NULL ORDER BY id",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map(params![id, collection], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(failed)?;
            let mut listed = Vec::new();
            for row in rows {
                let (document, text) = row.map_err(failed)?;
                let doc: Value = serde_json::from_str(&text)?;
                listed.push(json!({ "id": document, "doc": doc }));
            }
            Ok(Value::Array(listed))
        })
    }

    /// Writes `change` to one document in one transaction, keeping the mod's counts, and answers
    /// what the document reads as after it, or None when nothing was there to change.
    fn write(
        &self,
        (id, collection, document): (&str, &str, &str),
        change: Change,
        host: &str,
        by: &str,
    ) -> CoreResult<Option<Value>> {
        self.with(|open| {
            let transaction = open
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let stored: Option<(String, Option<String>)> = transaction
                .query_row(
                    "SELECT stamped, value FROM documents
                     WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                    params![id, collection, document],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            if stored.is_none() && matches!(change, Change::Remove) {
                return Ok(None);
            }
            let (stored, old_size, was_shown) = match stored {
                Some((stamped, value)) => {
                    (serde_json::from_str(&stamped)?, stamped.len(), value.is_some())
                }
                None => (Document::default(), 0, false),
            };
            let clock = open.clock.tick(epoch_ms());
            let stamp = Stamp {
                ms: clock.ms,
                n: clock.n,
                host: host.to_owned(),
                by: by.to_owned(),
            };
            let merged = merge(stored.clone(), change.stamped(&stored, &stamp));
            let value = shown(&merged).map(Value::Object);
            let text = value.as_ref().map(Value::to_string);
            let stamped = serde_json::to_string(&merged)?;
            let size = stamped.len();
            if size > MAX_DOCUMENT_BYTES && size > old_size {
                return Err(format!(
                    "a mod's document, with the record of its changes, holds at most 60 KiB and this one would be {} KiB; split it into more documents, or set it whole to drop the record of fields taken out",
                    size.div_ceil(1024)
                )
                .into());
            }
            let (changes, documents, bytes): (i64, i64, i64) = transaction
                .query_row(
                    "SELECT changes, documents, bytes FROM mods WHERE mod = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(failed)?
                .unwrap_or_default();
            let documents = documents + i64::from(text.is_some()) - i64::from(was_shown);
            let bytes = bytes + size as i64 - old_size as i64;
            if text.is_some() && !was_shown && documents > MAX_DOCUMENTS {
                return Err(
                    "a mod keeps at most 10,000 documents; remove some before adding more".into(),
                );
            }
            if size > old_size && bytes > MAX_MOD_BYTES {
                return Err("a mod keeps at most 16 MiB of data, with the record of its changes; remove some before adding more".into());
            }
            let changes = changes + 1;
            transaction
                .execute(
                    "INSERT INTO documents (mod, collection, id, stamped, value, changed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (mod, collection, id) DO UPDATE
                     SET stamped = excluded.stamped, value = excluded.value, changed = excluded.changed",
                    params![id, collection, document, stamped, text, changes],
                )
                .map_err(failed)?;
            transaction
                .execute(
                    "INSERT INTO mods (mod, changes, documents, bytes) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (mod) DO UPDATE
                     SET changes = excluded.changes, documents = excluded.documents, bytes = excluded.bytes",
                    params![id, changes, documents, bytes],
                )
                .map_err(failed)?;
            transaction
                .execute(
                    "INSERT INTO clock (one, ms, n) VALUES (0, ?1, ?2)
                     ON CONFLICT (one) DO UPDATE SET ms = excluded.ms, n = excluded.n",
                    params![clock.ms as i64, clock.n],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(Some(value.unwrap_or(Value::Null)))
        })
    }

    /// Forgets every document of the mod `id`.
    pub(crate) fn clear(&self, id: &str) -> CoreResult<()> {
        self.with(|open| {
            let transaction = open.connection.transaction().map_err(failed)?;
            transaction
                .execute("DELETE FROM documents WHERE mod = ?1", params![id])
                .map_err(failed)?;
            transaction
                .execute("DELETE FROM mods WHERE mod = ?1", params![id])
                .map_err(failed)?;
            transaction.commit().map_err(failed)
        })
    }
}

/// Whether `name` can name a collection or a document: 1 to 64 letters, digits, dashes,
/// underscores or dots, not starting with a dot.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn named<'a>(params: &'a Value, what: &str) -> CoreResult<&'a str> {
    let name = params[what].as_str().unwrap_or_default();
    match is_name(name) {
        true => Ok(name),
        false => Err(format!(
            "a {what} is named with 1 to 64 letters, digits, dashes, underscores or dots, not starting with a dot"
        )
        .into()),
    }
}

fn object(params: &mut Value, what: &str) -> CoreResult<Map<String, Value>> {
    match params[what].take() {
        Value::Object(fields) => Ok(fields),
        _ => Err(format!("a document's {what} must be an object").into()),
    }
}

/// Whether `name` is a call on the mod's own data, and whether it writes.
pub(super) fn writes(name: &str) -> Option<bool> {
    match name {
        "data" | "data.get" | "data.list" => Some(false),
        "data.set" | "data.patch" | "data.remove" => Some(true),
        _ => None,
    }
}

/// Runs the call `name` on the mod `id`'s own data, for its page.
pub(super) async fn call(
    core: &Arc<Core>,
    id: &str,
    name: &str,
    mut params: Value,
) -> CoreResult<Value> {
    let folder = super::shown_folder(core, id)?;
    let collection = named(&params, "collection")?.to_owned();
    let document = match name {
        "data" | "data.list" => None,
        _ => Some(named(&params, "id")?.to_owned()),
    };
    let change = match name {
        "data.set" => Some(Change::Set(object(&mut params, "doc")?)),
        "data.patch" => Some(Change::Patch(object(&mut params, "patch")?)),
        "data.remove" => Some(Change::Remove),
        _ => None,
    };
    let host = match change {
        Some(_) => core
            .remote()
            .core_id()
            .ok_or_else(|| CoreError::from("this host's key has not loaded yet; try again"))?,
        None => String::new(),
    };
    let (worker, id) = (core.clone(), id.to_owned());
    let path = format!("{id}/{collection}");
    let (answer, wrote) = blocking(move || {
        if !folder.is_dir() {
            return Err(CoreError::from(format!(
                "there is no mod in {}",
                folder.display()
            )));
        }
        let store = &worker.mods().data;
        match (change, document) {
            (Some(change), Some(document)) => {
                let written = store.write((&id, &collection, &document), change, &host, PERSON)?;
                Ok((written.clone().unwrap_or(Value::Null), written.is_some()))
            }
            (None, Some(document)) => Ok((store.get(&id, &collection, &document)?, false)),
            _ => Ok((store.list(&id, &collection)?, false)),
        }
    })
    .await?;
    if wrote {
        core.changed(What::ReadIn(Topic::ModData, None, Some(vec![path])));
    }
    Ok(answer)
}
