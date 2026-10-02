use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use tokio::process::Command;

use super::recent::{Found, Hit, Listed, PageScan};
use crate::agents::executable::apply_login_environment;
use crate::agents::AgentSession;

// ---- hermes — `sessions` table in ~/.hermes/state.db (SQLite) -----------
fn hermes_db() -> Option<Connection> {
    let home = std::env::var("HOME").ok()?;
    let db = PathBuf::from(&home).join(".hermes/state.db");
    Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Sessions that ran in any of these folders, newest first.
fn hermes_sessions_in(conn: &Connection, projects: &[&str]) -> Vec<Found> {
    if projects.is_empty() {
        return Vec::new();
    }
    let placeholders = vec!["?"; projects.len()].join(", ");
    let sql = format!(
        "SELECT id, \
         COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
         CAST(COALESCE(started_at, 0) AS INTEGER) AS mtime, \
         cwd \
         FROM sessions WHERE cwd IN ({placeholders}) ORDER BY started_at DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(projects), |row| {
        Ok(Found {
            project: row.get::<_, String>(3)?,
            session: AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
            },
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

pub(super) fn hermes_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(conn) = hermes_db() else {
        return Vec::new();
    };
    hermes_sessions_in(&conn, &[cwd])
        .into_iter()
        .take(400)
        .map(|found| found.session)
        .collect()
}

pub(super) fn hermes_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let Some(conn) = hermes_db() else {
        return Vec::new();
    };
    let projects: Vec<&str> = scan.projects().map(String::as_str).collect();
    let listed = hermes_sessions_in(&conn, &projects)
        .into_iter()
        .map(|found| Listed {
            at_ms: found.session.mtime * 1000,
            key: found.session.id.clone(),
            item: found,
        })
        .collect();
    scan.collect(listed, |found| Some(found.clone()))
}

const HERMES_RENAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Renames through Hermes's own command, which keeps titles unique and marks the
/// name as the user's so generated titles never replace it.
pub(super) async fn rename_hermes_session(
    executable: &Path,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    command
        .args(["sessions", "rename", "--", session_id, name])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(HERMES_RENAME_TIMEOUT, command.output())
        .await
        .map_err(|_| "Hermes took too long to rename the chat".to_string())?
        .map_err(|_| "Could not start Hermes".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    // Hermes prints why it refused, such as a title already in use, on stdout.
    let printed = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let reason = printed
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("unknown error");
    Err(format!(
        "Hermes could not rename the chat: {}",
        reason.trim_start_matches("Error: ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_listed_only_for_the_folder_they_ran_in() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT, title TEXT, started_at REAL, cwd TEXT);
             INSERT INTO sessions VALUES ('a', 'Here', 200, '/repo');
             INSERT INTO sessions VALUES ('b', 'Elsewhere', 300, '/other');
             INSERT INTO sessions VALUES ('c', '', 100, '/repo');",
        )
        .unwrap();
        let ids = |projects: &[&str]| {
            hermes_sessions_in(&conn, projects)
                .into_iter()
                .map(|found| (found.session.id, found.project))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&["/repo"]),
            vec![("a".into(), "/repo".into()), ("c".into(), "/repo".into())]
        );
        assert_eq!(ids(&["/repo", "/other"]).len(), 3);
        assert!(ids(&[]).is_empty());
        assert_eq!(hermes_sessions_in(&conn, &["/repo"])[1].session.title, "c");
    }
}
