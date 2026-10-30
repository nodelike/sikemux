use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::history;
use super::recent::{Found, Hit, Listed, PageScan};
use super::roots::environment_path;
use super::search::Prose;
use super::AgentSession;
use super::FileStamp;
use crate::agent_accounts::defaults::json_model;
use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_ops::Pending;
use crate::agent_ops::{AgentOps, SessionPlaces};
use crate::launch::providers::ChatRoute;
use crate::launch::skills;
use crate::launch::tools::TerminalSetup;
use serde_json::Value;
use sikemux_process::paths::home;
use std::collections::BTreeMap;
use std::future::ready;

// ---- opencode — SQLite in the user's opencode data dir ------------------
pub fn opencode_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = environment_path("OPENCODE_DATA_DIR") {
        dirs.push(dir);
    }
    if let Some(home) = sikemux_process::paths::home() {
        dirs.push(home.join(".local/share/opencode"));
        dirs.push(home.join("Library/Application Support/opencode"));
        dirs.push(home.join(".opencode/data"));
    }
    dirs
}

pub(super) fn opencode_db_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in opencode_data_dirs() {
        let direct = dir.join("opencode.db");
        if direct.exists() {
            paths.push(direct);
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with("opencode")
                && name.ends_with(".db")
                && !paths.iter().any(|p| p == &path)
            {
                paths.push(path);
            }
        }
    }
    paths
}

fn normalize_unix_secs(raw: u64) -> u64 {
    if raw > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

pub(super) fn opencode_sessions(cwd: &str) -> Vec<AgentSession> {
    let mut out = Vec::new();
    for db in opencode_db_paths() {
        let Ok(conn) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        out.extend(opencode_sessions_from_conn(&conn, cwd));
    }
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.id.clone()));
    out.truncate(400);
    out
}

/// OpenCode keeps sessions in SQLite, so each project is one indexed query.
pub(super) fn opencode_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let mut seen = HashSet::new();
    let mut listed = Vec::new();
    for db in opencode_db_paths() {
        let Ok(conn) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let found: Vec<(String, AgentSession)> = match scan.projects() {
            Some(projects) => projects
                .flat_map(|project| {
                    opencode_sessions_from_conn(&conn, project)
                        .into_iter()
                        .map(|session| (project.clone(), session))
                })
                .collect(),
            None => opencode_every_session(&conn),
        };
        for (project, session) in found {
            if !seen.insert(session.id.clone()) {
                continue;
            }
            listed.push(Listed {
                at_ms: session.mtime * 1000,
                key: session.id.clone(),
                item: Found {
                    project,
                    session,
                    place: Some(db.clone()),
                },
            });
        }
    }
    scan.collect(listed, |found| Some(found.clone()))
}

fn opencode_every_session(conn: &Connection) -> Vec<(String, AgentSession)> {
    let sql = "\
        SELECT id, \
               COALESCE(NULLIF(TRIM(title), ''), NULLIF(TRIM(slug), ''), substr(id, 1, 13)) AS title, \
               CAST(COALESCE(time_updated, time_created, 0) AS INTEGER) AS mtime, \
               directory \
        FROM session \
        ORDER BY COALESCE(time_updated, time_created, 0) DESC \
        LIMIT 2000";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(3)?,
            AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: normalize_unix_secs(row.get::<_, i64>(2).unwrap_or(0).max(0) as u64),
            },
        ))
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

fn opencode_sessions_from_conn(conn: &Connection, cwd: &str) -> Vec<AgentSession> {
    let with_project = "\
        SELECT s.id, \
               COALESCE(NULLIF(TRIM(s.title), ''), NULLIF(TRIM(s.slug), ''), substr(s.id, 1, 13)) AS title, \
               CAST(COALESCE(s.time_updated, s.time_created, 0) AS INTEGER) AS mtime \
        FROM session s \
        LEFT JOIN project p ON p.id = s.project_id \
        WHERE s.directory = ?1 OR s.path = ?1 OR p.worktree = ?1 \
        ORDER BY COALESCE(s.time_updated, s.time_created, 0) DESC \
        LIMIT 400";
    if let Some(rows) = opencode_query(conn, with_project, cwd) {
        return rows;
    }

    let session_only = "\
        SELECT id, \
               COALESCE(NULLIF(TRIM(title), ''), NULLIF(TRIM(slug), ''), substr(id, 1, 13)) AS title, \
               CAST(COALESCE(time_updated, time_created, 0) AS INTEGER) AS mtime \
        FROM session \
        WHERE directory = ?1 OR path = ?1 \
        ORDER BY COALESCE(time_updated, time_created, 0) DESC \
        LIMIT 400";
    if let Some(rows) = opencode_query(conn, session_only, cwd) {
        return rows;
    }

    let minimal = "\
        SELECT id, \
               COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
               CAST(COALESCE(time_updated, time_created, 0) AS INTEGER) AS mtime \
        FROM session \
        WHERE directory = ?1 \
        ORDER BY COALESCE(time_updated, time_created, 0) DESC \
        LIMIT 400";
    opencode_query(conn, minimal, cwd).unwrap_or_default()
}

fn opencode_query(conn: &Connection, sql: &str, cwd: &str) -> Option<Vec<AgentSession>> {
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([cwd], |row| {
            Ok(AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: normalize_unix_secs(row.get::<_, i64>(2).unwrap_or(0).max(0) as u64),
            })
        })
        .ok()?;
    Some(rows.filter_map(|r| r.ok()).collect())
}

/// Sets the title the way OpenCode's own rename does; a title it did not generate is never replaced.
pub(super) fn rename_opencode_session(session_id: &str, name: &str) -> Result<(), String> {
    rename_in_databases(&opencode_db_paths(), session_id, name)
}

fn rename_in_databases(databases: &[PathBuf], session_id: &str, name: &str) -> Result<(), String> {
    for db in databases {
        let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE) else {
            continue;
        };
        let _ = conn.busy_timeout(Duration::from_secs(5));
        let changed = conn
            .execute(
                "UPDATE session SET title = ?1 WHERE id = ?2",
                (name, session_id),
            )
            .map_err(|error| error.to_string())?;
        if changed > 0 {
            return Ok(());
        }
    }
    Err("session not found".to_string())
}

/// The provider and model of the session's last reply.
pub(super) fn opencode_last_model(session_id: &str) -> Option<(String, String)> {
    opencode_db_paths().iter().find_map(|db| {
        let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        conn.query_row(
            "SELECT json_extract(data, '$.providerID'), json_extract(data, '$.modelID') \
             FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'assistant' \
             AND json_extract(data, '$.providerID') IS NOT NULL \
             ORDER BY time_created DESC, id DESC LIMIT 1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .ok()
    })
}

/// What the person and OpenCode wrote in the session: its text parts, never tool calls.
pub(super) fn opencode_prose(db: &Path, session_id: &str, prose: &mut Prose) {
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT substr(json_extract(data, '$.text'), 1, 8192) FROM part \
         WHERE session_id = ?1 AND json_extract(data, '$.type') = 'text' \
         AND COALESCE(json_extract(data, '$.synthetic'), 0) = 0 \
         ORDER BY time_created, id",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([session_id], |row| row.get::<_, Option<String>>(0)) else {
        return;
    };
    for text in rows.map_while(Result::ok).flatten() {
        prose.push(&text);
    }
}

pub(crate) struct Opencode;

impl AgentOps for Opencode {
    fn sessions(&self, cwd: &str, _config_path: Option<&str>) -> Vec<AgentSession> {
        opencode_sessions(cwd)
    }

    fn recent(&self, scan: &PageScan<'_>, _config_path: Option<&str>) -> Vec<Hit> {
        opencode_recent(scan)
    }

    fn session_places(&self, _cwd: &str, _config_path: Option<&str>) -> SessionPlaces {
        SessionPlaces {
            sessions: Vec::new(),
            listings: opencode_data_dirs(),
        }
    }

    fn history(&self, session_id: &str, _cwd: &Path) -> Result<Vec<Value>, String> {
        history::opencode(&opencode_db_paths(), session_id)
    }

    fn last_model(&self, session_id: &str) -> Option<(String, String)> {
        opencode_last_model(session_id)
    }

    fn prose_stamp(&self, found: &Found) -> Option<FileStamp> {
        Some(FileStamp::at_secs(found.session.mtime))
    }

    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()> {
        opencode_prose(found.place.as_deref()?, &found.session.id, prose);
        Some(())
    }

    fn rename_saved(
        &self,
        _cwd: &str,
        session_id: &str,
        name: &str,
        _config_path: Option<&str>,
    ) -> Option<Result<(), String>> {
        Some(rename_opencode_session(session_id, name))
    }

    fn terminal_tools<'a>(
        &'a self,
        setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>> {
        Box::pin(ready(setup.opencode_config()))
    }

    fn chat_skills(
        &self,
        _data_dir: &Path,
        library: &Path,
        _args: &mut Vec<String>,
        env: &mut BTreeMap<String, String>,
    ) {
        skills::add_to_opencode_chat(library, env);
    }

    fn default_model(&self, _config_path: Option<&str>) -> Option<String> {
        let home = home()?;
        let path = environment_path("OPENCODE_CONFIG").unwrap_or_else(|| {
            environment_path("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("opencode/opencode.json")
        });
        json_model(&path, "model")
    }

    fn sign_in_status<'a>(
        &self,
        program: &'a Path,
        config_path: Option<&'a str>,
        providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(status::opencode_sign_in(program, config_path, providers))
    }

    fn route_chat(
        &self,
        route: &ChatRoute<'_>,
        env: &mut BTreeMap<String, String>,
        _args: &mut Vec<String>,
    ) {
        let mut config = skills::inherited_opencode_config()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        if !config["provider"].is_object() {
            config["provider"] = serde_json::json!({});
        }
        config["provider"][route.id.as_str()] = serde_json::json!({
            "npm": "@ai-sdk/openai-compatible",
            "name": route.label,
            "options": {
                "baseURL": route.base_url,
                "apiKey": format!("{{env:{}}}", route.key_name),
            },
            "models": { route.model: {
                "name": route.model,
                "attachment": route.images,
                "modalities": { "input": route.input(), "output": ["text"] },
            } },
        });
        config["model"] = route.agent_model.clone().into();
        env.insert("OPENCODE_CONFIG_CONTENT".into(), config.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{opencode_sessions_from_conn, rename_in_databases};
    use rusqlite::Connection;

    #[test]
    fn a_renamed_opencode_session_lists_under_its_new_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER);
             INSERT INTO session VALUES ('ses_1', 'New session - 2026-09-30T10:00:00Z', '/repo', 1, 2);",
        )
        .unwrap();

        rename_in_databases(std::slice::from_ref(&db), "ses_1", "Flaky test fix").unwrap();

        assert_eq!(
            opencode_sessions_from_conn(&conn, "/repo")[0].title,
            "Flaky test fix"
        );
        let updated: i64 = conn
            .query_row("SELECT time_updated FROM session", [], |row| row.get(0))
            .unwrap();
        assert_eq!(updated, 2);
        assert!(rename_in_databases(&[db], "ses_missing", "Name").is_err());
    }
}
