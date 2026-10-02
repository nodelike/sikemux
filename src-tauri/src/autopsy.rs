//! Evidence capture for a detected UI hang.
//!
//! The watchdog in [`crate::observability`] already notices that the UI stopped
//! making progress. This module answers the next question — why — by sampling
//! native stacks of the WebKit renderer and of our own process, pairing them
//! with the last thing the UI reported doing, and writing the pair to disk as a
//! self-contained bundle that can be read without the app running.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::bounded_process;
use crate::error::{AppError, AppResult};
use crate::observability::{
    global_observability, HangListener, HangSignal, Metadata, ObservabilitySnapshot, ScalarValue,
    UiActivityLog, UiActivitySnapshot,
};

const REPORT_VERSION: u32 = 1;
const REPORT_FILE: &str = "report.json";
const DIRECTORY_PREFIX: &str = "hang-";
const RETAINED_AUTOPSIES: usize = 5;
const MIN_AUTOPSY_INTERVAL: Duration = Duration::from_secs(120);
const SAMPLE_BINARY: &str = "/usr/bin/sample";
const SAMPLE_SECONDS: u32 = 2;
const SAMPLE_GRACE: Duration = Duration::from_secs(20);
const MAX_SAMPLE_STDOUT_BYTES: usize = 1024 * 1024;

/// The renderer pid, resolved from the main thread and reused by every later
/// capture. Zero means it has not been resolved yet.
static WEB_CONTENT_PID: AtomicI32 = AtomicI32::new(0);
static PENDING_PID_REQUEST: AtomicBool = AtomicBool::new(false);

/// One `sample` run against one process.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackSample {
    pub target: String,
    pub pid: i32,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// The JSON half of an autopsy bundle.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HangReport {
    pub version: u32,
    pub captured_at: String,
    pub captured_at_ms: u64,
    pub app_version: String,
    pub build: String,
    pub pid: u32,
    pub web_content_pid: Option<i32>,
    pub watchdog: String,
    pub delay_ms: u64,
    pub threshold_ms: u64,
    pub heartbeat_sequence: u64,
    pub visible: bool,
    pub activity: Option<UiActivitySnapshot>,
    pub activity_history: Vec<UiActivitySnapshot>,
    pub observability: ObservabilitySnapshot,
    pub samples: Vec<StackSample>,
}

/// One autopsy as listed by the `hang_reports` command and by `sikemux doctor`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HangReportSummary {
    pub directory: String,
    pub report: String,
    pub captured_at: String,
    pub captured_at_ms: u64,
    pub delay_ms: u64,
    pub summary: String,
    pub stacks: Vec<String>,
}

/// The fields of a written report that listing needs. Everything else in the
/// bundle is for a human reading the file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportHeader {
    captured_at: String,
    captured_at_ms: u64,
    watchdog: String,
    delay_ms: u64,
    #[serde(default)]
    activity: Option<UiActivitySnapshot>,
    #[serde(default)]
    samples: Vec<StackSample>,
}

/// Captures native stacks of a process into a file.
pub trait StackCapturer: Send + Sync + 'static {
    fn capture(&self, pid: i32, seconds: u32, destination: &Path) -> Result<(), String>;
}

/// The real capturer: macOS ships `/usr/bin/sample`, which walks another
/// process's threads and symbolicates them.
pub struct SampleCapturer;

impl StackCapturer for SampleCapturer {
    fn capture(&self, pid: i32, seconds: u32, destination: &Path) -> Result<(), String> {
        let mut command = sikemux_process::user_environment::command(SAMPLE_BINARY);
        command
            .arg(pid.to_string())
            .arg(seconds.to_string())
            .arg("-file")
            .arg(destination);
        let timeout = Duration::from_secs(u64::from(seconds)) + SAMPLE_GRACE;
        match bounded_process::run(&mut command, None, timeout, MAX_SAMPLE_STDOUT_BYTES, None) {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => Err(sample_failure(&output)),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn sample_failure(output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no output");
    format!("sample exited with {}: {}", output.status, detail.trim())
}

/// Allows one action per interval. Time is passed in so the policy is testable
/// without waiting two minutes.
#[derive(Debug)]
struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: Mutex::new(None),
        }
    }

    fn allow(&self, now: Instant) -> bool {
        let mut last = match self.last.lock() {
            Ok(last) => last,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(previous) = *last {
            if now.duration_since(previous) < self.min_interval {
                return false;
            }
        }
        *last = Some(now);
        true
    }
}

/// Turns a hang signal into an autopsy bundle on a thread of its own.
pub struct AutopsyWriter {
    directory: PathBuf,
    activity: Arc<UiActivityLog>,
    capturer: Arc<dyn StackCapturer>,
    limiter: RateLimiter,
}

impl AutopsyWriter {
    pub fn new(
        directory: PathBuf,
        activity: Arc<UiActivityLog>,
        capturer: Arc<dyn StackCapturer>,
    ) -> Self {
        Self {
            directory,
            activity,
            capturer,
            limiter: RateLimiter::new(MIN_AUTOPSY_INTERVAL),
        }
    }
}

impl HangListener for AutopsyWriter {
    fn on_hang(&self, signal: HangSignal) {
        if !self.limiter.allow(Instant::now()) {
            return;
        }

        let (activity, activity_history) = self.activity.snapshot();
        let request = AutopsyRequest {
            directory: self.directory.clone(),
            signal,
            activity,
            activity_history,
            observability: global_observability().snapshot(),
            web_content_pid: web_content_pid(),
            captured_at_ms: now_ms(),
        };
        let capturer = Arc::clone(&self.capturer);
        // Sampling runs a child process for several seconds. It must never
        // happen on the watchdog thread, which has to keep watching.
        let spawned = std::thread::Builder::new()
            .name("sikemux-hang-autopsy".to_owned())
            .spawn(move || announce(write_autopsy(request, capturer.as_ref())));
        if spawned.is_err() {
            let _ = global_observability().increment_counter("autopsy.failed", 1);
        }
    }
}

fn announce(result: io::Result<PathBuf>) {
    let observer = global_observability();
    let mut metadata = Metadata::new();
    match result {
        Ok(path) => {
            metadata.insert(
                "path".to_owned(),
                ScalarValue::String(path.to_string_lossy().into_owned()),
            );
            let _ = observer.increment_counter("autopsy.written", 1);
            observer.record_event("autopsy.written", None, metadata);
        }
        Err(error) => {
            metadata.insert("error".to_owned(), ScalarValue::String(error.to_string()));
            let _ = observer.increment_counter("autopsy.failed", 1);
            observer.record_event("autopsy.failed", None, metadata);
        }
    }
}

/// Everything the capture thread needs, collected while the hang is fresh.
pub struct AutopsyRequest {
    pub directory: PathBuf,
    pub signal: HangSignal,
    pub activity: Option<UiActivitySnapshot>,
    pub activity_history: Vec<UiActivitySnapshot>,
    pub observability: ObservabilitySnapshot,
    pub web_content_pid: Option<i32>,
    pub captured_at_ms: u64,
}

/// Samples the interesting processes and writes the bundle, returning its
/// directory. A failed sample is recorded in the report instead of aborting it.
pub fn write_autopsy(request: AutopsyRequest, capturer: &dyn StackCapturer) -> io::Result<PathBuf> {
    let directory = request.directory.join(format!(
        "{DIRECTORY_PREFIX}{}",
        stamp(request.captured_at_ms)
    ));
    fs::create_dir_all(&directory)?;

    let own_pid = i32::try_from(std::process::id()).unwrap_or(-1);
    let targets = [
        ("webcontent", request.web_content_pid),
        ("app", Some(own_pid)),
    ];
    let mut samples = Vec::with_capacity(targets.len());
    for (target, pid) in targets {
        samples.push(sample_target(target, pid, &directory, capturer));
    }

    let report = HangReport {
        version: REPORT_VERSION,
        captured_at: iso8601(request.captured_at_ms),
        captured_at_ms: request.captured_at_ms,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        build: if cfg!(debug_assertions) {
            "debug".to_owned()
        } else {
            "release".to_owned()
        },
        pid: std::process::id(),
        web_content_pid: request.web_content_pid,
        watchdog: request.signal.watchdog,
        delay_ms: request.signal.delay_us / 1_000,
        threshold_ms: request.signal.threshold_us / 1_000,
        heartbeat_sequence: request.signal.heartbeat_sequence,
        visible: request.signal.visible,
        activity: request.activity,
        activity_history: request.activity_history,
        observability: request.observability,
        samples,
    };
    let encoded = serde_json::to_vec_pretty(&report).map_err(io::Error::other)?;
    fs::write(directory.join(REPORT_FILE), encoded)?;
    prune(&request.directory, RETAINED_AUTOPSIES)?;
    Ok(directory)
}

fn sample_target(
    target: &str,
    pid: Option<i32>,
    directory: &Path,
    capturer: &dyn StackCapturer,
) -> StackSample {
    let file_name = format!("{target}.sample.txt");
    let Some(pid) = pid.filter(|pid| *pid > 0) else {
        return StackSample {
            target: target.to_owned(),
            pid: 0,
            file: None,
            error: Some("process identifier is unavailable".to_owned()),
            duration_ms: 0,
        };
    };

    let started = Instant::now();
    let outcome = capturer.capture(pid, SAMPLE_SECONDS, &directory.join(&file_name));
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(()) => StackSample {
            target: target.to_owned(),
            pid,
            file: Some(file_name),
            error: None,
            duration_ms,
        },
        Err(error) => StackSample {
            target: target.to_owned(),
            pid,
            file: None,
            error: Some(error),
            duration_ms,
        },
    }
}

/// Keeps the newest `keep` autopsies. Names sort chronologically, so the
/// oldest are simply the first ones.
pub fn prune(directory: &Path, keep: usize) -> io::Result<()> {
    let mut bundles = match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && is_bundle(path))
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if bundles.len() <= keep {
        return Ok(());
    }

    bundles.sort();
    let excess = bundles.len() - keep;
    for bundle in bundles.into_iter().take(excess) {
        fs::remove_dir_all(bundle)?;
    }
    Ok(())
}

fn is_bundle(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(DIRECTORY_PREFIX))
}

/// Reads every autopsy bundle in `directory`, newest first.
pub fn list_reports(directory: &Path) -> Vec<HangReportSummary> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut summaries = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_bundle(path))
        .filter_map(|path| read_summary(&path))
        .collect::<Vec<_>>();
    summaries.sort_by_key(|summary| std::cmp::Reverse(summary.captured_at_ms));
    summaries
}

fn read_summary(directory: &Path) -> Option<HangReportSummary> {
    let report = directory.join(REPORT_FILE);
    let header: ReportHeader = serde_json::from_slice(&fs::read(&report).ok()?).ok()?;
    let stacks = header
        .samples
        .iter()
        .filter_map(|sample| sample.file.as_ref())
        .map(|file| directory.join(file).to_string_lossy().into_owned())
        .collect();
    Some(HangReportSummary {
        directory: directory.to_string_lossy().into_owned(),
        report: report.to_string_lossy().into_owned(),
        captured_at: header.captured_at.clone(),
        captured_at_ms: header.captured_at_ms,
        delay_ms: header.delay_ms,
        summary: summarize(&header),
        stacks,
    })
}

fn summarize(header: &ReportHeader) -> String {
    let mut parts = vec![format!(
        "{} froze for {}",
        header.watchdog,
        human_duration(header.delay_ms)
    )];
    if let Some(activity) = &header.activity {
        if let Some(pane) = &activity.focus_pane {
            parts.push(format!("focus {pane}"));
        }
        if let Some(inflight) = activity.inflight.first() {
            parts.push(format!(
                "inflight {} ({})",
                inflight.command,
                human_duration(inflight.age_ms)
            ));
        }
        let rejections: u64 = activity
            .rejections
            .iter()
            .fold(0, |total, rejection| total.saturating_add(rejection.count));
        if rejections == 1 {
            parts.push("1 rejection".to_owned());
        } else if rejections > 1 {
            parts.push(format!("{rejections} rejections"));
        }
    }
    if header.samples.iter().all(|sample| sample.file.is_none()) {
        parts.push("no stacks captured".to_owned());
    }
    parts.join(" · ")
}

pub fn human_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{}.{}s", ms / 1_000, (ms % 1_000) / 100)
    }
}

/// Where autopsies live. Debug and release stay separate so a development run
/// cannot bury the evidence from an installed build.
pub fn autopsy_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let name = if cfg!(debug_assertions) {
        "autopsy.dev"
    } else {
        "autopsy"
    };
    Some(PathBuf::from(home).join(".config/sikemux").join(name))
}

/// Builds the listener the UI watchdog notifies. `None` when there is nowhere
/// to write, which leaves hang detection working and evidence capture off.
pub fn listener(activity: Arc<UiActivityLog>) -> Option<Arc<dyn HangListener>> {
    let directory = autopsy_dir()?;
    Some(Arc::new(AutopsyWriter::new(
        directory,
        activity,
        Arc::new(SampleCapturer),
    )))
}

/// Resolves the renderer pid from the thread that owns the webview.
///
/// Asking during a hang would be too late: the answer has to come from the
/// main thread, which is exactly the thread a hang may be holding. Callers run
/// this while the app is healthy and it costs one atomic load once resolved.
pub fn ensure_web_content_pid<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if WEB_CONTENT_PID.load(Ordering::Acquire) > 0 {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        // One question at a time, so a busy main thread never accumulates a
        // queue of these behind whatever is already holding it.
        if PENDING_PID_REQUEST.swap(true, Ordering::AcqRel) {
            return;
        }
        let dispatched = app.get_webview_window("main").map(|webview| {
            // `with_webview` hands the closure to the main thread and returns
            // immediately, so a caller on any thread stays unblocked.
            webview.with_webview(|platform| {
                if let Some(pid) = macos::web_process_identifier(platform.inner()) {
                    WEB_CONTENT_PID.store(pid, Ordering::Release);
                }
                PENDING_PID_REQUEST.store(false, Ordering::Release);
            })
        });
        if !matches!(dispatched, Some(Ok(()))) {
            PENDING_PID_REQUEST.store(false, Ordering::Release);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// Loading a page can replace the renderer, so its pid has to be asked for
/// again rather than sampling a process that has gone.
pub fn forget_web_content_pid() {
    WEB_CONTENT_PID.store(0, Ordering::Release);
}

pub fn web_content_pid() -> Option<i32> {
    match WEB_CONTENT_PID.load(Ordering::Acquire) {
        pid if pid > 0 => Some(pid),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;

    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_web_kit::WKWebView;

    /// WebKit runs the page in a separate WebContent process and only exposes
    /// its pid through this private selector, the same way Safari's own tooling
    /// finds it.
    ///
    /// SAFETY: `pointer` must be a live `WKWebView*`, and this must run on the
    /// main thread.
    pub fn web_process_identifier(pointer: *mut c_void) -> Option<i32> {
        // SAFETY: the only caller passes `platform.inner()` from inside `with_webview`: the
        // live WKWebView, on the main thread.
        let webview: Retained<WKWebView> =
            unsafe { Retained::retain(pointer.cast::<WKWebView>()) }?;
        // SAFETY: `_webProcessIdentifier` is a private WKWebView method that takes nothing
        // and returns the pid as an int. Unlike `keep_running_when_covered`, its
        // existence is not checked first.
        let pid: i32 = unsafe { msg_send![&*webview, _webProcessIdentifier] };
        (pid > 0).then_some(pid)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// `20260918-142233-471`, which sorts chronologically as a directory name.
fn stamp(ms: u64) -> String {
    let (year, month, day, hour, minute, second, millis) = utc_parts(ms);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}-{millis:03}")
}

pub(crate) fn iso8601(ms: u64) -> String {
    let (year, month, day, hour, minute, second, millis) = utc_parts(ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn utc_parts(ms: u64) -> (i64, u32, u32, u64, u64, u64, u64) {
    let seconds = ms / 1_000;
    let millis = ms % 1_000;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    (
        year,
        month,
        day,
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60,
        millis,
    )
}

/// Days since 1970-01-01 to a calendar date, using Howard Hinnant's shift of
/// the year start to March so leap days land at the end of the cycle.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era as i64 + era * 400;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Lists the autopsies written so far, newest first.
#[tauri::command]
pub fn hang_reports() -> AppResult<Vec<HangReportSummary>> {
    let directory = autopsy_dir().ok_or(AppError::BadArg("home directory is unavailable"))?;
    Ok(list_reports(&directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::{Observability, UiActivityRejection, UiInflightCommand};

    struct FakeCapturer {
        body: &'static str,
        fail: bool,
    }

    impl StackCapturer for FakeCapturer {
        fn capture(&self, pid: i32, _seconds: u32, destination: &Path) -> Result<(), String> {
            if self.fail {
                return Err("sample is unavailable".to_owned());
            }
            fs::write(destination, format!("pid {pid}\n{}\n", self.body))
                .map_err(|error| error.to_string())
        }
    }

    fn request(directory: &Path, captured_at_ms: u64) -> AutopsyRequest {
        AutopsyRequest {
            directory: directory.to_path_buf(),
            signal: HangSignal {
                watchdog: "ui".to_owned(),
                delay_us: 4_200_000,
                threshold_us: 2_000_000,
                heartbeat_sequence: 8_123,
                visible: true,
            },
            activity: Some(UiActivitySnapshot {
                at_ms: captured_at_ms,
                inflight: vec![UiInflightCommand {
                    command: "git_status".to_owned(),
                    age_ms: 4_100,
                }],
                recent: Vec::new(),
                focus_pane: Some("editor".to_owned()),
                interactions: Vec::new(),
                rejections: vec![UiActivityRejection {
                    message: "Maximum call stack size exceeded".to_owned(),
                    count: 12,
                }],
            }),
            activity_history: Vec::new(),
            observability: Observability::default().snapshot(),
            web_content_pid: Some(4_242),
            captured_at_ms,
        }
    }

    #[test]
    fn a_bundle_holds_a_readable_report_and_both_stacks() {
        let root = tempfile::tempdir().unwrap();
        let capturer = FakeCapturer {
            body: "1000 WebCore::layout",
            fail: false,
        };
        let bundle = write_autopsy(request(root.path(), 1_789_000_000_000), &capturer).unwrap();

        assert_eq!(
            bundle.file_name().unwrap().to_str().unwrap(),
            "hang-20260910-002640-000"
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join(REPORT_FILE)).unwrap()).unwrap();
        assert_eq!(report["version"], 1);
        assert_eq!(report["capturedAt"], "2026-09-10T00:26:40.000Z");
        assert_eq!(report["delayMs"], 4_200);
        assert_eq!(report["thresholdMs"], 2_000);
        assert_eq!(report["heartbeatSequence"], 8_123);
        assert_eq!(report["webContentPid"], 4_242);
        assert_eq!(report["activity"]["focusPane"], "editor");
        assert_eq!(report["activity"]["inflight"][0]["ageMs"], 4_100);
        assert!(report["observability"]["counters"].is_object());
        assert_eq!(report["samples"][0]["target"], "webcontent");
        assert_eq!(report["samples"][0]["file"], "webcontent.sample.txt");
        assert_eq!(report["samples"][1]["target"], "app");
        assert!(bundle.join("webcontent.sample.txt").exists());
        assert!(bundle.join("app.sample.txt").exists());
    }

    #[test]
    fn a_missing_sampler_is_reported_rather_than_losing_the_autopsy() {
        let root = tempfile::tempdir().unwrap();
        let capturer = FakeCapturer {
            body: "",
            fail: true,
        };
        let bundle = write_autopsy(request(root.path(), 1_789_000_000_000), &capturer).unwrap();

        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join(REPORT_FILE)).unwrap()).unwrap();
        assert_eq!(report["samples"][0]["error"], "sample is unavailable");
        assert!(report["samples"][0]["file"].is_null());

        let summary = list_reports(root.path()).remove(0);
        assert!(
            summary.summary.ends_with("no stacks captured"),
            "{summary:?}"
        );
        assert!(summary.stacks.is_empty());
    }

    #[test]
    fn only_the_five_newest_autopsies_survive() {
        let root = tempfile::tempdir().unwrap();
        let capturer = FakeCapturer {
            body: "stack",
            fail: false,
        };
        for index in 0..8 {
            write_autopsy(
                request(root.path(), 1_789_000_000_000 + index * 60_000),
                &capturer,
            )
            .unwrap();
        }

        let summaries = list_reports(root.path());
        assert_eq!(summaries.len(), RETAINED_AUTOPSIES);
        assert_eq!(summaries[0].captured_at_ms, 1_789_000_000_000 + 7 * 60_000);
        assert_eq!(summaries[4].captured_at_ms, 1_789_000_000_000 + 3 * 60_000);
    }

    #[test]
    fn pruning_ignores_directories_that_are_not_bundles() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("notes")).unwrap();
        for index in 0..3 {
            fs::create_dir(root.path().join(format!("hang-2026070{index}-000000-000"))).unwrap();
        }

        prune(root.path(), 1).unwrap();
        assert!(root.path().join("notes").exists());
        assert!(root.path().join("hang-20260702-000000-000").exists());
        assert!(!root.path().join("hang-20260700-000000-000").exists());
    }

    #[test]
    fn a_pathological_hang_produces_one_autopsy_per_interval() {
        let limiter = RateLimiter::new(Duration::from_secs(120));
        let start = Instant::now();
        assert!(limiter.allow(start));
        assert!(!limiter.allow(start + Duration::from_secs(1)));
        assert!(!limiter.allow(start + Duration::from_secs(119)));
        assert!(limiter.allow(start + Duration::from_secs(120)));
        assert!(!limiter.allow(start + Duration::from_secs(121)));
    }

    #[test]
    fn a_single_rejection_is_not_reported_as_plural() {
        let root = tempfile::tempdir().unwrap();
        let capturer = FakeCapturer {
            body: "stack",
            fail: false,
        };
        let mut only_one = request(root.path(), 1_789_000_000_000);
        only_one.activity.as_mut().unwrap().rejections[0].count = 1;
        write_autopsy(only_one, &capturer).unwrap();

        let summary = list_reports(root.path()).remove(0);
        assert!(summary.summary.ends_with("1 rejection"), "{summary:?}");
    }

    #[test]
    fn a_summary_names_the_stall_the_pane_and_the_work_in_flight() {
        let root = tempfile::tempdir().unwrap();
        let capturer = FakeCapturer {
            body: "stack",
            fail: false,
        };
        write_autopsy(request(root.path(), 1_789_000_000_000), &capturer).unwrap();

        let summary = list_reports(root.path()).remove(0);
        assert_eq!(
            summary.summary,
            "ui froze for 4.2s · focus editor · inflight git_status (4.1s) · 12 rejections"
        );
        assert_eq!(summary.delay_ms, 4_200);
        assert_eq!(summary.stacks.len(), 2);
        assert!(summary.stacks[0].ends_with("webcontent.sample.txt"));
    }

    #[test]
    fn calendar_dates_survive_leap_years_and_century_boundaries() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        // 2000-02-29 and 2024-02-29 are leap days; 1900 was not a leap year.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(iso8601(1_789_000_000_123), "2026-09-10T00:26:40.123Z");
    }

    #[test]
    fn durations_read_as_seconds_once_they_matter() {
        assert_eq!(human_duration(0), "0ms");
        assert_eq!(human_duration(999), "999ms");
        assert_eq!(human_duration(1_000), "1.0s");
        assert_eq!(human_duration(4_299), "4.2s");
    }
}
