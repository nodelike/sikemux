//! `sikemux doctor`: the latest panic and hang reports, read straight off disk.

use super::*;

/// Prints the most recent panic and hang autopsies and where to read the rest of them.
///
/// This reads the reports straight off disk rather than asking the app, so it
/// still answers while the app is the thing that is stuck.
pub(super) fn doctor() -> Result<i32, String> {
    let directory = crate::hang_reports::autopsy_dir().ok_or("home directory is unavailable")?;
    print!(
        "{}",
        doctor_report(
            &directory,
            &crate::panic_reports::list_reports(&directory),
            &crate::hang_reports::list_reports(&directory)
        )
    );
    Ok(0)
}

pub(super) fn doctor_report(
    directory: &Path,
    panics: &[(PathBuf, crate::panic_reports::PanicReport)],
    reports: &[crate::hang_reports::HangReportSummary],
) -> String {
    let mut out = format!(
        "Sikemux {}\nAutopsies {}\n",
        env!("CARGO_PKG_VERSION"),
        directory.display()
    );
    if let Some(((path, latest), earlier)) = panics.split_first() {
        out.push_str(&format!("\nLatest panic {}\n", latest.captured_at));
        out.push_str(&format!(
            "  {} {} on thread {}: {}\n",
            latest.process, latest.version, latest.thread, latest.message
        ));
        out.push_str(&format!("  at     {}\n", latest.location));
        out.push_str(&format!("  report {}\n", path.display()));
        if !earlier.is_empty() {
            out.push_str("\nEarlier panics\n");
            for (_, report) in earlier {
                out.push_str(&format!(
                    "  {} {} {}: {}\n",
                    report.captured_at, report.process, report.location, report.message
                ));
            }
        }
    }
    let Some((latest, earlier)) = reports.split_first() else {
        out.push_str("\nNo hang has been recorded yet.\n");
        return out;
    };

    out.push_str(&format!("\nLatest hang {}\n", latest.captured_at));
    out.push_str(&format!("  {}\n", latest.summary));
    out.push_str(&format!("  report {}\n", latest.report));
    for stack in &latest.stacks {
        out.push_str(&format!("  stack  {stack}\n"));
    }
    if !earlier.is_empty() {
        out.push_str("\nEarlier hangs\n");
        for report in earlier {
            out.push_str(&format!("  {} {}\n", report.captured_at, report.summary));
        }
    }
    out
}
