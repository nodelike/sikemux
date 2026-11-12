//! What was read from each session file, kept on disk. A listing reads only the files written
//! since it was last read, and a core that starts again reads none it read before.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{params, Connection};
use sikemux_process::sync::Locked;

use super::FileStamp;
use crate::agent_catalog::AgentKind;

const DATABASE: &str = "sessions.sqlite3";
/// Bump when what is read from a session file changes, so rows read the old way are dropped.
const READ_VERSION: i64 = 1;

/// What a listing needs from one session file.
#[derive(Default)]
pub(super) struct Indexed {
    pub(super) id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) title: Option<String>,
}

type Key = (AgentKind, PathBuf);

#[derive(Clone)]
struct Row {
    stamp: FileStamp,
    read: Arc<Indexed>,
}

struct Index {
    rows: Mutex<HashMap<Key, Row>>,
    writes: Option<Sender<(Key, Row)>>,
}

static PLACE: OnceLock<PathBuf> = OnceLock::new();

/// Keeps the index in `data_dir`. Without it the index lasts only as long as the process.
pub fn open(data_dir: &Path) {
    let _ = PLACE.set(data_dir.join(DATABASE));
}

/// What `read` finds in the file at `path`, read again only when the file was written since.
pub(super) fn indexed(
    agent: AgentKind,
    path: &Path,
    stamp: FileStamp,
    read: impl FnOnce() -> Indexed,
) -> Arc<Indexed> {
    let index = index();
    let key = (agent, path.to_path_buf());
    if let Some(row) = index
        .rows
        .locked()
        .get(&key)
        .filter(|row| row.stamp == stamp)
    {
        return row.read.clone();
    }
    let row = Row {
        stamp,
        read: Arc::new(read()),
    };
    if let Some(writes) = &index.writes {
        let _ = writes.send((key.clone(), row.clone()));
    }
    let read = row.read.clone();
    index.rows.locked().insert(key, row);
    read
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let opened = PLACE.get().and_then(|path| connect(path).ok());
        let Some(connection) = opened else {
            return Index {
                rows: Mutex::new(HashMap::new()),
                writes: None,
            };
        };
        let rows = load(&connection).unwrap_or_default();
        let known: Vec<Key> = rows.keys().cloned().collect();
        let (writes, written) = mpsc::channel();
        let writer = std::thread::Builder::new()
            .name("session-index".into())
            .spawn(move || keep(connection, known, written));
        Index {
            rows: Mutex::new(rows),
            writes: writer.is_ok().then_some(writes),
        }
    })
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != READ_VERSION {
        connection.execute_batch(&format!(
            "DROP TABLE IF EXISTS sessions; PRAGMA user_version = {READ_VERSION};"
        ))?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
             agent TEXT NOT NULL,
             path TEXT NOT NULL,
             modified_ns INTEGER NOT NULL,
             len INTEGER NOT NULL,
             id TEXT,
             cwd TEXT,
             title TEXT,
             PRIMARY KEY (agent, path)
         ) WITHOUT ROWID;",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(connection)
}

fn load(connection: &Connection) -> rusqlite::Result<HashMap<Key, Row>> {
    let mut statement =
        connection.prepare("SELECT agent, path, modified_ns, len, id, cwd, title FROM sessions")?;
    let rows = statement.query_map([], |row| {
        let agent: String = row.get(0)?;
        let path: String = row.get(1)?;
        let modified_ns: i64 = row.get(2)?;
        let len: i64 = row.get(3)?;
        let read = Indexed {
            id: row.get(4)?,
            cwd: row.get(5)?,
            title: row.get(6)?,
        };
        let stamp = FileStamp {
            modified_ns: modified_ns as u128,
            len: len as u64,
        };
        Ok(AgentKind::from_name(&agent).map(|agent| {
            let row = Row {
                stamp,
                read: Arc::new(read),
            };
            ((agent, PathBuf::from(path)), row)
        }))
    })?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

/// Drops the rows of files that are gone, then writes what is read as it comes, a batch at a time.
fn keep(mut connection: Connection, known: Vec<Key>, written: Receiver<(Key, Row)>) {
    let gone: Vec<Key> = known
        .into_iter()
        .filter(|(_, path)| !path.exists())
        .collect();
    if !gone.is_empty() {
        let mut rows = index().rows.locked();
        for key in &gone {
            rows.remove(key);
        }
        drop(rows);
        let _ = forget(&mut connection, &gone);
    }
    while let Ok(first) = written.recv() {
        let batch: Vec<(Key, Row)> = std::iter::once(first).chain(written.try_iter()).collect();
        let _ = store(&mut connection, &batch);
    }
}

fn forget(connection: &mut Connection, gone: &[Key]) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut statement =
            transaction.prepare("DELETE FROM sessions WHERE agent = ?1 AND path = ?2")?;
        for (agent, path) in gone {
            statement.execute(params![agent.as_str(), path.to_string_lossy()])?;
        }
    }
    transaction.commit()
}

fn store(connection: &mut Connection, batch: &[(Key, Row)]) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut statement = transaction.prepare(
            "INSERT OR REPLACE INTO sessions (agent, path, modified_ns, len, id, cwd, title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for ((agent, path), row) in batch {
            statement.execute(params![
                agent.as_str(),
                path.to_string_lossy(),
                row.stamp.modified_ns as i64,
                row.stamp.len as i64,
                row.read.id,
                row.read.cwd,
                row.read.title,
            ])?;
        }
    }
    transaction.commit()
}
