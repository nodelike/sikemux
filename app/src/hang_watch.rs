//! Notices when the window stops answering and writes down what the app's threads were doing,
//! where `sikemux doctor` reads it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::{App, Window};
use serde_json::json;
use sikemux_host::hang_reports::{autopsy_dir, is_bundle, DIRECTORY_PREFIX, REPORT_FILE};

const BEAT_EVERY: Duration = Duration::from_millis(500);
/// A window that has not answered for this long is stuck, not busy.
const STUCK_AFTER: Duration = Duration::from_secs(2);
const SAMPLE_SECONDS: u32 = 2;
const BETWEEN_REPORTS: Duration = Duration::from_secs(120);
const REPORTS_KEPT: usize = 5;

static STARTED: OnceLock<Instant> = OnceLock::new();
/// When the window last answered, in milliseconds since [`STARTED`].
static BEAT: AtomicU64 = AtomicU64::new(0);
/// Whether the window is in front. One behind other apps can be put to sleep by the system,
/// which is not it being stuck, so it goes unwatched and nothing wakes it.
static WATCHED: AtomicBool = AtomicBool::new(false);
static WATCHER: OnceLock<std::thread::Thread> = OnceLock::new();

fn now() -> u64 {
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Watches the window while it is in front: a thread of its own asks it to answer every
/// [`BEAT_EVERY`], and the answer only notes the time, so nothing redraws.
pub fn start(window: &Window, cx: &mut App) {
    if !cfg!(target_os = "macos") || STARTED.get().is_some() {
        return;
    }
    let Some(directory) = autopsy_dir() else {
        return;
    };
    let (ask, mut asked) = tokio::sync::mpsc::unbounded_channel::<()>();
    cx.spawn(async move |_| {
        while asked.recv().await.is_some() {
            BEAT.store(now(), Ordering::Relaxed);
        }
    })
    .detach();
    let watcher = std::thread::Builder::new()
        .name("hang-watch".into())
        .spawn(move || {
            let mut stuck = false;
            let mut last_report: Option<Instant> = None;
            loop {
                if !WATCHED.load(Ordering::Relaxed) {
                    stuck = false;
                    std::thread::park();
                    continue;
                }
                if ask.send(()).is_err() {
                    return;
                }
                std::thread::sleep(BEAT_EVERY);
                let silent =
                    Duration::from_millis(now().saturating_sub(BEAT.load(Ordering::Relaxed)));
                let was_stuck = std::mem::replace(&mut stuck, silent >= STUCK_AFTER);
                let due = last_report.is_none_or(|last| last.elapsed() >= BETWEEN_REPORTS);
                if stuck && !was_stuck && WATCHED.load(Ordering::Relaxed) && due {
                    last_report = Some(Instant::now());
                    if let Err(error) = report(&directory, silent) {
                        eprintln!(
                            "sikemux: the window was stuck, but no report was written: {error}"
                        );
                    }
                }
            }
        });
    if let Ok(watcher) = watcher {
        let _ = WATCHER.set(watcher.thread().clone());
    }
    in_front(window.is_window_active());
}

/// Called as the window comes to the front and leaves it.
pub fn in_front(active: bool) {
    BEAT.store(now(), Ordering::Relaxed);
    WATCHED.store(active, Ordering::Relaxed);
    if let Some(watcher) = WATCHER.get().filter(|_| active) {
        watcher.unpark();
    }
}

/// Samples this process's threads into a new report folder, and keeps only the newest few.
fn report(directory: &Path, silent: Duration) -> std::io::Result<()> {
    let captured = chrono::Utc::now();
    let folder = directory.join(format!(
        "{DIRECTORY_PREFIX}{}",
        captured.format("%Y%m%dT%H%M%S%.3fZ")
    ));
    std::fs::create_dir_all(&folder)?;
    let pid = std::process::id();
    let file = "app.sample.txt";
    let sampled = sikemux_process::user_environment::command("/usr/bin/sample")
        .arg(pid.to_string())
        .arg(SAMPLE_SECONDS.to_string())
        .arg("-file")
        .arg(folder.join(file))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let (file, error) = match sampled {
        Ok(status) if status.success() => (Some(file), None),
        Ok(status) => (None, Some(format!("sample exited with {status}"))),
        Err(error) => (None, Some(error.to_string())),
    };
    let report = json!({
        "version": 1,
        "capturedAt": captured.to_rfc3339(),
        "capturedAtMs": captured.timestamp_millis(),
        "appVersion": env!("CARGO_PKG_VERSION"),
        "pid": pid,
        "watchdog": "ui",
        "delayMs": silent.as_millis() as u64,
        "thresholdMs": STUCK_AFTER.as_millis() as u64,
        "samples": [{ "target": "app", "pid": pid, "file": file, "error": error }],
    });
    std::fs::write(
        folder.join(REPORT_FILE),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut reports: Vec<_> = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_bundle(path))
        .collect();
    reports.sort();
    let excess = reports.len().saturating_sub(REPORTS_KEPT);
    for old in reports.into_iter().take(excess) {
        std::fs::remove_dir_all(old)?;
    }
    Ok(())
}
