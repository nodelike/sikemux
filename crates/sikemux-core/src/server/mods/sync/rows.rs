//! What the store keeps of the account's copy of each document, and how what the account sends
//! is taken in.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};

use super::super::super::CoreResult;
use super::super::data::{
    counts_of, failed, forget, keep_version, merge, save_clock, save_counts, shown, Clock, Counts,
    Document, Stamp, Store,
};
use super::{sid_of, FUTURE_MS, NAMING_ID};
use crate::accounts::vault::ModStored;

/// One document as the account keeps it, sealed.
pub(super) struct SealedCopy {
    pub module: String,
    pub sid: String,
    pub revision: i64,
    pub generation: i64,
    pub sealed: String,
}

/// Something the account sent, in the order it gave it. A place is a server revision and the
/// vault key generation it was sealed under.
pub(super) enum Arrival {
    /// Every document of the mod went from the account.
    Purge(String),
    Naming {
        module: String,
        key: String,
        place: (i64, i64),
    },
    /// The account holds no naming key for the mod.
    Unnamed(String),
    Document {
        module: String,
        sid: String,
        collection: String,
        id: String,
        document: Document,
        place: (i64, i64),
    },
    /// Sealed under a vault key this host does not hold now.
    Unopened(SealedCopy),
    /// Sealed under the key this host holds, and still did not open.
    Unreadable(String, String),
}

/// A document to send, with the revision it is built on and its change count when read.
pub(super) struct Outgoing {
    pub module: String,
    pub sid: String,
    pub collection: String,
    pub id: String,
    pub document: Document,
    pub revision: i64,
    pub changed: i64,
}

/// A document a merge changed, with who its newest stamp names and what it reads as now, none
/// once removed.
pub(super) struct MergedIn {
    pub module: String,
    pub collection: String,
    pub id: String,
    pub by: String,
    pub doc: Option<Map<String, Value>>,
}

/// Who changed a document, and what it reads as now, none once removed.
type Changed = (String, Option<Map<String, Value>>);

/// A document a push carried.
pub(super) struct Sent {
    pub module: String,
    pub sid: String,
    pub collection: String,
    pub id: String,
    pub changed: i64,
}

/// The counts of each mod a list of arrivals changed, and the history batch each writer's
/// changes to a mod go in.
#[derive(Default)]
struct Books {
    counts: HashMap<String, Counts>,
    batches: HashMap<(String, String), i64>,
}

impl Books {
    fn forget(&mut self, module: &str) {
        self.counts.remove(module);
        self.batches.retain(|(kept, _), _| kept != module);
    }

    fn save(self, transaction: &Connection) -> CoreResult<()> {
        for (module, counts) in &self.counts {
            save_counts(transaction, module, counts)?;
        }
        Ok(())
    }
}

fn counts_in<'a>(
    counts: &'a mut HashMap<String, Counts>,
    transaction: &Connection,
    module: &str,
) -> CoreResult<&'a mut Counts> {
    if !counts.contains_key(module) {
        counts.insert(module.to_owned(), counts_of(transaction, module)?);
    }
    Ok(counts.get_mut(module).expect("just inserted"))
}

/// `document` without the stamps written after `limit`.
fn within(mut document: Document, limit: u64) -> Document {
    document.fields.retain(|_, field| field.stamp.ms <= limit);
    if document
        .removed
        .as_ref()
        .is_some_and(|removed| removed.ms > limit)
    {
        document.removed = None;
    }
    document
}

/// The newest stamp `merged` holds that `ours` did not.
fn newest_new(ours: &Document, merged: &Document) -> Option<Stamp> {
    let fields = merged
        .fields
        .iter()
        .filter(|(name, field)| {
            ours.fields.get(*name).map(|kept| &kept.stamp) != Some(&field.stamp)
        })
        .map(|(_, field)| &field.stamp);
    let removed = merged
        .removed
        .as_ref()
        .filter(|removed| ours.removed.as_ref() != Some(*removed));
    fields.chain(removed).max().cloned()
}

/// Merges `theirs` into this host's copy of the document, refusing stamps from too far ahead of
/// this host's clock, and keeps the server's place for it. A change it brings goes in the
/// mod's history as made by whoever its stamp names. Answers who changed it and what it reads as
/// now, when what it reads as changed.
fn merge_in(
    transaction: &Connection,
    (clock, books, now): (&mut Clock, &mut Books, u64),
    (module, sid, collection, id): (&str, &str, &str, &str),
    theirs: Document,
    (revision, generation): (i64, i64),
) -> CoreResult<Option<Changed>> {
    let stored: Option<String> = transaction
        .query_row(
            "SELECT stamped FROM documents WHERE mod = ?1 AND collection = ?2 AND id = ?3",
            params![module, collection, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let ours: Document = match &stored {
        Some(stamped) => serde_json::from_str(stamped)?,
        None => Document::default(),
    };
    let theirs = merge(Document::default(), theirs);
    let accepted = within(theirs.clone(), now + FUTURE_MS);
    for field in accepted.fields.values() {
        clock.pass(&field.stamp);
    }
    if let Some(removed) = &accepted.removed {
        clock.pass(removed);
    }
    let merged = merge(ours.clone(), accepted);
    let dirty = merged != theirs;
    let newest = match newest_new(&ours, &merged) {
        Some(newest) if merged != ours => newest,
        _ => {
            transaction
                .execute(
                    "UPDATE documents SET revision = ?4, generation = ?5, dirty = ?6, sid = ?7
                     WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                    params![module, collection, id, revision, generation, dirty, sid],
                )
                .map_err(failed)?;
            return Ok(None);
        }
    };
    let before = shown(&ours);
    let after = shown(&merged);
    let stamped = serde_json::to_string(&merged)?;
    let text = after
        .as_ref()
        .map(|fields| Value::Object(fields.clone()).to_string());
    let counts = counts_in(&mut books.counts, transaction, module)?;
    counts.documents += i64::from(after.is_some()) - i64::from(before.is_some());
    counts.bytes += stamped.len() as i64 - stored.map_or(0, |stored| stored.len()) as i64;
    counts.changes += 1;
    let version = counts.changes;
    transaction
        .execute(
            "INSERT INTO documents
                 (mod, collection, id, stamped, value, changed, who, ms, revision, generation, dirty, sid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (mod, collection, id) DO UPDATE
             SET stamped = excluded.stamped, value = excluded.value, changed = excluded.changed,
                 who = excluded.who, ms = excluded.ms, revision = excluded.revision,
                 generation = excluded.generation, dirty = excluded.dirty, sid = excluded.sid",
            params![
                module,
                collection,
                id,
                stamped,
                text,
                version,
                newest.by,
                newest.ms as i64,
                revision,
                generation,
                dirty,
                sid
            ],
        )
        .map_err(failed)?;
    if before == after {
        return Ok(None);
    }
    let empty = Map::new();
    let (old, new) = (
        before.as_ref().unwrap_or(&empty),
        after.as_ref().unwrap_or(&empty),
    );
    let was: Map<String, Value> = old
        .keys()
        .chain(new.keys())
        .filter(|name| old.get(*name) != new.get(*name))
        .map(|name| (name.clone(), old.get(name).cloned().unwrap_or(Value::Null)))
        .collect();
    let batch = *books
        .batches
        .entry((module.to_owned(), newest.by.clone()))
        .or_insert(version);
    keep_version(
        transaction,
        (module, version, batch),
        (collection, id),
        (&newest.by, newest.ms),
        was,
        before.is_some(),
    )?;
    Ok(Some((newest.by, after)))
}

/// The account holds no naming key for `module`, so nothing of this host's copy is on it.
fn unnamed(transaction: &Connection, module: &str) -> CoreResult<()> {
    transaction
        .execute("DELETE FROM namings WHERE mod = ?1", params![module])
        .map_err(failed)?;
    transaction
        .execute(
            "UPDATE documents SET revision = 0, generation = 0, sid = NULL WHERE mod = ?1",
            params![module],
        )
        .map_err(failed)?;
    Ok(())
}

fn named(transaction: &Connection, module: &str, key: &str, place: (i64, i64)) -> CoreResult<()> {
    let known: Option<String> = transaction
        .query_row(
            "SELECT key FROM namings WHERE mod = ?1",
            params![module],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    if known.as_deref().is_some_and(|known| known != key) {
        unnamed(transaction, module)?;
    }
    transaction
        .execute(
            "INSERT INTO namings (mod, key, revision, generation) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (mod) DO UPDATE
             SET key = excluded.key, revision = excluded.revision, generation = excluded.generation",
            params![module, key, place.0, place.1],
        )
        .map_err(failed)?;
    opened(transaction, module, NAMING_ID)
}

/// Whether this host keeps `copy` at its revision or later in plain, as when it sent it itself,
/// so it need not keep the sealed copy to open later.
fn held(transaction: &Connection, copy: &SealedCopy) -> CoreResult<bool> {
    let query = match copy.sid == NAMING_ID {
        true => "SELECT 1 FROM namings WHERE mod = ?1 AND ?2 = ?2 AND revision >= ?3",
        false => "SELECT 1 FROM documents WHERE mod = ?1 AND sid = ?2 AND revision >= ?3",
    };
    transaction
        .prepare_cached(query)
        .and_then(|mut statement| statement.exists(params![copy.module, copy.sid, copy.revision]))
        .map_err(failed)
}

fn opened(transaction: &Connection, module: &str, sid: &str) -> CoreResult<()> {
    transaction
        .execute(
            "DELETE FROM unopened WHERE mod = ?1 AND sid = ?2",
            params![module, sid],
        )
        .map_err(failed)?;
    Ok(())
}

/// The collections of `module` that hold documents, as change paths.
fn paths_of(transaction: &Connection, module: &str) -> CoreResult<Vec<String>> {
    let mut statement = transaction
        .prepare_cached(
            "SELECT DISTINCT collection FROM documents WHERE mod = ?1 AND value IS NOT NULL",
        )
        .map_err(failed)?;
    let rows = statement
        .query_map(params![module], |row| row.get::<_, String>(0))
        .map_err(failed)?;
    rows.map(|collection| Ok(format!("{module}/{}", collection.map_err(failed)?)))
        .collect()
}

fn naming_keys(transaction: &Connection) -> CoreResult<HashMap<String, Vec<u8>>> {
    let mut statement = transaction
        .prepare_cached("SELECT mod, key FROM namings")
        .map_err(failed)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(failed)?;
    let mut keys = HashMap::new();
    for row in rows {
        let (module, key) = row.map_err(failed)?;
        if let Ok(key) = hex::decode(key) {
            keys.insert(module, key);
        }
    }
    Ok(keys)
}

impl Store {
    fn changing<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>, &mut Clock) -> CoreResult<T>,
    ) -> CoreResult<T> {
        self.with(|open| {
            let transaction = open.connection.transaction().map_err(failed)?;
            let done = work(&transaction, &mut open.clock)?;
            save_clock(&transaction, open.clock)?;
            transaction.commit().map_err(failed)?;
            Ok(done)
        })
    }

    /// Where this host's reading of `account`'s changes stands, or none while the store holds
    /// another account's copy.
    pub(super) fn cursor(&self, account: &str) -> CoreResult<Option<i64>> {
        self.with(|open| {
            if open.account.as_deref() != Some(account) {
                return Ok(None);
            }
            let seq = open
                .connection
                .query_row("SELECT seq FROM synced", [], |row| row.get(0))
                .optional()
                .map_err(failed)?;
            Ok(Some(seq.unwrap_or(0)))
        })
    }

    pub(super) fn pulled_to(&self, seq: i64) -> CoreResult<()> {
        self.changing(|transaction, _| {
            transaction
                .execute(
                    "INSERT INTO synced (one, seq) VALUES (0, ?1)
                     ON CONFLICT (one) DO UPDATE SET seq = excluded.seq",
                    params![seq],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    /// Takes in `arrivals` in order, in one transaction, and answers the collections whose
    /// documents now read differently, as change paths.
    pub(super) fn take_in(
        &self,
        arrivals: Vec<Arrival>,
        now: u64,
    ) -> CoreResult<(BTreeSet<String>, Vec<MergedIn>)> {
        self.changing(|transaction, clock| {
            let mut books = Books::default();
            let mut paths = BTreeSet::new();
            let mut merged = Vec::new();
            for arrival in arrivals {
                match arrival {
                    Arrival::Purge(module) => {
                        paths.extend(paths_of(transaction, &module)?);
                        books.forget(&module);
                        forget(transaction, &module)?;
                    }
                    Arrival::Naming { module, key, place } => {
                        named(transaction, &module, &key, place)?
                    }
                    Arrival::Unnamed(module) => unnamed(transaction, &module)?,
                    Arrival::Document {
                        module,
                        sid,
                        collection,
                        id,
                        document,
                        place,
                    } => {
                        let changed = merge_in(
                            transaction,
                            (clock, &mut books, now),
                            (&module, &sid, &collection, &id),
                            document,
                            place,
                        )?;
                        opened(transaction, &module, &sid)?;
                        if let Some((by, doc)) = changed {
                            paths.insert(format!("{module}/{collection}"));
                            merged.push(MergedIn {
                                module,
                                collection,
                                id,
                                by,
                                doc,
                            });
                        }
                    }
                    Arrival::Unopened(copy) => {
                        if held(transaction, &copy)? {
                            continue;
                        }
                        transaction
                            .execute(
                                "INSERT INTO unopened (mod, sid, generation, revision, sealed)
                                 VALUES (?1, ?2, ?3, ?4, ?5)
                                 ON CONFLICT (mod, sid) DO UPDATE
                                 SET generation = excluded.generation, revision = excluded.revision,
                                     sealed = excluded.sealed
                                 WHERE excluded.revision >= unopened.revision",
                                params![
                                    copy.module,
                                    copy.sid,
                                    copy.generation,
                                    copy.revision,
                                    copy.sealed
                                ],
                            )
                            .map_err(failed)?;
                    }
                    Arrival::Unreadable(module, sid) => opened(transaction, &module, &sid)?,
                }
            }
            books.save(transaction)?;
            Ok((paths, merged))
        })
    }

    /// After a pull the server started over: forgets every document, naming key and sealed copy
    /// the pages did not list, keeping documents with a change of this host's not sent yet to
    /// send again. Answers the collections that lost documents, as change paths.
    pub(super) fn drop_unlisted(
        &self,
        listed: &HashSet<(String, String)>,
    ) -> CoreResult<BTreeSet<String>> {
        let on_account =
            |module: &str, sid: &str| listed.contains(&(module.to_owned(), sid.to_owned()));
        self.changing(|transaction, _| {
            for module in naming_keys(transaction)?.into_keys() {
                if !on_account(&module, NAMING_ID) {
                    unnamed(transaction, &module)?;
                }
            }
            let keys = naming_keys(transaction)?;
            let unlisted: Vec<(String, String)> = {
                let mut statement = transaction
                    .prepare_cached("SELECT mod, sid FROM unopened")
                    .map_err(failed)?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(failed)?;
                rows.collect::<rusqlite::Result<Vec<(String, String)>>>()
                    .map_err(failed)?
                    .into_iter()
                    .filter(|(module, sid)| !on_account(module, sid))
                    .collect()
            };
            for (module, sid) in unlisted {
                opened(transaction, &module, &sid)?;
            }
            let documents: Vec<(String, String, String, bool, bool, i64)> = {
                let mut statement = transaction
                    .prepare_cached(
                        "SELECT mod, collection, id, dirty, value IS NOT NULL, LENGTH(stamped)
                         FROM documents",
                    )
                    .map_err(failed)?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    })
                    .map_err(failed)?;
                rows.collect::<rusqlite::Result<_>>().map_err(failed)?
            };
            let mut books = Books::default();
            let mut paths = BTreeSet::new();
            for (module, collection, id, dirty, shows, bytes) in documents {
                let listed = keys.get(&module).is_some_and(|key| {
                    on_account(&module, &sid_of(key, &module, &collection, &id))
                });
                if listed {
                    continue;
                }
                if dirty {
                    transaction
                        .execute(
                            "UPDATE documents SET revision = 0, generation = 0
                             WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                            params![module, collection, id],
                        )
                        .map_err(failed)?;
                    continue;
                }
                transaction
                    .execute(
                        "DELETE FROM documents WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                        params![module, collection, id],
                    )
                    .map_err(failed)?;
                let counts = counts_in(&mut books.counts, transaction, &module)?;
                counts.documents -= i64::from(shows);
                counts.bytes -= bytes;
                if shows {
                    paths.insert(format!("{module}/{collection}"));
                }
            }
            books.save(transaction)?;
            Ok(paths)
        })
    }

    /// After a pull the server started over holding less than this host had read, so it lost its
    /// record: every naming key and document this host keeps goes to it again as new, and sealed
    /// copies it no longer lists are forgotten.
    pub(super) fn send_all_again(&self, listed: &HashSet<(String, String)>) -> CoreResult<()> {
        self.changing(|transaction, _| {
            let unlisted: Vec<(String, String)> = {
                let mut statement = transaction
                    .prepare_cached("SELECT mod, sid FROM unopened")
                    .map_err(failed)?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(failed)?;
                rows.collect::<rusqlite::Result<Vec<(String, String)>>>()
                    .map_err(failed)?
                    .into_iter()
                    .filter(|copy| !listed.contains(copy))
                    .collect()
            };
            for (module, sid) in unlisted {
                opened(transaction, &module, &sid)?;
            }
            transaction
                .execute_batch(
                    "UPDATE namings SET revision = 0, generation = 0;
                     UPDATE documents SET revision = 0, generation = 0, dirty = 1;",
                )
                .map_err(failed)
        })
    }

    /// The documents kept sealed under the vault key generation `generation`, to open now.
    pub(super) fn unopened(&self, generation: i64) -> CoreResult<Vec<SealedCopy>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached(
                    "SELECT mod, sid, revision, sealed FROM unopened WHERE generation = ?1",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map(params![generation], |row| {
                    Ok(SealedCopy {
                        module: row.get(0)?,
                        sid: row.get(1)?,
                        revision: row.get(2)?,
                        generation,
                        sealed: row.get(3)?,
                    })
                })
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    /// Forgets removal markers written before `before` that hold nothing left to send.
    pub(super) fn clear_markers(&self, before: u64) -> CoreResult<()> {
        self.changing(|transaction, _| {
            let freed: Vec<(String, i64)> = {
                let mut statement = transaction
                    .prepare_cached(
                        "SELECT mod, SUM(LENGTH(stamped)) FROM documents
                         WHERE value IS NULL AND dirty = 0 AND ms < ?1 GROUP BY mod",
                    )
                    .map_err(failed)?;
                let rows = statement
                    .query_map(params![before as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(failed)?;
                rows.collect::<rusqlite::Result<_>>().map_err(failed)?
            };
            if freed.is_empty() {
                return Ok(());
            }
            transaction
                .execute(
                    "DELETE FROM documents WHERE value IS NULL AND dirty = 0 AND ms < ?1",
                    params![before as i64],
                )
                .map_err(failed)?;
            let mut books = Books::default();
            for (module, bytes) in freed {
                counts_in(&mut books.counts, transaction, &module)?.bytes -= bytes;
            }
            books.save(transaction)
        })
    }

    /// The mods removed here whose documents the account still holds.
    pub(super) fn purging(&self) -> CoreResult<Vec<String>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached("SELECT mod FROM purging")
                .map_err(failed)?;
            let rows = statement.query_map([], |row| row.get(0)).map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    pub(super) fn purged(&self, module: &str) -> CoreResult<()> {
        self.changing(|transaction, _| {
            transaction
                .execute("DELETE FROM purging WHERE mod = ?1", params![module])
                .map_err(failed)?;
            Ok(())
        })
    }

    /// The mods with documents to send and no naming key to send them under.
    pub(super) fn unnamed(&self) -> CoreResult<Vec<String>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached(
                    "SELECT DISTINCT mod FROM documents WHERE dirty = 1
                     AND mod NOT IN (SELECT mod FROM namings)
                     AND mod NOT IN (SELECT mod FROM purging)
                     AND mod NOT IN (SELECT mod FROM unopened WHERE sid = ?1)",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map(params![NAMING_ID], |row| row.get(0))
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    /// The naming keys the account does not hold, or keeps sealed under a vault key older than
    /// `generation`, with the revision each is at.
    pub(super) fn stale_namings(&self, generation: i64) -> CoreResult<Vec<(String, String, i64)>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached(
                    "SELECT mod, key, revision FROM namings
                     WHERE (revision = 0 OR generation < ?1)
                     AND mod NOT IN (SELECT mod FROM purging)",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map(params![generation], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    /// At most `most` documents to send: those changed here, then those the account keeps
    /// sealed under a vault key older than `generation`. Leaves out a document whose account
    /// copy this host cannot open yet, since it could not merge with it.
    pub(super) fn outgoing(&self, generation: i64, most: usize) -> CoreResult<Vec<Outgoing>> {
        self.with(|open| {
            let connection = &open.connection;
            let mut statement = connection
                .prepare_cached(
                    "SELECT d.mod, d.collection, d.id, d.stamped, d.revision, d.changed, n.key
                     FROM documents d JOIN namings n ON n.mod = d.mod AND n.revision > 0
                     WHERE d.dirty = 1 AND d.mod NOT IN (SELECT mod FROM purging)
                     UNION ALL
                     SELECT d.mod, d.collection, d.id, d.stamped, d.revision, d.changed, n.key
                     FROM documents d JOIN namings n ON n.mod = d.mod AND n.revision > 0
                     WHERE d.dirty = 0 AND d.revision > 0 AND d.generation < ?1
                     AND d.mod NOT IN (SELECT mod FROM purging)
                     LIMIT ?2",
                )
                .map_err(failed)?;
            let mut waiting = connection
                .prepare_cached("SELECT 1 FROM unopened WHERE mod = ?1 AND sid = ?2")
                .map_err(failed)?;
            let rows = statement
                .query_map(params![generation, (most * 2) as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(failed)?;
            let mut outgoing = Vec::new();
            for row in rows {
                let (module, collection, id, stamped, revision, changed, key) =
                    row.map_err(failed)?;
                let Ok(key) = hex::decode(key) else { continue };
                let sid = sid_of(&key, &module, &collection, &id);
                if waiting.exists(params![module, sid]).map_err(failed)? {
                    continue;
                }
                outgoing.push(Outgoing {
                    document: serde_json::from_str(&stamped)?,
                    module,
                    sid,
                    collection,
                    id,
                    revision,
                    changed,
                });
                if outgoing.len() == most {
                    break;
                }
            }
            Ok(outgoing)
        })
    }

    /// Keeps the server's place for each document a push stored. One written here again since
    /// it was read stays to be sent.
    pub(super) fn stored(
        &self,
        sent: &[Sent],
        stored: &[ModStored],
        generation: i64,
    ) -> CoreResult<()> {
        self.changing(|transaction, _| {
            for stored in stored {
                let Some(sent) = sent
                    .iter()
                    .find(|sent| sent.module == stored.r#mod && sent.sid == stored.id)
                else {
                    continue;
                };
                transaction
                    .execute(
                        "UPDATE documents
                         SET revision = ?4, generation = ?5, dirty = changed != ?6, sid = ?7
                         WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                        params![
                            sent.module,
                            sent.collection,
                            sent.id,
                            stored.revision,
                            generation,
                            sent.changed,
                            sent.sid
                        ],
                    )
                    .map_err(failed)?;
            }
            Ok(())
        })
    }

    /// The account no longer holds these documents it was sent at a revision: they go again as
    /// new.
    pub(super) fn missing(&self, sent: &[Sent], missing: &[(String, String)]) -> CoreResult<()> {
        if missing.is_empty() {
            return Ok(());
        }
        self.changing(|transaction, _| {
            for (module, sid) in missing {
                let Some(sent) = sent
                    .iter()
                    .find(|sent| sent.module == *module && sent.sid == *sid)
                else {
                    continue;
                };
                transaction
                    .execute(
                        "UPDATE documents SET revision = 0, generation = 0
                         WHERE mod = ?1 AND collection = ?2 AND id = ?3",
                        params![sent.module, sent.collection, sent.id],
                    )
                    .map_err(failed)?;
            }
            Ok(())
        })
    }
}
