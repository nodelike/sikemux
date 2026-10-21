use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use regex::Regex;
use serde::Serialize;

use super::detect::{CARGO_TEST_LOG, GO_TEST_LOG, JEST_REPORT, PYTEST_REPORT, VITEST_REPORT};
use super::read_text;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestFailure {
    pub class_name: String,
    pub name: String,
    pub message: String,
    /// Absolute path of the test's source file, when it was found.
    pub file: Option<String>,
    /// One-based line the failure was thrown from, when the stack trace names it.
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestResults {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub failures: Vec<TestFailure>,
}

/// Where Maven's surefire and Gradle write their JUnit XML reports, relative to the module.
const REPORT_DIRS: [&str; 2] = ["target/surefire-reports", "build/test-results/test"];
const TEST_SOURCES: [&str; 2] = ["src/test/java", "src/test/kotlin"];
const MAX_FAILURES: usize = 50;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// Jest colours its failure messages for a terminal.
static TERMINAL_COLOURS: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\[[0-9;]*m"));
static REPORT_NAME: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^TEST-[^\n\r\u{2028}\u{2029}]*\.xml$"));
static SCRIPT_FILE: LazyLock<Regex> = LazyLock::new(|| regex(r"\.[cm]?[jt]sx?$"));
static PYTHON_FRAME: LazyLock<Regex> = LazyLock::new(|| regex(r"\.py:[0-9]+"));
static GO_MODULE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^module\s+(\S+)"));
static GO_LOGGED: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s+(\S+\.go):([0-9]+): ([^\n\r\u{2028}\u{2029}]*)$"));
static GO_RESULT: LazyLock<Regex> = LazyLock::new(|| regex(r"^--- (PASS|FAIL|SKIP): (\S+)"));
static GO_DONE: LazyLock<Regex> = LazyLock::new(|| regex(r"^(?:FAIL|ok)\s+(\S+)"));
static CARGO_TOTAL: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"test result: [0-9A-Za-z_]+\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored")
});
static CARGO_BLOCK: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^---- "));
static CARGO_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\S+) stdout ----"));
static CARGO_PANIC: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"panicked at ([^:\n]+):([0-9]+):[0-9]+:\n([\s\S]*?)(?:\nnote:|\n\n|$)")
});
static JEST_VALUE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(?:Expected|Received)(?:[^0-9A-Za-z_]|$)"));
static GO_TEST_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9A-Za-z_]+$"));
static CARGO_TEST_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9A-Za-z_:]+$"));
static MAVEN: LazyLock<Regex> = LazyLock::new(|| regex(r"mvnw?$"));
static GRADLE: LazyLock<Regex> = LazyLock::new(|| regex(r"gradlew?$"));
static METHOD_ARGUMENTS: LazyLock<Regex> = LazyLock::new(|| regex(r"\([^\n\r\u{2028}\u{2029}]*$"));
static SHELL_SAFE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9_./:=@+-]+$"));

fn shell_quote(value: &str) -> String {
    if SHELL_SAFE.is_match(value) {
        return value.to_string();
    }
    if cfg!(windows) {
        return format!("'{}'", value.replace('\'', "''"));
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn strip_colours(text: &str) -> String {
    TERMINAL_COLOURS.replace_all(text, "").into_owned()
}

fn first_line(text: &str) -> &str {
    text.split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

fn join(cwd: &Path, parts: &[&str]) -> PathBuf {
    let mut path = cwd.to_path_buf();
    for part in parts {
        let part = part.replace('\\', "/");
        let part = part.trim_matches('/');
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

fn existing_file(path: PathBuf) -> Option<String> {
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

fn number(text: &str) -> Option<u32> {
    text.parse().ok()
}

/// The stack frame inside the test class itself, such as `at com.x.FooTest.saves(FooTest.java:57)`.
fn failing_frame(stack: &str, class_name: &str) -> Option<(String, Option<u32>)> {
    let frame = Regex::new(&format!(
        r"at {}\.[0-9A-Za-z_$<>]+\(([0-9A-Za-z_$]+\.(?:java|kt)):([0-9]+)\)",
        regex::escape(class_name)
    ))
    .ok()?;
    let captures = frame.captures(stack)?;
    Some((captures[1].to_string(), number(&captures[2])))
}

fn source_file(cwd: &Path, class_name: &str, file_name: &str) -> Option<String> {
    let segments: Vec<&str> = class_name.split('.').collect();
    let package_path = segments[..segments.len() - 1].join("/");
    TEST_SOURCES
        .iter()
        .find_map(|root| existing_file(join(cwd, &[root, &package_path, file_name])))
}

/// Vitest names a test case by its file, such as `lib/price.test.ts`, where Java names a class.
pub fn is_file_path(class_name: &str) -> bool {
    class_name.contains('/') || SCRIPT_FILE.is_match(class_name)
}

/// The first `file:line` in a stack trace that points into the test's own file.
fn line_in(stack: &str, file: &str) -> Option<u32> {
    let name = file.rsplit('/').next().unwrap_or(file);
    let pattern = Regex::new(&format!(r"{}:([0-9]+)", regex::escape(name))).ok()?;
    number(&pattern.captures(stack)?[1])
}

fn report_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in REPORT_DIRS {
        files.extend(
            list_files(&join(cwd, &[dir]))
                .into_iter()
                .filter(|(name, _)| REPORT_NAME.is_match(name))
                .map(|(_, path)| path),
        );
    }
    for report in [VITEST_REPORT, PYTEST_REPORT] {
        let path = join(cwd, &[report]);
        if path.is_file() {
            files.push(path);
        }
    }
    files
}

/// The files directly in `dir` by name, ignoring case, or none when it cannot be listed.
fn list_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return Vec::new();
        };
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if !is_dir {
            files.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    files.sort_by_cached_key(|(name, _)| name.to_lowercase());
    files
}

struct Failing {
    class_name: String,
    name: String,
    message: String,
    stack: String,
}

#[derive(Default)]
struct JunitTotals {
    passed: u32,
    skipped: u32,
    failing: Vec<Failing>,
}

struct OpenCase {
    depth: usize,
    class_name: String,
    name: String,
    problem: Option<Problem>,
    skipped: bool,
}

struct Problem {
    message: Option<String>,
    text: String,
    open_at: Option<usize>,
}

fn attribute(element: &BytesStart, name: &str) -> Option<String> {
    let value = element.try_get_attribute(name).ok()??;
    value
        .normalized_value(XmlVersion::Implicit1_0)
        .ok()
        .map(|value| value.into_owned())
}

impl OpenCase {
    fn new(element: &BytesStart, depth: usize) -> Self {
        OpenCase {
            depth,
            class_name: attribute(element, "classname").unwrap_or_default(),
            name: attribute(element, "name").unwrap_or_default(),
            problem: None,
            skipped: false,
        }
    }

    fn child(&mut self, element: &BytesStart, depth: usize, empty: bool) {
        match element.name().as_ref() {
            "failure" | "error" if self.problem.is_none() => {
                self.problem = Some(Problem {
                    message: attribute(element, "message"),
                    text: String::new(),
                    open_at: (!empty).then_some(depth),
                });
            }
            "skipped" => self.skipped = true,
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(problem) = &mut self.problem {
            if problem.open_at.is_some() {
                problem.text.push_str(text);
            }
        }
    }

    fn close(self, totals: &mut JunitTotals) {
        match self.problem {
            Some(problem) => {
                let message = match problem.message {
                    Some(message) if !message.is_empty() => message,
                    _ => first_line(&problem.text).to_string(),
                };
                totals.failing.push(Failing {
                    class_name: self.class_name,
                    name: self.name,
                    message,
                    stack: problem.text,
                });
            }
            None if self.skipped => totals.skipped += 1,
            None => totals.passed += 1,
        }
    }
}

fn read_junit(xml: &str, totals: &mut JunitTotals) {
    let mut reader = Reader::from_str(xml);
    let mut depth = 0;
    let mut case: Option<OpenCase> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                depth += 1;
                match &mut case {
                    Some(open) => open.child(&element, depth, false),
                    None if element.name().as_ref() == "testcase" => {
                        case = Some(OpenCase::new(&element, depth));
                    }
                    None => {}
                }
            }
            Ok(Event::Empty(element)) => match &mut case {
                Some(open) => open.child(&element, depth + 1, true),
                None if element.name().as_ref() == "testcase" => {
                    OpenCase::new(&element, depth + 1).close(totals);
                }
                None => {}
            },
            Ok(Event::Text(text)) => {
                if let Some(open) = &mut case {
                    open.text(&text.xml10_content());
                }
            }
            Ok(Event::CData(text)) => {
                if let Some(open) = &mut case {
                    open.text(&text.xml10_content());
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(open) = &mut case {
                    match reference.resolve_char_ref() {
                        Ok(Some(character)) => open.text(character.encode_utf8(&mut [0; 4])),
                        Ok(None) => {
                            if let Some(resolved) = resolve_predefined_entity(&reference) {
                                open.text(resolved);
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some(problem) = case.as_mut().and_then(|open| open.problem.as_mut()) {
                    if problem.open_at == Some(depth) {
                        problem.open_at = None;
                    }
                }
                if case.as_ref().is_some_and(|open| open.depth == depth) {
                    if let Some(open) = case.take() {
                        open.close(totals);
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

struct GoFailure {
    name: String,
    message: String,
    file_name: Option<String>,
    line: Option<u32>,
}

#[derive(Default)]
struct GoLogged {
    message: String,
    file_name: Option<String>,
    line: Option<u32>,
}

/// `go test -v` prints a test's log lines between its `=== RUN` and its `--- FAIL`, and the package after its tests.
fn read_go_results(cwd: &Path) -> Option<TestResults> {
    let log = read_text(&join(cwd, &[GO_TEST_LOG])).filter(|log| !log.is_empty())?;
    let go_mod = read_text(&join(cwd, &["go.mod"])).unwrap_or_default();
    let module = GO_MODULE
        .captures(&go_mod)
        .map(|captures| captures[1].to_string())
        .unwrap_or_default();
    let mut passed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();
    let mut pending: Vec<GoFailure> = Vec::new();
    let mut current = GoLogged::default();
    for raw in log.split('\n') {
        let line = strip_colours(raw);
        if line.starts_with("=== RUN") {
            current = GoLogged::default();
        }
        if let Some(logged) = GO_LOGGED.captures(&line) {
            if current.message.is_empty() {
                current = GoLogged {
                    file_name: Some(logged[1].to_string()),
                    line: number(&logged[2]),
                    message: logged[3].to_string(),
                };
            }
        }
        if let Some(result) = GO_RESULT.captures(&line) {
            match &result[1] {
                "PASS" => passed += 1,
                "SKIP" => skipped += 1,
                _ => pending.push(GoFailure {
                    name: result[2].to_string(),
                    message: current.message.clone(),
                    file_name: current.file_name.clone(),
                    line: current.line,
                }),
            }
        }
        if let Some(done) = GO_DONE.captures(&line) {
            if pending.is_empty() {
                continue;
            }
            let package = &done[1];
            let dir = if !module.is_empty() && package.starts_with(&module) {
                package.get(module.len() + 1..).unwrap_or("")
            } else {
                ""
            };
            for failure in pending.drain(..) {
                let file = failure
                    .file_name
                    .as_deref()
                    .and_then(|file_name| existing_file(join(cwd, &[dir, file_name])));
                failures.push(TestFailure {
                    class_name: package.to_string(),
                    name: failure.name,
                    message: failure.message,
                    file,
                    line: failure.line,
                });
            }
        }
    }
    Some(capped(passed, skipped, failures))
}

fn capped(passed: u32, skipped: u32, mut failures: Vec<TestFailure>) -> TestResults {
    let failed = failures.len() as u32;
    failures.truncate(MAX_FAILURES);
    TestResults {
        passed,
        failed,
        skipped,
        failures,
    }
}

/// `cargo test` names failures in `---- name stdout ----` blocks, and totals each test binary in a `test result:` line.
fn read_cargo_results(cwd: &Path) -> Option<TestResults> {
    let log = read_text(&join(cwd, &[CARGO_TEST_LOG])).filter(|log| !log.is_empty())?;
    let text = strip_colours(&log);
    let mut passed: u32 = 0;
    let mut skipped: u32 = 0;
    for total in CARGO_TOTAL.captures_iter(&text) {
        passed = passed.saturating_add(number(&total[1]).unwrap_or(0));
        skipped = skipped.saturating_add(number(&total[3]).unwrap_or(0));
    }
    let mut failures = Vec::new();
    for block in CARGO_BLOCK.split(&text).skip(1) {
        let Some(name) = CARGO_NAME
            .captures(block)
            .map(|captures| captures[1].to_string())
        else {
            continue;
        };
        let panic = CARGO_PANIC.captures(block);
        let message = panic
            .as_ref()
            .map(|panic| {
                panic[3]
                    .split('\n')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .unwrap_or_default();
        let file = panic
            .as_ref()
            .and_then(|panic| existing_file(join(cwd, &[&panic[1]])));
        let segments: Vec<&str> = name.split("::").collect();
        failures.push(TestFailure {
            class_name: segments[..segments.len() - 1].join("::"),
            name: segments[segments.len() - 1].to_string(),
            message,
            file,
            line: panic.as_ref().and_then(|panic| number(&panic[2])),
        });
    }
    Some(capped(passed, skipped, failures))
}

/// Jest's first line names the matcher; the values it compared come a few lines later.
fn jest_message(text: &str) -> String {
    let mut parts = vec![first_line(text)];
    parts.extend(
        text.split('\n')
            .map(str::trim)
            .filter(|line| JEST_VALUE.is_match(line)),
    );
    parts.join(" · ")
}

fn string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

fn array_field<'a>(value: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Jest's `--json` report, which the detected test task asks it to write.
fn read_jest_results(cwd: &Path) -> Option<TestResults> {
    let text = read_text(&join(cwd, &[JEST_REPORT])).filter(|text| !text.is_empty())?;
    let report: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cwd_prefix = format!("{}/", cwd.to_string_lossy());
    let mut passed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();
    for suite in array_field(&report, "testResults") {
        let file = string_field(suite, "name").unwrap_or("");
        let class_name = file.strip_prefix(&cwd_prefix).unwrap_or(file);
        for test in array_field(suite, "assertionResults") {
            match string_field(test, "status") {
                Some("passed") => passed += 1,
                Some("failed") => {
                    let stack = array_field(test, "failureMessages")
                        .iter()
                        .map(|message| message.as_str().unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let name = [string_field(test, "fullName"), string_field(test, "title")]
                        .into_iter()
                        .flatten()
                        .find(|name| !name.is_empty())
                        .unwrap_or("");
                    failures.push(TestFailure {
                        class_name: class_name.to_string(),
                        name: name.to_string(),
                        message: jest_message(&strip_colours(&stack)),
                        file: (!file.is_empty()).then(|| file.to_string()),
                        line: line_in(&stack, file),
                    });
                }
                _ => skipped += 1,
            }
        }
    }
    Some(capped(passed, skipped, failures))
}

fn junit_failure(cwd: &Path, failing: Failing) -> TestFailure {
    let Failing {
        class_name,
        name,
        message,
        stack,
    } = failing;
    let (file, line) = if is_file_path(&class_name) {
        (
            existing_file(join(cwd, &[&class_name])),
            line_in(&stack, &class_name),
        )
    } else if PYTHON_FRAME.is_match(&stack) {
        let file = existing_file(join(
            cwd,
            &[&format!("{}.py", class_name.replace('.', "/"))],
        ));
        let line = file.as_deref().and_then(|file| line_in(&stack, file));
        (file, line)
    } else {
        match failing_frame(&stack, &class_name) {
            Some((file_name, line)) => (source_file(cwd, &class_name, &file_name), line),
            None => (None, None),
        }
    };
    TestFailure {
        class_name,
        name,
        message,
        file,
        line,
    }
}

/// The latest results a test task left in `cwd`, or none.
pub fn read(cwd: &Path) -> Option<TestResults> {
    let files = report_files(cwd);
    if files.is_empty() {
        return read_jest_results(cwd)
            .or_else(|| read_go_results(cwd))
            .or_else(|| read_cargo_results(cwd));
    }
    let mut totals = JunitTotals::default();
    for file in &files {
        read_junit(&read_text(file).unwrap_or_default(), &mut totals);
    }
    let failed = totals.failing.len() as u32;
    let failures = totals
        .failing
        .into_iter()
        .take(MAX_FAILURES)
        .map(|failing| junit_failure(cwd, failing))
        .collect();
    Some(TestResults {
        passed: totals.passed,
        failed,
        skipped: totals.skipped,
        failures,
    })
}

fn unique<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen = HashSet::new();
    values.filter(|value| seen.insert(*value)).collect()
}

fn method_name(name: &str) -> String {
    METHOD_ARGUMENTS.replace(name, "").into_owned()
}

/// A test runner whose failures a test task's command can be narrowed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Runner<'a> {
    Pytest,
    Go,
    Cargo,
    Vitest,
    Jest,
    /// With the program the command starts, `mvn` or the project's `./mvnw`.
    Maven(&'a str),
    Gradle(&'a str),
}

/// The file each runner's detected command writes its results to, which
/// tells the runner from the command.
const REPORTED: [(Runner<'static>, &str); 5] = [
    (Runner::Pytest, PYTEST_REPORT),
    (Runner::Go, GO_TEST_LOG),
    (Runner::Cargo, CARGO_TEST_LOG),
    (Runner::Vitest, VITEST_REPORT),
    (Runner::Jest, JEST_REPORT),
];

impl<'a> Runner<'a> {
    /// The runner `command` starts, by the report it writes or else the
    /// build tool it runs.
    fn of(command: &'a str) -> Option<Self> {
        if let Some((runner, _)) = REPORTED.iter().find(|(_, report)| command.contains(report)) {
            return Some(*runner);
        }
        let tool = command.split(' ').next().unwrap_or("");
        if MAVEN.is_match(tool) {
            Some(Self::Maven(tool))
        } else if GRADLE.is_match(tool) {
            Some(Self::Gradle(tool))
        } else {
            None
        }
    }
}

/// The same test command narrowed to the tests that failed, or none for a build tool it does not know.
pub fn rerun_failed_command(command: &str, failures: &[TestFailure]) -> Option<String> {
    match Runner::of(command)? {
        Runner::Pytest => Some(format!("{command} --lf")),
        // Go and Cargo commands sit inside `sh -c '…'`, where quoting cannot nest, so only plain identifiers go in.
        Runner::Go => {
            let names = unique(
                failures
                    .iter()
                    .map(|failure| failure.name.split('/').next().unwrap_or("")),
            );
            if !names.iter().all(|name| GO_TEST_NAME.is_match(name)) {
                return None;
            }
            Some(command.replacen(
                "go test -v ./...",
                &format!(r#"go test -v -run "^({})\$" ./..."#, names.join("|")),
                1,
            ))
        }
        Runner::Cargo => {
            let names: Vec<String> = failures
                .iter()
                .map(|failure| {
                    if failure.class_name.is_empty() {
                        failure.name.clone()
                    } else {
                        format!("{}::{}", failure.class_name, failure.name)
                    }
                })
                .collect();
            if !names.iter().all(|name| CARGO_TEST_NAME.is_match(name)) {
                return None;
            }
            Some(command.replacen(
                "cargo test",
                &format!("cargo test -- --exact {}", names.join(" ")),
                1,
            ))
        }
        Runner::Vitest | Runner::Jest => {
            let files: Vec<String> =
                unique(failures.iter().map(|failure| failure.class_name.as_str()))
                    .into_iter()
                    .map(shell_quote)
                    .collect();
            Some(format!("{command} {}", files.join(" ")))
        }
        Runner::Maven(tool) => {
            let mut by_class: Vec<(&str, Vec<String>)> = Vec::new();
            for failure in failures {
                let simple = failure.class_name.rsplit('.').next().unwrap_or("");
                let method = method_name(&failure.name);
                match by_class.iter_mut().find(|(class, _)| *class == simple) {
                    Some((_, methods)) => methods.push(method),
                    None => by_class.push((simple, vec![method])),
                }
            }
            let selector = by_class
                .iter()
                .map(|(class, methods)| format!("{class}#{}", methods.join("+")))
                .collect::<Vec<_>>()
                .join(",");
            Some(format!(
                "{tool} test -Dtest={} -Dsurefire.failIfNoSpecifiedTests=false",
                shell_quote(&selector)
            ))
        }
        Runner::Gradle(tool) => {
            let filters = failures
                .iter()
                .map(|failure| {
                    format!(
                        "--tests {}",
                        shell_quote(&format!(
                            "{}.{}",
                            failure.class_name,
                            method_name(&failure.name)
                        ))
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            Some(format!("{tool} test {filters}"))
        }
    }
}
