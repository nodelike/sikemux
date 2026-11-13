//! What the store keeps of each mod's folder: the copy this host and the account last agreed on,
//! the files waiting to go up, and those no copy names any more.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use super::super::super::CoreResult;
use super::super::data::{failed, Store, OWN};
use super::super::folder::Shared;

pub(super) const FOLDER: &str = "folder";
pub(super) const FILES: &str = ".files";

/// The copy this host and the account last agreed on, and whether this host made it.
#[derive(Default)]
pub(super) struct Matched {
    pub digest: Option<String>,
    pub own: bool,
}

impl Store {
    /// The account's copy of the mod's folder, and when it was written.
    pub(super) fn shared(&self, id: &str) -> CoreResult<Option<(Shared, u64)>> {
        let Some(listed) = self.one(id, OWN, FOLDER, &|_| None)? else {
            return Ok(None);
        };
        Ok(serde_json::from_value(listed.doc[FOLDER].clone())
            .ok()
            .map(|shared| (shared, listed.at)))
    }

    /// The content of the mod's text file with SHA-256 `hash`, when the store holds it.
    pub(super) fn text_of(&self, id: &str, hash: &str) -> CoreResult<Option<String>> {
        Ok(self
            .one(id, FILES, hash, &|_| None)?
            .and_then(|listed| listed.doc["text"].as_str().map(str::to_owned)))
    }

    /// The mods whose folder the account keeps.
    pub(super) fn shared_mods(&self) -> CoreResult<Vec<String>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached(
                    "SELECT mod FROM documents
                     WHERE collection = ?1 AND id = ?2 AND value IS NOT NULL",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map(params![OWN, FOLDER], |row| row.get(0))
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    pub(super) fn matched(&self, id: &str) -> CoreResult<Matched> {
        self.with(|open| {
            let found = open
                .connection
                .query_row(
                    "SELECT matched, own FROM folders WHERE mod = ?1",
                    params![id],
                    |row| {
                        Ok(Matched {
                            digest: row.get(0)?,
                            own: row.get(1)?,
                        })
                    },
                )
                .optional()
                .map_err(failed)?;
            Ok(found.unwrap_or_default())
        })
    }

    /// Keeps that this host's folder and the account agree on `digest`, and the note shown for
    /// the mod, cleared on none.
    pub(super) fn set_matched(
        &self,
        id: &str,
        digest: &str,
        own: bool,
        note: Option<&str>,
    ) -> CoreResult<()> {
        self.with(|open| {
            open.connection
                .execute(
                    "INSERT INTO folders (mod, matched, own, note) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (mod) DO UPDATE
                     SET matched = excluded.matched, own = excluded.own, note = excluded.note",
                    params![id, digest, own, note],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    pub(super) fn set_note(&self, id: &str, note: Option<&str>) -> CoreResult<()> {
        self.with(|open| {
            open.connection
                .execute(
                    "INSERT INTO folders (mod, note) VALUES (?1, ?2)
                     ON CONFLICT (mod) DO UPDATE SET note = excluded.note",
                    params![id, note],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    /// What each mod's folder says to the person about following the account, by id.
    pub(in super::super) fn folder_notes(&self) -> CoreResult<BTreeMap<String, String>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached("SELECT mod, note FROM folders WHERE note IS NOT NULL")
                .map_err(failed)?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    pub(super) fn queue_uploads(&self, id: &str, files: &[(String, String)]) -> CoreResult<()> {
        self.with(|open| {
            for (hash, path) in files {
                open.connection
                    .execute(
                        "INSERT INTO file_uploads (mod, hash, path) VALUES (?1, ?2, ?3)
                         ON CONFLICT (mod, hash) DO UPDATE SET path = excluded.path",
                        params![id, hash, path],
                    )
                    .map_err(failed)?;
            }
            Ok(())
        })
    }

    /// Each file waiting to go up: its mod, hash and path in the mod's folder.
    pub(super) fn uploads(&self) -> CoreResult<Vec<(String, String, String)>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached("SELECT mod, hash, path FROM file_uploads ORDER BY mod")
                .map_err(failed)?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    pub(super) fn uploaded(&self, id: &str, hash: &str) -> CoreResult<()> {
        self.with(|open| {
            open.connection
                .execute(
                    "DELETE FROM file_uploads WHERE mod = ?1 AND hash = ?2",
                    params![id, hash],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    /// Files to remove from the account at `after` unless a copy of the folder names them again,
    /// each with whether it is kept as a document.
    pub(super) fn queue_drops(
        &self,
        id: &str,
        files: &[(String, bool)],
        after: u64,
    ) -> CoreResult<()> {
        self.with(|open| {
            for (hash, text) in files {
                open.connection
                    .execute(
                        "INSERT INTO file_drops (mod, hash, text, after) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (mod, hash) DO UPDATE SET after = excluded.after",
                        params![id, hash, text, after as i64],
                    )
                    .map_err(failed)?;
            }
            Ok(())
        })
    }

    pub(super) fn due_drops(&self, now: u64) -> CoreResult<Vec<(String, String, bool)>> {
        self.with(|open| {
            let mut statement = open
                .connection
                .prepare_cached("SELECT mod, hash, text FROM file_drops WHERE after <= ?1")
                .map_err(failed)?;
            let rows = statement
                .query_map(params![now as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(failed)?;
            rows.collect::<rusqlite::Result<_>>().map_err(failed)
        })
    }

    pub(super) fn dropped(&self, id: &str, hash: &str) -> CoreResult<()> {
        self.with(|open| {
            open.connection
                .execute(
                    "DELETE FROM file_drops WHERE mod = ?1 AND hash = ?2",
                    params![id, hash],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    /// The mod's naming key, once the account holds one.
    pub(super) fn naming(&self, id: &str) -> CoreResult<Option<Vec<u8>>> {
        self.with(|open| {
            let key: Option<String> = open
                .connection
                .query_row(
                    "SELECT key FROM namings WHERE mod = ?1 AND revision > 0",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            Ok(key.and_then(|key| hex::decode(key).ok()))
        })
    }
}

/// The fields of the document a text file is kept in.
pub(super) fn text_fields(text: &str) -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([("text".to_owned(), Value::String(text.to_owned()))])
}
