use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize)]
pub struct AgentSession {
    id: String,
    title: String,
    mtime: u64,
}

#[derive(Serialize)]
pub struct AgentInfo {
    #[serde(rename = "type")]
    kind: &'static str,
    label: &'static str,
    command: &'static str,
}

#[derive(Serialize, Clone)]
struct AgentSessionsChanged {
    agent: &'static str,
    cwd: String,
}

struct AgentDef {
    kind: &'static str,
    label: &'static str,
    command: &'static str,
}

const AGENT_DEFS: &[AgentDef] = &[
    AgentDef {
        kind: "claude",
        label: "Claude",
        command: "claude",
    },
    AgentDef {
        kind: "codex",
        label: "Codex",
        command: "codex",
    },
    AgentDef {
        kind: "hermes",
        label: "Hermes",
        command: "hermes",
    },
    AgentDef {
        kind: "pi",
        label: "Pi",
        command: "pi",
    },
    AgentDef {
        kind: "opencode",
        label: "OpenCode",
        command: "opencode",
    },
];

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Hermes,
    Pi,
    Opencode,
}

impl AgentKind {
    fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Hermes => "hermes",
            AgentKind::Pi => "pi",
            AgentKind::Opencode => "opencode",
        }
    }
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

/// Agent CLIs that are installed for the current user. The app's PATH is fixed
/// from the login shell during boot, so this matches what spawned PTYs can run.
#[tauri::command]
pub fn available_agents() -> Vec<AgentInfo> {
    AGENT_DEFS
        .iter()
        .filter(|def| executable_in_path(def.kind, def.command))
        .map(|def| AgentInfo {
            kind: def.kind,
            label: def.label,
            command: def.command,
        })
        .collect()
}

/// Existing on-disk conversations for an agent.
#[tauri::command]
pub fn agent_sessions(agent: AgentKind, cwd: String) -> Vec<AgentSession> {
    match agent {
        AgentKind::Claude => claude_sessions(&cwd),
        AgentKind::Codex => codex_sessions(&cwd),
        AgentKind::Hermes => hermes_sessions(),
        AgentKind::Pi => pi_sessions(&cwd),
        AgentKind::Opencode => opencode_sessions(&cwd),
    }
}

const AGENT_DEBOUNCE_MS: u64 = 200;

fn home_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

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

fn agent_watch_dirs(agent: AgentKind, cwd: &str) -> Vec<AgentWatchTarget> {
    let mut out = Vec::new();
    match agent {
        AgentKind::Claude => {
            if let Some(home) = home_path() {
                let projects = home.join(".claude/projects");
                let project = projects.join(cwd.replace('/', "-"));
                if project.is_dir() {
                    push_watch_target(&mut out, project, RecursiveMode::Recursive);
                } else {
                    push_watch_target(&mut out, projects, RecursiveMode::NonRecursive);
                }
            }
        }
        AgentKind::Codex => {
            if let Some(home) = home_path() {
                let codex = home.join(".codex");
                let sessions = codex.join("sessions");
                if sessions.is_dir() {
                    push_watch_target(&mut out, sessions, RecursiveMode::Recursive);
                } else {
                    push_watch_target(&mut out, codex, RecursiveMode::NonRecursive);
                }
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
    }
    out
}

fn agent_event_interesting(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn spawn_agent_debouncer(
    app: AppHandle,
    agent: &'static str,
    cwd: String,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    tauri::async_runtime::spawn(async move {
        while rx.recv().await.is_some() {
            let sleep = tokio::time::sleep(Duration::from_millis(AGENT_DEBOUNCE_MS));
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
                        sleep
                            .as_mut()
                            .reset(tokio::time::Instant::now() + Duration::from_millis(AGENT_DEBOUNCE_MS));
                    }
                }
            }
            if closed {
                return;
            }
            let _ = app.emit(
                "agent_sessions_changed",
                AgentSessionsChanged {
                    agent,
                    cwd: cwd.clone(),
                },
            );
        }
    });
}

#[tauri::command]
pub fn agent_sessions_watch_start(
    app: AppHandle,
    agent: AgentKind,
    cwd: String,
) -> Result<u32, String> {
    let dirs = agent_watch_dirs(agent, &cwd);
    let id = NEXT_WATCH_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    spawn_agent_debouncer(app, agent.as_str(), cwd, rx);

    let mut watchers = Vec::new();
    for target in dirs {
        let tx_events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !agent_event_interesting(&event) {
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
pub fn agent_sessions_watch_stop(id: u32) -> Result<(), String> {
    watch_registry()
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&id);
    Ok(())
}

fn executable_in_path(agent: &str, bin: &str) -> bool {
    crate::system::find_executable_matching(bin, |candidate| allowed_agent_path(agent, candidate))
        .is_some()
}

fn allowed_agent_path(agent: &str, path: &Path) -> bool {
    if agent != "opencode" {
        return true;
    }
    let Ok(home) = std::env::var("HOME") else {
        return true;
    };
    allowed_agent_path_for_home(agent, path, Path::new(&home))
}

fn allowed_agent_path_for_home(agent: &str, path: &Path, home: &Path) -> bool {
    if agent != "opencode" {
        return true;
    }
    // OpenCode leaves a runnable self-contained binary under ~/.opencode/bin.
    // Treat that as app data/cache rather than a user-visible system install;
    // otherwise stale copies keep showing up in the agent rail after uninstall.
    path.parent() != Some(home.join(".opencode").join("bin").as_path())
}

fn mtime_of(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleCacheStamp {
    modified_ns: u128,
    len: u64,
}

fn title_cache_stamp(path: &Path) -> TitleCacheStamp {
    let metadata = fs::metadata(path).ok();
    TitleCacheStamp {
        modified_ns: metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or(0),
        len: metadata.map(|value| value.len()).unwrap_or(0),
    }
}

fn condense(text: &str) -> Option<String> {
    let c = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if c.is_empty() || c.starts_with('<') {
        return None;
    }
    Some(c.chars().take(72).collect())
}

fn text_from_content(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    content.as_array()?.iter().find_map(|b| {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            b.get("text").and_then(|t| t.as_str()).map(String::from)
        } else {
            None
        }
    })
}

/// Cached title per transcript: `path -> (high-resolution stamp, title)`.
type TitleCache = HashMap<PathBuf, (TitleCacheStamp, Option<String>)>;

/// Per-file title cache keyed by a high-resolution file stamp. Titles are derived from transcript
/// content that only ever grows, so an unchanged nanosecond timestamp and file
/// length means an unchanged title. Length prevents same-tick appends from
/// preserving a cached title-less result on coarse filesystems.
/// This turns the palette's cold scan of every session into a one-time cost:
/// reopening it re-reads only the sessions that have actually changed.
fn title_cache() -> &'static Mutex<TitleCache> {
    static CACHE: OnceLock<Mutex<TitleCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the cached title for `path` when its stamp matches, otherwise run
/// `compute`, store the result (including `None`, so title-less files are not
/// re-scanned), and return it.
fn cached_title<F>(path: &Path, stamp: TitleCacheStamp, compute: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if let Ok(cache) = title_cache().lock() {
        if let Some((cached_stamp, title)) = cache.get(path) {
            if *cached_stamp == stamp {
                return title.clone();
            }
        }
    }
    let title = compute();
    if let Ok(mut cache) = title_cache().lock() {
        cache.insert(path.to_path_buf(), (stamp, title.clone()));
    }
    title
}

/// Read up to `n` bytes from the start of `file` as lossy UTF-8.
fn read_prefix(file: &mut fs::File, n: u64) -> Option<String> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = Vec::new();
    file.by_ref().take(n).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Read from `start` to the end of `file` as lossy UTF-8.
fn read_suffix(file: &mut fs::File, start: u64) -> Option<String> {
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---- claude — ~/.claude/projects/<cwd-dashed>/<uuid>.jsonl --------------
fn claude_sessions(cwd: &str) -> Vec<AgentSession> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let dir = PathBuf::from(&home)
        .join(".claude/projects")
        .join(cwd.replace('/', "-"));
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    // Titles come from reading each transcript, so fan the per-file work out
    // across rayon's pool instead of scanning sessions one at a time.
    let mut out: Vec<AgentSession> = paths
        .par_iter()
        .filter_map(|path| {
            let id = path.file_stem().and_then(|s| s.to_str())?;
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || claude_title(path))
                .unwrap_or_else(|| id.chars().take(8).collect());
            Some(AgentSession {
                id: id.to_string(),
                title,
                mtime,
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

/// Pull a title out of one Claude transcript line, updating the running
/// `ai_title` (last write wins) and `first_user` (first write wins).
fn scan_claude_line(line: &str, ai_title: &mut Option<String>, first_user: &mut Option<String>) {
    if line.contains("\"type\":\"ai-title\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()).and_then(condense) {
                *ai_title = Some(t);
            }
        }
    } else if first_user.is_none() && line.contains("\"type\":\"user\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user") {
                *first_user = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(text_from_content)
                    .and_then(|text| condense(&text));
            }
        }
    }
}

// Bound the per-file read: the first user prompt sits near the top and Claude
// emits `ai-title` entries continuously (so the freshest title sits at the end),
// which lets us skip the middle of multi-MB transcripts.
const CLAUDE_HEAD_BYTES: u64 = 128 * 1024;
const CLAUDE_TAIL_BYTES: u64 = 128 * 1024;

fn claude_title(path: &Path) -> Option<String> {
    // Claude writes the human-readable title (auto-generated, then overwritten by
    // `/rename`) as `{"type":"ai-title","aiTitle":...}` entries appended as the
    // session grows — last one wins. This is what Claude's own /resume picker
    // shows. Prefer it; fall back to the first user prompt for sessions that have
    // no title yet. Cheap substring guards keep us from JSON-parsing every line.
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut ai_title: Option<String> = None;
    let mut first_user: Option<String> = None;

    // Head: captures the first user prompt and any early ai-title. For small
    // transcripts this covers the whole file, keeping the result exact.
    let head = read_prefix(&mut file, CLAUDE_HEAD_BYTES.min(len))?;
    for line in head.lines() {
        scan_claude_line(line, &mut ai_title, &mut first_user);
    }

    // Tail: the most recent ai-title lives at the end of large transcripts. Skip
    // the first (likely partial) line, then take the last ai-title we can parse.
    if len > CLAUDE_HEAD_BYTES {
        if let Some(tail) = read_suffix(&mut file, len.saturating_sub(CLAUDE_TAIL_BYTES)) {
            for line in tail.lines().skip(1) {
                if line.contains("\"type\":\"ai-title\"") {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        if let Some(t) =
                            v.get("aiTitle").and_then(|t| t.as_str()).and_then(condense)
                        {
                            ai_title = Some(t);
                        }
                    }
                }
            }
        }
    }

    ai_title.or(first_user)
}

// ---- codex — ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl ----------------
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn codex_sessions(cwd: &str) -> Vec<AgentSession> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(&PathBuf::from(&home).join(".codex/sessions"), &mut files, 0);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            let file = fs::File::open(path).ok()?;
            let mut first = String::new();
            BufReader::new(file).read_line(&mut first).ok()?;
            let v = serde_json::from_str::<Value>(first.trim()).ok()?;
            if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
                return None;
            }
            let payload = v.get("payload")?;
            if payload.get("cwd").and_then(|c| c.as_str()) != Some(cwd) {
                return None;
            }
            let id = payload.get("id").and_then(|i| i.as_str())?;
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || codex_title(path))
                .unwrap_or_else(|| id.chars().take(8).collect());
            Some(AgentSession {
                id: id.to_string(),
                title,
                mtime,
            })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
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

// ---- pi — ~/.pi/agent/sessions/**/<session>.jsonl ----------------------
fn pi_session_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        return Some(PathBuf::from(dir).join("sessions"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".pi/agent/sessions"))
}

fn pi_sessions(cwd: &str) -> Vec<AgentSession> {
    let Some(root) = pi_session_dir() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files, 0);

    let mut out: Vec<AgentSession> = files
        .par_iter()
        .filter_map(|path| {
            let file = fs::File::open(path).ok()?;
            let mut first = String::new();
            BufReader::new(file).read_line(&mut first).ok()?;
            let v = serde_json::from_str::<Value>(first.trim()).ok()?;
            if v.get("type").and_then(|t| t.as_str()) != Some("session") {
                return None;
            }
            if v.get("cwd").and_then(|c| c.as_str()) != Some(cwd) {
                return None;
            }
            let id = path.to_string_lossy().to_string();
            let mtime = mtime_of(path);
            let title = cached_title(path, title_cache_stamp(path), || pi_title(path))
                .or_else(|| v.get("id").and_then(|i| i.as_str()).and_then(condense))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("session")
                        .chars()
                        .take(13)
                        .collect()
                });
            Some(AgentSession { id, title, mtime })
        })
        .collect();
    out.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    out
}

fn pi_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut first_user: Option<String> = None;
    let mut named: Option<String> = None;
    for line in BufReader::new(file).lines().take(220).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_info") => {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()).and_then(condense) {
                    named = Some(name);
                }
            }
            Some("message") if first_user.is_none() => {
                let Some(message) = v.get("message") else {
                    continue;
                };
                if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                    continue;
                }
                if let Some(text) = message
                    .get("content")
                    .and_then(text_from_content)
                    .and_then(|t| condense(&t))
                {
                    first_user = Some(text);
                }
            }
            _ => {}
        }
    }
    named.or(first_user)
}

// ---- hermes — `sessions` table in ~/.hermes/state.db (SQLite) -----------
fn hermes_sessions() -> Vec<AgentSession> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let db = PathBuf::from(&home).join(".hermes/state.db");
    if !db.exists() {
        return Vec::new();
    }

    let conn = match Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, \
         COALESCE(NULLIF(TRIM(title), ''), substr(id, 1, 13)) AS title, \
         CAST(COALESCE(started_at, 0) AS INTEGER) AS mtime \
         FROM sessions ORDER BY started_at DESC LIMIT 400",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        Ok(AgentSession {
            id: row.get::<_, String>(0)?,
            title: row.get::<_, String>(1)?,
            mtime: row.get::<_, i64>(2).unwrap_or(0) as u64,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// ---- opencode — SQLite in the user's opencode data dir ------------------
fn opencode_data_dirs() -> Vec<PathBuf> {
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

fn opencode_sessions(cwd: &str) -> Vec<AgentSession> {
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

#[cfg(test)]
mod executable_tests {
    use super::{allowed_agent_path_for_home, cached_title, codex_title, title_cache_stamp};
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn opencode_cache_executables_are_rejected_with_any_windows_suffix() {
        let home = Path::new("/home/tester");
        for name in ["opencode", "opencode.exe", "opencode.cmd"] {
            assert!(!allowed_agent_path_for_home(
                "opencode",
                &home.join(".opencode").join("bin").join(name),
                home,
            ));
        }
        assert!(allowed_agent_path_for_home(
            "opencode",
            Path::new("/usr/local/bin/opencode"),
            home,
        ));
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
            codex_title(transcript.path()).as_deref(),
            Some("Explain this codebase")
        );
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
