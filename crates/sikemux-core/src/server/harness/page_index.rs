//! Every page an agent showed, findable again: one row per page, saying who showed it, in which
//! project, when, and the ask or call it went with. The same agent showing the same title again
//! makes a new version of that page. Pages are thrown away unless kept: a call's a day after the
//! call settles, any other a week after it was last shown. A page shared through a link is kept,
//! and its link shows its latest version.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sikemux_calls::harness::{ListedPage, PageLife, PageShare};
use sikemux_process::clock::epoch_ms;

use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::parts::Parts;

const DATABASE: &str = "pages.sqlite3";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const CALL_PAGE_STAYS_MS: u64 = DAY_MS;
const PAGE_STAYS_MS: u64 = 7 * DAY_MS;
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// How many pages an agent's page_list gives.
pub(crate) const MOST_LISTED: i64 = 50;
/// How many pages a project's library holds.
pub(crate) const LIBRARY_HOLDS: i64 = 1000;

#[derive(Clone, Debug, Default)]
pub(crate) struct Shown {
    pub id: String,
    pub title: String,
    pub project: String,
    pub agent: String,
    pub name: String,
    pub ask: Option<u64>,
}

fn open(data_dir: &Path) -> Result<Connection, String> {
    let failed = |error: rusqlite::Error| format!("the pages index could not be read: {error}");
    let path = data_dir.join(DATABASE);
    let _ = sikemux_process::files::private_database(&path);
    let mut connection = Connection::open(path).map_err(failed)?;
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
                call TEXT,
                kept INTEGER NOT NULL DEFAULT 0,
                settled INTEGER,
                seen INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS pages_version ON pages(agent, title, at);
            CREATE TABLE IF NOT EXISTS page_shares (
                agent TEXT NOT NULL,
                title TEXT NOT NULL,
                id TEXT NOT NULL,
                key TEXT NOT NULL,
                anyone INTEGER NOT NULL,
                expires INTEGER NOT NULL,
                theme TEXT NOT NULL,
                PRIMARY KEY (agent, title)
            );",
        )
        .map_err(failed)?;
    add_lifetimes(&mut connection).map_err(failed)?;
    Ok(connection)
}

/// Gives an index made before pages were thrown away what says how long each stays, starting
/// every page's week now.
fn add_lifetimes(connection: &mut Connection) -> rusqlite::Result<()> {
    let has = |connection: &Connection| {
        connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('pages') WHERE name = 'seen'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };
    if has(connection)? > 0 {
        return Ok(());
    }
    let adding = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if has(&adding)? == 0 {
        adding.execute_batch(
            "ALTER TABLE pages ADD COLUMN kept INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pages ADD COLUMN settled INTEGER;
             ALTER TABLE pages ADD COLUMN seen INTEGER NOT NULL DEFAULT 0;",
        )?;
        adding.execute("UPDATE pages SET seen = ?1", [epoch_ms() as i64])?;
    }
    adding.commit()
}

/// Indexes a page just shown. A new version of a kept page is kept too.
pub(crate) fn add(data_dir: &Path, shown: &Shown) -> Result<(), String> {
    let now = epoch_ms() as i64;
    open(data_dir)?
        .execute(
            "INSERT OR REPLACE INTO pages (id, title, project, agent, name, at, ask, seen, kept)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, COALESCE(
                (SELECT MAX(kept) FROM pages WHERE agent = ?4 AND title = ?2), 0))",
            params![
                shown.id,
                shown.title,
                shown.project,
                shown.agent,
                shown.name,
                now,
                shown.ask.map(|ask| ask as i64)
            ],
        )
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Page `id` was shown again: its week starts over, and it goes with `ask` too when it went
/// with none. Its `at` moves on only when what it holds `changed`.
pub(crate) fn shown_again(
    data_dir: &Path,
    id: &str,
    ask: Option<u64>,
    changed: bool,
) -> Result<(), String> {
    open(data_dir)?
        .execute(
            "UPDATE pages SET ask = COALESCE(ask, ?2), seen = ?3, at = IIF(?4, ?3, at)
             WHERE id = ?1",
            params![id, ask.map(|ask| ask as i64), epoch_ms() as i64, changed],
        )
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Keeps every version of page `id`, or lets them go again, each starting its time over. A
/// shared page stays kept until sharing stops.
pub(crate) fn keep(data_dir: &Path, id: &str, keep: bool) -> Result<(), String> {
    let connection = open(data_dir)?;
    if !keep && share_of_with(&connection, id)?.is_some() {
        return Err("the page is shared; stop sharing it first".into());
    }
    let changed = connection
        .execute(
            "UPDATE pages SET kept = ?2,
                seen = ?3,
                settled = CASE WHEN settled IS NULL THEN NULL ELSE ?3 END
             WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)",
            params![id, keep, epoch_ms() as i64],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        return Err("that page is gone".into());
    }
    Ok(())
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

/// The agent that showed page `id`, and the folder of its project.
pub(crate) fn shown_by(data_dir: &Path, id: &str) -> Option<(String, String)> {
    open(data_dir)
        .ok()?
        .query_row(
            "SELECT agent, project FROM pages WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()
}

/// The latest version of at most `most` pages whose title holds every one of `words`, newest
/// first, from `project` alone when one is named. `open` names the calls still open.
pub(crate) fn list(
    data_dir: &Path,
    words: &[String],
    project: Option<&str>,
    open_calls: &HashSet<String>,
    most: i64,
) -> Result<Vec<ListedPage>, String> {
    let connection = open(data_dir)?;
    settle(&connection, open_calls).map_err(|error| error.to_string())?;
    let mut sql = String::from(
        "SELECT p.id, p.title, p.project, p.name, p.at, p.versions, p.kept, p.call, p.settled,
            p.seen, s.id, s.key, s.anyone, s.expires FROM (
            SELECT *, COUNT(*) OVER version AS versions,
                ROW_NUMBER() OVER (version ORDER BY at DESC) AS newest
            FROM pages WINDOW version AS (PARTITION BY agent, title)
        ) AS p LEFT JOIN page_shares AS s
            ON (s.agent, s.title) = (p.agent, p.title) AND s.expires > ?1
        WHERE p.newest = 1",
    );
    let mut values: Vec<String> = vec![epoch_ms().to_string()];
    if let Some(project) = project {
        values.push(project.to_owned());
        sql.push_str(&format!(" AND p.project = ?{}", values.len()));
    }
    for word in words {
        let escaped = word
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(format!("%{escaped}%"));
        sql.push_str(&format!(" AND p.title LIKE ?{} ESCAPE '\\'", values.len()));
    }
    sql.push_str(&format!(" ORDER BY p.at DESC LIMIT {most}"));
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            Ok(ListedPage {
                id: row.get(0)?,
                title: row.get(1)?,
                project: row.get(2)?,
                agent: row.get(3)?,
                shown_at: row.get::<_, i64>(4)? as u64,
                versions: row.get::<_, i64>(5)? as u64,
                life: life_of(row, 6)?,
                shared: match row.get::<_, Option<String>>(10)? {
                    Some(link) => Some(PageShare {
                        url: crate::server::share::page::url(&link, &row.get::<_, String>(11)?),
                        anyone: row.get(12)?,
                        expires_at: row.get::<_, i64>(13)? as u64,
                    }),
                    None => None,
                },
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|error| error.to_string())
}

/// The link page `id` is shared through, while it works.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub id: String,
    pub key: String,
    pub anyone: bool,
    pub expires: u64,
    /// The `:root` rule of the theme the page was shared in, which each new version is sealed in.
    pub theme: String,
}

pub(crate) fn share_of(data_dir: &Path, id: &str) -> Result<Option<Link>, String> {
    share_of_with(&open(data_dir)?, id)
}

fn share_of_with(connection: &Connection, id: &str) -> Result<Option<Link>, String> {
    connection
        .query_row(
            "SELECT s.id, s.key, s.anyone, s.expires, s.theme FROM page_shares AS s
             JOIN pages AS p ON (s.agent, s.title) = (p.agent, p.title)
             WHERE p.id = ?1 AND s.expires > ?2",
            params![id, epoch_ms() as i64],
            |row| {
                Ok(Link {
                    id: row.get(0)?,
                    key: row.get(1)?,
                    anyone: row.get(2)?,
                    expires: row.get::<_, i64>(3)? as u64,
                    theme: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// Page `id` is shared through `link` now, which keeps it.
pub(crate) fn shared(data_dir: &Path, id: &str, link: &Link) -> Result<(), String> {
    keep(data_dir, id, true)?;
    open(data_dir)?
        .execute(
            "INSERT OR REPLACE INTO page_shares (agent, title, id, key, anyone, expires, theme)
             SELECT agent, title, ?2, ?3, ?4, ?5, ?6 FROM pages WHERE id = ?1",
            params![
                id,
                link.id,
                link.key,
                link.anyone,
                link.expires as i64,
                link.theme
            ],
        )
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Page `id` is no longer shared. The link it was shared through, if it was.
pub(crate) fn unshared(data_dir: &Path, id: &str) -> Result<Option<String>, String> {
    let connection = open(data_dir)?;
    let link = share_of_with(&connection, id)?;
    connection
        .execute(
            "DELETE FROM page_shares
             WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)",
            [id],
        )
        .map_err(|error| error.to_string())?;
    Ok(link.map(|link| link.id))
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
        .prepare("INSERT OR IGNORE INTO pages (id, title, at, seen) VALUES (?1, ?2, ?3, ?4)")
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
        let _ = insert.execute(params![id, title, at, epoch_ms() as i64]);
    }
    connection
        .execute_batch("PRAGMA user_version = 1")
        .map_err(|error| error.to_string())
}

/// How long the page in `row` stays, from its `kept`, `call`, `settled` and `seen` columns,
/// which start at column `from`.
fn life_of(row: &rusqlite::Row, from: usize) -> rusqlite::Result<PageLife> {
    let kept: bool = row.get(from)?;
    let call: Option<String> = row.get(from + 1)?;
    let settled: Option<i64> = row.get(from + 2)?;
    let seen: i64 = row.get(from + 3)?;
    Ok(match (kept, call, settled) {
        (true, ..) => PageLife::Kept,
        (false, Some(_), None) => PageLife::WithCall,
        (false, Some(_), Some(settled)) => PageLife::Goes {
            at: settled as u64 + CALL_PAGE_STAYS_MS,
        },
        (false, None, _) => PageLife::Goes {
            at: seen as u64 + PAGE_STAYS_MS,
        },
    })
}

/// Marks settled now the pages whose call is no longer among `open_calls`. How many it marked.
fn settle(connection: &Connection, open_calls: &HashSet<String>) -> rusqlite::Result<usize> {
    let waiting: Vec<(String, String)> = connection
        .prepare("SELECT id, call FROM pages WHERE call IS NOT NULL AND settled IS NULL")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let now = epoch_ms() as i64;
    let mut marked = 0;
    for (id, call) in waiting {
        if !open_calls.contains(&call) {
            marked += connection.execute(
                "UPDATE pages SET settled = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        }
    }
    Ok(marked)
}

/// Throws away the pages whose time is up, with their files, and drops the rows of pages whose
/// file is gone. How many rows went.
fn sweep(connection: &Connection, pages: &Path) -> Result<usize, String> {
    let now = epoch_ms();
    let rows: Vec<(String, PageLife)> = connection
        .prepare("SELECT id, kept, call, settled, seen FROM pages")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, life_of(row, 1)?)))?
                .collect::<rusqlite::Result<_>>()
        })
        .map_err(|error| error.to_string())?;
    let mut gone = Vec::new();
    for (id, life) in rows {
        let over = matches!(life, PageLife::Goes { at } if at <= now);
        if over {
            let _ = std::fs::remove_file(pages.join(format!("{id}.html")));
            let _ = std::fs::remove_file(pages.join(format!("{id}.jpg")));
            let _ = std::fs::remove_file(crate::pages::written_in_file(pages, &id));
            let _ = std::fs::remove_dir_all(crate::pages::media_folder(pages, &id));
        }
        if over || !pages.join(format!("{id}.html")).exists() {
            gone.push(id);
        }
    }
    let mut remove = connection
        .prepare("DELETE FROM pages WHERE id = ?1")
        .map_err(|error| error.to_string())?;
    for id in &gone {
        let _ = remove.execute([id]);
    }
    let ended = connection
        .execute("DELETE FROM page_shares WHERE expires <= ?1", [now as i64])
        .map_err(|error| error.to_string())?;
    Ok(gone.len() + ended)
}

/// Indexes the pages kept before the index, then each hour marks the pages whose call settled
/// and throws away those whose time is up.
pub(crate) async fn serve(core: std::sync::Arc<super::Core>, data_dir: PathBuf) {
    let pages = data_dir.join(crate::pages::FOLDER);
    let mut ticker = tokio::time::interval(SWEEP_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut filled = false;
    loop {
        ticker.tick().await;
        let open_calls = core.harness().questions.open_ids();
        let (data_dir, pages) = (data_dir.clone(), pages.clone());
        let swept = super::blocking(move || {
            let pass = || {
                if !filled {
                    backfill(&data_dir, &pages)?;
                }
                let connection = open(&data_dir)?;
                let settled =
                    settle(&connection, &open_calls).map_err(|error| error.to_string())?;
                Ok(settled + sweep(&connection, &pages)? > 0)
            };
            Ok(pass())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|swept| swept);
        filled = true;
        match swept {
            Ok(true) => core.changed(What::Read(Topic::Pages)),
            Ok(false) => {}
            Err(error) => eprintln!("sikemux core: the pages were not swept: {error}"),
        }
    }
}
