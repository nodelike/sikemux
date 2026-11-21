use std::collections::{BTreeMap, HashMap};
use std::future::ready;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use sikemux_process::paths::home;

use super::grok::percent_decode;
use super::recent::{Found, Hit, Listed, PageScan};
use super::AgentSession;
use super::FileStamp;
use crate::agent_accounts::status::AgentStatus;
use crate::agent_ops::{AgentOps, Pending, SessionPlaces};
use crate::launch::tools::TerminalSetup;

fn antigravity_root() -> Option<PathBuf> {
    Some(home()?.join(".gemini/antigravity-cli"))
}

fn summaries_db() -> Option<Connection> {
    let db = antigravity_root()?.join("conversation_summaries.db");
    Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
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
        _config_path: Option<&'a str>,
        providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(crate::agent_accounts::status::antigravity_sign_in(
            program, providers,
        ))
    }

    fn usage<'a>(
        &self,
        program: &'a Path,
        _config_path: Option<&'a str>,
    ) -> Option<Pending<'a, Result<crate::agent_accounts::usage::Usage, String>>> {
        Some(Box::pin(crate::agent_accounts::plan_limits::antigravity(
            program,
        )))
    }

    fn key_sign_in(&self, env: &BTreeMap<String, String>) -> Option<&'static str> {
        env.get("GEMINI_API_KEY")
            .is_some_and(|key| !key.trim().is_empty())
            .then_some("gemini-api-key")
    }
}
