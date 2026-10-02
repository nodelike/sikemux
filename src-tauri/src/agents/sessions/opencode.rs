use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::recent::{Found, Hit, Listed, PageScan};
use crate::agents::AgentSession;

// ---- opencode — SQLite in the user's opencode data dir ------------------
pub(in crate::agents) fn opencode_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("OPENCODE_DATA_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/opencode"));
        dirs.push(PathBuf::from(&home).join("Library/Application Support/opencode"));
        dirs.push(PathBuf::from(&home).join(".opencode/data"));
    }
    dirs
}

fn opencode_db_paths() -> Vec<PathBuf> {
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
        for project in scan.projects() {
            for session in opencode_sessions_from_conn(&conn, project) {
                if !seen.insert(session.id.clone()) {
                    continue;
                }
                listed.push(Listed {
                    at_ms: session.mtime * 1000,
                    key: session.id.clone(),
                    item: Found {
                        project: project.clone(),
                        session,
                    },
                });
            }
        }
    }
    scan.collect(listed, |found| Some(found.clone()))
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
