use crate::agents::{session_transcript_path, AgentKind};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::async_runtime::spawn_blocking;

const DAY_MS: i64 = 86_400_000;
const HEATMAP_DAYS: i64 = 371;
const MAX_TRANSCRIPT_READ: u64 = 64 * 1024 * 1024;
const MAX_TURN_COMMITS: usize = 500;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

fn database_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "activity.dev.sqlite3"
    } else {
        "activity.sqlite3"
    };
    Some(crate::state::state_path()?.with_file_name(filename))
}

fn database() -> MutexGuard<'static, Option<Connection>> {
    static DATABASE: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    DATABASE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(3))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS launches (
             at_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL,
             transport TEXT NOT NULL,
             resumed INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS turns (
             started_ms INTEGER NOT NULL,
             ended_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS commits (
             sha TEXT PRIMARY KEY,
             at_ms INTEGER NOT NULL,
             project TEXT NOT NULL,
             agent TEXT
         );
         CREATE TABLE IF NOT EXISTS tokens (
             at_ms INTEGER NOT NULL,
             agent TEXT NOT NULL,
             project TEXT NOT NULL,
             input INTEGER NOT NULL,
             output INTEGER NOT NULL,
             cache_read INTEGER NOT NULL,
             cache_write INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS transcripts (
             session TEXT PRIMARY KEY,
             path TEXT NOT NULL,
             offset INTEGER NOT NULL,
             mark TEXT NOT NULL
         );",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(connection)
}

/// Runs `work` against the activity database. Activity is a record, never a
/// reason for the action it describes to fail, so errors are dropped here.
fn with_database<T>(work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
    let path = database_path()?;
    let mut cache = database();
    if cache.is_none() {
        *cache = Some(open(&path).ok()?);
    }
    let result = work(cache.as_ref()?);
    if result.is_err() {
        *cache = None;
    }
    result.ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn git(repo: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(repo)
        .args(args);
    let output =
        crate::bounded_process::run(&mut command, None, GIT_TIMEOUT, 1024 * 1024, None).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn head(repo: &str) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
}

fn parse_agent(agent: &str) -> Option<AgentKind> {
    serde_json::from_value(Value::String(agent.to_string())).ok()
}

// ---- launches ----------------------------------------------------------

pub fn record_launch(
    agent: &str,
    project: &str,
    transport: &str,
    resumed_session: Option<&str>,
    config_path: Option<&str>,
) {
    let at_ms = now_ms();
    with_database(|db| {
        db.execute(
            "INSERT INTO launches (at_ms, agent, project, transport, resumed) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![at_ms, agent, project, transport, resumed_session.is_some()],
        )
    });
    if let (Some(kind), Some(session)) = (parse_agent(agent), resumed_session) {
        skip_earlier_history(kind, project, session, config_path);
    }
}

/// A loaded session's transcript already holds turns from before it came
/// into Sikemux. Only what is written from here on is Sikemux's.
fn skip_earlier_history(kind: AgentKind, cwd: &str, session: &str, config_path: Option<&str>) {
    let Some(path) = session_transcript_path(kind, cwd, session, config_path) else {
        return;
    };
    let Ok(len) = fs::metadata(&path).map(|meta| meta.len()) else {
        return;
    };
    let key = transcript_key(kind, session);
    with_database(|db| {
        db.execute(
            "INSERT INTO transcripts (session, path, offset, mark) VALUES (?1, ?2, ?3, '')
             ON CONFLICT(session) DO UPDATE SET path = excluded.path, offset = excluded.offset, mark = ''",
            params![key, path.to_string_lossy(), len as i64],
        )
    });
}

/// The session a direct agent command resumes, read from the arguments
/// `agentLaunchArgs` builds on the frontend.
pub fn resumed_session_in_args<'a>(agent: &str, args: &'a [String]) -> Option<&'a str> {
    if agent == "codex" {
        return (args.first().map(String::as_str) == Some("resume"))
            .then(|| args.last().map(String::as_str))
            .flatten();
    }
    let flag = match agent {
        "claude" | "hermes" | "omp" | "grok" => "--resume",
        "pi" | "opencode" => "--session",
        _ => return None,
    };
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

// ---- turns -------------------------------------------------------------

struct OpenTurn {
    started_ms: i64,
    head: Option<String>,
}

fn open_turns() -> MutexGuard<'static, HashMap<String, OpenTurn>> {
    static TURNS: OnceLock<Mutex<HashMap<String, OpenTurn>>> = OnceLock::new();
    TURNS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tauri::command]
pub async fn activity_turn_started(agent_id: String, cwd: String) {
    let started_ms = now_ms();
    {
        let mut turns = open_turns();
        if turns.contains_key(&agent_id) {
            return;
        }
        turns.insert(
            agent_id.clone(),
            OpenTurn {
                started_ms,
                head: None,
            },
        );
    }
    let head = spawn_blocking(move || head(&cwd)).await.ok().flatten();
    if let Some(turn) = open_turns()
        .get_mut(&agent_id)
        .filter(|turn| turn.started_ms == started_ms)
    {
        turn.head = head;
    }
}

#[tauri::command]
pub async fn activity_turn_ended(
    agent_id: String,
    agent: String,
    cwd: String,
    session_id: Option<String>,
    config_path: Option<String>,
) {
    let Some(turn) = open_turns().remove(&agent_id) else {
        return;
    };
    let ended_ms = now_ms();
    let _ = spawn_blocking(move || {
        with_database(|db| {
            db.execute(
                "INSERT INTO turns (started_ms, ended_ms, agent, project) VALUES (?1, ?2, ?3, ?4)",
                params![turn.started_ms, ended_ms, agent, cwd],
            )
        });
        if let Some(start) = turn.head.as_deref() {
            record_commits_since(&cwd, start, Some(&agent));
        }
        if let (Some(kind), Some(session)) = (parse_agent(&agent), session_id.as_deref()) {
            record_new_tokens(kind, &cwd, session, config_path.as_deref(), ended_ms);
        }
    })
    .await;
}

// ---- commits -----------------------------------------------------------

fn record_commits_since(repo: &str, start: &str, agent: Option<&str>) {
    let range = format!("{start}..HEAD");
    let Some(log) = git(repo, &["log", "--format=%H %ct", &range]) else {
        return;
    };
    let commits: Vec<(&str, i64)> = log
        .lines()
        .take(MAX_TURN_COMMITS)
        .filter_map(|line| {
            let (sha, seconds) = line.split_once(' ')?;
            Some((sha, seconds.trim().parse::<i64>().ok()? * 1000))
        })
        .collect();
    insert_commits(repo, &commits, agent);
}

fn insert_commits(repo: &str, commits: &[(&str, i64)], agent: Option<&str>) {
    if commits.is_empty() {
        return;
    }
    with_database(|db| {
        let mut insert = db.prepare_cached(
            "INSERT OR IGNORE INTO commits (sha, at_ms, project, agent) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (sha, at_ms) in commits {
            insert.execute(params![sha, at_ms, repo, agent])?;
        }
        Ok(())
    });
}

/// A commit made from Sikemux's own commit box.
pub fn record_commit(repo: &str) {
    if let Some(sha) = head(repo) {
        insert_commits(repo, &[(&sha, now_ms())], None);
    }
}

// ---- tokens ------------------------------------------------------------

#[derive(Default, Debug, PartialEq)]
struct TokenCount {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl TokenCount {
    fn is_empty(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write == 0
    }
}

fn transcript_key(kind: AgentKind, session: &str) -> String {
    format!("{}:{session}", kind.as_str())
}

fn record_new_tokens(
    kind: AgentKind,
    cwd: &str,
    session: &str,
    config_path: Option<&str>,
    at_ms: i64,
) {
    let key = transcript_key(kind, session);
    let known = with_database(|db| {
        db.query_row(
            "SELECT path, offset, mark FROM transcripts WHERE session = ?1",
            params![key],
            |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
    })
    .flatten()
    .filter(|(path, _, _)| path.is_file());
    let (path, offset, mark) = match known {
        Some(known) => known,
        None => match session_transcript_path(kind, cwd, session, config_path) {
            Some(path) => (path, 0, String::new()),
            None => return,
        },
    };
    let Some((text, next_offset)) = read_complete_lines(&path, offset) else {
        return;
    };
    let (count, next_mark) = match kind {
        AgentKind::Claude => count_claude_tokens(&text, &mark),
        AgentKind::Codex => count_codex_tokens(&text, &mark),
        _ => return,
    };
    with_database(|db| {
        db.execute(
            "INSERT INTO transcripts (session, path, offset, mark) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session) DO UPDATE SET path = excluded.path, offset = excluded.offset, mark = excluded.mark",
            params![key, path.to_string_lossy(), next_offset as i64, next_mark],
        )?;
        if !count.is_empty() {
            db.execute(
                "INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    at_ms,
                    kind.as_str(),
                    cwd,
                    count.input as i64,
                    count.output as i64,
                    count.cache_read as i64,
                    count.cache_write as i64
                ],
            )?;
        }
        Ok(())
    });
}

/// Everything after `offset` up to the last full line, and where that ends.
/// A file that shrank was rewritten, so reading restarts past its new end.
fn read_complete_lines(path: &Path, offset: u64) -> Option<(String, u64)> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < offset {
        return Some((String::new(), len));
    }
    let start = offset.max(len.saturating_sub(MAX_TRANSCRIPT_READ));
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(len - start).read_to_end(&mut bytes).ok()?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    bytes.truncate(complete);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some((text, start + complete as u64))
}

/// Claude writes one line per content block, each repeating its message's
/// usage, so a message counts once. `mark` is the last message counted.
fn count_claude_tokens(text: &str, mark: &str) -> (TokenCount, String) {
    let mut count = TokenCount::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut last = mark.to_string();
    if !mark.is_empty() {
        seen.insert(mark.to_string());
    }
    for line in text.lines() {
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
            continue;
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !id.is_empty() && !seen.insert(id.clone()) {
            continue;
        }
        let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        count.input += field("input_tokens");
        count.output += field("output_tokens");
        count.cache_read += field("cache_read_input_tokens");
        count.cache_write += field("cache_creation_input_tokens");
        if !id.is_empty() {
            last = id;
        }
    }
    (count, last)
}

/// Codex reports each turn's usage beside a running total, and sometimes
/// repeats a report. `mark` is the running total last counted.
fn count_codex_tokens(text: &str, mark: &str) -> (TokenCount, String) {
    let mut count = TokenCount::default();
    let mut last = mark.to_string();
    for line in text.lines() {
        if !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(info) = value.pointer("/payload/info") else {
            continue;
        };
        let Some(total) = info
            .pointer("/total_token_usage/total_tokens")
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let total = total.to_string();
        if total == last {
            continue;
        }
        last = total;
        let Some(turn) = info.get("last_token_usage") else {
            continue;
        };
        let field = |key: &str| turn.get(key).and_then(Value::as_u64).unwrap_or(0);
        let cached = field("cached_input_tokens");
        count.input += field("input_tokens").saturating_sub(cached);
        count.cache_read += cached;
        count.output += field("output_tokens");
    }
    (count, last)
}

// ---- summary -----------------------------------------------------------

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTotals {
    sessions: i64,
    resumed: i64,
    turns: i64,
    agent_ms: i64,
    commits: i64,
    agent_commits: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    first_at_ms: Option<i64>,
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    day: i64,
    sessions: i64,
    agent_ms: i64,
    commits: i64,
    tokens: i64,
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityShare {
    name: String,
    sessions: i64,
    agent_ms: i64,
    commits: i64,
    tokens: i64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummary {
    totals: ActivityTotals,
    /// Local calendar days, counted from the Unix epoch, with anything on them.
    days: Vec<ActivityDay>,
    agents: Vec<ActivityShare>,
    projects: Vec<ActivityShare>,
}

/// A query yielding `(at_ms, agent, project, amount)` rows, and where each amount adds up.
type Tally = (
    &'static str,
    fn(&mut ActivityDay, i64),
    fn(&mut ActivityShare, i64),
);

fn summarize(db: &Connection, offset_ms: i64, now_ms: i64) -> rusqlite::Result<ActivitySummary> {
    let mut totals = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(resumed), 0), MIN(at_ms) FROM launches",
        [],
        |row| {
            Ok(ActivityTotals {
                sessions: row.get(0)?,
                resumed: row.get(1)?,
                first_at_ms: row.get(2)?,
                ..ActivityTotals::default()
            })
        },
    )?;
    (totals.turns, totals.agent_ms) = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(ended_ms - started_ms), 0) FROM turns",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    (totals.commits, totals.agent_commits) =
        db.query_row("SELECT COUNT(*), COUNT(agent) FROM commits", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    (
        totals.input,
        totals.output,
        totals.cache_read,
        totals.cache_write,
    ) = db.query_row(
        "SELECT COALESCE(SUM(input), 0), COALESCE(SUM(output), 0),
                COALESCE(SUM(cache_read), 0), COALESCE(SUM(cache_write), 0) FROM tokens",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    let first_day = (now_ms + offset_ms).div_euclid(DAY_MS) - HEATMAP_DAYS;
    let mut days: HashMap<i64, ActivityDay> = HashMap::new();
    let mut agents: HashMap<String, ActivityShare> = HashMap::new();
    let mut projects: HashMap<String, ActivityShare> = HashMap::new();
    let queries: [Tally; 4] = [
        (
            "SELECT at_ms, agent, project, 1 FROM launches",
            |day, n| day.sessions += n,
            |share, n| share.sessions += n,
        ),
        (
            "SELECT started_ms, agent, project, ended_ms - started_ms FROM turns",
            |day, n| day.agent_ms += n,
            |share, n| share.agent_ms += n,
        ),
        (
            "SELECT at_ms, agent, project, 1 FROM commits",
            |day, n| day.commits += n,
            |share, n| share.commits += n,
        ),
        (
            "SELECT at_ms, agent, project, input + output + cache_write FROM tokens",
            |day, n| day.tokens += n,
            |share, n| share.tokens += n,
        ),
    ];
    for (sql, add_day, add_share) in queries {
        let mut statement = db.prepare(sql)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let at_ms: i64 = row.get(0)?;
            let agent: Option<String> = row.get(1)?;
            let project: String = row.get(2)?;
            let amount: i64 = row.get(3)?;
            let day = (at_ms + offset_ms).div_euclid(DAY_MS);
            if day > first_day {
                add_day(
                    days.entry(day).or_insert_with(|| ActivityDay {
                        day,
                        ..ActivityDay::default()
                    }),
                    amount,
                );
            }
            if let Some(agent) = agent {
                add_share(
                    agents
                        .entry(agent.clone())
                        .or_insert_with(|| ActivityShare {
                            name: agent,
                            ..ActivityShare::default()
                        }),
                    amount,
                );
            }
            add_share(
                projects
                    .entry(project.clone())
                    .or_insert_with(|| ActivityShare {
                        name: project,
                        ..ActivityShare::default()
                    }),
                amount,
            );
        }
    }
    let mut days: Vec<ActivityDay> = days.into_values().collect();
    days.sort_by_key(|day| day.day);
    let ranked = |shares: HashMap<String, ActivityShare>| {
        let mut shares: Vec<ActivityShare> = shares.into_values().collect();
        shares.sort_by(|a, b| {
            (b.agent_ms, b.sessions, b.commits)
                .cmp(&(a.agent_ms, a.sessions, a.commits))
                .then_with(|| a.name.cmp(&b.name))
        });
        shares
    };
    Ok(ActivitySummary {
        totals,
        days,
        agents: ranked(agents),
        projects: ranked(projects),
    })
}

/// `utc_offset_minutes` places each event on the user's own calendar day.
#[tauri::command]
pub async fn activity_summary(utc_offset_minutes: i64) -> ActivitySummary {
    let offset_ms = utc_offset_minutes.clamp(-24 * 60, 24 * 60) * 60_000;
    spawn_blocking(move || with_database(|db| summarize(db, offset_ms, now_ms())))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_line(id: &str, input: u64, output: u64, read: u64, write: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{write}}}}}}}"#
        )
    }

    #[test]
    fn a_claude_message_split_across_lines_counts_once() {
        let text = [
            claude_line("m1", 2, 40, 100, 10),
            claude_line("m1", 2, 40, 100, 10),
            claude_line("m2", 3, 5, 200, 0),
        ]
        .join("\n");
        let (count, mark) = count_claude_tokens(&text, "");
        assert_eq!(
            count,
            TokenCount {
                input: 5,
                output: 45,
                cache_read: 300,
                cache_write: 10
            }
        );
        assert_eq!(mark, "m2");
    }

    #[test]
    fn a_claude_message_counted_before_the_last_read_is_not_counted_again() {
        let text = [
            claude_line("m2", 3, 5, 200, 0),
            claude_line("m3", 1, 1, 0, 0),
        ]
        .join("\n");
        let (count, _) = count_claude_tokens(&text, "m2");
        assert_eq!(count.output, 1);
    }

    #[test]
    fn a_repeated_codex_report_counts_once() {
        let report = |total: u64, input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
            )
        };
        let text = [
            report(100, 90, 60, 10),
            report(100, 90, 60, 10),
            report(150, 45, 40, 5),
        ]
        .join("\n");
        let (count, mark) = count_codex_tokens(&text, "");
        assert_eq!(
            count,
            TokenCount {
                input: 35,
                output: 15,
                cache_read: 100,
                cache_write: 0
            }
        );
        assert_eq!(mark, "150");
    }

    #[test]
    fn only_complete_lines_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        fs::write(&path, "one\ntwo\nthr").unwrap();
        let (text, next) = read_complete_lines(&path, 4).unwrap();
        assert_eq!(text, "two\n");
        assert_eq!(next, 8);
    }

    #[test]
    fn a_resumed_session_is_found_in_its_launch_arguments() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            resumed_session_in_args("claude", &args(&["--model", "opus", "--resume", "abc"])),
            Some("abc")
        );
        assert_eq!(
            resumed_session_in_args("codex", &args(&["resume", "--sandbox", "x", "abc"])),
            Some("abc")
        );
        assert_eq!(
            resumed_session_in_args("pi", &args(&["--session", "s1"])),
            Some("s1")
        );
        assert_eq!(
            resumed_session_in_args("claude", &args(&["--model", "opus"])),
            None
        );
    }

    #[test]
    fn the_summary_buckets_events_on_local_days_and_ranks_by_agent_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("a.sqlite3")).unwrap();
        let day = 20_000 * DAY_MS;
        db.execute_batch(&format!(
            "INSERT INTO launches VALUES ({day}, 'claude', '/p/a', 'chat', 0);
             INSERT INTO launches VALUES ({day}, 'codex', '/p/b', 'terminal', 1);
             INSERT INTO turns VALUES ({day}, {}, 'codex', '/p/b');
             INSERT INTO commits VALUES ('s1', {day}, '/p/b', 'codex');
             INSERT INTO commits VALUES ('s2', {day}, '/p/b', NULL);
             INSERT INTO tokens VALUES ({day}, 'claude', '/p/a', 1, 2, 50, 3);",
            day + 60_000
        ))
        .unwrap();
        let summary = summarize(&db, -60_000, day + DAY_MS).unwrap();
        assert_eq!(summary.totals.sessions, 2);
        assert_eq!(summary.totals.resumed, 1);
        assert_eq!(summary.totals.agent_ms, 60_000);
        assert_eq!(summary.totals.commits, 2);
        assert_eq!(summary.totals.agent_commits, 1);
        assert_eq!(summary.days.len(), 1);
        assert_eq!(summary.days[0].day, 19_999);
        assert_eq!(summary.days[0].tokens, 6);
        assert_eq!(summary.agents[0].name, "codex");
        assert_eq!(summary.projects[0].name, "/p/b");
        assert_eq!(summary.projects[0].commits, 2);
    }
}
