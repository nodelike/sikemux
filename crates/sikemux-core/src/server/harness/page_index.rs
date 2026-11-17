//! Every page an agent showed, findable again: one row per page, saying who showed it, in which
//! project, when, and the ask or call it went with. The same agent showing the same title again
//! makes a new version of that page. Old versions nothing links go after a month; the latest
//! version of every page stays.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sikemux_process::clock::epoch_ms;

const DATABASE: &str = "pages.sqlite3";
const KEEP_OLD_VERSIONS_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const MOST_LISTED: i64 = 50;

#[derive(Clone, Debug, Default)]
pub(crate) struct Shown {
    pub id: String,
    pub title: String,
    pub project: String,
    pub agent: String,
    pub name: String,
    pub ask: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Listed {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    pub shown_at: u64,
    pub versions: u64,
}

fn open(data_dir: &Path) -> Result<Connection, String> {
    let failed = |error: rusqlite::Error| format!("the pages index could not be read: {error}");
    let connection = Connection::open(data_dir.join(DATABASE)).map_err(failed)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(failed)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS pages (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                project TEXT NOT NULL DEFAULT '',
                agent TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL DEFAULT '',
                at INTEGER NOT NULL,
                ask INTEGER,
                call TEXT
            );
            CREATE INDEX IF NOT EXISTS pages_version ON pages(agent, title, at);",
        )
        .map_err(failed)?;
    Ok(connection)
}

pub(crate) fn add(data_dir: &Path, shown: &Shown) -> Result<(), String> {
    open(data_dir)?
        .execute(
            "INSERT OR REPLACE INTO pages (id, title, project, agent, name, at, ask)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                shown.id,
                shown.title,
                shown.project,
                shown.agent,
                shown.name,
                epoch_ms() as i64,
                shown.ask.map(|ask| ask as i64)
            ],
        )
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Page `id` was shown again: it goes with `ask` too when it went with none.
pub(crate) fn shown_again(data_dir: &Path, id: &str, ask: Option<u64>) -> Result<(), String> {
    open(data_dir)?
        .execute(
            "UPDATE pages SET ask = COALESCE(ask, ?2) WHERE id = ?1",
            params![id, ask.map(|ask| ask as i64)],
        )
        .map(drop)
        .map_err(|error| error.to_string())
}

pub(crate) fn link_call(data_dir: &Path, id: &str, call: &str) {
    if let Ok(connection) = open(data_dir) {
        let _ = connection.execute(
            "UPDATE pages SET call = ?2 WHERE id = ?1",
            params![id, call],
        );
    }
}

pub(crate) fn title_of(data_dir: &Path, id: &str) -> Option<String> {
    open(data_dir)
        .ok()?
        .query_row("SELECT title FROM pages WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .ok()
        .flatten()
}

/// The latest version of each page whose title holds every one of `words`, newest first, from
/// `project` alone when one is named.
pub(crate) fn list(
    data_dir: &Path,
    words: &[String],
    project: Option<&str>,
) -> Result<Vec<Listed>, String> {
    let connection = open(data_dir)?;
    let mut sql = String::from(
        "SELECT id, title, project, name, at, versions FROM (
            SELECT *, COUNT(*) OVER version AS versions,
                ROW_NUMBER() OVER (version ORDER BY at DESC) AS newest
            FROM pages WINDOW version AS (PARTITION BY agent, title)
        ) WHERE newest = 1",
    );
    let mut values: Vec<String> = Vec::new();
    if let Some(project) = project {
        values.push(project.to_owned());
        sql.push_str(&format!(" AND project = ?{}", values.len()));
    }
    for word in words {
        let escaped = word
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(format!("%{escaped}%"));
        sql.push_str(&format!(" AND title LIKE ?{} ESCAPE '\\'", values.len()));
    }
    sql.push_str(&format!(" ORDER BY at DESC LIMIT {MOST_LISTED}"));
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            Ok(Listed {
                id: row.get(0)?,
                title: row.get(1)?,
                project: row.get(2)?,
                agent: row.get(3)?,
                shown_at: row.get::<_, i64>(4)? as u64,
                versions: row.get::<_, i64>(5)? as u64,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|error| error.to_string())
}

/// Indexes the pages kept before there was an index, by their title and when they were written.
fn backfill(data_dir: &Path, pages: &Path) -> Result<(), String> {
    let connection = open(data_dir)?;
    let filled: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if filled > 0 {
        return Ok(());
    }
    let mut insert = connection
        .prepare("INSERT OR IGNORE INTO pages (id, title, at) VALUES (?1, ?2, ?3)")
        .map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(pages).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(id) = (path.extension().is_some_and(|it| it == "html"))
            .then(|| path.file_stem()?.to_str())
            .flatten()
            .filter(|id| crate::pages::is_page_id(id))
        else {
            continue;
        };
        let Some(title) = crate::pages::title(pages, id) else {
            continue;
        };
        let at = (entry.metadata().ok())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as i64);
        let _ = insert.execute(params![id, title, at]);
    }
    connection
        .execute_batch("PRAGMA user_version = 1")
        .map_err(|error| error.to_string())
}

/// Drops the versions older than a month that a newer one replaced and no call or kept ask
/// links, with their files, and the rows of pages whose file is gone.
fn sweep(
    data_dir: &Path,
    pages: &Path,
    asks: &BTreeSet<u64>,
    opened: &[String],
) -> Result<usize, String> {
    let connection = open(data_dir)?;
    let before = epoch_ms().saturating_sub(KEEP_OLD_VERSIONS_MS) as i64;
    let mut statement = connection
        .prepare(
            "SELECT id, ask FROM pages AS old WHERE at < ?1 AND call IS NULL AND EXISTS (
                SELECT 1 FROM pages AS new
                WHERE new.agent = old.agent AND new.title = old.title AND new.at > old.at
            )",
        )
        .map_err(|error| error.to_string())?;
    let stale: Vec<String> = statement
        .query_map([before], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|error| error.to_string())?
        .flatten()
        .filter(|(id, ask)| {
            !ask.is_some_and(|ask| asks.contains(&(ask as u64)))
                && !opened.iter().any(|url| url.contains(id.as_str()))
        })
        .map(|(id, _)| id)
        .collect();
    let mut gone: Vec<String> = connection
        .prepare("SELECT id FROM pages")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|id| !pages.join(format!("{id}.html")).exists())
        .collect();
    for id in &stale {
        let _ = std::fs::remove_file(pages.join(format!("{id}.html")));
        let _ = std::fs::remove_file(pages.join(format!("{id}.jpg")));
        let _ = std::fs::remove_file(crate::pages::written_in_file(pages, id));
        let _ = std::fs::remove_dir_all(crate::pages::media_folder(pages, id));
    }
    gone.extend(stale.iter().cloned());
    let mut remove = connection
        .prepare("DELETE FROM pages WHERE id = ?1")
        .map_err(|error| error.to_string())?;
    for id in &gone {
        let _ = remove.execute([id]);
    }
    Ok(stale.len())
}

/// Indexes the pages kept before the index, then sweeps old versions once a day.
pub(crate) async fn serve(core: std::sync::Arc<super::Core>, data_dir: PathBuf) {
    let pages = data_dir.join(crate::pages::FOLDER);
    let mut ticker = tokio::time::interval(SWEEP_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut filled = false;
    loop {
        ticker.tick().await;
        let (asks, opened) = crate::server::now::asks::kept(&core);
        let (data_dir, pages) = (data_dir.clone(), pages.clone());
        let swept = super::blocking(move || {
            let filled = if filled {
                Ok(())
            } else {
                backfill(&data_dir, &pages)
            };
            Ok(filled.and_then(|()| sweep(&data_dir, &pages, &asks, &opened)))
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|swept| swept);
        filled = true;
        if let Err(error) = swept {
            eprintln!("sikemux core: the pages were not swept: {error}");
        }
    }
}
