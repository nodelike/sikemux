//! Every page an agent showed, findable again: one row per page, saying who showed it, in which
//! project, when, and the ask or call it went with. The same agent showing the same title again
//! makes a new version of that page. Pages are thrown away unless kept: a call's a day after the
//! call settles, any other a week after it was last shown. A page shared through a link is kept,
//! and its link shows its latest version.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sikemux_calls::harness::{ListedPage, PageLife, PageShare};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::parts::Parts;

const DATABASE: &str = "pages.sqlite3";
const SCHEMA_VERSION: i64 = 2;
const BACKFILLED_VERSION: i64 = 3;
const WAL_SIZE_LIMIT: i64 = 8 * 1024 * 1024;
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

/// The link page `id` is shared through, while it works.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub id: String,
    pub key: String,
    pub anyone: bool,
    pub expires: u64,
    /// The `:root` rule of the theme the page was shared in, which each new version is sealed in.
    pub theme: String,
    /// Whether the page's title and the sharer's name go with it unsealed, for the link's preview.
    pub told: bool,
}

/// A shared page whose stored copy on the server is behind its latest version.
#[derive(Clone, Debug)]
pub(crate) struct PendingShare {
    pub link: Link,
    pub behind: i64,
    pub page_id: Option<String>,
    pub title: String,
}

pub(crate) struct PageIndex {
    path: Option<PathBuf>,
    connection: Mutex<Option<Connection>>,
}

impl PageIndex {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join(DATABASE)),
            connection: Mutex::new(None),
        }
    }

    fn with<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| "this core keeps no pages index".to_owned())?;
        let mut guard = self.connection.locked();
        if guard.is_none() {
            *guard = Some(open(path)?);
        }
        let Some(connection) = guard.as_mut() else {
            return Err("the pages index could not be opened".into());
        };
        work(connection)
    }

    /// Indexes a page just shown. A new version of a kept page is kept too.
    pub(crate) fn add(&self, shown: &Shown, keep: bool) -> Result<(), String> {
        let now = epoch_ms() as i64;
        self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            tx.execute(
                "INSERT OR REPLACE INTO pages (id, title, project, agent, name, at, ask, seen, kept)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, CASE WHEN ?8 THEN 1 ELSE COALESCE(
                    (SELECT MAX(kept) FROM pages WHERE agent = ?4 AND title = ?2), 0) END)",
                params![
                    shown.id,
                    shown.title,
                    shown.project,
                    shown.agent,
                    shown.name,
                    now,
                    shown.ask.map(|ask| ask as i64),
                    keep
                ],
            )
            .map_err(|error| error.to_string())?;
            if keep {
                tx.execute(
                    "UPDATE pages SET kept = 1,
                        seen = ?3,
                        settled = CASE WHEN settled IS NULL THEN NULL ELSE ?3 END
                     WHERE agent = ?1 AND title = ?2",
                    params![shown.agent, shown.title, now],
                )
                .map_err(|error| error.to_string())?;
            }
            tx.execute(
                "UPDATE page_shares
                 SET behind = behind + 1, problem = NULL
                 WHERE agent = ?1 AND title = ?2 AND expires > ?3",
                params![shown.agent, shown.title, now],
            )
            .map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())
        })
    }

    /// Page `id` was shown again: its week starts over, and it goes with `ask` too when it went
    /// with none. Its `at` moves on only when what it holds `changed`. Answers its indexed title.
    pub(crate) fn shown_again(
        &self,
        id: &str,
        ask: Option<u64>,
        changed: bool,
        keep: bool,
    ) -> Result<Option<String>, String> {
        let now = epoch_ms() as i64;
        self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let title: Option<String> = tx
                .query_row("SELECT title FROM pages WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| error.to_string())?;
            tx.execute(
                "UPDATE pages SET ask = COALESCE(ask, ?2), seen = ?3, at = IIF(?4, ?3, at)
                 WHERE id = ?1",
                params![id, ask.map(|ask| ask as i64), now, changed],
            )
            .map_err(|error| error.to_string())?;
            if keep {
                tx.execute(
                    "UPDATE pages SET kept = 1,
                        seen = ?2,
                        settled = CASE WHEN settled IS NULL THEN NULL ELSE ?2 END
                     WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)",
                    params![id, now],
                )
                .map_err(|error| error.to_string())?;
            }
            if changed {
                tx.execute(
                    "UPDATE page_shares
                     SET behind = behind + 1, problem = NULL
                     WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)
                       AND expires > ?2",
                    params![id, now],
                )
                .map_err(|error| error.to_string())?;
            }
            tx.commit().map_err(|error| error.to_string())?;
            Ok(title)
        })
    }

    /// Keeps every version of page `id`, or lets them go again, each starting its time over. A
    /// shared page stays kept until sharing stops.
    pub(crate) fn keep(&self, id: &str, keep: bool) -> Result<(), String> {
        self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            if !keep && share_of_with(&tx, id)?.is_some() {
                return Err("the page is shared; stop sharing it first".into());
            }
            let changed = tx
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
            tx.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn link_calls(&self, ids: &[String], call: &str) {
        let _ = self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            {
                let mut stmt = tx
                    .prepare_cached("UPDATE pages SET call = ?2 WHERE id = ?1")
                    .map_err(|error| error.to_string())?;
                for id in ids {
                    let _ = stmt.execute(params![id, call]);
                }
            }
            tx.commit().map_err(|error| error.to_string())
        });
    }

    pub(crate) fn title_of(&self, id: &str) -> Option<String> {
        self.with(|connection| {
            connection
                .query_row("SELECT title FROM pages WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| error.to_string())
        })
        .ok()
        .flatten()
    }

    /// The agent that showed page `id`, and the folder of its project.
    pub(crate) fn shown_by(&self, id: &str) -> Option<(String, String)> {
        self.with(|connection| {
            connection
                .query_row(
                    "SELECT agent, project FROM pages WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())
        })
        .ok()
        .flatten()
    }

    /// The latest version of at most `most` pages whose title holds every one of `words`, newest
    /// first, from `project` alone when one is named. `open` names the calls still open.
    pub(crate) fn list(
        &self,
        words: &[String],
        project: Option<&str>,
        open_calls: &HashSet<String>,
        most: i64,
    ) -> Result<Vec<ListedPage>, String> {
        self.with(|connection| {
            settle(connection, open_calls).map_err(|error| error.to_string())?;
            let mut sql = String::from(
                "SELECT p.id, p.title, p.project, p.name, p.at, p.versions, p.kept, p.call, p.settled,
                    p.seen, s.id, s.key, s.anyone, s.expires, s.problem FROM (
                    SELECT *, COUNT(*) OVER version AS versions,
                        ROW_NUMBER() OVER (version ORDER BY at DESC, rowid DESC) AS newest
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
                                url: crate::server::share::page::url(
                                    &link,
                                    &row.get::<_, String>(11)?,
                                ),
                                anyone: row.get(12)?,
                                expires_at: row.get::<_, i64>(13)? as u64,
                                problem: row.get(14)?,
                            }),
                            None => None,
                        },
                    })
                })
                .map_err(|error| error.to_string())?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn share_of(&self, id: &str) -> Result<Option<Link>, String> {
        self.with(|connection| share_of_with(connection, id))
    }

    /// The latest version of page `id`, with its live share's `behind` counter if shared.
    pub(crate) fn latest_for_page(
        &self,
        id: &str,
    ) -> Result<Option<(String, String, Option<i64>)>, String> {
        self.with(|connection| {
            connection
                .query_row(
                    "SELECT latest.id, latest.title, s.behind
                     FROM pages AS target
                     JOIN pages AS latest
                         ON latest.agent = target.agent AND latest.title = target.title
                     LEFT JOIN page_shares AS s
                         ON s.agent = target.agent AND s.title = target.title AND s.expires > ?2
                     WHERE target.id = ?1
                     ORDER BY latest.at DESC, latest.rowid DESC
                     LIMIT 1",
                    params![id, epoch_ms() as i64],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| error.to_string())
        })
    }

    /// Page `id` is shared through `link` now, which keeps it. Answers whether a newer version
    /// arrived while this upload was in flight and still needs to upload.
    pub(crate) fn shared(
        &self,
        id: &str,
        link: &Link,
        expected_behind: Option<i64>,
    ) -> Result<bool, String> {
        self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let changed = tx
                .execute(
                    "UPDATE pages SET kept = 1,
                        seen = ?2,
                        settled = CASE WHEN settled IS NULL THEN NULL ELSE ?2 END
                     WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)",
                    params![id, epoch_ms() as i64],
                )
                .map_err(|error| error.to_string())?;
            if changed == 0 {
                return Err("that page is gone".into());
            }
            tx.execute(
                "INSERT INTO page_shares (agent, title, id, key, anyone, expires, theme, behind, problem, told)
                 SELECT agent, title, ?2, ?3, ?4, ?5, ?6, 0, NULL, ?8 FROM pages WHERE id = ?1
                 ON CONFLICT(agent, title) DO UPDATE SET
                     id = excluded.id,
                     key = excluded.key,
                     anyone = excluded.anyone,
                     expires = excluded.expires,
                     theme = excluded.theme,
                     told = excluded.told,
                     behind = CASE
                         WHEN page_shares.id = excluded.id AND page_shares.behind = ?7 THEN 0
                         WHEN page_shares.id != excluded.id THEN 0
                         ELSE page_shares.behind
                     END,
                     problem = NULL",
                params![
                    id,
                    link.id,
                    link.key,
                    link.anyone,
                    link.expires as i64,
                    link.theme,
                    expected_behind.unwrap_or(0),
                    link.told
                ],
            )
            .map_err(|error| error.to_string())?;
            let behind: i64 = tx
                .query_row(
                    "SELECT behind FROM page_shares WHERE id = ?1",
                    [&link.id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .unwrap_or(0);
            tx.commit().map_err(|error| error.to_string())?;
            Ok(behind > 0)
        })
    }

    /// Drops expired shares and lists the link ids of live shares waiting to upload.
    pub(crate) fn pending_shares(&self) -> Result<(Vec<String>, bool), String> {
        self.with(|connection| {
            let dropped = connection
                .execute(
                    "DELETE FROM page_shares WHERE expires <= ?1",
                    [epoch_ms() as i64],
                )
                .map_err(|error| error.to_string())?;
            let mut statement = connection
                .prepare_cached("SELECT id FROM page_shares WHERE behind > 0 AND problem IS NULL")
                .map_err(|error| error.to_string())?;
            let ids = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| error.to_string())?
                .collect::<rusqlite::Result<Vec<String>>>()
                .map_err(|error| error.to_string())?;
            Ok((ids, dropped > 0))
        })
    }

    /// The share `link_id` if it is still behind and not refused, with its newest page version.
    pub(crate) fn pending_for(&self, link_id: &str) -> Result<Option<PendingShare>, String> {
        self.with(|connection| {
            connection
                .query_row(
                    "SELECT s.id, s.key, s.anyone, s.expires, s.theme, s.behind, s.title, p.id, p.title, s.told
                     FROM page_shares AS s
                     LEFT JOIN pages AS p
                         ON p.agent = s.agent AND p.title = s.title
                     WHERE s.id = ?1 AND s.behind > 0 AND s.problem IS NULL
                     ORDER BY p.at DESC, p.rowid DESC
                     LIMIT 1",
                    [link_id],
                    |row| {
                        let page_title: Option<String> = row.get(8)?;
                        Ok(PendingShare {
                            link: Link {
                                id: row.get(0)?,
                                key: row.get(1)?,
                                anyone: row.get(2)?,
                                expires: row.get::<_, i64>(3)? as u64,
                                theme: row.get(4)?,
                                told: row.get(9)?,
                            },
                            behind: row.get(5)?,
                            title: page_title.unwrap_or(row.get(6)?),
                            page_id: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| error.to_string())
        })
    }

    /// Marks `link_id` caught up when no newer version arrived since `seen_behind`.
    /// Answers whether a newer version is still waiting.
    pub(crate) fn mark_synced(&self, link_id: &str, seen_behind: i64) -> Result<bool, String> {
        self.with(|connection| {
            connection
                .execute(
                    "UPDATE page_shares SET behind = 0, problem = NULL WHERE id = ?1 AND behind = ?2",
                    params![link_id, seen_behind],
                )
                .map_err(|error| error.to_string())?;
            let behind: i64 = connection
                .query_row(
                    "SELECT behind FROM page_shares WHERE id = ?1",
                    [link_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .unwrap_or(0);
            Ok(behind > 0)
        })
    }

    /// Records why `link_id` stopped retrying, if no newer version arrived since `seen_behind`.
    pub(crate) fn set_problem(
        &self,
        link_id: &str,
        seen_behind: i64,
        problem: &str,
    ) -> Result<bool, String> {
        self.with(|connection| {
            let changed = connection
                .execute(
                    "UPDATE page_shares SET problem = ?3 WHERE id = ?1 AND behind = ?2",
                    params![link_id, seen_behind, problem],
                )
                .map_err(|error| error.to_string())?;
            Ok(changed > 0)
        })
    }

    /// Clears `problem` on shares still behind so they retry again, such as after signing back in.
    pub(crate) fn clear_problem(&self, problem: &str) -> Result<usize, String> {
        self.with(|connection| {
            connection
                .execute(
                    "UPDATE page_shares SET problem = NULL WHERE problem = ?1 AND behind > 0",
                    [problem],
                )
                .map_err(|error| error.to_string())
        })
    }

    /// Drops a share by its link id, such as once it expired or ended on the server.
    pub(crate) fn drop_share(&self, link_id: &str) -> Result<bool, String> {
        self.with(|connection| {
            let dropped = connection
                .execute("DELETE FROM page_shares WHERE id = ?1", [link_id])
                .map_err(|error| error.to_string())?;
            Ok(dropped > 0)
        })
    }

    /// Page `id` is no longer shared. The link it was shared through, if it was.
    pub(crate) fn unshared(&self, id: &str) -> Result<Option<String>, String> {
        self.with(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let link = share_of_with(&tx, id)?;
            tx.execute(
                "DELETE FROM page_shares
                 WHERE (agent, title) = (SELECT agent, title FROM pages WHERE id = ?1)",
                [id],
            )
            .map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())?;
            Ok(link.map(|link| link.id))
        })
    }

    /// Indexes the pages kept before there was an index, by their title and when they were written.
    fn backfill(&self, pages: &Path) -> Result<(), String> {
        self.with(|connection| {
            let filled: i64 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if filled >= BACKFILLED_VERSION {
                return Ok(());
            }
            let now = epoch_ms() as i64;
            let entries: Vec<(String, String, i64)> = std::fs::read_dir(pages)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let id = (path.extension().is_some_and(|it| it == "html"))
                        .then(|| path.file_stem()?.to_str())
                        .flatten()
                        .filter(|id| crate::pages::is_page_id(id))?
                        .to_owned();
                    let title = crate::pages::title(pages, &id)?;
                    let at = (entry.metadata().ok())
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_millis() as i64);
                    Some((id, title, at))
                })
                .collect();
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            {
                let mut insert = tx
                    .prepare(
                        "INSERT OR IGNORE INTO pages (id, title, at, seen) VALUES (?1, ?2, ?3, ?4)",
                    )
                    .map_err(|error| error.to_string())?;
                for (id, title, at) in entries {
                    let _ = insert.execute(params![id, title, at, now]);
                }
            }
            tx.execute_batch(&format!("PRAGMA user_version = {BACKFILLED_VERSION};"))
                .map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())
        })
    }

    fn settle_and_sweep(&self, open_calls: &HashSet<String>, pages: &Path) -> Result<bool, String> {
        self.with(|connection| {
            let settled = settle(connection, open_calls).map_err(|error| error.to_string())?;
            let swept = sweep(connection, pages)?;
            let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            Ok(settled + swept > 0)
        })
    }
}

fn open(path: &Path) -> Result<Connection, String> {
    let failed = |error: rusqlite::Error| format!("the pages index could not be read: {error}");
    let _ = sikemux_process::files::private_database(path);
    let mut connection = Connection::open(path).map_err(failed)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(failed)?;
    connection
        .execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA journal_size_limit = {WAL_SIZE_LIMIT};"
        ))
        .map_err(failed)?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(failed)?;
    if version < SCHEMA_VERSION {
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
                    behind INTEGER NOT NULL DEFAULT 0,
                    problem TEXT,
                    told INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (agent, title)
                );
                CREATE INDEX IF NOT EXISTS page_shares_by_id ON page_shares(id);",
            )
            .map_err(failed)?;
        add_lifetimes(&mut connection).map_err(failed)?;
        add_share_sync(&mut connection).map_err(failed)?;
        add_told(&mut connection).map_err(failed)?;
        let next = if version == 1 {
            BACKFILLED_VERSION
        } else {
            SCHEMA_VERSION
        };
        connection
            .execute_batch(&format!("PRAGMA user_version = {next};"))
            .map_err(failed)?;
    }
    Ok(connection)
}

fn add_share_sync(connection: &mut Connection) -> rusqlite::Result<()> {
    let has = |connection: &Connection| {
        connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('page_shares') WHERE name = 'behind'",
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
            "ALTER TABLE page_shares ADD COLUMN behind INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE page_shares ADD COLUMN problem TEXT;",
        )?;
    }
    adding.commit()
}

/// Gives an index made before links sent a title what says which may.
fn add_told(connection: &mut Connection) -> rusqlite::Result<()> {
    let has = |connection: &Connection| {
        connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('page_shares') WHERE name = 'told'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };
    if has(connection)? > 0 {
        return Ok(());
    }
    let adding = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if has(&adding)? == 0 {
        adding
            .execute_batch("ALTER TABLE page_shares ADD COLUMN told INTEGER NOT NULL DEFAULT 0;")?;
    }
    adding.commit()
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

fn share_of_with(connection: &Connection, id: &str) -> Result<Option<Link>, String> {
    connection
        .query_row(
            "SELECT s.id, s.key, s.anyone, s.expires, s.theme, s.told FROM page_shares AS s
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
                    told: row.get(5)?,
                })
            },
        )
        .optional()
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
fn settle(connection: &mut Connection, open_calls: &HashSet<String>) -> rusqlite::Result<usize> {
    let to_mark: Vec<String> = connection
        .prepare_cached("SELECT id, call FROM pages WHERE call IS NOT NULL AND settled IS NULL")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|(id, call)| (!open_calls.contains(&call)).then_some(id))
        .collect();
    if to_mark.is_empty() {
        return Ok(0);
    }
    let now = epoch_ms() as i64;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut marked = 0;
    {
        let mut update = tx.prepare_cached("UPDATE pages SET settled = ?2 WHERE id = ?1")?;
        for id in &to_mark {
            marked += update.execute(params![id, now])?;
        }
    }
    tx.commit()?;
    Ok(marked)
}

/// Throws away the pages whose time is up, with their files, and drops the rows of pages whose
/// file is gone. How many rows went.
fn sweep(connection: &mut Connection, pages: &Path) -> Result<usize, String> {
    let now = epoch_ms();
    let rows: Vec<(String, PageLife)> = connection
        .prepare_cached("SELECT id, kept, call, settled, seen FROM pages")
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
            let _ = std::fs::remove_file(crate::pages::source_file(pages, &id));
            let _ = std::fs::remove_dir_all(crate::pages::media_folder(pages, &id));
        }
        if over || !pages.join(format!("{id}.html")).exists() {
            gone.push(id);
        }
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    {
        let mut remove = tx
            .prepare_cached("DELETE FROM pages WHERE id = ?1")
            .map_err(|error| error.to_string())?;
        for id in &gone {
            let _ = remove.execute([id]);
        }
    }
    let ended = tx
        .execute("DELETE FROM page_shares WHERE expires <= ?1", [now as i64])
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
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
        let index = core.harness().page_index.clone();
        let pages = pages.clone();
        let swept = super::blocking(move || {
            let pass = || {
                if !filled {
                    index.backfill(&pages)?;
                }
                index.settle_and_sweep(&open_calls, &pages)
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
