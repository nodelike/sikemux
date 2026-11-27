use std::collections::{BTreeMap, HashMap};
use std::future::ready;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use sikemux_process::paths::home;

use super::grok::percent_decode;
use super::recent::{Found, Hit, Listed, PageScan};
use super::AgentSession;
use super::FileStamp;
use crate::agent_accounts::antigravity;
use crate::agent_accounts::sign_in::SignIn;
use crate::agent_accounts::status::AgentStatus;
use crate::agent_ops::{AgentOps, Pending, SessionPlaces};
use crate::launch::named_account;
use crate::launch::tools::TerminalSetup;

fn antigravity_root() -> Option<PathBuf> {
    Some(home()?.join(".gemini/antigravity-cli"))
}

fn summaries_db() -> Option<Connection> {
    let db = antigravity_root()?.join("conversation_summaries.db");
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(100))
        .ok()?;
    Some(conn)
}

fn is_conversation_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Where Antigravity writes subagent conversation `child`'s steps as they happen, one JSON
/// object a line.
pub fn subagent_log(child: &str) -> Option<PathBuf> {
    is_conversation_id(child).then_some(())?;
    Some(antigravity_root()?.join(format!(
        "brain/{child}/.system_generated/logs/transcript.jsonl"
    )))
}

/// The subagent conversation `parent` started with `prompt`, other than those in `taken`.
/// Its log opens with a system message that carries the prompt it was given.
pub fn subagent_started(parent: &str, prompt: &str, taken: &[String]) -> Option<String> {
    let conn = summaries_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT conversation_id FROM conversation_summaries \
             WHERE parent_conversation_id = ?1 ORDER BY last_modified_time DESC",
        )
        .ok()?;
    let children: Vec<String> = stmt
        .query_map([parent], |row| row.get(0))
        .ok()?
        .filter_map(Result::ok)
        .filter(|child| !taken.contains(child))
        .collect();
    let prompt = prompt.trim();
    children.into_iter().find(|child| {
        let opening = subagent_log(child)
            .and_then(|log| std::fs::File::open(log).ok())
            .and_then(|file| {
                std::io::BufRead::lines(std::io::BufReader::new(file))
                    .next()?
                    .ok()
            });
        opening.is_some_and(|line| {
            serde_json::from_str::<serde_json::Value>(&line)
                .ok()
                .and_then(|step| {
                    step.get("content")?
                        .as_str()
                        .map(|said| said.contains(prompt))
                })
                .unwrap_or(false)
        })
    })
}

/// How far a subagent conversation has got, as Antigravity's summary of it says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubagentRun {
    Working,
    Idle,
    Killed,
    Gone,
}

pub fn subagent_run(child: &str) -> SubagentRun {
    let row = summaries_db().and_then(|conn| {
        conn.query_row(
            "SELECT status, killed, not_fully_idle FROM conversation_summaries \
             WHERE conversation_id = ?1",
            [child],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .ok()
    });
    match row {
        None => SubagentRun::Gone,
        Some((_, true, _)) => SubagentRun::Killed,
        Some((status, _, false)) if status == "CASCADE_RUN_STATUS_IDLE" => SubagentRun::Idle,
        Some(_) => SubagentRun::Working,
    }
}

/// The step Antigravity records when an error ends a turn.
const ERROR_STEP: i64 = 17;

fn steps_db(session: &str) -> Option<Connection> {
    if !is_conversation_id(session) {
        return None;
    }
    let db = antigravity_root()?.join(format!("conversations/{session}.db"));
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(100))
        .ok()?;
    Some(conn)
}

/// The index of the last step conversation `session` recorded, or -1 before its first.
pub fn last_step(session: &str) -> i64 {
    steps_db(session)
        .and_then(|conn| {
            conn.query_row("SELECT max(idx) FROM steps", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
        })
        .unwrap_or(-1)
}

/// The error that ended conversation `session`'s turn, when its last step after step `after`
/// is one. Antigravity answers such a turn as ended normally, its error said only as text.
pub fn turn_error(session: &str, after: i64) -> Option<String> {
    let conn = steps_db(session)?;
    let (kind, payload) = conn
        .query_row(
            "SELECT step_type, substr(step_payload, 1, 16384) FROM steps \
             WHERE idx > ?1 ORDER BY idx DESC LIMIT 1",
            [after],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .ok()?;
    (kind == ERROR_STEP).then(|| error_line(&String::from_utf8_lossy(&payload)))
}

/// The line of an error step's record that names the error, such as
/// `RESOURCE_EXHAUSTED (code 429): …`, among the binary fields around it.
fn error_line(payload: &str) -> String {
    let lines: Vec<&str> = payload
        .split(|c: char| c.is_control() || c == char::REPLACEMENT_CHARACTER)
        .map(str::trim)
        .filter(|line| line.len() > 8)
        .collect();
    let names_error = |line: &&&str| {
        line.contains(" (code ") || (line.contains("Error ") && line.contains(", Message: "))
    };
    lines
        .iter()
        .find(names_error)
        .or_else(|| lines.iter().find(|line| line.contains("error")))
        .map(|line| (*line).to_owned())
        .unwrap_or_else(|| "Antigravity stopped this turn because of an error.".to_owned())
}

/// The folder a conversation ran in: the first of its workspaces, given as `file://` addresses.
fn workspace_folder(uris: &str) -> Option<String> {
    let uris: Vec<String> = serde_json::from_str(uris).ok()?;
    let path = uris.first()?.strip_prefix("file://")?;
    percent_decode(path).filter(|path| path.starts_with('/'))
}

fn folders<'a>(projects: &[&'a str]) -> HashMap<String, &'a str> {
    let mut folders = HashMap::new();
    for project in projects {
        folders.insert((*project).to_owned(), *project);
        if let Ok(resolved) = sikemux_process::files::real_path(project) {
            folders
                .entry(resolved.to_string_lossy().into_owned())
                .or_insert(*project);
        }
    }
    folders
}

/// The person's own conversations in any of these folders, or in any folder at all,
/// newest first. Subagents' conversations, stopped ones and ones never prompted are left out.
fn sessions_in(conn: &Connection, projects: Option<&[&str]>) -> Vec<Found> {
    let folders = projects.map(folders);
    if folders.as_ref().is_some_and(HashMap::is_empty) {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT conversation_id, \
         COALESCE(NULLIF(TRIM(title), ''), NULLIF(TRIM(preview), ''), substr(conversation_id, 1, 13)), \
         CAST(COALESCE(strftime('%s', last_modified_time), 0) AS INTEGER), \
         workspace_uris \
         FROM conversation_summaries \
         WHERE parent_conversation_id = '' AND NOT killed AND step_count > 0 \
         ORDER BY last_modified_time DESC",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(3)?,
            AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
            },
        ))
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok)
        .filter_map(|(uris, session)| {
            let folder = workspace_folder(&uris)?;
            let project = match &folders {
                Some(folders) => (*folders.get(&folder)?).to_owned(),
                None => folder,
            };
            Some(Found {
                project,
                session,
                place: None,
                stamp: None,
            })
        })
        .collect()
}

pub struct Antigravity;

impl AgentOps for Antigravity {
    fn home(&self) -> Option<PathBuf> {
        antigravity_root()
    }

    fn sessions(&self, cwd: &str, _config_path: Option<&str>) -> Vec<AgentSession> {
        let Some(conn) = summaries_db() else {
            return Vec::new();
        };
        sessions_in(&conn, Some(&[cwd]))
            .into_iter()
            .take(400)
            .map(|found| found.session)
            .collect()
    }

    fn recent(&self, scan: &PageScan<'_>, _config_path: Option<&str>) -> Vec<Hit> {
        let Some(conn) = summaries_db() else {
            return Vec::new();
        };
        let projects: Option<Vec<&str>> = scan
            .projects()
            .map(|projects| projects.map(String::as_str).collect());
        let listed = sessions_in(&conn, projects.as_deref())
            .into_iter()
            .map(|found| Listed {
                at_ms: found.session.mtime * 1000,
                key: found.session.id.clone(),
                item: found,
            })
            .collect();
        scan.collect(listed, |found| Some(found.clone()))
    }

    fn session_places(&self, _cwd: &str, _config_path: Option<&str>) -> SessionPlaces {
        SessionPlaces {
            sessions: Vec::new(),
            listings: antigravity_root().into_iter().collect(),
        }
    }

    fn prose_stamp(&self, found: &Found) -> Option<FileStamp> {
        Some(FileStamp::at_secs(found.session.mtime))
    }

    fn terminal_tools<'a>(
        &'a self,
        _setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>> {
        Box::pin(ready(Ok(Vec::new())))
    }

    fn sign_in_status<'a>(
        &self,
        program: &'a Path,
        config_path: Option<&'a str>,
        providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(crate::agent_accounts::status::antigravity_sign_in(
            program,
            config_path,
            providers,
        ))
    }

    fn seed_account(&self, home: &Path, directory: &Path) -> Result<(), String> {
        antigravity::seed(home, directory)
    }

    fn signed_in(&self, env: &BTreeMap<String, String>) -> Option<SignIn> {
        let account = antigravity::added_in(env).transpose()?;
        Some(match account.is_ok_and(|account| account.signed_in()) {
            true => SignIn::Account(named_account("antigravity", env)?.to_owned()),
            false => SignIn::SignedOut,
        })
    }

    fn account_sign_in(&self, config_path: Option<&str>) -> Option<Vec<String>> {
        Some(antigravity::added(config_path).ok()??.sign_in_args())
    }

    fn usage<'a>(
        &self,
        program: &'a Path,
        config_path: Option<&'a str>,
    ) -> Option<Pending<'a, Result<crate::agent_accounts::usage::Usage, String>>> {
        Some(Box::pin(crate::agent_accounts::plan_limits::antigravity(
            program,
            config_path,
        )))
    }

    fn key_sign_in(&self, env: &BTreeMap<String, String>) -> Option<&'static str> {
        env.get("GEMINI_API_KEY")
            .is_some_and(|key| !key.trim().is_empty())
            .then_some("gemini-api-key")
    }
}
