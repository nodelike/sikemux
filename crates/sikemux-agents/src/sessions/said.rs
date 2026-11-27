//! What the person and the agent wrote in every saved session, indexed on disk, so a chat
//! is found by its words. A session's transcript is read again only from where the last
//! read stopped, so a long chat costs only its new messages.

use std::collections::{HashMap, HashSet};
use std::fs::File;
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
/// How many words before the first ones found a line keeps.
const LEAD_WORDS: usize = 4;
const MARK_START: char = '\u{2}';
const MARK_END: char = '\u{3}';
/// Interactive search catches up at most this many newest sessions and bytes per file.
const FRESH_SEARCH_SESSIONS: usize = 64;
const FRESH_FILE_BYTES: u64 = 512 * 1024;
const COMMIT_BATCH: usize = 16;
const BACKGROUND_COOLDOWN: Duration = Duration::from_secs(60);

/// A line of a session that holds the words looked for, in runs that alternate between
/// the words found and what is around them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundLine {
    pub runs: Vec<FoundRun>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundRun {
    pub text: String,
    /// Whether these are words looked for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub found: bool,
}

impl FoundLine {
    /// The first words that matched, for a find in the chat to look for.
    pub fn first_found(&self) -> Option<&str> {
        let run = self.runs.iter().find(|run| run.found)?;
        Some(&run.text)
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

pub(super) fn is_open() -> bool {
    index().is_some()
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
    let _ = sikemux_process::files::private_database(path);
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA journal_size_limit = 16777216;
         PRAGMA cache_size = -2048;",
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
    let mut last_run: Option<Instant> = None;
    while let Ok(first) = asked.recv() {
        let mut providers = asked.try_iter().last().unwrap_or(first);
        if let Some(wait) = last_run.and_then(|at| BACKGROUND_COOLDOWN.checked_sub(at.elapsed())) {
            let deadline = Instant::now() + wait;
            while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                match asked.recv_timeout(remaining) {
                    Ok(newer) => providers = newer,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
        catch_up(&providers, None, MAX_SESSIONS, None);
        last_run = Some(Instant::now());
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
        let (written, ok) = catch_up_slice(index, hits.iter(), deadline, None, Some(&mut seen));
        read += written;
        if !ok {
            whole = false;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (read, 0);
        }
    }
    let gone = if whole {
        let agents: HashSet<&str> = providers.iter().map(|p| p.agent.as_str()).collect();
        index.locked().forget_unseen(&agents, &seen)
    } else {
        0
    };
    if read > 0 || gone > 0 {
        let mode = if whole { "TRUNCATE" } else { "PASSIVE" };
        let _ = index
            .locked()
            .connection
            .execute_batch(&format!("PRAGMA wal_checkpoint({mode});"));
    }
    (read, gone)
}

/// Catches up the newest scoped search hits within `deadline`, bounding bytes per file so
/// an unindexed huge transcript never stalls a search.
pub(super) fn catch_up_hits(hits: &[(Hit, &RecentProvider)], deadline: Instant) {
    let Some(index) = index() else {
        return;
    };
    let newest = hits.iter().take(FRESH_SEARCH_SESSIONS).map(|(hit, _)| hit);
    let _ = catch_up_slice(index, newest, Some(deadline), Some(FRESH_FILE_BYTES), None);
}

fn catch_up_slice<'a>(
    index: &Mutex<Index>,
    hits: impl Iterator<Item = &'a Hit>,
    deadline: Option<Instant>,
    max_bytes: Option<u64>,
    mut seen: Option<&mut HashSet<Key>>,
) -> (usize, bool) {
    let mut read = 0;
    let mut ok = true;
    let mut pending = hits.peekable();
    while pending.peek().is_some() {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (read, false);
        }
        let mut batch = Vec::new();
        let started = Instant::now();
        while batch.len() < COMMIT_BATCH {
            if !batch.is_empty() && started.elapsed() >= HOLD {
                break;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                ok = false;
                break;
            }
            let Some(hit) = pending.next() else {
                break;
            };
            let Some(agent) = AgentKind::from_name(hit.agent) else {
                continue;
            };
            let key = (hit.agent, hit.found.session.id.clone());
            let existing = {
                let guard = index.locked();
                guard.heard.get(&key).map(|heard| (heard.stamp, heard.read))
            };
            if let Some(seen) = seen.as_deref_mut() {
                seen.insert(key.clone());
            }
            if let Some(item) = read_session(agent, &hit.found, key, existing, deadline, max_bytes)
            {
                batch.push(item);
            }
        }
        if !batch.is_empty() {
            match index.locked().commit_batch(batch) {
                Ok(written) => read += written,
                Err(error) => {
                    eprintln!("sikemux: the index of what chats said could not take one: {error}");
                    ok = false;
                }
            }
        }
    }
    (read, ok)
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
        let end = text.floor_char_boundary(MAX_MESSAGE);
        self.0.push(text.get(..end).unwrap_or(text).to_owned());
    }
}

struct ReadSession {
    key: Key,
    stamp: FileStamp,
    from: u64,
    read: u64,
    messages: Vec<String>,
}

fn read_session(
    agent: AgentKind,
    found: &Found,
    key: Key,
    existing: Option<(FileStamp, u64)>,
    deadline: Option<Instant>,
    max_bytes: Option<u64>,
) -> Option<ReadSession> {
    let ops = agent.ops();
    let stamp = ops.prose_stamp(found)?;
    if existing.is_some_and(|(kept_stamp, _)| kept_stamp == stamp) {
        return None;
    }
    let by_line = ops
        .prose_line()
        .zip(found.place.as_deref())
        .filter(|(_, place)| !is_compressed(place));
    let mut messages = Messages::default();
    match by_line {
        Some((parse, place)) => {
            let from = existing
                .map(|(_, read)| read)
                .filter(|read| *read <= stamp.len)
                .unwrap_or(0);
            let (read, reached_eof) =
                read_lines_bounded(place, from, deadline, max_bytes, |line| {
                    parse(line, &mut messages)
                });
            if !reached_eof && read == from {
                return None;
            }
            let stored_stamp = if reached_eof || read >= stamp.len {
                stamp
            } else {
                FileStamp {
                    modified_ns: 0,
                    len: read,
                }
            };
            Some(ReadSession {
                key,
                stamp: stored_stamp,
                from,
                read,
                messages: messages.0,
            })
        }
        None => {
            ops.prose(found, &mut messages);
            Some(ReadSession {
                key,
                stamp,
                from: 0,
                read: 0,
                messages: messages.0,
            })
        }
    }
}

impl Index {
    fn commit_batch(&mut self, batch: Vec<ReadSession>) -> rusqlite::Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut next_number = self.next_number;
        let mut updates: Vec<(Key, Heard)> = Vec::with_capacity(batch.len());
        for item in batch {
            let current = self.heard.get(&item.key);
            if current.is_some_and(|heard| {
                heard.stamp == item.stamp || (item.from > 0 && heard.read > item.from)
            }) {
                continue;
            }
            let (number, kept) = match current {
                Some(heard) if item.from > 0 && heard.read == item.from => {
                    (heard.number, heard.messages)
                }
                Some(heard) => {
                    clear_in(&transaction, heard.number)?;
                    (heard.number, 0)
                }
                None => {
                    let allocated = next_number;
                    next_number += 1;
                    (allocated, 0)
                }
            };
            let mut count = kept;
            {
                let mut insert =
                    transaction.prepare_cached("INSERT INTO said (rowid, text) VALUES (?1, ?2)")?;
                for text in &item.messages {
                    if count >= MAX_MESSAGES {
                        break;
                    }
                    insert.execute(params![(number << MESSAGE_BITS) + count, text])?;
                    count += 1;
                }
            }
            transaction.execute(
                "INSERT INTO heard (number, agent, session, modified_ns, len, read, messages)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (agent, session) DO UPDATE SET
                     modified_ns = ?4, len = ?5, read = ?6, messages = ?7",
                params![
                    number,
                    item.key.0,
                    item.key.1,
                    item.stamp.modified_ns as i64,
                    item.stamp.len as i64,
                    item.read as i64,
                    count
                ],
            )?;
            updates.push((
                item.key,
                Heard {
                    number,
                    stamp: item.stamp,
                    read: item.read,
                    messages: count,
                },
            ));
        }
        transaction.commit()?;
        let written = updates.len();
        self.next_number = next_number;
        for (key, heard) in updates {
            self.heard.insert(key, heard);
        }
        Ok(written)
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
                clear_in(&transaction, *number)?;
                transaction.execute("DELETE FROM heard WHERE number = ?1", [number])?;
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

fn clear_in(connection: &Connection, number: i64) -> rusqlite::Result<()> {
    connection
        .prepare_cached("DELETE FROM said WHERE rowid >= ?1 AND rowid < ?2")?
        .execute(params![
            number << MESSAGE_BITS,
            (number + 1) << MESSAGE_BITS
        ])?;
    Ok(())
}

/// Reads whole lines of `path` from byte `from` on, stopping if `deadline` or `max_bytes`
/// is reached. Answers where the last whole line ended and whether EOF was reached.
fn read_lines_bounded(
    path: &Path,
    from: u64,
    deadline: Option<Instant>,
    max_bytes: Option<u64>,
    mut visit: impl FnMut(&str),
) -> (u64, bool) {
    let Ok(mut file) = File::open(path) else {
        return (from, true);
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return (from, true);
    }
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line = Vec::new();
    let mut at = from;
    let mut lines = 0usize;
    loop {
        if let Some(limit) = max_bytes {
            if at - from >= limit {
                return (at, false);
            }
        }
        if lines & 63 == 0 && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (at, false);
        }
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return (at, true),
            Ok(_) if line.last() != Some(&b'\n') => return (at, true),
            Ok(read) => at += read as u64,
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            visit(text.trim_end());
        }
        lines += 1;
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

pub(super) struct ScoredIndex {
    pub indexed: Vec<bool>,
    pub mentions: HashMap<usize, Vec<usize>>,
}

/// Counts per-word matching messages (up to `cap`) across indexed candidate sessions.
pub(super) fn score_hits(
    hits: &[(Hit, &RecentProvider)],
    words: &[String],
    cap: usize,
) -> Option<ScoredIndex> {
    let index = index()?.locked();
    let mut indexed = Vec::with_capacity(hits.len());
    let mut by_number: HashMap<i64, usize> = HashMap::new();
    for (i, (hit, _)) in hits.iter().enumerate() {
        if let Some(heard) = index.heard.get(&(hit.agent, hit.found.session.id.clone())) {
            indexed.push(true);
            by_number.insert(heard.number, i);
        } else {
            indexed.push(false);
        }
    }
    let mut mentions: HashMap<usize, Vec<usize>> = HashMap::new();
    if by_number.is_empty() || words.is_empty() {
        return Some(ScoredIndex { indexed, mentions });
    }
    let Ok(mut statement) = index
        .connection
        .prepare_cached("SELECT rowid FROM said WHERE said MATCH ?1")
    else {
        return Some(ScoredIndex { indexed, mentions });
    };
    for (word_idx, word) in words.iter().enumerate() {
        let query = format!("\"{word}\"*");
        let Ok(rows) = statement.query_map([&query], |row| row.get::<_, i64>(0)) else {
            continue;
        };
        let mut last_num = -1i64;
        let mut last_hit: Option<usize> = None;
        let mut last_count = 0usize;
        for rowid in rows.flatten() {
            let num = rowid >> MESSAGE_BITS;
            if num == last_num {
                if last_count < cap {
                    last_count += 1;
                }
                continue;
            }
            if let Some(hit_idx) = last_hit {
                if let Some(slot) = mentions
                    .entry(hit_idx)
                    .or_insert_with(|| vec![0; words.len()])
                    .get_mut(word_idx)
                {
                    *slot = last_count;
                }
            }
            last_num = num;
            last_hit = by_number.get(&num).copied();
            last_count = usize::from(last_hit.is_some());
        }
        if let Some(hit_idx) = last_hit {
            if let Some(slot) = mentions
                .entry(hit_idx)
                .or_insert_with(|| vec![0; words.len()])
                .get_mut(word_idx)
            {
                *slot = last_count;
            }
        }
    }
    Some(ScoredIndex { indexed, mentions })
}

/// Fetches the best matching indexed messages for each winning session for snippet extraction.
pub(super) fn matching_messages(
    sessions: &[(&str, &str)],
    words: &[String],
    per_session: usize,
) -> Vec<Vec<String>> {
    if sessions.is_empty() || words.is_empty() || per_session == 0 {
        return vec![Vec::new(); sessions.len()];
    }
    let Some(index) = index() else {
        return vec![Vec::new(); sessions.len()];
    };
    let index = index.locked();
    let query = words
        .iter()
        .map(|word| format!("\"{word}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let Ok(mut statement) = index.connection.prepare_cached(
        "SELECT text FROM said
         WHERE said MATCH ?1 AND rowid >= ?2 AND rowid < ?3
         ORDER BY bm25(said) LIMIT ?4",
    ) else {
        return vec![Vec::new(); sessions.len()];
    };
    sessions
        .iter()
        .map(|(agent, session)| {
            let Some(heard) = agent_name(agent)
                .and_then(|agent| index.heard.get(&(agent, (*session).to_owned())))
            else {
                return Vec::new();
            };
            statement
                .query_map(
                    params![
                        &query,
                        heard.number << MESSAGE_BITS,
                        (heard.number + 1) << MESSAGE_BITS,
                        per_session as i64,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        })
        .collect()
}

/// The newest message of the session holding every word, cut to the stretch around them.
pub fn excerpt(words: &Words, agent: &str, session: &str) -> Option<FoundLine> {
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

/// The snippet on one line, in runs split where its marks were.
fn marked(snippet: &str) -> FoundLine {
    let mut runs: Vec<FoundRun> = Vec::new();
    let mut run = FoundRun {
        text: String::new(),
        found: false,
    };
    let mut space = false;
    let mut started = false;
    for char in snippet.chars() {
        let found = match char {
            MARK_START => true,
            MARK_END => false,
            char if char.is_whitespace() => {
                space = started;
                continue;
            }
            char => {
                if std::mem::take(&mut space) {
                    run.text.push(' ');
                }
                run.text.push(char);
                started = true;
                continue;
            }
        };
        if std::mem::take(&mut space) {
            run.text.push(' ');
        }
        let ended = std::mem::replace(
            &mut run,
            FoundRun {
                text: String::new(),
                found,
            },
        );
        if !ended.text.is_empty() {
            runs.push(ended);
        }
    }
    if !run.text.is_empty() {
        runs.push(run);
    }
    lead_in(&mut runs);
    FoundLine { runs }
}

/// Keeps only the last few words before the first words found, so a narrow row still shows them.
fn lead_in(runs: &mut [FoundRun]) {
    let Some(first) = runs.first_mut().filter(|run| !run.found) else {
        return;
    };
    let words: Vec<&str> = first.text.split(' ').collect();
    if words.len() <= LEAD_WORDS + 1 {
        return;
    }
    let shown = words.get(words.len() - LEAD_WORDS - 1..).unwrap_or(&words);
    first.text = format!("…{}", shown.join(" "));
}
