use std::collections::HashMap;
#[cfg(unix)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;

use crate::{
    aws::LogsTailManager,
    error::{AppError, AppResult},
    pty::PtyManager,
    rundeck::{RundeckLogsManager, RundeckWatchManager},
    state::state_load_sync,
};

/// macOS GUI apps launched from Finder/Dock inherit launchd's minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), missing `~/.local/bin`,
/// `/opt/homebrew/bin`, `~/.cargo/bin`, `~/.opencode/bin`, etc. that the
/// user actually has tools in. `make dev` works because the dev binary is
/// launched from a terminal that already has the right PATH.
///
/// Fix: at startup, exec the user's login shell with `-l -c 'printf %s
/// "$PATH"'` to extract the real PATH, then set it on our own process so
/// every `Command::new(...)` (hermes, rnd, aws, claude, …) inherits it.
/// Standard "fix-path" pattern Electron + Tauri apps have used for years.
#[cfg(unix)]
pub fn fix_path_from_login_shell() {
    let shell = configured_shell();

    let mut shell_path = String::new();
    let script =
        "printf %s '@@SIKEMUX_PATH@@'; printf %s \"$PATH\"; printf %s '@@SIKEMUX_PATH_END@@'";
    let mut command = Command::new(&shell);
    command
        .args(["-l", "-i", "-c", script])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(o) = command.output() {
        if o.status.success() {
            shell_path = parse_login_shell_path(&o.stdout).unwrap_or_default();
        }
    }

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
    let mut parts: Vec<&str> = shell_path.split(':').filter(|s| !s.is_empty()).collect();
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
    let new_path = parts.join(":");
    // SAFETY: called once at startup before any threads spawn — env::set_var
    // is unsound under multi-threaded mutation but we're single-threaded.
    unsafe { std::env::set_var("PATH", new_path) };
}

#[cfg(unix)]
fn parse_login_shell_path(stdout: &[u8]) -> Option<String> {
    const START: &[u8] = b"@@SIKEMUX_PATH@@";
    const END: &[u8] = b"@@SIKEMUX_PATH_END@@";
    let start = rfind_bytes(stdout, START)? + START.len();
    let payload = &stdout[start..];
    let end = rfind_bytes(payload, END)?;
    let path = std::str::from_utf8(&payload[..end]).ok()?.trim();
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(windows)]
pub fn fix_path_from_login_shell() {
    // Windows desktop applications inherit the user's PATH. Unlike macOS,
    // there is no login-shell environment to recover here.
}

/// Opens the payload, fencing it off from whatever an interactive rc file
/// prints on the way past — banners, `clear`, prompt-init escapes. Only bytes
/// after the last occurrence are parsed, so an rc file that echoes the command
/// line back cannot inject entries.
#[cfg(unix)]
const LOGIN_ENV_SENTINEL: &str = "@@SIKEMUX_ENV@@";

/// Closes the payload. `env -0` terminates its last record with a NUL, so
/// anything a `zshexit`/`precmd` hook prints afterwards would otherwise become
/// a trailing record — and parse as a real entry if it happened to contain an
/// `=`. Fencing both ends means only what the capture itself emitted is read.
#[cfg(unix)]
const LOGIN_ENV_SENTINEL_END: &str = "@@SIKEMUX_ENV_END@@";

/// A profile that blocks forever must never stop the app from starting. The
/// capture runs on its own thread and drains the pipe, so the child cannot
/// deadlock on a full one; past this deadline we launch with what we have.
#[cfg(unix)]
const LOGIN_ENV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Keys the PTY layer owns. Importing these would let a profile rename the
/// terminal, redirect shell integration, or hand a pane the capture shell's
/// own working directory and launchd-scoped temp dir. `PATH` is excluded
/// because `fix_path_from_login_shell` already unions it process-wide.
///
/// This list is about keys whose *values* would be wrong in a pane, not about
/// identity: `configure_pty_environment` scrubs `OPTIONAL_PTY_ENV` after the
/// fill, and that is what stops a profile forging a pane's identity.
///
/// `EDITOR`/`VISUAL` are deliberately absent — the PTY layer already installs
/// its own editor only when neither is set, so importing them keeps a user who
/// exports one in their profile from getting different behaviour depending on
/// whether the app was launched from a terminal or from the Dock.
#[cfg(unix)]
const LOGIN_ENV_SIKEMUX_OWNED: &[&str] = &[
    "COLORTERM",
    "COLUMNS",
    "LINES",
    "OLDPWD",
    "PATH",
    "PWD",
    "SHELL",
    "SHLVL",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "TMPDIR",
    "ZDOTDIR",
    "_",
];

/// What the user's shell profile exports, captured once per app run.
///
/// A macOS GUI app inherits launchd's environment, never the user's profile.
/// `fix_path_from_login_shell` recovers `PATH` from it; this recovers the
/// rest. That matters most for a PTY launched with a direct command — an agent
/// CLI running as the PTY's own process with no shell in between — because it
/// reads no profile at all. An API key or token the user keeps in `.zshrc` is
/// simply absent there, so the CLI comes up asking them to log in while the
/// very same CLI works in their own terminal.
///
/// `-i` is the load-bearing flag: zsh sources `.zshrc` only when interactive,
/// and that is where people put exports. `-l` alone reads `.zprofile` and
/// misses them, which is why the `PATH` capture above cannot be reused.
pub fn login_shell_environment() -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        #[cfg(unix)]
        {
            capture_login_shell_environment()
        }
        // Windows desktop apps already inherit the user's environment.
        #[cfg(windows)]
        {
            HashMap::new()
        }
    })
}

/// Populate the `login_shell_environment` cache from the startup thread.
///
/// The capture blocks for as long as the profile takes, up to
/// `LOGIN_ENV_TIMEOUT`. It is first *needed* inside `pty_spawn`, an async
/// command, so initialising it lazily there would park an async runtime worker
/// for that whole time and make concurrent spawns during session restore queue
/// behind the same one-time initialisation. Called from `run()` it costs
/// nothing extra: startup is already waiting on a login shell for `PATH`.
pub fn warm_login_shell_environment() {
    let _ = login_shell_environment();
}

#[cfg(unix)]
fn capture_login_shell_environment() -> HashMap<String, String> {
    let shell = configured_shell();
    // `env -0` rather than newline records: a value may contain a newline, but
    // never a NUL.
    let script =
        format!("printf %s '{LOGIN_ENV_SENTINEL}'; env -0; printf %s '{LOGIN_ENV_SENTINEL_END}'");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut command = Command::new(&shell);
        command
            .args(["-l", "-i", "-c", &script])
            // An rc file that reads stdin sees EOF instead of blocking.
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for key in crate::pty::OPTIONAL_PTY_ENV {
            command.env_remove(key);
        }
        let _ = sender.send(command.output().ok());
    });
    // On timeout the capture thread stays parked in `output()` until the shell
    // it is waiting on exits, so a profile that blocks forever leaks one thread
    // and one process for the life of the app. That is bounded — this runs
    // exactly once — and the alternative is process-group teardown for a case
    // that ends the moment the user fixes their profile.
    match receiver.recv_timeout(LOGIN_ENV_TIMEOUT) {
        Ok(Some(output)) if output.status.success() => {
            parse_login_shell_environment(&output.stdout)
        }
        _ => HashMap::new(),
    }
}

#[cfg(unix)]
fn parse_login_shell_environment(stdout: &[u8]) -> HashMap<String, String> {
    // Work on bytes, not `from_utf8_lossy`: a value that is not valid UTF-8
    // would otherwise have replacement characters substituted into it and be
    // imported in corrupted form, which is a silent way to break a token. Each
    // record is converted individually so one bad value drops itself instead of
    // the whole capture.
    let Some(start) = rfind_bytes(stdout, LOGIN_ENV_SENTINEL.as_bytes()) else {
        return HashMap::new();
    };
    let payload = &stdout[start + LOGIN_ENV_SENTINEL.len()..];
    // Without the closing fence we cannot tell the payload from whatever an
    // exit hook printed after it, so refuse rather than guess.
    let Some(end) = rfind_bytes(payload, LOGIN_ENV_SENTINEL_END.as_bytes()) else {
        return HashMap::new();
    };
    payload[..end]
        .split(|byte| *byte == 0)
        .filter_map(|record| std::str::from_utf8(record).ok())
        .filter_map(|record| record.split_once('='))
        .filter(|(key, _)| {
            !key.is_empty()
                && !key.contains(char::is_whitespace)
                && !LOGIN_ENV_SIKEMUX_OWNED.contains(key)
        })
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[cfg(unix)]
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellPlatform {
    Unix,
    Windows,
}

const CURRENT_SHELL_PLATFORM: ShellPlatform = if cfg!(windows) {
    ShellPlatform::Windows
} else {
    ShellPlatform::Unix
};

fn resolve_configured_shell(
    platform: ShellPlatform,
    sikemux_shell: Option<&str>,
    unix_shell: Option<&str>,
) -> String {
    match platform {
        ShellPlatform::Unix => unix_shell.unwrap_or("/bin/zsh"),
        ShellPlatform::Windows => sikemux_shell.unwrap_or("powershell.exe"),
    }
    .to_string()
}

/// The executable a newly spawned interactive PTY will use. Keep health
/// reporting and PTY launch on this one platform policy so the frontend never
/// configures integration for a different shell than native will execute.
pub(crate) fn configured_shell() -> String {
    let sikemux_shell = std::env::var("SIKEMUX_SHELL").ok();
    let unix_shell = std::env::var("SHELL").ok();
    resolve_configured_shell(
        CURRENT_SHELL_PLATFORM,
        sikemux_shell.as_deref(),
        unix_shell.as_deref(),
    )
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
pub fn integration_health() -> IntegrationHealth {
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
    let Some(paths) = std::env::var_os("PATH") else {
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
pub fn recent_dirs() -> Vec<String> {
    zoxide_dirs()
}

fn zoxide_dirs() -> Vec<String> {
    for bin in [
        "zoxide",
        "/opt/homebrew/bin/zoxide",
        "/usr/local/bin/zoxide",
    ] {
        if let Ok(out) = Command::new(bin).args(["query", "--list"]).output() {
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
    recent: Vec<String>,
}

#[derive(Serialize)]
pub struct RuntimeDiagnostics {
    pid: u32,
    fd_count: Option<usize>,
    fd_limit_soft: Option<u64>,
    fd_limit_hard: Option<u64>,
    ptys: usize,
    pty_subscribers: usize,
    pty_output_reads: u64,
    pty_output_broadcasts: u64,
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
    aws_log_tails: usize,
    rundeck_watchers: usize,
    rundeck_log_tails: usize,
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
    unsafe {
        let mut lim = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return (None, None);
        }
        (Some(lim.rlim_cur as u64), Some(lim.rlim_max as u64))
    }
}

#[cfg(not(unix))]
fn current_fd_limit() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[tauri::command]
pub fn runtime_diagnostics(
    ptys: tauri::State<'_, PtyManager>,
    aws_logs: tauri::State<'_, LogsTailManager>,
    rundeck_watch: tauri::State<'_, RundeckWatchManager>,
    rundeck_logs: tauri::State<'_, RundeckLogsManager>,
) -> RuntimeDiagnostics {
    let (pty_count, pty_subscribers) = ptys.counts();
    let pty_diagnostics = ptys.diagnostics();
    let (lsp_open_documents, lsp_idle_servers) = crate::lsp::document_counts();
    let (fd_limit_soft, fd_limit_hard) = current_fd_limit();
    RuntimeDiagnostics {
        pid: std::process::id(),
        fd_count: current_fd_count(),
        fd_limit_soft,
        fd_limit_hard,
        ptys: pty_count,
        pty_subscribers,
        pty_output_reads: pty_diagnostics.output_reads,
        pty_output_broadcasts: pty_diagnostics.output_broadcasts,
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
        aws_log_tails: aws_logs.count(),
        rundeck_watchers: rundeck_watch.count(),
        rundeck_log_tails: rundeck_logs.count(),
        observability: crate::observability::global_observability().snapshot(),
    }
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
pub fn battery_status() -> BatteryStatus {
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
        let out = match Command::new("pmset").args(["-g", "batt"]).output() {
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
    tauri::async_runtime::spawn_blocking(|| BootInfo {
        home: home_dir(),
        state: state_load_sync(),
        recent: zoxide_dirs(),
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

    #[cfg(unix)]
    #[test]
    fn login_shell_path_ignores_interactive_profile_output() {
        let output = b"welcome\n\x1b]7;file://host/tmp\x07@@SIKEMUX_PATH@@/Users/test/.mise/shims:/usr/bin@@SIKEMUX_PATH_END@@prompt noise";
        assert_eq!(
            parse_login_shell_path(output).as_deref(),
            Some("/Users/test/.mise/shims:/usr/bin")
        );
    }

    #[test]
    fn configured_shell_resolver_preserves_unix_shell_and_default() {
        assert_eq!(
            resolve_configured_shell(
                ShellPlatform::Unix,
                Some("ignored-windows-override"),
                Some("/opt/homebrew/bin/fish"),
            ),
            "/opt/homebrew/bin/fish"
        );
        assert_eq!(
            resolve_configured_shell(ShellPlatform::Unix, None, None),
            "/bin/zsh"
        );
    }

    #[test]
    fn configured_shell_resolver_gives_windows_override_precedence() {
        assert_eq!(
            resolve_configured_shell(
                ShellPlatform::Windows,
                Some(r"C:\Program Files\PowerShell\7\pwsh.exe"),
                Some("ignored-unix-shell"),
            ),
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        );
        assert_eq!(
            resolve_configured_shell(ShellPlatform::Windows, None, Some("ignored-unix-shell")),
            "powershell.exe"
        );
    }

    /// Interactive rc files print banners, run `clear`, and emit prompt-init
    /// escapes before the payload ever appears. Only what follows the sentinel
    /// is environment.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_discards_interactive_rc_chatter_before_the_sentinel() {
        let stdout = format!(
            "\u{1b}[2J\u{1b}[Hbanner line\nANTHROPIC_API_KEY=decoy\n{}HOME=/Users/x\0ANTHROPIC_API_KEY=sk-real\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-real")
        );
        assert_eq!(parsed.get("HOME").map(String::as_str), Some("/Users/x"));
        assert_eq!(parsed.len(), 2);
    }

    /// An rc file that echoes the capture command back would otherwise let a
    /// second sentinel smuggle entries in; the last one always wins.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_honours_only_the_final_sentinel() {
        let sentinel = super::LOGIN_ENV_SENTINEL;
        let end = super::LOGIN_ENV_SENTINEL_END;
        let stdout = format!("{sentinel}INJECTED=yes\0{sentinel}REAL=yes\0{end}");

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("REAL").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("INJECTED"));
    }

    /// `env -0` ends its last record with a NUL, so anything a `zshexit` or
    /// `precmd` hook prints after the payload arrives as a trailing record —
    /// and parses as an entry if it contains an `=`. The closing fence is what
    /// keeps a hook from injecting one.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_ignores_anything_printed_after_the_payload() {
        let stdout = format!(
            "{}REAL=yes\0{}\nrestored session: TRAILING=injected\n",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("REAL").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("TRAILING"));
        assert_eq!(parsed.len(), 1);
    }

    /// A value that is not valid UTF-8 must drop itself rather than be imported
    /// with replacement characters substituted in — a corrupted token is worse
    /// than an absent one. Its neighbours still arrive intact.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_drops_non_utf8_records_without_corrupting_them() {
        let mut stdout = Vec::new();
        stdout.extend_from_slice(super::LOGIN_ENV_SENTINEL.as_bytes());
        stdout.extend_from_slice(b"GOOD=yes\0BAD=");
        stdout.extend_from_slice(&[0xff, 0xfe]);
        stdout.extend_from_slice(b"\0ALSO_GOOD=yes\0");
        stdout.extend_from_slice(super::LOGIN_ENV_SENTINEL_END.as_bytes());

        let parsed = super::parse_login_shell_environment(&stdout);

        assert_eq!(parsed.get("GOOD").map(String::as_str), Some("yes"));
        assert_eq!(parsed.get("ALSO_GOOD").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("BAD"));
    }

    /// The user's own editor choice reaches a pane the same way whether the app
    /// was launched from a terminal or from the Dock. The PTY layer still
    /// installs its own editor when the profile sets neither.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_imports_the_editor_the_profile_chose() {
        let stdout = format!(
            "{}EDITOR=hx\0VISUAL=hx\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("EDITOR").map(String::as_str), Some("hx"));
        assert_eq!(parsed.get("VISUAL").map(String::as_str), Some("hx"));
    }

    /// NUL delimiting is what makes a multi-line export survive the round trip.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_keeps_values_containing_newlines_and_equals_signs() {
        let stdout = format!(
            "{}NODE_EXTRA_CA_CERTS=-----BEGIN-----\nline2\n-----END-----\0CONN=a=b=c\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(
            parsed.get("NODE_EXTRA_CA_CERTS").map(String::as_str),
            Some("-----BEGIN-----\nline2\n-----END-----")
        );
        assert_eq!(parsed.get("CONN").map(String::as_str), Some("a=b=c"));
    }

    /// Keys the PTY layer owns are dropped at the source, so a profile can
    /// never rename the terminal or hand a pane the capture shell's own working
    /// directory. `PATH` is excluded because it is already unioned separately.
    /// Pane identity is not defended here — `configure_pty_environment` scrubs
    /// `OPTIONAL_PTY_ENV` after the fill and owns that invariant.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_drops_keys_the_pty_layer_owns() {
        let stdout = format!(
            "{}PWD=/tmp/capture\0ZDOTDIR=/tmp/z\0TERM=xterm-kitty\0PATH=/only/profile\0KEEP=yes\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        for owned in ["PWD", "ZDOTDIR", "TERM", "PATH"] {
            assert!(
                !parsed.contains_key(owned),
                "{owned} should not be imported"
            );
        }
        assert_eq!(parsed.get("KEEP").map(String::as_str), Some("yes"));
    }

    /// A shell that fails, or output with no sentinel at all, must degrade to
    /// "no profile environment" rather than to garbage entries.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_yields_nothing_without_a_sentinel() {
        assert!(super::parse_login_shell_environment(b"HOME=/Users/x\0").is_empty());
        assert!(super::parse_login_shell_environment(b"").is_empty());
        // Opening fence but no closing one: the shell died mid-capture, so the
        // payload is unterminated and cannot be told apart from later output.
        let truncated = format!("{}HOME=/Users/x\0", super::LOGIN_ENV_SENTINEL);
        assert!(super::parse_login_shell_environment(truncated.as_bytes()).is_empty());
    }
}
