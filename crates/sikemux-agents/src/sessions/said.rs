//! What the person and the agent wrote in every saved session, indexed on disk, so a chat
//! is found by its words. A session's transcript is read again only from where the last
//! read stopped, so a long chat costs only its new messages.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::recent::{provider_hits, Found, Hit, PageScan, RecentProvider};
use super::search::{Hear, Speaker};
use super::{is_compressed, FileStamp};
use crate::agent_catalog::AgentKind;

const DATABASE: &str = "said.sqlite3";
/// Bump when what is kept of a message changes, so the index is read afresh.
const READ_VERSION: i64 = 1;
/// The most sessions of one CLI the index keeps.
const MAX_SESSIONS: usize = 4_000;
/// A message's rowid is its session's number shifted by this, plus its place in the session.
const MESSAGE_BITS: u32 = 24;
const MAX_MESSAGES: i64 = (1 << MESSAGE_BITS) - 1;
const MAX_MESSAGE: usize = 16 * 1024;
/// How long the index is held at a time while it reads in the background.
const HOLD: Duration = Duration::from_millis(40);
/// The most sessions a search lists as holding the words.
const MAX_MATCHED: usize = 5_000;
const SNIPPET_WORDS: i64 = 14;
const MARK_START: char = '\u{2}';
const MARK_END: char = '\u{3}';

/// A line of a session that holds the words looked for, and where in it they are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    pub text: String,
    /// Byte ranges of `text` that matched.
    pub marks: Vec<[usize; 2]>,
}

impl Excerpt {
    /// The first words that matched, for a find in the chat to look for.
    pub fn first_mark(&self) -> Option<&str> {
        let [start, end] = *self.marks.first()?;
        self.text.get(start..end)
    }
}

struct Heard {
    number: i64,
    stamp: FileStamp,
    /// Bytes of its transcript read, for a transcript read a line at a time.
    read: u64,
    messages: i64,
}

type Key = (&'static str, String);

struct Index {
    connection: Connection,
    heard: HashMap<Key, Heard>,
    next_number: i64,
}

static PLACE: OnceLock<PathBuf> = OnceLock::new();

/// Keeps the index in `data_dir`. Without it nothing is indexed and no chat is found by its words.
pub fn open(data_dir: &Path) {
    let _ = PLACE.set(data_dir.join(DATABASE));
}

fn index() -> Option<&'static Mutex<Index>> {
    static INDEX: OnceLock<Option<Mutex<Index>>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let connection = connect(PLACE.get()?).ok()?;
            let heard = load(&connection).ok()?;
            let next_number = heard.values().map(|heard| heard.number).max().unwrap_or(0) + 1;
            Some(Mutex::new(Index {
                connection,
                heard,
                next_number,
            }))
        })
        .as_ref()
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -2048;",
    )?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != READ_VERSION {
        connection.execute_batch(&format!(
            "DROP TABLE IF EXISTS heard; DROP TABLE IF EXISTS said;
             PRAGMA user_version = {READ_VERSION};"
        ))?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS heard (
             number INTEGER PRIMARY KEY,
             agent TEXT NOT NULL,
             session TEXT NOT NULL,
             modified_ns INTEGER NOT NULL,
             len INTEGER NOT NULL,
             read INTEGER NOT NULL,
             messages INTEGER NOT NULL,
             UNIQUE (agent, session)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS said USING fts5(
             text, tokenize = 'unicode61 remove_diacritics 2', detail = none
         );",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(connection)
}

fn load(connection: &Connection) -> rusqlite::Result<HashMap<Key, Heard>> {
    let mut statement = connection
        .prepare("SELECT number, agent, session, modified_ns, len, read, messages FROM heard")?;
    let rows = statement.query_map([], |row| {
        let agent: String = row.get(1)?;
        let heard = Heard {
            number: row.get(0)?,
            stamp: FileStamp {
                modified_ns: row.get::<_, i64>(3)? as u128,
                len: row.get::<_, i64>(4)? as u64,
            },
            read: row.get::<_, i64>(5)? as u64,
            messages: row.get(6)?,
        };
        let session: String = row.get(2)?;
        Ok(AgentKind::from_name(&agent).map(|agent| ((agent.as_str(), session), heard)))
    })?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

/// Brings the index up to date with every session `providers` keep, in the background.
pub fn heed(providers: Vec<RecentProvider>) {
    static READER: OnceLock<Option<Sender<Vec<RecentProvider>>>> = OnceLock::new();
    let reader = READER.get_or_init(|| {
        index()?;
        let (send, asked) = mpsc::channel();
        std::thread::Builder::new()
            .name("said-index".into())
            .spawn(move || read_in_background(asked))
            .ok()?;
        Some(send)
    });
    if let Some(reader) = reader {
        let _ = reader.send(providers);
    }
}

fn read_in_background(asked: Receiver<Vec<RecentProvider>>) {
    lower_priority();
    while let Ok(first) = asked.recv() {
        let providers = asked.try_iter().last().unwrap_or(first);
        catch_up(&providers, None, MAX_SESSIONS, None);
    }
}

#[cfg(target_os = "macos")]
fn lower_priority() {
    // SAFETY: it only lowers the calling thread's own priority.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn lower_priority() {}

/// Reads what changed in the newest `limit` sessions of each provider in `projects`, or
/// every project, until `deadline`. Answers how many were read, and how many were gone.
/// A whole pass, with no projects and no deadline, also forgets sessions that are gone.
pub fn catch_up(
    providers: &[RecentProvider],
    projects: Option<&HashSet<String>>,
    limit: usize,
    deadline: Option<Instant>,
) -> (usize, usize) {
    let Some(index) = index() else {
        return (0, 0);
    };
    let exclude = HashSet::new();
    let mut seen: HashSet<Key> = HashSet::new();
    let mut read = 0;
    let mut whole = projects.is_none() && deadline.is_none();
    for provider in providers {
        let scan = PageScan::all(provider.agent, projects, limit, &exclude);
        let hits: Vec<Hit> = provider_hits(provider, &scan);
        let mut pending = hits.iter().peekable();
        while pending.peek().is_some() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return (read, 0);
            }
            let mut index = index.locked();
            let held = Instant::now();
            if index.connection.execute_batch("BEGIN").is_err() {
                return (read, 0);
            }
            while let Some(hit) = pending.next_if(|_| held.elapsed() < HOLD) {
                let key = (hit.agent, hit.found.session.id.clone());
                match index.hear(provider.agent, &hit.found, &key) {
                    Ok(changed) => read += usize::from(changed),
                    Err(error) => {
                        eprintln!(
                            "sikemux: the index of what chats said could not take one: {error}"
                        );
                        whole = false;
                    }
                }
                seen.insert(key);
            }
            if index.connection.execute_batch("COMMIT").is_err() {
                let _ = index.connection.execute_batch("ROLLBACK");
                whole = false;
            }
        }
    }
    let gone = if whole {
        let agents: HashSet<&str> = providers.iter().map(|p| p.agent.as_str()).collect();
        index.locked().forget_unseen(&agents, &seen)
    } else {
        0
    };
    (read, gone)
}

/// Collects what a reader hears, a message at a time.
#[derive(Default)]
struct Messages(Vec<String>);

impl Hear for Messages {
    fn push(&mut self, _by: Speaker, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut end = text.len().min(MAX_MESSAGE);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push(text[..end].to_owned());
    }
}

impl Index {
    /// Reads what changed in the session since it was last read. Whether anything had.
    fn hear(&mut self, agent: AgentKind, found: &Found, key: &Key) -> rusqlite::Result<bool> {
        let ops = agent.ops();
        let Some(stamp) = ops.prose_stamp(found) else {
            return Ok(false);
        };
        if self
            .heard
            .get(key)
            .is_some_and(|heard| heard.stamp == stamp)
        {
            return Ok(false);
        }
        let by_line = ops
            .prose_line()
            .zip(found.place.as_deref())
            .filter(|(_, place)| !is_compressed(place));
        let mut messages = Messages::default();
        let (from, read) = match by_line {
            Some((parse, place)) => {
                let from = self
                    .heard
                    .get(key)
                    .map(|heard| heard.read)
                    .filter(|read| *read <= stamp.len)
                    .unwrap_or(0);
                let read = read_lines(place, from, |line| parse(line, &mut messages));
                (from, read)
            }
            None => {
                ops.prose(found, &mut messages);
                (0, 0)
            }
        };
        let (number, kept) = match self.heard.get(key) {
            Some(heard) if from > 0 => (heard.number, heard.messages),
            Some(heard) => {
                self.clear(heard.number)?;
                (heard.number, 0)
            }
            None => {
                self.next_number += 1;
                (self.next_number - 1, 0)
            }
        };
        let mut insert = self
            .connection
            .prepare_cached("INSERT INTO said (rowid, text) VALUES (?1, ?2)")?;
        let mut count = kept;
        for text in &messages.0 {
            if count >= MAX_MESSAGES {
                break;
            }
            insert.execute(params![(number << MESSAGE_BITS) + count, text])?;
            count += 1;
        }
        drop(insert);
        self.connection.execute(
            "INSERT INTO heard (number, agent, session, modified_ns, len, read, messages)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (agent, session) DO UPDATE SET
                 modified_ns = ?4, len = ?5, read = ?6, messages = ?7",
            params![
                number,
                key.0,
                key.1,
                stamp.modified_ns as i64,
                stamp.len as i64,
                read as i64,
                count
            ],
        )?;
        self.heard.insert(
            key.clone(),
            Heard {
                number,
                stamp,
                read,
                messages: count,
            },
        );
        Ok(true)
    }

    fn clear(&self, number: i64) -> rusqlite::Result<()> {
        self.connection
            .prepare_cached("DELETE FROM said WHERE rowid >= ?1 AND rowid < ?2")?
            .execute(params![
                number << MESSAGE_BITS,
                (number + 1) << MESSAGE_BITS
            ])?;
        Ok(())
    }

    /// Forgets the sessions of `agents` a whole pass did not see. How many it forgot.
    fn forget_unseen(&mut self, agents: &HashSet<&str>, seen: &HashSet<Key>) -> usize {
        let gone: Vec<(Key, i64)> = self
            .heard
            .iter()
            .filter(|(key, _)| agents.contains(key.0) && !seen.contains(*key))
            .map(|(key, heard)| (key.clone(), heard.number))
            .collect();
        if gone.is_empty() {
            return 0;
        }
        let forgot = (|| -> rusqlite::Result<()> {
            let transaction = self.connection.unchecked_transaction()?;
            for (_, number) in &gone {
                self.clear(*number)?;
                self.connection
                    .execute("DELETE FROM heard WHERE number = ?1", [number])?;
            }
            transaction.commit()
        })();
        if forgot.is_err() {
            return 0;
        }
        for (key, _) in &gone {
            self.heard.remove(key);
        }
        gone.len()
    }
}

/// Reads the whole lines of `path` from byte `from` on. Answers where the last whole line ends.
fn read_lines(path: &Path, from: u64, mut visit: impl FnMut(&str)) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return from;
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return from;
    }
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line = Vec::new();
    let mut at = from;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return at,
            Ok(_) if line.last() != Some(&b'\n') => return at,
            Ok(read) => at += read as u64,
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            visit(text.trim_end());
        }
    }
}

/// The words of a query, each looked for at the start of a word, all of them in one message.
pub struct Words(String);

impl Words {
    /// None when the query has too little to look for in what was said.
    pub fn parse(query: &str) -> Option<Self> {
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(|word| format!("\"{}\"*", word.to_lowercase()))
            .collect();
        let letters: usize = query.chars().filter(|c| c.is_alphanumeric()).count();
        (letters >= 3).then(|| Words(words.join(" ")))
    }
}

/// The sessions, by agent and id, with a message holding every word.
pub fn matching(words: &Words) -> HashSet<Key> {
    let Some(index) = index() else {
        return HashSet::new();
    };
    let index = index.locked();
    let numbers = (|| -> rusqlite::Result<HashSet<i64>> {
        let mut statement = index.connection.prepare_cached(&format!(
            "SELECT DISTINCT rowid >> {MESSAGE_BITS} FROM said WHERE said MATCH ?1 LIMIT {MAX_MATCHED}"
        ))?;
        let numbers = statement.query_map([&words.0], |row| row.get(0))?;
        numbers.collect()
    })()
    .unwrap_or_default();
    index
        .heard
        .iter()
        .filter(|(_, heard)| numbers.contains(&heard.number))
        .map(|(key, _)| key.clone())
        .collect()
}

/// The newest message of the session holding every word, cut to the stretch around them.
pub fn excerpt(words: &Words, agent: &str, session: &str) -> Option<Excerpt> {
    let index = index()?.locked();
    let number = index
        .heard
        .get(&(agent_name(agent)?, session.to_owned()))?
        .number;
    let snippet: String = index
        .connection
        .prepare_cached(&format!(
            "SELECT snippet(said, 0, char(2), char(3), '…', {SNIPPET_WORDS}) FROM said
             WHERE said MATCH ?1 AND rowid >= ?2 AND rowid < ?3 ORDER BY rowid DESC LIMIT 1"
        ))
        .ok()?
        .query_row(
            params![
                words.0,
                number << MESSAGE_BITS,
                (number + 1) << MESSAGE_BITS
            ],
            |row| row.get(0),
        )
        .optional()
        .ok()??;
    Some(marked(&snippet))
}

fn agent_name(agent: &str) -> Option<&'static str> {
    AgentKind::from_name(agent).map(AgentKind::as_str)
}

/// The snippet's text on one line, with where its marks were.
fn marked(snippet: &str) -> Excerpt {
    let mut text = String::with_capacity(snippet.len());
    let mut marks = Vec::new();
    let mut open = None;
    let mut space = false;
    for char in snippet.chars() {
        match char {
            MARK_START => {
                if std::mem::take(&mut space) {
                    text.push(' ');
                }
                open = Some(text.len());
            }
            MARK_END => {
                if let Some(start) = open.take() {
                    marks.push([start, text.len()]);
                }
            }
            char if char.is_whitespace() => space = !text.is_empty(),
            char => {
                if std::mem::take(&mut space) {
                    text.push(' ');
                }
                text.push(char);
            }
        }
    }
    Excerpt { text, marks }
}
