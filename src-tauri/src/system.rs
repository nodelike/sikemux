#[cfg(unix)]
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use tauri::async_runtime::spawn_blocking;

use crate::{
    error::{AppError, AppResult},
    pty::PtyManager,
    state::state_load_sync,
};

pub use sikemux_host::environment::{child_path, user_environment};

pub fn user_home() -> PathBuf {
    sikemux_process::paths::home().unwrap_or_default()
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

/// WebKitGTK only draws on the GPU through its DMA-BUF renderer; without it every
/// frame is painted on the CPU. With NVIDIA's driver that renderer leaves the
/// window blank on X11, so it is turned off there. On Wayland it closes the window
/// a second after it opens, because NVIDIA asks the compositor for explicit sync
/// on a window GTK also draws plain buffers into; turning that sync off is enough.
#[cfg(target_os = "linux")]
pub fn keep_webkit_rendering_on_nvidia() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
        || !std::path::Path::new("/proc/driver/nvidia/version").exists()
    {
        return;
    }
    let on_x11 = std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var("GDK_BACKEND").is_ok_and(|backend| backend.starts_with("x11"));
    // SAFETY: run() calls this before the Tauri runtime starts threads.
    unsafe {
        if on_x11 {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        } else if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none() {
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
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
        let out = match sikemux_process::user_environment::command("/usr/bin/pmset")
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

/// Home dir and persisted state in one round-trip on boot. SQLite's busy wait
/// and recovery I/O run on the blocking pool so a locked database cannot stall
/// unrelated Tauri commands.
#[tauri::command]
pub async fn boot_init() -> AppResult<BootInfo> {
    spawn_blocking(|| BootInfo {
        home: user_home().to_string_lossy().into_owned(),
        state: state_load_sync(),
    })
    .await
    .map_err(|error| AppError::Other(format!("boot_init join: {error}")))
}
