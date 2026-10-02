use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::async_runtime::spawn_blocking;
use tauri::{AppHandle, Emitter};

use super::config::{agent_config_root, grok_root, omp_session_dirs};
use super::sessions::opencode::opencode_data_dirs;
use super::sessions::pi::pi_session_dir;
use super::{home_path, AgentKind};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentSessionsChanged {
    agent: &'static str,
    cwd: String,
    config_path: Option<String>,
}

struct AgentWatchTarget {
    dir: PathBuf,
    mode: RecursiveMode,
}

struct AgentWatchHandle {
    _watchers: Vec<RecommendedWatcher>,
}

fn watch_registry() -> &'static Mutex<HashMap<u32, Arc<AgentWatchHandle>>> {
    static R: OnceLock<Mutex<HashMap<u32, Arc<AgentWatchHandle>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn watch_count() -> usize {
    watch_registry().lock().map(|r| r.len()).unwrap_or(0)
}

static NEXT_WATCH_ID: AtomicU32 = AtomicU32::new(1);

const AGENT_DEBOUNCE_MS: u64 = 200;
/// While a turn is running its transcript is appended to constantly. Rescanning
/// the whole group at every append buys nothing, so the wait gets longer until
/// the turn ends.
const AGENT_STREAMING_DEBOUNCE_MS: u64 = 2_000;

fn push_watch_target(out: &mut Vec<AgentWatchTarget>, dir: PathBuf, mode: RecursiveMode) {
    if !dir.is_dir() {
        return;
    }
    if let Some(existing) = out.iter_mut().find(|target| target.dir == dir) {
        if matches!(mode, RecursiveMode::Recursive) {
            existing.mode = RecursiveMode::Recursive;
        }
        return;
    }
    out.push(AgentWatchTarget { dir, mode });
}

fn push_existing_or_parent(
    out: &mut Vec<AgentWatchTarget>,
    path: PathBuf,
    existing_mode: RecursiveMode,
) {
    if path.is_dir() {
        push_watch_target(out, path, existing_mode);
    } else if let Some(parent) = path.parent() {
        push_watch_target(out, parent.to_path_buf(), RecursiveMode::NonRecursive);
    }
}

fn agent_watch_dirs(
    agent: AgentKind,
    cwd: &str,
    config_path: Option<&str>,
) -> Vec<AgentWatchTarget> {
    let mut out = Vec::new();
    match agent {
        AgentKind::Claude => {
            if let Some(root) = agent_config_root("claude", config_path) {
                let projects = root.join("projects");
                let project = projects.join(cwd.replace('/', "-"));
                if project.is_dir() {
                    push_watch_target(&mut out, project, RecursiveMode::Recursive);
                } else {
                    push_watch_target(&mut out, projects, RecursiveMode::NonRecursive);
                }
            }
        }
        AgentKind::Codex => {
            if let Some(codex) = agent_config_root("codex", config_path) {
                let sessions = codex.join("sessions");
                push_watch_target(&mut out, codex, RecursiveMode::NonRecursive);
                push_watch_target(&mut out, sessions, RecursiveMode::Recursive);
            }
        }
        AgentKind::Hermes => {
            if let Some(home) = home_path() {
                push_watch_target(&mut out, home.join(".hermes"), RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Pi => {
            if let Some(root) = pi_session_dir() {
                push_existing_or_parent(&mut out, root, RecursiveMode::Recursive);
            }
        }
        AgentKind::Opencode => {
            for dir in opencode_data_dirs() {
                push_existing_or_parent(&mut out, dir, RecursiveMode::NonRecursive);
            }
        }
        AgentKind::Omp => {
            for dir in omp_session_dirs() {
                push_existing_or_parent(&mut out, dir, RecursiveMode::Recursive);
            }
        }
        AgentKind::Grok => {
            if let Some(root) = grok_root() {
                push_existing_or_parent(&mut out, root.join("sessions"), RecursiveMode::Recursive);
            }
        }
    }
    out
}

fn agent_event_interesting(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Session ids with a turn in flight, per watch group.
fn streaming_sessions() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stream_group_key(agent: &str, cwd: &str, config_path: Option<&str>) -> String {
    format!("{agent}\0{cwd}\0{}", config_path.unwrap_or(""))
}

/// Tells the session watcher which conversation is being written to right now.
/// The chat pane already has every word of it, so the writes it makes are not
/// worth a rescan of the whole project.
pub fn note_streaming_session(
    agent: &str,
    cwd: &str,
    config_path: Option<&str>,
    session_id: &str,
    streaming: bool,
) {
    let key = stream_group_key(agent, cwd, config_path);
    let Ok(mut sessions) = streaming_sessions().lock() else {
        return;
    };
    if streaming {
        sessions
            .entry(key)
            .or_default()
            .insert(session_id.to_string());
        return;
    }
    if let Some(group) = sessions.get_mut(&key) {
        group.remove(session_id);
        if group.is_empty() {
            sessions.remove(&key);
        }
    }
}

fn group_is_streaming(group: &str) -> bool {
    streaming_sessions()
        .lock()
        .is_ok_and(|sessions| sessions.contains_key(group))
}

/// True when every changed file names a session this group is streaming. Both
/// Claude and Codex put the session id in the transcript's file name.
fn streaming_transcripts_only(group: &str, paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let Ok(sessions) = streaming_sessions().lock() else {
        return false;
    };
    let Some(ids) = sessions.get(group) else {
        return false;
    };
    paths.iter().all(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| ids.iter().any(|id| name.contains(id.as_str())))
    })
}

fn spawn_agent_debouncer(
    app: AppHandle,
    agent: &'static str,
    cwd: String,
    config_path: Option<String>,
    group: String,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    tauri::async_runtime::spawn(async move {
        while rx.recv().await.is_some() {
            let wait = || {
                Duration::from_millis(if group_is_streaming(&group) {
                    AGENT_STREAMING_DEBOUNCE_MS
                } else {
                    AGENT_DEBOUNCE_MS
                })
            };
            let sleep = tokio::time::sleep(wait());
            tokio::pin!(sleep);
            let mut closed = false;
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    msg = rx.recv() => {
                        if msg.is_none() {
                            closed = true;
                            break;
                        }
                        sleep.as_mut().reset(tokio::time::Instant::now() + wait());
                    }
                }
            }
            if closed {
                return;
            }
            let _ = app.emit_to(
                "main",
                "agent_sessions_changed",
                AgentSessionsChanged {
                    agent,
                    cwd: cwd.clone(),
                    config_path: config_path.clone(),
                },
            );
        }
    });
}

fn start_agent_watch(
    app: AppHandle,
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Result<u32, String> {
    let dirs = agent_watch_dirs(agent, &cwd, config_path.as_deref());
    let id = NEXT_WATCH_ID.fetch_add(1, Ordering::Relaxed);
    let group = stream_group_key(agent.as_str(), &cwd, config_path.as_deref());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    spawn_agent_debouncer(app, agent.as_str(), cwd, config_path, group.clone(), rx);

    let mut watchers = Vec::new();
    for target in dirs {
        let tx_events = tx.clone();
        let group = group.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !agent_event_interesting(&event) || streaming_transcripts_only(&group, &event.paths)
            {
                return;
            }
            let _ = tx_events.send(());
        })
        .map_err(|e| e.to_string())?;
        if watcher.watch(&target.dir, target.mode).is_ok() {
            watchers.push(watcher);
        }
    }

    watch_registry().lock().map_err(|e| e.to_string())?.insert(
        id,
        Arc::new(AgentWatchHandle {
            _watchers: watchers,
        }),
    );
    Ok(id)
}

#[tauri::command]
pub async fn agent_sessions_watch_start(
    app: AppHandle,
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
) -> Result<u32, String> {
    spawn_blocking(move || start_agent_watch(app, agent, cwd, config_path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn agent_sessions_watch_stop(id: u32) -> Result<(), String> {
    spawn_blocking(move || {
        watch_registry()
            .lock()
            .map_err(|e| e.to_string())?
            .remove(&id);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::{
        agent_watch_dirs, group_is_streaming, note_streaming_session, stream_group_key,
        streaming_transcripts_only, AgentKind,
    };
    use std::path::PathBuf;

    #[test]
    fn the_watcher_ignores_writes_to_a_streaming_transcript() {
        let group = stream_group_key("claude", "/repo", None);
        let transcript = PathBuf::from("/home/me/.claude/projects/-repo/session-1.jsonl");
        let other = PathBuf::from("/home/me/.claude/projects/-repo/session-2.jsonl");

        assert!(!group_is_streaming(&group));
        assert!(!streaming_transcripts_only(
            &group,
            std::slice::from_ref(&transcript)
        ));

        note_streaming_session("claude", "/repo", None, "session-1", true);
        assert!(group_is_streaming(&group));
        assert!(streaming_transcripts_only(
            &group,
            std::slice::from_ref(&transcript)
        ));
        assert!(!streaming_transcripts_only(
            &group,
            &[transcript.clone(), other]
        ));
        assert!(!streaming_transcripts_only(&group, &[]));

        note_streaming_session("claude", "/repo", None, "session-1", false);
        assert!(!group_is_streaming(&group));
        assert!(!streaming_transcripts_only(&group, &[transcript]));
    }

    #[test]
    fn codex_title_watch_covers_transcripts_and_the_session_index() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        std::fs::create_dir(&sessions).unwrap();

        let targets = agent_watch_dirs(AgentKind::Codex, "/repo", root.path().to_str());

        assert!(targets.iter().any(|target| target.dir == root.path()));
        assert!(targets.iter().any(|target| target.dir == sessions));
    }
}
