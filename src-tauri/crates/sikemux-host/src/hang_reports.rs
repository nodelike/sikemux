//! Reads the hang reports the app writes when its window stops answering, for
//! `sikemux doctor`, which still works while the app is the thing that is stuck.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const REPORT_FILE: &str = "report.json";
pub const DIRECTORY_PREFIX: &str = "hang-";

/// One autopsy as listed by `sikemux doctor`.
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
    activity: Option<Activity>,
    #[serde(default)]
    samples: Vec<Sample>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Activity {
    focus_pane: Option<String>,
    inflight: Vec<Inflight>,
    rejections: Vec<Rejection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inflight {
    command: String,
    age_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Rejection {
    count: u64,
}

#[derive(Debug, Deserialize)]
struct Sample {
    #[serde(default)]
    file: Option<String>,
}

pub fn is_bundle(path: &Path) -> bool {
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
    sikemux_process::paths::config_file("autopsy", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_seconds_once_they_matter() {
        assert_eq!(human_duration(0), "0ms");
        assert_eq!(human_duration(999), "999ms");
        assert_eq!(human_duration(1_000), "1.0s");
        assert_eq!(human_duration(4_299), "4.2s");
    }
}
