use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use tokio::process::Command;

use crate::agent_catalog::AgentKind;

use super::apply_login_environment;
use super::recent::{Found, Hit, Listed, PageScan};
use super::search::Prose;
use super::AgentSession;
use super::FileStamp;
use crate::agent_accounts::defaults::{
    clean_effort, clean_model, qualify_model, yaml_agent_reasoning_effort, yaml_model_section,
};
use crate::agent_ops::Pending;
use crate::agent_ops::{AgentOps, RenameCommand, SessionPlaces};
use crate::launch::tools::TerminalSetup;
use crate::launch::tools::Tools;
use sikemux_process::user_environment::shell_var;
use std::fs;

// ---- hermes — `sessions` table in ~/.hermes/state.db (SQLite) -----------
fn hermes_db() -> Option<Connection> {
    let db = AgentKind::Hermes.home_root()?.join("state.db");
    Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Sessions that ran in any of these folders, or in any folder at all, newest first.
fn hermes_sessions_in(conn: &Connection, projects: Option<&[&str]>) -> Vec<Found> {
    let filter = match projects {
        Some([]) => return Vec::new(),
        Some(projects) => format!("WHERE cwd IN ({})", vec!["?"; projects.len()].join(", ")),
        None => "WHERE cwd IS NOT NULL".to_owned(),
    };
    let sql = format!(
        "SELECT id, \
         COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
         CAST(COALESCE(started_at, 0) AS INTEGER) AS mtime, \
         cwd \
         FROM sessions {filter} ORDER BY started_at DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let params = projects.unwrap_or_default();
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(Found {
            project: row.get::<_, String>(3)?,
            session: AgentSession {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                mtime: row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
            },
            place: None,
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
    hermes_sessions_in(&conn, Some(&[cwd]))
        .into_iter()
        .take(400)
        .map(|found| found.session)
        .collect()
}

pub(super) fn hermes_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let Some(conn) = hermes_db() else {
        return Vec::new();
    };
    let projects: Option<Vec<&str>> = scan
        .projects()
        .map(|projects| projects.map(String::as_str).collect());
    let listed = hermes_sessions_in(&conn, projects.as_deref())
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
    let mut command = Command::from(sikemux_process::user_environment::agent_command(executable));
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

/// Changes whenever a message is added to the session.
pub(super) fn hermes_stamp(session_id: &str) -> Option<FileStamp> {
    let conn = hermes_db()?;
    let (last, count) = conn
        .query_row(
            "SELECT COALESCE(MAX(id), 0), COUNT(*) FROM messages WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .ok()?;
    Some(FileStamp {
        modified_ns: last.max(0) as u128,
        len: count.max(0) as u64,
    })
}

/// What the person and Hermes wrote in the session.
pub(super) fn hermes_prose(session_id: &str, prose: &mut Prose) {
    let Some(conn) = hermes_db() else {
        return;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT substr(content, 1, 8192) FROM messages \
         WHERE session_id = ?1 AND role IN ('user', 'assistant') AND content IS NOT NULL \
         ORDER BY id",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([session_id], |row| row.get::<_, String>(0)) else {
        return;
    };
    for text in rows.map_while(Result::ok) {
        prose.push(&text);
    }
}

pub(crate) struct Hermes;

impl AgentOps for Hermes {
    fn sessions(&self, cwd: &str, _config_path: Option<&str>) -> Vec<AgentSession> {
        hermes_sessions(cwd)
    }

    fn recent(&self, scan: &PageScan<'_>, _config_path: Option<&str>) -> Vec<Hit> {
        hermes_recent(scan)
    }

    fn session_places(&self, _cwd: &str, _config_path: Option<&str>) -> SessionPlaces {
        SessionPlaces {
            sessions: Vec::new(),
            listings: AgentKind::Hermes.home_root().into_iter().collect(),
        }
    }

    fn prose_stamp(&self, found: &Found) -> Option<FileStamp> {
        hermes_stamp(&found.session.id)
    }

    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()> {
        hermes_prose(&found.session.id, prose);
        Some(())
    }

    fn rename_command(&self) -> Option<RenameCommand> {
        Some(|executable, _config_path, session_id, name| {
            Box::pin(rename_hermes_session(executable, session_id, name))
        })
    }

    fn terminal_tools<'a>(
        &'a self,
        setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>> {
        Box::pin(setup.hermes_home())
    }

    fn chat_home<'a>(
        &self,
        tools: &'a Tools,
        agent_id: &'a str,
        program: &'a str,
        library: &'a Path,
    ) -> Option<Pending<'a, Result<(String, String), String>>> {
        Some(Box::pin(tools.hermes_home(
            "hermes-chat",
            agent_id,
            program,
            None,
            Some(library),
        )))
    }

    fn default_model(&self, _config_path: Option<&str>) -> Option<String> {
        shell_var("HERMES_INFERENCE_MODEL")
            .and_then(|value| clean_model(&value))
            .or_else(|| {
                let text =
                    fs::read_to_string(AgentKind::Hermes.home_root()?.join("config.yaml")).ok()?;
                let (provider, model) = yaml_model_section(&text);
                qualify_model(provider.as_deref(), model.as_deref())
            })
    }

    fn default_effort(&self, _config_path: Option<&str>) -> Option<String> {
        fs::read_to_string(AgentKind::Hermes.home_root()?.join("config.yaml"))
            .ok()
            .and_then(|text| yaml_agent_reasoning_effort(&text))
            .and_then(|value| clean_effort("hermes", &value))
    }
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
            hermes_sessions_in(&conn, Some(projects))
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
        assert_eq!(hermes_sessions_in(&conn, None).len(), 3);
        assert_eq!(
            hermes_sessions_in(&conn, Some(&["/repo"]))[1].session.title,
            "c"
        );
    }
}
