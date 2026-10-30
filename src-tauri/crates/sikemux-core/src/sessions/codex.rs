use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rayon::prelude::*;
use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{ChildStdin, Command};

use super::context::{last_line_matching, SessionContext};
use super::index::{indexed, Indexed};
use super::recent::{Found, Hit, Listed, PageScan};
use super::roots::agent_config_root;
use super::search::each_line;
use super::search::Prose;
use super::AgentSession;
use super::{apply_login_environment, apply_process_config};
use super::{
    collect_transcripts, condense, file_stamp, is_compressed, is_jsonl, stamped_transcripts,
    transcript_lines, FileStamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};
use crate::agent_catalog::AgentKind;
use crate::agent_ops::{AgentOps, DeleteCommand, RenameCommand, SessionPlaces};
use notify::RecursiveMode;

// ---- codex — ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl[.zst] ----------

pub(super) fn is_rollout(path: &Path) -> bool {
    is_jsonl(path) || is_compressed(path)
}

/// The parsed Codex session index, kept until the file it came from is written
/// again. It is one file for every project, so re-reading it per scan was the
/// bulk of a Codex listing.
fn codex_index_cache() -> &'static Mutex<CodexIndexCache> {
    static CACHE: OnceLock<Mutex<CodexIndexCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

type CodexTitles = HashMap<String, String>;
type CodexIndexCache = HashMap<PathBuf, (FileStamp, Arc<CodexTitles>)>;

fn codex_indexed_titles(root: &Path) -> Arc<CodexTitles> {
    let path = root.join("session_index.jsonl");
    let stamp = file_stamp(&path);
    if let Some((cached, titles)) = codex_index_cache().locked().get(&path) {
        if *cached == stamp {
            return titles.clone();
        }
    }
    let titles = Arc::new(read_codex_index(&path));
    codex_index_cache()
        .locked()
        .insert(path, (stamp, titles.clone()));
    titles
}

fn read_codex_index(path: &Path) -> CodexTitles {
    let Ok(file) = fs::File::open(path) else {
        return HashMap::new();
    };
    let mut titles = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let Some(title) = value
            .get("thread_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        titles.insert(id.to_string(), title.to_string());
    }
    titles
}

/// Which conversation a rollout holds, from its first line, and the person's first message in it.
fn read_rollout(path: &Path) -> Indexed {
    let Some(mut lines) = transcript_lines(path).map(|reader| reader.lines().map_while(Result::ok))
    else {
        return Indexed::default();
    };
    let Some(header) = lines.next().and_then(|first| rollout_header(&first)) else {
        return Indexed::default();
    };
    Indexed {
        title: lines.take(199).find_map(|line| codex_user_message(&line)),
        ..header
    }
}

fn rollout_header(line: &str) -> Option<Indexed> {
    let value = serde_json::from_str::<Value>(line.trim()).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    Some(Indexed {
        id: Some(payload.get("id").and_then(|i| i.as_str())?.to_string()),
        cwd: Some(payload.get("cwd").and_then(|c| c.as_str())?.to_string()),
        title: None,
    })
}

pub(super) fn codex_sessions(cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    let Some(root) = agent_config_root("codex", config_path) else {
        return Vec::new();
    };
    let indexed_titles = codex_indexed_titles(&root);
    let mut paths = Vec::new();
    collect_transcripts(&root.join("sessions"), is_rollout, &mut paths, 0);
    let mut files = stamped_transcripts(paths.into_iter());
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|(path, stamp)| {
            codex_session(path, *stamp, &indexed_titles, Some(cwd)).map(|found| found.session)
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

/// The session in one rollout, when it ran in `cwd`.
pub(super) fn codex_session_at(
    path: &Path,
    cwd: &str,
    config_path: Option<&str>,
) -> Option<AgentSession> {
    let root = agent_config_root("codex", config_path)?;
    let indexed_titles = codex_indexed_titles(&root);
    codex_session(path, file_stamp(path), &indexed_titles, Some(cwd)).map(|found| found.session)
}

/// Rollouts for every project share one folder, so the page lists them all
/// newest first and opens headers only until it has enough for these projects.
pub(super) fn codex_recent(scan: &PageScan<'_>, config_path: Option<&str>) -> Vec<Hit> {
    let Some(root) = agent_config_root("codex", config_path) else {
        return Vec::new();
    };
    let indexed_titles = codex_indexed_titles(&root);
    let mut paths = Vec::new();
    collect_transcripts(&root.join("sessions"), is_rollout, &mut paths, 0);
    let listed = paths
        .into_iter()
        .map(|path| {
            let stamp = file_stamp(&path);
            Listed {
                at_ms: stamp.unix_millis(),
                key: path.to_string_lossy().into_owned(),
                item: (path, stamp),
            }
        })
        .collect();
    scan.collect(listed, |(path, stamp)| {
        codex_session(path, *stamp, &indexed_titles, None)
    })
}

/// The session in one rollout, when it ran in `project` or no project is asked for.
fn codex_session(
    path: &Path,
    stamp: FileStamp,
    indexed_titles: &CodexTitles,
    project: Option<&str>,
) -> Option<Found> {
    let read = indexed(AgentKind::Codex, path, stamp, || read_rollout(path));
    let (Some(id), Some(cwd)) = (&read.id, &read.cwd) else {
        return None;
    };
    if project.is_some_and(|project| project != cwd) {
        return None;
    }
    let title = indexed_titles
        .get(id)
        .or(read.title.as_ref())
        .cloned()
        .unwrap_or_else(|| id.chars().take(8).collect());
    Some(Found {
        project: cwd.clone(),
        session: AgentSession {
            id: id.clone(),
            title,
            mtime: stamp.unix_secs(),
        },
        place: Some(path.to_path_buf()),
    })
}

fn codex_user_message(line: &str) -> Option<String> {
    let v = serde_json::from_str::<Value>(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("user_message") {
        return None;
    }
    condense(
        payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or(""),
    )
}

const CODEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CODEX_REQUEST_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

/// Renames a thread through Codex's own app server, which updates both its
/// thread database and `session_index.jsonl`; the database copy wins in Codex's
/// resume list, so writing the index alone would not show.
pub(super) async fn rename_codex_session(
    executable: &Path,
    config_path: Option<&str>,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let params = serde_json::json!({ "threadId": session_id, "name": name });
    codex_thread_request(executable, config_path, "thread/name/set", params, "rename").await
}

/// Deletes a thread through Codex's own app server, which removes its rollout
/// and its row in the thread database, so Codex's resume list drops it too.
pub(super) async fn delete_codex_session(
    executable: &Path,
    config_path: Option<&str>,
    session_id: &str,
) -> Result<(), String> {
    let params = serde_json::json!({ "threadId": session_id });
    codex_thread_request(executable, config_path, "thread/delete", params, "delete").await
}

async fn codex_thread_request(
    executable: &Path,
    config_path: Option<&str>,
    method: &str,
    params: Value,
    action: &str,
) -> Result<(), String> {
    let mut command = Command::from(sikemux_process::user_environment::agent_command(executable));
    apply_login_environment(&mut command);
    command
        .args(["app-server", "--stdio"])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    apply_process_config(&mut command, "codex", config_path);
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Codex".to_string())?;
    let mut stdin = child.stdin.take().ok_or("Could not talk to Codex")?;
    let stdout = child.stdout.take().ok_or("Could not talk to Codex")?;
    let mut lines = AsyncBufReader::new(stdout).lines();

    let answered = tokio::time::timeout(CODEX_REQUEST_TIMEOUT, async {
        let initialize = serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": { "clientInfo": { "name": "sikemux", "version": env!("CARGO_PKG_VERSION") } },
        });
        let request = serde_json::json!({ "id": 2, "method": method, "params": params });
        send_line(&mut stdin, &initialize.to_string()).await?;
        let mut output_bytes = 0usize;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "Could not read Codex's answer".to_string())?
        {
            output_bytes = output_bytes.saturating_add(line.len());
            if output_bytes > CODEX_REQUEST_OUTPUT_LIMIT {
                return Err("Codex's answer was too large".to_string());
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let answer = value.get("id").and_then(Value::as_u64);
            if let (Some(1 | 2), Some(error)) = (answer, value.get("error")) {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("Codex could not {action} the chat: {message}"));
            }
            match answer {
                Some(1) => {
                    send_line(&mut stdin, r#"{"method":"initialized"}"#).await?;
                    send_line(&mut stdin, &request.to_string()).await?;
                }
                Some(2) => return Ok(()),
                _ => {}
            }
        }
        Err(format!("Codex closed before it could {action} the chat"))
    })
    .await;

    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;
    answered.map_err(|_| format!("Codex took too long to {action} the chat"))?
}

async fn send_line(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    stdin
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|_| "Could not talk to Codex".to_string())?;
    stdin
        .flush()
        .await
        .map_err(|_| "Could not talk to Codex".to_string())
}

/// What one rollout line adds to what the person and Codex wrote.
pub(super) fn codex_prose(line: &str, prose: &mut Prose) {
    if !line.contains("\"event_msg\"")
        || !(line.contains("\"user_message\"") || line.contains("\"agent_message\""))
    {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let Some(payload) = value.get("payload") else {
        return;
    };
    if matches!(
        payload.get("type").and_then(Value::as_str),
        Some("user_message" | "agent_message")
    ) {
        if let Some(text) = payload.get("message").and_then(Value::as_str) {
            prose.push(text);
        }
    }
}

fn newest_first(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    found.sort_unstable_by(|a, b| b.cmp(a));
    found
}

/// Codex keeps each session in `YYYY/MM/DD/rollout-<time>-<session>.jsonl`, compressed to
/// `.jsonl.zst` once the session closes.
fn codex_transcript(sessions: &Path, id: &str) -> Option<PathBuf> {
    let plain = format!("-{id}.jsonl");
    let compressed = format!("{plain}.zst");
    newest_first(sessions)
        .iter()
        .flat_map(|year| newest_first(year))
        .flat_map(|month| newest_first(&month))
        .flat_map(|day| newest_first(&day))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("rollout-")
                        && (name.ends_with(&plain) || name.ends_with(&compressed))
                })
        })
}

/// Mirrors the Codex ACP adapter: the last turn's total against the model's window.
fn codex_context_line(line: &str) -> Option<SessionContext> {
    if !line.contains("\"token_count\"") {
        return None;
    }
    let value = serde_json::from_str::<Value>(line).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info")?;
    let used = info
        .get("last_token_usage")?
        .get("total_tokens")?
        .as_u64()?;
    let size = info
        .get("model_context_window")?
        .as_u64()
        .filter(|size| *size > 0)?;
    Some(SessionContext {
        used,
        size: Some(size),
    })
}

pub(crate) struct Codex;

impl AgentOps for Codex {
    fn sessions(&self, cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
        codex_sessions(cwd, config_path)
    }

    fn recent(&self, scan: &PageScan<'_>, config_path: Option<&str>) -> Vec<Hit> {
        codex_recent(scan, config_path)
    }

    fn session_place(&self, path: &Path) -> Option<PathBuf> {
        is_rollout(path).then(|| path.to_path_buf())
    }

    fn session_at(
        &self,
        place: &Path,
        cwd: &str,
        config_path: Option<&str>,
    ) -> Option<AgentSession> {
        codex_session_at(place, cwd, config_path)
    }

    fn session_places(&self, _cwd: &str, config_path: Option<&str>) -> SessionPlaces {
        let mut places = SessionPlaces::default();
        if let Some(root) = agent_config_root("codex", config_path) {
            places
                .sessions
                .push((root.join("sessions"), RecursiveMode::Recursive));
            places.listings.push(root.join("session_index.jsonl"));
        }
        places
    }

    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()> {
        each_line(found.place.as_deref()?, |line| codex_prose(line, prose));
        Some(())
    }

    fn transcript(&self, root: &Path, _cwd: Option<&str>, id: &str) -> Option<PathBuf> {
        codex_transcript(&root.join("sessions"), id)
    }

    fn context(&self, transcript: &Path) -> Option<SessionContext> {
        last_line_matching(transcript, codex_context_line)
    }

    fn rename_command(&self) -> Option<RenameCommand> {
        Some(|executable, config_path, session_id, name| {
            Box::pin(rename_codex_session(
                executable,
                config_path,
                session_id,
                name,
            ))
        })
    }

    fn delete_command(&self) -> Option<DeleteCommand> {
        Some(|executable, config_path, session_id| {
            Box::pin(delete_codex_session(executable, config_path, session_id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{codex_indexed_titles, codex_sessions, file_stamp, indexed, read_rollout};
    use crate::agent_catalog::AgentKind;
    use crate::sessions::index::Indexed;
    use crate::sessions::FileStamp;
    use std::io::Write;
    use std::path::Path;
    use std::sync::Arc;

    fn read(path: &Path, stamp: FileStamp) -> Arc<Indexed> {
        indexed(AgentKind::Codex, path, stamp, || read_rollout(path))
    }

    #[test]
    fn codex_title_reads_the_current_user_message_shape() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            r#"{{"type":"session_meta","payload":{{"id":"session-1","cwd":"/repo"}}}}"#
        )
        .unwrap();
        writeln!(
            transcript,
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"Explain this codebase"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();

        assert_eq!(
            read_rollout(transcript.path()).title.as_deref(),
            Some("Explain this codebase")
        );
    }

    #[test]
    fn codex_sessions_use_the_latest_indexed_thread_name() {
        let root = tempfile::tempdir().unwrap();
        let sessions_dir = root.path().join("sessions");
        std::fs::create_dir(&sessions_dir).unwrap();
        std::fs::write(
            root.path().join("session_index.jsonl"),
            concat!(
                "{\"id\":\"session-1\",\"thread_name\":\"Initial title\"}\n",
                "{\"id\":\"session-1\",\"thread_name\":\"Add draggable project sorting\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("rollout-session-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\",\"cwd\":\"/repo\"}}\n",
        )
        .unwrap();

        assert_eq!(
            codex_indexed_titles(root.path())
                .get("session-1")
                .map(String::as_str),
            Some("Add draggable project sorting")
        );
        let sessions = codex_sessions("/repo", root.path().to_str());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Add draggable project sorting");
    }

    #[test]
    fn codex_index_is_reparsed_only_after_the_file_changes() {
        let root = tempfile::tempdir().unwrap();
        let index = root.path().join("session_index.jsonl");
        std::fs::write(&index, "{\"id\":\"session-1\",\"thread_name\":\"First\"}\n").unwrap();
        let first = codex_indexed_titles(root.path());
        assert!(Arc::ptr_eq(&first, &codex_indexed_titles(root.path())));

        std::fs::write(
            &index,
            concat!(
                "{\"id\":\"session-1\",\"thread_name\":\"First\"}\n",
                "{\"id\":\"session-2\",\"thread_name\":\"Second\"}\n"
            ),
        )
        .unwrap();
        let reparsed = codex_indexed_titles(root.path());
        assert!(!Arc::ptr_eq(&first, &reparsed));
        assert_eq!(
            reparsed.get("session-2").map(String::as_str),
            Some("Second")
        );
    }

    #[test]
    fn a_rollout_header_is_read_once_per_write() {
        let root = tempfile::tempdir().unwrap();
        let rollout = root.path().join("rollout-session-1.jsonl");
        std::fs::write(
            &rollout,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\",\"cwd\":\"/repo\"}}\n",
        )
        .unwrap();

        let stamp = file_stamp(&rollout);
        let first = read(&rollout, stamp);
        assert_eq!(first.cwd.as_deref(), Some("/repo"));
        assert!(Arc::ptr_eq(&first, &read(&rollout, stamp)));

        std::fs::remove_file(&rollout).unwrap();
        assert!(read(&rollout, stamp).cwd.is_some());
        assert!(read(&rollout, file_stamp(&rollout)).cwd.is_none());
    }

    #[test]
    fn the_index_rereads_a_transcript_after_a_same_tick_append() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            r#"{{"type":"session_meta","payload":{{"id":"session-1","cwd":"/repo"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let first_stamp = file_stamp(transcript.path());
        assert_eq!(read(transcript.path(), first_stamp).title, None);

        writeln!(
            transcript,
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"Hello"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let second_stamp = file_stamp(transcript.path());
        assert_ne!(first_stamp.len, second_stamp.len);
        assert_eq!(
            read(transcript.path(), second_stamp).title.as_deref(),
            Some("Hello")
        );
    }
}
