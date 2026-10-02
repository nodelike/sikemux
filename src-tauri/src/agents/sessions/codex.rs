use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rayon::prelude::*;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{ChildStdin, Command};

use super::recent::{Found, Hit, Listed, PageScan};
use super::{
    cached_title, collect_jsonl, condense, next_title_cache_access, stamped_transcripts,
    title_cache_stamp, TitleCacheStamp, MAX_AGENT_TRANSCRIPTS_INSPECTED,
};
use crate::agents::config::agent_config_root;
use crate::agents::executable::{apply_login_environment, apply_process_config};
use crate::agents::AgentSession;

// ---- codex — ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl ----------------

/// The parsed Codex session index, kept until the file it came from is written
/// again. It is one file for every project, so re-reading it per scan was the
/// bulk of a Codex listing.
fn codex_index_cache() -> &'static Mutex<CodexIndexCache> {
    static CACHE: OnceLock<Mutex<CodexIndexCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

type CodexTitles = HashMap<String, String>;
type CodexIndexCache = HashMap<PathBuf, (TitleCacheStamp, Arc<CodexTitles>)>;

fn codex_indexed_titles(root: &Path) -> Arc<CodexTitles> {
    let path = root.join("session_index.jsonl");
    let stamp = title_cache_stamp(&path);
    if let Ok(cache) = codex_index_cache().lock() {
        if let Some((cached, titles)) = cache.get(&path) {
            if *cached == stamp {
                return titles.clone();
            }
        }
    }
    let titles = Arc::new(read_codex_index(&path));
    if let Ok(mut cache) = codex_index_cache().lock() {
        cache.insert(path, (stamp, titles.clone()));
    }
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

/// What the first line of a rollout says about the conversation in it.
#[derive(Clone)]
struct CodexRollout {
    id: String,
    cwd: String,
}

type CodexHeaderCache = HashMap<PathBuf, (TitleCacheStamp, Option<Arc<CodexRollout>>, u64)>;
const MAX_CODEX_HEADER_ENTRIES: usize = 8_192;

/// Which conversation each rollout holds, so a listing only opens the rollouts
/// that have been written since the last one.
fn codex_header_cache() -> &'static Mutex<CodexHeaderCache> {
    static CACHE: OnceLock<Mutex<CodexHeaderCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn read_codex_rollout(path: &Path) -> Option<CodexRollout> {
    let file = fs::File::open(path).ok()?;
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).ok()?;
    let value = serde_json::from_str::<Value>(first.trim()).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    Some(CodexRollout {
        id: payload.get("id").and_then(|i| i.as_str())?.to_string(),
        cwd: payload.get("cwd").and_then(|c| c.as_str())?.to_string(),
    })
}

fn cached_codex_rollout(path: &Path, stamp: TitleCacheStamp) -> Option<Arc<CodexRollout>> {
    if let Ok(mut cache) = codex_header_cache().lock() {
        if let Some((cached, rollout, access)) = cache.get_mut(path) {
            if *cached == stamp {
                *access = next_title_cache_access();
                return rollout.clone();
            }
        }
    }
    let rollout = read_codex_rollout(path).map(Arc::new);
    if let Ok(mut cache) = codex_header_cache().lock() {
        if cache.len() >= MAX_CODEX_HEADER_ENTRIES && !cache.contains_key(path) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, _, access))| *access)
                .map(|(path, _)| path.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            path.to_path_buf(),
            (stamp, rollout.clone(), next_title_cache_access()),
        );
    }
    rollout
}

pub(super) fn codex_sessions(cwd: &str, config_path: Option<&str>) -> Vec<AgentSession> {
    let Some(root) = agent_config_root("codex", config_path) else {
        return Vec::new();
    };
    let indexed_titles = codex_indexed_titles(&root);
    let mut paths = Vec::new();
    collect_jsonl(&root.join("sessions"), &mut paths, 0);
    let mut files = stamped_transcripts(paths.into_iter());
    files.truncate(MAX_AGENT_TRANSCRIPTS_INSPECTED);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|(path, stamp)| {
            codex_session(path, *stamp, &indexed_titles)
                .filter(|found| found.project == cwd)
                .map(|found| found.session)
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

/// Rollouts for every project share one folder, so the page lists them all
/// newest first and opens headers only until it has enough for these projects.
pub(super) fn codex_recent(scan: &PageScan<'_>, config_path: Option<&str>) -> Vec<Hit> {
    let Some(root) = agent_config_root("codex", config_path) else {
        return Vec::new();
    };
    let indexed_titles = codex_indexed_titles(&root);
    let mut paths = Vec::new();
    collect_jsonl(&root.join("sessions"), &mut paths, 0);
    let listed = paths
        .into_iter()
        .map(|path| {
            let stamp = title_cache_stamp(&path);
            Listed {
                at_ms: stamp.unix_millis(),
                key: path.to_string_lossy().into_owned(),
                item: (path, stamp),
            }
        })
        .collect();
    scan.collect(listed, |(path, stamp)| {
        codex_session(path, *stamp, &indexed_titles)
    })
}

fn codex_session(
    path: &Path,
    stamp: TitleCacheStamp,
    indexed_titles: &CodexTitles,
) -> Option<Found> {
    let rollout = cached_codex_rollout(path, stamp)?;
    let id = rollout.id.as_str();
    let title = indexed_titles
        .get(id)
        .cloned()
        .or_else(|| cached_title(path, stamp, || codex_title(path)))
        .unwrap_or_else(|| id.chars().take(8).collect());
    Some(Found {
        project: rollout.cwd.clone(),
        session: AgentSession {
            id: id.to_string(),
            title,
            mtime: stamp.unix_secs(),
        },
    })
}

fn codex_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(200).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = v.get("payload");
        if payload.and_then(|p| p.get("type")).and_then(|t| t.as_str()) != Some("user_message") {
            continue;
        }
        let msg = payload
            .and_then(|p| p.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        if let Some(t) = condense(msg) {
            return Some(t);
        }
    }
    None
}

const CODEX_RENAME_TIMEOUT: Duration = Duration::from_secs(15);
const CODEX_RENAME_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

/// Renames a thread through Codex's own app server, which updates both its
/// thread database and `session_index.jsonl`; the database copy wins in Codex's
/// resume list, so writing the index alone would not show.
pub(super) async fn rename_codex_session(
    executable: &Path,
    config_path: Option<&str>,
    session_id: &str,
    name: &str,
) -> Result<(), String> {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
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

    let rename = tokio::time::timeout(CODEX_RENAME_TIMEOUT, async {
        let initialize = serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": { "clientInfo": { "name": "sikemux", "version": env!("CARGO_PKG_VERSION") } },
        });
        let set_name = serde_json::json!({
            "id": 2,
            "method": "thread/name/set",
            "params": { "threadId": session_id, "name": name },
        });
        send_line(&mut stdin, &initialize.to_string()).await?;
        let mut output_bytes = 0usize;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "Could not read Codex's answer".to_string())?
        {
            output_bytes = output_bytes.saturating_add(line.len());
            if output_bytes > CODEX_RENAME_OUTPUT_LIMIT {
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
                return Err(format!("Codex could not rename the chat: {message}"));
            }
            match answer {
                Some(1) => {
                    send_line(&mut stdin, r#"{"method":"initialized"}"#).await?;
                    send_line(&mut stdin, &set_name.to_string()).await?;
                }
                Some(2) => return Ok(()),
                _ => {}
            }
        }
        Err("Codex closed before renaming the chat".to_string())
    })
    .await;

    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;
    rename.map_err(|_| "Codex took too long to rename the chat".to_string())?
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

#[cfg(test)]
mod tests {
    use super::{
        cached_codex_rollout, cached_title, codex_indexed_titles, codex_sessions, codex_title,
        title_cache_stamp,
    };
    use std::io::Write;
    use std::sync::Arc;

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
            codex_title(transcript.path()).as_deref(),
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

        let stamp = title_cache_stamp(&rollout);
        let first = cached_codex_rollout(&rollout, stamp).unwrap();
        assert_eq!(first.cwd, "/repo");
        let again = cached_codex_rollout(&rollout, stamp).unwrap();
        assert!(Arc::ptr_eq(&first, &again));

        std::fs::remove_file(&rollout).unwrap();
        assert!(cached_codex_rollout(&rollout, stamp).is_some());
        assert!(cached_codex_rollout(&rollout, title_cache_stamp(&rollout)).is_none());
    }

    #[test]
    fn title_cache_rechecks_a_transcript_after_a_same_tick_append() {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            transcript,
            r#"{{"type":"session_meta","payload":{{"id":"session-1","cwd":"/repo"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let first_stamp = title_cache_stamp(transcript.path());
        assert_eq!(
            cached_title(transcript.path(), first_stamp, || codex_title(
                transcript.path()
            )),
            None
        );

        writeln!(
            transcript,
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"Hello"}}}}"#
        )
        .unwrap();
        transcript.flush().unwrap();
        let second_stamp = title_cache_stamp(transcript.path());
        assert_ne!(first_stamp.len, second_stamp.len);
        assert_eq!(
            cached_title(transcript.path(), second_stamp, || codex_title(
                transcript.path()
            ))
            .as_deref(),
            Some("Hello")
        );
    }
}
