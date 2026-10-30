use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::apply_login_environment;
use super::recent::{Found, Hit, Listed, PageScan};
use super::roots::muse_data_root;
use super::search::Prose;
use super::AgentSession;
use super::FileStamp;
use crate::agent_ops::Pending;
use crate::agent_ops::{AgentOps, RenameCommand, SessionPlaces};
use crate::launch::tools::TerminalSetup;
use std::future::ready;

// ---- muse — `sessions` table in ~/.local/share/muse/session-index.db -----
fn muse_db() -> Option<Connection> {
    let db = muse_data_root()?.join("session-index.db");
    Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Muse records the folder a session ran in as the path it resolves to, such
/// as `/private/tmp` for `/tmp`, so each project is looked up both ways.
fn folders<'a>(projects: &[&'a str]) -> HashMap<String, &'a str> {
    let mut folders = HashMap::new();
    for project in projects {
        folders.insert((*project).to_owned(), *project);
        if let Ok(resolved) = std::fs::canonicalize(project) {
            folders
                .entry(resolved.to_string_lossy().into_owned())
                .or_insert(*project);
        }
    }
    folders
}

/// The person's own sessions that ran in any of these folders, or in any folder at all,
/// newest first. Subagents' sessions and ones never prompted are left out.
fn muse_sessions_in(conn: &Connection, projects: Option<&[&str]>) -> Vec<Found> {
    let folders = projects.map(folders);
    let filter = match &folders {
        Some(folders) if folders.is_empty() => return Vec::new(),
        Some(folders) => format!(
            "workspace_root IN ({})",
            vec!["?"; folders.len()].join(", ")
        ),
        None => "workspace_root IS NOT NULL".to_owned(),
    };
    let sql = format!(
        "SELECT session_id, \
         COALESCE(NULLIF(TRIM(session_name), ''), NULLIF(TRIM(effective_long_title), ''), \
                  NULLIF(TRIM(title), ''), substr(session_id, 1, 13)) AS title, \
         CAST(COALESCE(updated_at_us, created_at_us, 0) / 1000000 AS INTEGER) AS mtime, \
         workspace_root \
         FROM sessions \
         WHERE {filter} \
           AND COALESCE(msp_kind, 'root') = 'root' AND prompt_count > 0 \
         ORDER BY updated_at_us DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let keys: Vec<&String> = folders.iter().flat_map(|folders| folders.keys()).collect();
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(keys), |row| {
        let folder = row.get::<_, String>(3)?;
        Ok((
            folder,
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
        .filter_map(|(folder, session)| {
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

pub(super) fn muse_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(conn) = muse_db() else {
        return Vec::new();
    };
    muse_sessions_in(&conn, Some(&[cwd]))
        .into_iter()
        .take(400)
        .map(|found| found.session)
        .collect()
}

pub(super) fn muse_recent(scan: &PageScan<'_>) -> Vec<Hit> {
    let Some(conn) = muse_db() else {
        return Vec::new();
    };
    let projects: Option<Vec<&str>> = scan
        .projects()
        .map(|projects| projects.map(String::as_str).collect());
    let listed = muse_sessions_in(&conn, projects.as_deref())
        .into_iter()
        .map(|found| Listed {
            at_ms: found.session.mtime * 1000,
            key: found.session.id.clone(),
            item: found,
        })
        .collect();
    scan.collect(listed, |found| Some(found.clone()))
}

const MUSE_RENAME_TIMEOUT: Duration = Duration::from_secs(30);

fn command_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Renames through Muse's own session protocol: a short `muse serve` loads
/// the session and names it, which marks the name as the person's.
pub(super) async fn rename_muse_session(
    executable: &Path,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let mut command = Command::from(sikemux_process::user_environment::agent_command(executable));
    apply_login_environment(&mut command);
    command
        .arg("serve")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Muse".to_string())?;
    let mut stdin = child.stdin.take().ok_or("Could not talk to Muse")?;
    let stdout = child.stdout.take().ok_or("Could not talk to Muse")?;
    let mut lines = BufReader::new(stdout).lines();
    let steps = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "clientInfo": { "name": "sikemux", "version": env!("CARGO_PKG_VERSION") } } }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "session/resume",
                "params": { "commandId": command_id(), "sessionId": session_id, "excludeItems": true } }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "session/rename",
                "params": { "commandId": command_id(), "sessionId": session_id, "name": name } }),
    ];
    let answered = tokio::time::timeout(MUSE_RENAME_TIMEOUT, async {
        let send = |line: String| format!("{line}\n").into_bytes();
        stdin
            .write_all(&send(steps[0].to_string()))
            .await
            .map_err(|_| "Could not talk to Muse".to_string())?;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "Could not read Muse's answer".to_string())?
        {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(answer) = value.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("Muse could not rename the chat: {message}"));
            }
            let next = match answer {
                1 => {
                    let initialized = json!({ "jsonrpc": "2.0", "method": "initialized" });
                    format!("{initialized}\n{}", steps[1])
                }
                2 => steps[2].to_string(),
                3 => return Ok(()),
                _ => continue,
            };
            stdin
                .write_all(&send(next))
                .await
                .map_err(|_| "Could not talk to Muse".to_string())?;
        }
        Err("Muse closed before it could rename the chat".to_string())
    })
    .await;
    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;
    answered.map_err(|_| "Muse took too long to rename the chat".to_string())?
}

/// The text Muse keeps for searching the session, its own fields one a line.
pub(super) fn muse_prose(session_id: &str, prose: &mut Prose) {
    let Some(conn) = muse_db() else {
        return;
    };
    let text = conn.query_row(
        "SELECT search_text FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, String>(0),
    );
    if let Ok(text) = text {
        for field in text.split('\u{1f}') {
            prose.push(field);
        }
    }
}

pub(crate) struct Muse;

impl AgentOps for Muse {
    fn sessions(&self, cwd: &str, _config_path: Option<&str>) -> Vec<AgentSession> {
        muse_sessions(cwd)
    }

    fn recent(&self, scan: &PageScan<'_>, _config_path: Option<&str>) -> Vec<Hit> {
        muse_recent(scan)
    }

    fn session_places(&self, _cwd: &str, _config_path: Option<&str>) -> SessionPlaces {
        SessionPlaces {
            sessions: Vec::new(),
            listings: muse_data_root().into_iter().collect(),
        }
    }

    fn prose_stamp(&self, found: &Found) -> Option<FileStamp> {
        Some(FileStamp::at_secs(found.session.mtime))
    }

    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()> {
        muse_prose(&found.session.id, prose);
        Some(())
    }

    fn rename_command(&self) -> Option<RenameCommand> {
        Some(|executable, _config_path, session_id, name| {
            Box::pin(rename_muse_session(executable, session_id, name))
        })
    }

    fn terminal_tools<'a>(
        &'a self,
        setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>> {
        Box::pin(ready(setup.muse_args()))
    }
}
