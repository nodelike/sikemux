use std::collections::HashMap;
#[cfg(unix)]
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use sikemux_process::user_environment::UserEnvironment;
#[cfg(unix)]
use sikemux_pty::user_shell::login_shell_path;
use sikemux_pty::user_shell::{configured_shell, login_shell_environment};
use tauri::async_runtime::spawn_blocking;

use crate::{
    error::{AppError, AppResult},
    plugins,
    pty::PtyManager,
    state::state_load_sync,
};

/// macOS GUI apps launched from Finder/Dock inherit launchd's minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), missing `~/.local/bin`,
/// `/opt/homebrew/bin`, `~/.cargo/bin`, `~/.opencode/bin`, etc. that the
/// user actually has tools in. `make dev` works because the dev binary is
/// launched from a terminal that already has the right PATH.
///
/// So every child gets the `PATH` the user's login shell resolves, plus any
/// variable a plugin reads that the app was not already started with. Reading
/// the shell takes as long as the user's rc files do, so it runs beside window
/// creation rather than ahead of it.
pub fn user_environment() -> UserEnvironment {
    let shell = login_shell_environment();
    let mut variables: HashMap<String, String> = plugins::shell_variables()
        .into_iter()
        .filter(|name| std::env::var_os(name).is_none())
        .filter_map(|name| Some((name.clone(), shell.get(&name)?.clone())))
        .collect();
    if let Some(path) = user_path() {
        variables.insert("PATH".to_string(), path);
    }
    UserEnvironment { variables }
}

#[cfg(unix)]
fn user_path() -> Option<String> {
    let shell_path = login_shell_path().unwrap_or_default();

    // Always-union: even if the shell extraction succeeded, append the
    // common user-local bin dirs in case they live in ~/.zshrc (which
    // login shells don't source) or in non-zsh setups. Idempotent —
    // duplicates are harmless to PATH lookup.
    let home = std::env::var("HOME").unwrap_or_default();
    let extra = [
        format!("{home}/.local/bin"),
        format!("{home}/.cargo/bin"),
        format!("{home}/.opencode/bin"),
        format!("{home}/.config/shell/bin"),
        format!("{home}/go/bin"),
        // Node tooling: typescript-language-server, pyright, vue-lsp, etc.
        // installed via `pnpm add -g` land here on macOS.
        format!("{home}/Library/pnpm"),
        format!("{home}/.npm/bin"),
        format!("{home}/.bun/bin"),
        // Python virtualenv tooling (pipx, pyenv shims).
        format!("{home}/.pyenv/shims"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    let cli = crate::cli_paths::cli_link_directory().and_then(Path::to_str);
    let mut parts: Vec<&str> = cli.into_iter().collect();
    for part in shell_path.split(':').filter(|s| !s.is_empty()) {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    for d in &extra {
        if !parts.contains(&d.as_str()) {
            parts.push(d.as_str());
        }
    }
    // Fall back to the launchd minimal set if shell extraction failed AND
    // none of the extras hit — guarantees `/usr/bin` etc. stay reachable.
    if parts.is_empty() {
        parts = vec!["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"];
    }
    Some(parts.join(":"))
}

/// Windows desktop applications inherit the user's PATH, so only the CLI link
/// needs adding.
#[cfg(windows)]
fn user_path() -> Option<String> {
    let directory = crate::cli_paths::cli_link_directory()?;
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(directory.to_path_buf())
        .chain(std::env::split_paths(&existing).filter(|path| path != directory));
    std::env::join_paths(paths).ok()?.into_string().ok()
}

pub fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    find_executable_matching(name, |_| true)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationHealth {
    shell: String,
    git: bool,
    aws: bool,
    rnd: bool,
}

#[tauri::command]
pub async fn integration_health() -> IntegrationHealth {
    spawn_blocking(read_integration_health)
        .await
        .unwrap_or_else(|_| IntegrationHealth {
            shell: configured_shell(),
            git: false,
            aws: false,
            rnd: false,
        })
}

fn read_integration_health() -> IntegrationHealth {
    IntegrationHealth {
        shell: configured_shell(),
        git: find_executable("git").is_some(),
        aws: find_executable("aws").is_some(),
        rnd: find_executable("rnd").is_some(),
    }
}

pub fn find_executable_matching(name: &str, predicate: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    find_executables_matching(name, predicate)
        .into_iter()
        .next()
}

pub fn find_executables_matching(name: &str, predicate: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Some(paths) = sikemux_process::user_environment::var_os("PATH") else {
        return Vec::new();
    };
    #[cfg(windows)]
    let names: Vec<String> = if PathBuf::from(name).extension().is_some() {
        vec![name.to_string()]
    } else {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{name}{}", extension.to_ascii_lowercase()))
            .chain(std::iter::once(name.to_string()))
            .collect()
    };
    #[cfg(not(windows))]
    let names = vec![name.to_string()];

    find_executables_matching_in(std::env::split_paths(&paths), &names, &predicate)
}

#[cfg(test)]
fn find_executable_matching_in(
    paths: impl IntoIterator<Item = PathBuf>,
    names: &[String],
    predicate: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    find_executables_matching_in(paths, names, predicate)
        .into_iter()
        .next()
}

fn find_executables_matching_in(
    paths: impl IntoIterator<Item = PathBuf>,
    names: &[String],
    predicate: &impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for directory in paths {
        for candidate_name in names {
            let candidate = directory.join(candidate_name);
            if !candidate.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let Ok(metadata) = candidate.metadata() else {
                    continue;
                };
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            if predicate(&candidate) && !found.contains(&candidate) {
                found.push(candidate);
            }
        }
    }
    found
}

/// Existing modules use HOME for established ~/.config, ~/.ssh and ~/.aws
/// locations. Windows normally exposes USERPROFILE instead, so normalize it
/// once before Tauri creates worker threads rather than branching every
/// consumer independently.
pub fn normalize_user_environment() {
    if std::env::var_os("HOME").is_some() {
        return;
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        // SAFETY: run() calls this before the Tauri runtime starts threads.
        unsafe { std::env::set_var("HOME", home) };
    }
}

/// With NVIDIA's driver, WebKitGTK's DMA-BUF renderer closes the window about a
/// second after it opens on Wayland, and leaves it blank on X11.
#[cfg(target_os = "linux")]
pub fn avoid_webkit_dmabuf_renderer_on_nvidia() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
        || !Path::new("/proc/driver/nvidia/version").exists()
    {
        return;
    }
    // SAFETY: run() calls this before the Tauri runtime starts threads.
    unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
}

/// Raise this process's open-file-descriptor soft limit toward its hard
/// limit. See the call site in `lib.rs` for the why: macOS `launchd` hands
/// GUI-launched apps a soft `RLIMIT_NOFILE` of 256, but sikemux holds one
/// fd per live PTY plus the webview, language servers, fs watchers, and
/// sockets — a heavy multi-terminal/agent/project session blows past 256
/// and every fd-hungry op (git, spawning a process, opening a file) then
/// fails with EMFILE ("Too many open files"). A higher limit costs no
/// memory — it's a ceiling, not an allocation — so this is safe on low-end
/// devices too.
#[cfg(unix)]
pub fn raise_fd_limit() {
    // SAFETY: `rlimit` is a plain C struct for which all zeroes is valid, and both calls
    // only touch the struct we pass, during the call.
    unsafe {
        let mut lim = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        // Dev builds inherit the launching terminal's already-high ulimit;
        // nothing to do there.
        if lim.rlim_cur >= 65_536 {
            return;
        }
        // Try progressively smaller soft limits until one sticks. macOS
        // rejects a soft limit above `kern.maxfilesperproc` with EINVAL, so
        // we descend; even the smallest rung (10_240) dwarfs launchd's 256
        // and covers thousands of PTYs/sockets.
        for &cand in &[131_072u64, 65_536, 16_384, 10_240] {
            let cand = cand as libc::rlim_t;
            let hard = lim.rlim_max;
            let target = if hard == libc::RLIM_INFINITY || cand <= hard {
                cand
            } else {
                hard
            };
            if target <= lim.rlim_cur {
                continue;
            }
            let next = libc::rlimit {
                rlim_cur: target,
                rlim_max: lim.rlim_max,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &next) == 0 {
                return;
            }
        }
    }
}

#[cfg(not(unix))]
pub fn raise_fd_limit() {}

#[tauri::command]
pub fn home_dir() -> String {
    user_home().to_string_lossy().into_owned()
}

/// Frecency-ranked directories from zoxide, for the sesh picker.
#[tauri::command]
pub async fn recent_dirs() -> Vec<String> {
    spawn_blocking(zoxide_dirs).await.unwrap_or_default()
}

fn zoxide_dirs() -> Vec<String> {
    for bin in [
        "zoxide",
        "/opt/homebrew/bin/zoxide",
        "/usr/local/bin/zoxide",
    ] {
        if let Ok(out) = sikemux_process::user_environment::command(bin)
            .args(["query", "--list"])
            .output()
        {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

#[derive(Serialize)]
pub struct BootInfo {
    home: String,
    state: String,
}

#[derive(Serialize)]
pub struct RuntimeDiagnostics {
    pid: u32,
    fd_count: Option<usize>,
    fd_limit_soft: Option<u64>,
    fd_limit_hard: Option<u64>,
    pty_core_pid: Option<u32>,
    ptys: usize,
    pty_subscribers: usize,
    pty_output_frames: u64,
    pty_output_bytes: u64,
    agent_ptys_working: usize,
    agent_ptys_blocked: usize,
    agent_ptys_idle: usize,
    agent_ptys_unknown: usize,
    lsp_servers: usize,
    lsp_open_documents: usize,
    lsp_idle_servers: usize,
    repo_watchers: usize,
    agent_session_watchers: usize,
    plugin_streams: usize,
    observability: crate::observability::ObservabilitySnapshot,
}

#[cfg(unix)]
fn current_fd_count() -> Option<usize> {
    fs::read_dir("/dev/fd").ok().map(|rd| rd.count())
}

#[cfg(not(unix))]
fn current_fd_count() -> Option<usize> {
    None
}

#[cfg(unix)]
fn current_fd_limit() -> (Option<u64>, Option<u64>) {
    // SAFETY: `rlimit` is a plain C struct for which all zeroes is valid, and
    // `getrlimit` only writes into the struct we pass.
    unsafe {
        let mut lim = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return (None, None);
        }
        (Some(lim.rlim_cur), Some(lim.rlim_max))
    }
}

#[cfg(not(unix))]
fn current_fd_limit() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[tauri::command]
pub async fn runtime_diagnostics(
    ptys: tauri::State<'_, PtyManager>,
    plugins: tauri::State<'_, crate::plugins::PluginHost>,
) -> AppResult<RuntimeDiagnostics> {
    // Counting /dev/fd is a directory read, so it goes to the blocking pool
    // even though everything else here is in-memory bookkeeping.
    let (fd_count, fd_limit_soft, fd_limit_hard) = spawn_blocking(|| {
        let (soft, hard) = current_fd_limit();
        (current_fd_count(), soft, hard)
    })
    .await
    .unwrap_or((None, None, None));
    let pty_diagnostics = ptys.diagnostics().await;
    let (lsp_open_documents, lsp_idle_servers) = crate::lsp::document_counts();
    Ok(RuntimeDiagnostics {
        pid: std::process::id(),
        fd_count,
        fd_limit_soft,
        fd_limit_hard,
        pty_core_pid: pty_diagnostics.core_pid,
        ptys: pty_diagnostics.ptys,
        pty_subscribers: pty_diagnostics.subscribers,
        pty_output_frames: pty_diagnostics.output_frames,
        pty_output_bytes: pty_diagnostics.output_bytes,
        agent_ptys_working: pty_diagnostics.working_agents,
        agent_ptys_blocked: pty_diagnostics.blocked_agents,
        agent_ptys_idle: pty_diagnostics.idle_agents,
        agent_ptys_unknown: pty_diagnostics.unknown_agents,
        lsp_servers: crate::lsp::server_count(),
        lsp_open_documents,
        lsp_idle_servers,
        repo_watchers: crate::fs_watch::watch_count(),
        agent_session_watchers: crate::agents::watch_count(),
        plugin_streams: plugins.stream_count(),
        observability: crate::observability::global_observability().snapshot(),
    })
}

// ---- Battery -------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct BatteryStatus {
    /// 0..100, or None when there's no battery (desktop, external display).
    pub percent: Option<u8>,
    pub charging: bool,
    /// Free-form remaining time string from `pmset` (e.g. "4:23"), if any.
    pub time_remaining: Option<String>,
}

/// macOS battery via `pmset -g batt`. Cheap (sub-ms), polled by the TopBar.
/// Returns percent=None on machines without a battery so the chip hides
/// cleanly. Same approach as the user's existing `tmux-battery` script.
#[tauri::command]
pub async fn battery_status() -> BatteryStatus {
    spawn_blocking(read_battery_status)
        .await
        .unwrap_or(BatteryStatus {
            percent: None,
            charging: false,
            time_remaining: None,
        })
}

fn read_battery_status() -> BatteryStatus {
    #[cfg(not(target_os = "macos"))]
    {
        BatteryStatus {
            percent: None,
            charging: false,
            time_remaining: None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = match sikemux_process::user_environment::command("pmset")
            .args(["-g", "batt"])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => {
                return BatteryStatus {
                    percent: None,
                    charging: false,
                    time_remaining: None,
                }
            }
        };
        parse_pmset(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(target_os = "macos")]
fn parse_pmset(text: &str) -> BatteryStatus {
    // Format:
    //   Now drawing from 'AC Power' | 'Battery Power'
    //    -InternalBattery-0 (id=...)  87%; <state>; <time> remaining present: true
    // <state> ∈ {charging, discharging, charged, finishing charge, AC attached, ...}
    let mut percent: Option<u8> = None;
    let mut charging = false;
    let mut time_remaining: Option<String> = None;
    let drawing_from_ac = text.contains("'AC Power'");
    for line in text.lines() {
        let line = line.trim();
        if !line.contains("InternalBattery") {
            continue;
        }
        // Percent: first "<n>%" token.
        if let Some(pct_end) = line.find('%') {
            let start = line[..pct_end]
                .rfind(|c: char| !c.is_ascii_digit())
                .map(|i| i + 1)
                .unwrap_or(0);
            if let Ok(n) = line[start..pct_end].parse::<u8>() {
                percent = Some(n);
            }
        }
        let lower = line.to_lowercase();
        if lower.contains("charging") && !lower.contains("discharging") {
            charging = true;
        } else if lower.contains("charged") || lower.contains("finishing charge") {
            charging = drawing_from_ac;
        } else if drawing_from_ac {
            charging = true;
        }
        // Time remaining: "H:MM remaining"
        if let Some(idx) = lower.find(" remaining") {
            let head = &line[..idx];
            if let Some(last_space) = head.rfind(char::is_whitespace) {
                let candidate = head[last_space + 1..].trim();
                if candidate.contains(':') && !candidate.contains("0:00") {
                    time_remaining = Some(candidate.to_string());
                }
            }
        }
        break;
    }
    BatteryStatus {
        percent,
        charging,
        time_remaining,
    }
}

/// Single round-trip the renderer uses on boot — home dir + persisted state
/// + zoxide recents in one IPC instead of three. State validation, SQLite's
/// bounded busy wait, recovery I/O, and the zoxide subprocess all run on the
/// blocking pool so a locked database cannot stall unrelated Tauri commands.
#[tauri::command]
pub async fn boot_init() -> AppResult<BootInfo> {
    spawn_blocking(|| BootInfo {
        home: home_dir(),
        state: state_load_sync(),
    })
    .await
    .map_err(|error| AppError::Other(format!("boot_init join: {error}")))
}

#[cfg(test)]
mod executable_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn executable_lookup_continues_after_a_rejected_candidate() {
        let first = tempdir().expect("first path");
        let second = tempdir().expect("second path");
        let first_candidate = first.path().join("tool");
        let second_candidate = second.path().join("tool");
        std::fs::write(&first_candidate, b"first").expect("first executable");
        std::fs::write(&second_candidate, b"second").expect("second executable");

        #[cfg(unix)]
        for candidate in [&first_candidate, &second_candidate] {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(candidate)
                .expect("candidate metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(candidate, permissions).expect("mark executable");
        }

        let names = vec!["tool".to_string()];
        let result = find_executable_matching_in(
            [first.path().to_path_buf(), second.path().to_path_buf()],
            &names,
            &|candidate| candidate != first_candidate,
        );
        assert_eq!(result, Some(second_candidate));
    }

    #[test]
    fn executable_lookup_returns_every_healthy_candidate_in_path_order() {
        let first = tempdir().expect("first path");
        let second = tempdir().expect("second path");
        let first_candidate = first.path().join("tool");
        let second_candidate = second.path().join("tool");
        std::fs::write(&first_candidate, b"first").expect("first executable");
        std::fs::write(&second_candidate, b"second").expect("second executable");

        #[cfg(unix)]
        for candidate in [&first_candidate, &second_candidate] {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(candidate).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(candidate, permissions).unwrap();
        }

        let names = vec!["tool".to_string()];
        assert_eq!(
            find_executables_matching_in(
                [first.path().to_path_buf(), second.path().to_path_buf()],
                &names,
                &|_| true,
            ),
            vec![first_candidate, second_candidate]
        );
    }
}
