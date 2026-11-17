//! The errors and warnings a Gradle build prints, from the Kotlin and Java
//! compilers, Android's resource compiler and Android lint.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::read_text;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BuildError {
    /// Absolute.
    pub path: PathBuf,
    /// One-based.
    pub line: u32,
    /// One-based, when the tool names one.
    pub column: Option<u32>,
    pub severity: Severity,
    pub message: String,
    /// The tool that reported it.
    pub source: &'static str,
    /// Lint's issue id, such as `HardcodedText`.
    pub code: Option<String>,
}

const MAX_ERRORS: usize = 2_000;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// `e: file:///src/Main.kt:12:5 Unresolved reference 'x'.`
static KOTLIN: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^([ew]): (file:///?\S+?):([0-9]+):([0-9]+) (.+)$"));
/// Kotlin before 2.0: `e: /src/Main.kt: (12, 5): Unresolved reference: x`.
static KOTLIN_OLD: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^([ew]): (\S.*?\.kts?): \(([0-9]+), ([0-9]+)\): (.+)$"));
/// `/src/Main.java:12: error: cannot find symbol`
static JAVAC: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(\S.*?\.java):([0-9]+): (error|warning): (.+)$"));
/// `ERROR: /res/layout/main.xml:12: AAPT: error: attribute x not found.`
static AAPT: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"^(?:(?:ERROR|WARNING):\s*)?(\S.*?):([0-9]+)(?::([0-9]+)(?:-[0-9]+)?)?: AAPT: (error|warning): (.+)$",
    )
});
/// Resource linking names the file by its resource set, such as `main`:
/// `com.example.app-main-52:/layout/main.xml:9: error: attribute x not found.`
static RESOURCE_SET: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^(\S+?)-([A-Za-z0-9]+)-[0-9]+:/(\S+?):([0-9]+): (error|warning): (.+)$")
});
/// `/res/layout/main.xml:12: Error: Hardcoded string "Hi" [HardcodedText]`
static LINT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(\S.*?):([0-9]+): (Error|Warning): (.+?)(?: \[([A-Za-z0-9_]+)\])?$"));

fn absolute(cwd: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    path.is_file().then_some(path)
}

/// Lint prints only its first failure, then where its full report is.
const LINT_REPORT_NOTE: &str = "The full lint text report is located at:";

/// A resource set's file: in the module the build ran in, else in the
/// module its namespace ends with.
fn resource_file(cwd: &Path, owner: &str, set: &str, file: &str) -> Option<PathBuf> {
    let module = owner.rsplit('.').next().unwrap_or(owner);
    [cwd.to_path_buf(), cwd.join(module)]
        .into_iter()
        .map(|folder| folder.join("src").join(set).join("res").join(file))
        .find(|path| path.is_file())
}

fn number(text: &str) -> Option<u32> {
    text.parse().ok().filter(|number| *number > 0)
}

fn severity(word: &str) -> Severity {
    match word {
        "e" | "error" | "Error" => Severity::Error,
        _ => Severity::Warning,
    }
}

fn read_line(cwd: &Path, line: &str) -> Option<BuildError> {
    if let Some(found) = KOTLIN.captures(line) {
        let path = url::Url::parse(&found[2]).ok()?.to_file_path().ok()?;
        return Some(BuildError {
            path: path.is_file().then_some(path)?,
            line: number(&found[3])?,
            column: number(&found[4]),
            severity: severity(&found[1]),
            message: found[5].to_owned(),
            source: "Kotlin",
            code: None,
        });
    }
    if let Some(found) = KOTLIN_OLD.captures(line) {
        return Some(BuildError {
            path: absolute(cwd, &found[2])?,
            line: number(&found[3])?,
            column: number(&found[4]),
            severity: severity(&found[1]),
            message: found[5].to_owned(),
            source: "Kotlin",
            code: None,
        });
    }
    if let Some(found) = JAVAC.captures(line) {
        return Some(BuildError {
            path: absolute(cwd, &found[1])?,
            line: number(&found[2])?,
            column: None,
            severity: severity(&found[3]),
            message: found[4].to_owned(),
            source: "javac",
            code: None,
        });
    }
    if let Some(found) = AAPT.captures(line) {
        return Some(BuildError {
            path: absolute(cwd, &found[1])?,
            line: number(&found[2])?,
            column: found.get(3).and_then(|column| number(column.as_str())),
            severity: severity(&found[4]),
            message: found[5].to_owned(),
            source: "AAPT",
            code: None,
        });
    }
    if let Some(found) = RESOURCE_SET.captures(line) {
        return Some(BuildError {
            path: resource_file(cwd, &found[1], &found[2], &found[3])?,
            line: number(&found[4])?,
            column: None,
            severity: severity(&found[5]),
            message: found[6].to_owned(),
            source: "AAPT",
            code: None,
        });
    }
    let found = LINT.captures(line)?;
    Some(BuildError {
        path: absolute(cwd, &found[1])?,
        line: number(&found[2])?,
        column: None,
        severity: severity(&found[3]),
        message: found[4].to_owned(),
        source: "Android Lint",
        code: found.get(5).map(|code| code.as_str().to_owned()),
    })
}

/// Every error and warning in a Gradle run's output, once each, in the order
/// printed. Paths that are not files on this computer are left out.
pub fn gradle(output: &[String], cwd: &Path) -> Vec<BuildError> {
    let lines = output.iter().map(|line| line.trim());
    let lint_reports: Vec<String> = lines
        .clone()
        .zip(lines.clone().skip(1))
        .filter(|(line, _)| *line == LINT_REPORT_NOTE)
        .filter_map(|(_, path)| read_text(&absolute(cwd, path)?))
        .collect();
    let reported = lint_reports.iter().flat_map(|report| report.lines());
    let mut seen = HashSet::new();
    lines
        .chain(reported)
        .filter_map(|line| read_line(cwd, line.trim()))
        .filter(|error| seen.insert(error.clone()))
        .take(MAX_ERRORS)
        .collect()
}
