use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::history;
use super::opencode_server::Server;
use super::recent::{Found, Hit, Listed, PageScan};
use super::roots::environment_path;
use super::search::{Hear, Speaker};
use super::AgentSession;
use super::FileStamp;
use crate::agent_accounts::defaults::json_model;
use crate::agent_accounts::status::{self, AgentStatus};
use crate::agent_ops::Pending;
use crate::agent_ops::{AgentOps, DeleteCommand, RenameCommand, SessionPlaces};
use crate::launch::model_feed::{Cost, Model};
use crate::launch::providers::{
    ChatRoute, ListedProvider, LISTED_MAX_OUTPUT_TOKENS, MODEL_TOKEN_ENV,
};
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

/// How many sign-ins and keys OpenCode keeps, or `None` when it has no database to read.
pub fn saved_credentials() -> Option<usize> {
    let counts: Vec<i64> = opencode_db_paths()
        .iter()
        .filter_map(|db| {
            let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
            conn.query_row("SELECT COUNT(*) FROM credential", [], |row| row.get(0))
                .ok()
        })
        .collect();
    (!counts.is_empty()).then(|| counts.iter().sum::<i64>() as usize)
}

/// The API key OpenCode keeps for `integration`, the one it uses when it has several.
pub fn saved_key(integration: &str) -> Option<String> {
    opencode_db_paths().iter().find_map(|db| {
        let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let value: String = conn
            .query_row(
                "SELECT value FROM credential WHERE integration_id = ?1 \
                 ORDER BY active DESC, time_updated DESC LIMIT 1",
                [integration],
                |row| row.get(0),
            )
            .ok()?;
        let value: serde_json::Value = serde_json::from_str(&value).ok()?;
        (value.get("type").and_then(Value::as_str) == Some("key"))
            .then(|| value.get("key")?.as_str().map(str::to_owned))
            .flatten()
    })
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

/// How OpenCode 2 names a session it lists: its title, else the name it made up for it.
const TITLE: &str =
    "COALESCE(NULLIF(TRIM(s.title), ''), NULLIF(TRIM(s.slug), ''), substr(s.id, 1, 13))";

fn opencode_every_session(conn: &Connection) -> Vec<(String, AgentSession)> {
    let sql = format!(
        "SELECT s.id, {TITLE}, s.time_updated, s.directory FROM session_v2 s \
         WHERE s.parent_id IS NULL ORDER BY s.time_updated DESC LIMIT 2000"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(3)?, session_row(row)?)))
    else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// The sessions started in `cwd`, or in the project whose worktree it is, leaving out the
/// ones subagents ran in.
fn opencode_sessions_from_conn(conn: &Connection, cwd: &str) -> Vec<AgentSession> {
    let sql = format!(
        "SELECT s.id, {TITLE}, s.time_updated FROM session_v2 s \
         LEFT JOIN project p ON p.id = s.project_id \
         WHERE s.parent_id IS NULL AND (s.directory = ?1 OR p.worktree = ?1) \
         ORDER BY s.time_updated DESC LIMIT 400"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([cwd], session_row) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

fn session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSession> {
    Ok(AgentSession {
        id: row.get::<_, String>(0)?,
        title: row.get::<_, String>(1)?,
        mtime: normalize_unix_secs(row.get::<_, i64>(2).unwrap_or(0).max(0) as u64),
    })
}

/// Renames the session through a server OpenCode runs for it, as its own clients do, since
/// OpenCode keeps a session's name in its event log as well as in its tables.
pub(super) async fn rename_opencode_session(
    executable: &Path,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let server = session_server(executable, session_id).await?;
    let renamed = server
        .patch(
            &format!("/api/session/{session_id}"),
            serde_json::json!({ "title": name }),
        )
        .await;
    server.close().await;
    renamed.map(drop).map_err(String::from)
}

pub(super) async fn delete_opencode_session(
    executable: &Path,
    session_id: &str,
) -> Result<(), String> {
    let server = session_server(executable, session_id).await?;
    let deleted = server.delete(&format!("/api/session/{session_id}")).await;
    server.close().await;
    deleted.map(drop).map_err(String::from)
}

/// A server of OpenCode's own in the directory the session belongs to.
async fn session_server(executable: &Path, session_id: &str) -> Result<Server, String> {
    let directory = opencode_db_paths()
        .iter()
        .find_map(|db| session_directory(db, session_id))
        .ok_or("session not found")?;
    let command = sikemux_process::command::Command::into_async(
        sikemux_process::user_environment::agent_command(executable),
    );
    Server::start(command, &directory).await
}

fn session_directory(db: &Path, session_id: &str) -> Option<PathBuf> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT directory FROM session_v2 WHERE id = ?1",
        [session_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(PathBuf::from)
}

/// The provider and model of the session's last reply.
pub(super) fn opencode_last_model(session_id: &str) -> Option<(String, String)> {
    opencode_db_paths().iter().find_map(|db| {
        let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        conn.query_row(
            "SELECT json_extract(data, '$.model.providerID'), json_extract(data, '$.model.id') \
             FROM session_message WHERE session_id = ?1 AND type = 'assistant' \
             AND json_extract(data, '$.model.providerID') IS NOT NULL \
             ORDER BY seq DESC LIMIT 1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .ok()
    })
}

/// What the person and OpenCode wrote in the session: its text, never tool calls.
pub(super) fn opencode_prose(db: &Path, session_id: &str, prose: &mut dyn Hear) {
    const MAX_CHARS: usize = 8192;
    let Ok(messages) = history::opencode_messages(db, session_id) else {
        return;
    };
    for (_, kind, data) in messages {
        let (by, texts): (Speaker, Vec<&str>) = match kind.as_str() {
            "user" => (
                Speaker::Person,
                data.get("text")
                    .and_then(Value::as_str)
                    .into_iter()
                    .collect(),
            ),
            "assistant" => (
                Speaker::Agent,
                (data
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten())
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect(),
            ),
            _ => continue,
        };
        for text in texts {
            let end = text
                .char_indices()
                .nth(MAX_CHARS)
                .map_or(text.len(), |(at, _)| at);
            prose.push(by, text.get(..end).unwrap_or(text));
        }
    }
}

pub(crate) struct Opencode;

impl AgentOps for Opencode {
    fn usage<'a>(
        &self,
        _program: &'a Path,
        _config_path: Option<&'a str>,
    ) -> Option<Pending<'a, Result<crate::agent_accounts::usage::Usage, String>>> {
        Some(Box::pin(crate::agent_accounts::plan_limits::opencode()))
    }

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

    fn prose(&self, found: &Found, prose: &mut dyn Hear) -> Option<()> {
        opencode_prose(found.place.as_deref()?, &found.session.id, prose);
        Some(())
    }

    fn rename_command(&self) -> Option<RenameCommand> {
        Some(|executable, _config_path, session_id, name| {
            Box::pin(rename_opencode_session(executable, session_id, name))
        })
    }

    fn delete_command(&self) -> Option<DeleteCommand> {
        Some(|executable, _config_path, session_id| {
            Box::pin(delete_opencode_session(executable, session_id))
        })
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
        _program: &'a Path,
        _config_path: Option<&'a str>,
        providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(status::opencode_sign_in(providers))
    }

    fn route_chat(
        &self,
        route: &ChatRoute<'_>,
        env: &mut BTreeMap<String, String>,
        _args: &mut Vec<String>,
    ) {
        let mut config = provider_config(env);
        let models = provider_entry(
            &mut config,
            &route.id,
            route.label,
            route.base_url,
            route.key_name,
        );
        models[route.model] = model_entry(route.model, route.input(), route.about, route.cost);
        if let Some(fields) = config.as_object_mut() {
            fields.insert("model".to_owned(), route.agent_model.clone().into());
        }
        env.insert(CONFIG_ENV.into(), config.to_string());
    }

    fn list_connected(
        &self,
        listed: &[ListedProvider<'_>],
        _omp_extension: &Path,
        env: &mut BTreeMap<String, String>,
        _args: &mut Vec<String>,
    ) {
        let mut config = provider_config(env);
        for provider in listed {
            let models = provider_entry(
                &mut config,
                &provider.id,
                provider.label,
                &provider.base_url,
                MODEL_TOKEN_ENV,
            );
            for (id, model) in &provider.models {
                let input = ListedProvider::input(model);
                models[*id] = model_entry(id, input, Some(model), model.cost);
            }
        }
        env.insert(CONFIG_ENV.into(), config.to_string());
    }

    fn skills_folder(&self) -> Option<PathBuf> {
        let config_home = environment_path("XDG_CONFIG_HOME").unwrap_or_else(|| {
            sikemux_process::paths::home()
                .unwrap_or_default()
                .join(".config")
        });
        Some(config_home.join("opencode/skills"))
    }
}

const CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";

/// The inline config this launch has so far, else the person's own.
fn provider_config(env: &BTreeMap<String, String>) -> Value {
    let mut config = env
        .get(CONFIG_ENV)
        .cloned()
        .or_else(skills::inherited_opencode_config)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(fields) = config.as_object_mut() {
        if !fields.get("providers").is_some_and(Value::is_object) {
            fields.insert("providers".to_owned(), serde_json::json!({}));
        }
    }
    config
}

/// The provider `id` in `config`, reached at `base_url` with the key in `key_name`, answering
/// with its models to fill in.
#[expect(
    clippy::indexing_slicing,
    reason = "provider_config always gives an object holding a providers object"
)]
fn provider_entry<'a>(
    config: &'a mut Value,
    id: &str,
    label: &str,
    base_url: &str,
    key_name: &str,
) -> &'a mut Value {
    let entry = &mut config["providers"][id];
    let models = entry["models"].take();
    *entry = serde_json::json!({
        "name": label,
        "env": [key_name],
        "package": "@opencode/ai/providers/openai-compatible",
        "settings": { "baseURL": base_url },
        "models": if models.is_object() { models } else { serde_json::json!({}) },
    });
    &mut entry["models"]
}

/// The efforts a reasoning model takes over chat completions, which models.dev does not list.
const REASONING_EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// A model as OpenCode lists it, with the window, price and efforts models.dev knows of, so
/// OpenCode offers its efforts, fills its context meter and counts what it costs.
fn model_entry(id: &str, input: &[&str], about: Option<&Model>, cost: Option<Cost>) -> Value {
    let mut limit =
        serde_json::Map::from_iter([("output".to_owned(), LISTED_MAX_OUTPUT_TOKENS.into())]);
    let mut capabilities =
        serde_json::Map::from_iter([("input".to_owned(), serde_json::json!(input))]);
    if let Some(about) = about {
        if about.limit.context > 0 {
            limit.insert("context".to_owned(), about.limit.context.into());
        }
        if about.reasoning {
            capabilities.insert(
                "reasoning".to_owned(),
                serde_json::json!({ "supported": true, "effort": REASONING_EFFORTS }),
            );
        }
    }
    let mut entry = serde_json::Map::from_iter([
        ("name".to_owned(), id.into()),
        ("limit".to_owned(), Value::Object(limit)),
        ("capabilities".to_owned(), Value::Object(capabilities)),
    ]);
    if let Some(cost) = cost.filter(|_| about.is_some()) {
        entry.insert(
            "cost".to_owned(),
            serde_json::json!({
                "input": cost.input.unwrap_or_default(),
                "output": cost.output.unwrap_or_default(),
                "cache": {
                    "read": cost.cache_read.unwrap_or_default(),
                    "write": cost.cache_write.unwrap_or_default(),
                },
            }),
        );
    }
    Value::Object(entry)
}
