//! The run, test and build tasks a project's build files suggest, and the ones
//! a person added for themselves or for the team.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::tasks::config::CONFIG_FILE;
use crate::tasks::read_text;

pub const DETECTED_PREFIX: &str = "detected.";
pub const PERSONAL_PREFIX: &str = "personal.";
/// Tasks one person added for themselves, in a folder that keeps itself out of Git.
pub const PERSONAL_TASKS: &str = ".sikemux/tasks.json";

/// Go and Cargo write no report of their own, so their test tasks keep a copy
/// of the readable output for the results card. `.sikemux` ignores itself in
/// Git; Cargo's `target` is ignored already.
pub const GO_TEST_LOG: &str = ".sikemux/go-test.log";
pub const CARGO_TEST_LOG: &str = "target/sikemux-test.log";
/// Inside node_modules, so Git never sees them; the test results card reads them back.
pub const VITEST_REPORT: &str = "node_modules/.sikemux-junit.xml";
pub const JEST_REPORT: &str = "node_modules/.sikemux-jest.json";
/// pytest keeps its own cache folder out of Git, so its report can live there.
pub const PYTEST_REPORT: &str = ".pytest_cache/sikemux-junit.xml";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub id: String,
    pub label: String,
    pub command: String,
    /// Absolute.
    pub cwd: PathBuf,
    /// The Java major version the build files ask for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java: Option<u32>,
}

pub struct AddedTask {
    pub label: String,
    pub command: String,
    /// Relative to the project, "." for its root.
    pub cwd: String,
}

struct Task {
    id: String,
    label: String,
    command: String,
    java: Option<u32>,
}

impl Task {
    fn new(id: impl Into<String>, label: impl Into<String>, command: impl Into<String>) -> Self {
        Task {
            id: id.into(),
            label: label.into(),
            command: command.into(),
            java: None,
        }
    }
}

struct Entry {
    name: String,
    is_dir: bool,
}

/// Directories first, then files, both by name ignoring case; `.git` left out.
fn list(dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in read {
        let Ok(entry) = entry else {
            return Vec::new();
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push(Entry { name, is_dir });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

fn read_or_empty(path: &Path) -> String {
    read_text(path).unwrap_or_default()
}

fn regexes(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("valid pattern"))
        .collect()
}

macro_rules! regex {
    ($pattern:expr) => {{
        static PATTERN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("valid pattern"));
        &*PATTERN
    }};
}

static SPRING_BOOT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    regexes(&[
        r"spring-boot-starter-parent</artifactId>\s*<version>([0-9]+)",
        r#"org\.springframework\.boot["']?\)?\s*version\s*["'(]*([0-9]+)"#,
    ])
});
static POM_JAVA: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    regexes(&[
        r"<maven\.compiler\.release>([0-9]+)<",
        r"<release>([0-9]+)</release>",
        r"<java\.version>(?:1\.)?([0-9]+)<",
        r"<maven\.compiler\.source>(?:1\.)?([0-9]+)<",
    ])
});
static GRADLE_JAVA: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    regexes(&[
        r"JavaLanguageVersion\.of\(\s*([0-9]+)",
        r"sourceCompatibility\s*=?\s*JavaVersion\.VERSION_(?:1_)?([0-9]+)",
        r#"sourceCompatibility\s*=?\s*['"](?:1\.)?([0-9]+)"#,
        r#"jvmTarget\s*=?\s*(?:JvmTarget\.JVM_|['"])(?:1_|1\.)?([0-9]+)"#,
    ])
});
static JAVA_VERSION_FILE: LazyLock<Vec<Regex>> =
    LazyLock::new(|| regexes(&[r"(?:^|-)(?:1\.)?([0-9]+)"]));
static SDKMANRC: LazyLock<Vec<Regex>> =
    LazyLock::new(|| regexes(&[r"(?mR)^\s*java\s*=\s*([0-9]+)"]));
static TOOL_VERSIONS: LazyLock<Vec<Regex>> =
    LazyLock::new(|| regexes(&[r"(?mR)^\s*java\s+(?:[a-z]+-)*([0-9]+)"]));

static FOLDER_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("valid pattern"));

fn first_number(text: &str, patterns: &[Regex]) -> Option<u32> {
    let captures = patterns.iter().find_map(|pattern| pattern.captures(text))?;
    Some(captures[1].parse().unwrap_or(u32::MAX))
}

/// Spring Boot 3 and later need Java 17 even when the build names no version.
fn spring_boot_minimum(build: &str) -> Option<u32> {
    first_number(build, &SPRING_BOOT)
        .filter(|boot| *boot >= 3)
        .map(|_| 17)
}

fn java_from_pom(pom: &str) -> Option<u32> {
    first_number(pom, &POM_JAVA).or_else(|| spring_boot_minimum(pom))
}

fn java_from_gradle(build: &str) -> Option<u32> {
    first_number(build, &GRADLE_JAVA).or_else(|| spring_boot_minimum(build))
}

/// What a developer pinned with jenv, SDKMAN or asdf/mise wins over the build file's minimum.
fn java_from_version_files(dir: &Path, files: &HashSet<String>) -> Option<u32> {
    let pins: [(&str, &[Regex]); 3] = [
        (".java-version", &JAVA_VERSION_FILE),
        (".sdkmanrc", &SDKMANRC),
        (".tool-versions", &TOOL_VERSIONS),
    ];
    pins.into_iter()
        .filter(|(name, _)| files.contains(*name))
        .find_map(|(name, patterns)| {
            first_number(&read_or_empty(&dir.join(name)), patterns).filter(|pinned| *pinned != 0)
        })
}

fn with_java(tasks: Vec<Task>, java: Option<u32>) -> Vec<Task> {
    tasks
        .into_iter()
        .map(|task| Task { java, ..task })
        .collect()
}

fn maven_tasks(root: &Path, files: &HashSet<String>) -> Vec<Task> {
    if !files.contains("pom.xml") {
        return Vec::new();
    }
    let mvn = if files.contains("mvnw") {
        "./mvnw"
    } else {
        "mvn"
    };
    let pom = read_or_empty(&root.join("pom.xml"));
    let java = java_from_version_files(root, files).or_else(|| java_from_pom(&pom));
    let mut tasks = Vec::new();
    if pom.contains("spring-boot") {
        tasks.push(Task::new(
            "detected.maven.spring-boot-run",
            "Spring Boot: run",
            format!("{mvn} spring-boot:run"),
        ));
    }
    tasks.push(Task::new(
        "detected.maven.test",
        "Maven: test",
        format!("{mvn} test"),
    ));
    tasks.push(Task::new(
        "detected.maven.package",
        "Maven: package",
        format!("{mvn} package"),
    ));
    with_java(tasks, java)
}

/// What a folder's tasks may take from the project root above it: Gradle's
/// wrapper, settings and version catalog.
struct Above<'a> {
    dir: &'a Path,
    files: &'a HashSet<String>,
}

const GRADLE_WRAPPER: &str = if cfg!(windows) {
    "gradlew.bat"
} else {
    "gradlew"
};
const GRADLE_SETTINGS: [&str; 2] = ["settings.gradle.kts", "settings.gradle"];
const VERSION_CATALOG: &str = "gradle/libs.versions.toml";

/// A module of a bigger build runs the root's wrapper; a folder with its own
/// settings is a build of its own.
fn gradle_program(files: &HashSet<String>, above: Option<&Above>) -> String {
    let wrapper = |folder: &str| format!("{folder}{}{GRADLE_WRAPPER}", std::path::MAIN_SEPARATOR);
    if files.contains(GRADLE_WRAPPER) {
        return wrapper(".");
    }
    let own_build = GRADLE_SETTINGS.iter().any(|name| files.contains(*name));
    match above {
        Some(above) if !own_build && above.files.contains(GRADLE_WRAPPER) => wrapper(".."),
        _ => "gradle".to_owned(),
    }
}

/// The text of a Gradle file in this folder, else in the project root above it.
fn shared_gradle_file(dir: &Path, above: Option<&Above>, names: &[&str]) -> String {
    let folders = std::iter::once(dir).chain(above.map(|above| above.dir));
    folders
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .find_map(|path| read_text(&path))
        .unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AndroidModule {
    App,
    Library,
}

fn android_kind(plugin: &str) -> Option<AndroidModule> {
    let plugin = plugin.replace(['-', '_'], ".");
    if plugin.contains("android.application") {
        Some(AndroidModule::App)
    } else if plugin.contains("android.library") {
        Some(AndroidModule::Library)
    } else {
        None
    }
}

/// The version catalog's `[plugins]` table, from each alias as the build
/// files spell it to the plugin id it stands for.
fn catalog_plugins(catalog: &str) -> Vec<(String, String)> {
    let mut in_plugins = false;
    let entry =
        regex!(r#"^\s*([A-Za-z0-9._-]+)\s*=\s*(?:\{[^}\n]*?(?-u:\b)id\s*=\s*"([^"]+)"|"([^":]+))"#);
    let mut plugins = Vec::new();
    for line in catalog.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_plugins = line == "[plugins]";
            continue;
        }
        if !in_plugins {
            continue;
        }
        if let Some(found) = entry.captures(line) {
            let id = found.get(2).or(found.get(3)).map_or("", |id| id.as_str());
            plugins.push((found[1].replace(['-', '_'], "."), id.to_owned()));
        }
    }
    plugins
}

/// An Android app or library module: its build applies `com.android.application`
/// or `com.android.library`, by id or through a version catalog alias. A plugin
/// declared with `apply false`, as a root build does for its modules, does not count.
fn android_module(build: &str, catalog: &str) -> Option<AndroidModule> {
    let applied = regex!(
        r#"(?m)^\s*(?:id|alias|apply\s+plugin\s*:)\s*\(?\s*["']?([A-Za-z0-9._-]+)["']?\s*\)?([^\n]*)$"#
    );
    let plugins = catalog_plugins(catalog);
    applied
        .captures_iter(build)
        .filter(|found| !regex!(r"apply\s*\(?\s*false").is_match(&found[2]))
        .find_map(|found| {
            let name = &found[1];
            let Some((_, alias)) = name.split_once(".plugins.") else {
                return android_kind(name);
            };
            let id = plugins
                .iter()
                .find(|(known, _)| known == alias)
                .map(|(_, id)| id.as_str());
            id.and_then(android_kind).or_else(|| android_kind(alias))
        })
}

/// The Java the Android Gradle plugin runs on: 17 from version 8, 11 for 7.
fn android_plugin_java(gradle_files: &str) -> Option<u32> {
    let plugin_line =
        regex!(r"(?m)^[^\n]*com\.android\.(?:application|library|tools\.build)[^\n]*$");
    let inline = regex!(r#"(?:version\s*\(?\s*=?\s*["']|:)([0-9]+)\.[0-9]"#);
    let reference = regex!(r#"version\.ref\s*=\s*"([A-Za-z0-9._-]+)""#);
    let major = plugin_line.find_iter(gradle_files).find_map(|line| {
        let line = line.as_str();
        if let Some(found) = inline.captures(line) {
            return found[1].parse::<u32>().ok();
        }
        let name = reference.captures(line)?;
        let version = Regex::new(&format!(
            r#"(?m)^\s*{}\s*=\s*"([0-9]+)\."#,
            regex::escape(&name[1])
        ))
        .ok()?;
        version.captures(gradle_files)?[1].parse::<u32>().ok()
    })?;
    match major {
        0..=6 => None,
        7 => Some(11),
        _ => Some(17),
    }
}

/// The named entries directly inside the first `name { … }` block, with their
/// bodies: `create("staging") { … }`, `getByName("release")` or Groovy's `free { … }`.
fn gradle_block_entries(build: &str, name: &str) -> Vec<(String, String)> {
    let Some(opening) = Regex::new(&format!(r"(?-u:\b){name}\s*\{{"))
        .expect("valid pattern")
        .find(build)
    else {
        return Vec::new();
    };
    let call = regex!(
        r#"(?:create|register|getByName|maybeCreate|named)\s*\(\s*["']([A-Za-z][A-Za-z0-9_]*)["']\s*\)\s*$"#
    );
    let groovy = regex!(r"(?:^|[\s;])([A-Za-z][A-Za-z0-9_]*)\s*$");
    let named_call = regex!(
        r#"(?:create|register|getByName|maybeCreate|named)\s*\(\s*["']([A-Za-z][A-Za-z0-9_]*)["']"#
    );
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut add = |entry: String, body: String| {
        if !entries.iter().any(|(known, _)| *known == entry) {
            entries.push((entry, body));
        }
    };
    let mut depth = 0usize;
    let mut head = String::new();
    let mut body_start = None;
    let mut entry: Option<String> = None;
    let text = &build[opening.end()..];
    for (index, character) in text.char_indices() {
        match character {
            '{' => {
                if depth == 0 {
                    let named = call
                        .captures(&head)
                        .or_else(|| groovy.captures(&head))
                        .map(|found| found[1].to_owned());
                    for found in named_call.captures_iter(&head) {
                        add(found[1].to_owned(), String::new());
                    }
                    entry = named;
                    body_start = Some(index + 1);
                    head.clear();
                }
                depth += 1;
            }
            '}' if depth == 0 => {
                for found in named_call.captures_iter(&head) {
                    add(found[1].to_owned(), String::new());
                }
                break;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let (Some(name), Some(start)) = (entry.take(), body_start.take()) {
                        add(name, text[start..index].to_owned());
                    }
                }
            }
            _ if depth == 0 => head.push(character),
            _ => {}
        }
    }
    let ignored = [
        "all",
        "configureEach",
        "each",
        "forEach",
        "matching",
        "whenObjectAdded",
    ];
    entries.retain(|(name, _)| !ignored.contains(&name.as_str()));
    entries
}

fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The flavor combinations a build declares, one name each such as
/// `freeStaging`, in the order Gradle names them; one empty name when it
/// declares none.
fn flavor_combinations(build: &str) -> Vec<String> {
    let flavors = gradle_block_entries(build, "productFlavors");
    if flavors.is_empty() {
        return vec![String::new()];
    }
    let dimension_of = regex!(r#"dimension\s*(?:=\s*)?\(?\s*["']([A-Za-z0-9_]+)["']"#);
    let quoted = regex!(r#"["']([A-Za-z0-9_]+)["']"#);
    let mut dimensions: Vec<String> = regex!(r"(?m)flavorDimensions[^\n]*")
        .find_iter(build)
        .flat_map(|line| {
            quoted
                .captures_iter(line.as_str())
                .map(|found| found[1].to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    let flavor_dimension = |body: &str| {
        dimension_of
            .captures(body)
            .map(|found| found[1].to_owned())
            .unwrap_or_default()
    };
    for (_, body) in &flavors {
        let dimension = flavor_dimension(body);
        if !dimensions.contains(&dimension) {
            dimensions.push(dimension);
        }
    }
    let mut combinations = vec![String::new()];
    for dimension in dimensions {
        let in_dimension: Vec<&str> = flavors
            .iter()
            .filter(|(_, body)| flavor_dimension(body) == dimension)
            .map(|(name, _)| name.as_str())
            .collect();
        if in_dimension.is_empty() {
            continue;
        }
        combinations = combinations
            .iter()
            .flat_map(|prefix| {
                in_dimension.iter().map(move |flavor| {
                    if prefix.is_empty() {
                        (*flavor).to_owned()
                    } else {
                        format!("{prefix}{}", capitalized(flavor))
                    }
                })
            })
            .collect();
    }
    combinations
}

/// Variants past this many are left to the person's own tasks.
const MAX_ANDROID_VARIANTS: usize = 6;

fn variant_name(flavors: &str, build_type: &str) -> String {
    if flavors.is_empty() {
        build_type.to_owned()
    } else {
        format!("{flavors}{}", capitalized(build_type))
    }
}

fn android_tasks(gradle: &str, module: AndroidModule, build: &str) -> Vec<Task> {
    let mut build_types: Vec<String> = vec!["debug".into(), "release".into()];
    for (name, _) in gradle_block_entries(build, "buildTypes") {
        if !build_types.contains(&name) {
            build_types.push(name);
        }
    }
    let combinations = flavor_combinations(build);
    let declares_variants = build_types.len() > 2
        || combinations.len() > 1
        || !combinations[0].is_empty()
        || regex!(r"(?-u:\b)buildTypes\s*\{").is_match(build);
    let task = |gradle_task: String, id: String| {
        Task::new(
            format!("detected.android.{id}"),
            format!("Android: {gradle_task}"),
            format!("{gradle} {gradle_task}"),
        )
    };
    let debug = variant_name(&combinations[0], "debug");
    let debug_task = capitalized(&debug);
    let mut tasks = Vec::new();
    if module == AndroidModule::App {
        tasks.push(task(
            format!("install{debug_task}"),
            format!("install{debug_task}"),
        ));
    }
    tasks.push(task(
        format!("assemble{debug_task}"),
        format!("assemble{debug_task}"),
    ));
    tasks.push(task(format!("test{debug_task}UnitTest"), "test".into()));
    tasks.push(task("connectedAndroidTest".into(), "test.connected".into()));
    tasks.push(task("lint".into(), "lint".into()));
    if !declares_variants {
        return tasks;
    }
    let variants = combinations
        .iter()
        .flat_map(|flavors| {
            build_types
                .iter()
                .map(move |build_type| (variant_name(flavors, build_type), build_type == "debug"))
        })
        .filter(|(variant, _)| *variant != debug)
        .take(MAX_ANDROID_VARIANTS);
    for (variant, is_debug) in variants {
        let name = capitalized(&variant);
        if is_debug && module == AndroidModule::App {
            tasks.push(task(format!("install{name}"), format!("install{name}")));
        }
        tasks.push(task(format!("assemble{name}"), format!("assemble{name}")));
        if is_debug {
            tasks.push(task(
                format!("test{name}UnitTest"),
                format!("test.{variant}"),
            ));
            tasks.push(task(
                format!("connected{name}AndroidTest"),
                format!("test.connected.{variant}"),
            ));
        }
    }
    tasks
}

/// Gradle's `application` plugin; not `com.android.application`.
fn applies_application(build: &str) -> bool {
    regex!(r"(?-u:\b)application(?-u:\b)")
        .find_iter(build)
        .any(|found| {
            let before = &build[..found.start()];
            !before.ends_with("android.") && !before.ends_with("android-")
        })
}

fn gradle_tasks(root: &Path, files: &HashSet<String>, above: Option<&Above>) -> Vec<Task> {
    let Some(build_file) = ["build.gradle.kts", "build.gradle"]
        .into_iter()
        .find(|name| files.contains(*name))
    else {
        return Vec::new();
    };
    let gradle = gradle_program(files, above);
    let build = read_or_empty(&root.join(build_file));
    let catalog = shared_gradle_file(root, above, &[VERSION_CATALOG]);
    let settings = shared_gradle_file(root, above, &GRADLE_SETTINGS);
    let root_build = match above {
        Some(above) => shared_gradle_file(above.dir, None, &["build.gradle.kts", "build.gradle"]),
        None => String::new(),
    };
    let android_java =
        android_plugin_java(&[build.as_str(), &root_build, &settings, &catalog].join("\n"));
    let pinned = java_from_version_files(root, files).or(android_java);
    if let Some(module) = android_module(&build, &catalog) {
        return with_java(android_tasks(&gradle, module, &build), pinned);
    }
    let java = pinned.or_else(|| java_from_gradle(&build));
    let mut tasks = Vec::new();
    if build.contains("org.springframework.boot") {
        tasks.push(Task::new(
            "detected.gradle.boot-run",
            "Spring Boot: bootRun",
            format!("{gradle} bootRun"),
        ));
    }
    if applies_application(&build) {
        tasks.push(Task::new(
            "detected.gradle.run",
            "Gradle: run",
            format!("{gradle} run"),
        ));
    }
    tasks.push(Task::new(
        "detected.gradle.test",
        "Gradle: test",
        format!("{gradle} test"),
    ));
    tasks.push(Task::new(
        "detected.gradle.build",
        "Gradle: build",
        format!("{gradle} build"),
    ));
    with_java(tasks, java)
}

fn rust_tasks(root: &Path, files: &HashSet<String>) -> Vec<Task> {
    if !files.contains("Cargo.toml") {
        return Vec::new();
    }
    let mut tasks = Vec::new();
    if list(&root.join("src"))
        .iter()
        .any(|entry| entry.name == "main.rs")
    {
        tasks.push(Task::new("detected.cargo.run", "Cargo: run", "cargo run"));
    }
    let binary = regex!(r"^[A-Za-z0-9][A-Za-z0-9_-]*\.rs$");
    for entry in list(&root.join("src/bin")) {
        if entry.is_dir || !binary.is_match(&entry.name) {
            continue;
        }
        let name = entry.name.strip_suffix(".rs").unwrap_or(&entry.name);
        tasks.push(Task::new(
            format!("detected.cargo.run.{name}"),
            format!("Cargo: run {name}"),
            format!("cargo run --bin {name}"),
        ));
    }
    tasks.push(Task::new(
        "detected.cargo.test",
        "Cargo: test",
        format!(
            "sh -c 'set -o pipefail; mkdir -p target && cargo test 2>&1 | tee {CARGO_TEST_LOG}'"
        ),
    ));
    tasks.push(Task::new(
        "detected.cargo.build",
        "Cargo: build",
        "cargo build",
    ));
    tasks
}

fn go_tasks(root: &Path, files: &HashSet<String>) -> Vec<Task> {
    if !files.contains("go.mod") {
        return Vec::new();
    }
    let mut tasks = Vec::new();
    if files.contains("main.go") {
        tasks.push(Task::new("detected.go.run", "Go: run", "go run ."));
    }
    if files.contains("cmd/") {
        for entry in list(&root.join("cmd")) {
            if !entry.is_dir || !FOLDER_NAME.is_match(&entry.name) {
                continue;
            }
            let name = &entry.name;
            tasks.push(Task::new(
                format!("detected.go.run.{name}"),
                format!("Go: run {name}"),
                format!("go run ./cmd/{name}"),
            ));
        }
    }
    tasks.push(Task::new(
        "detected.go.test",
        "Go: test",
        format!(
            "sh -c 'set -o pipefail; mkdir -p .sikemux && echo \"*\" > .sikemux/.gitignore && go test -v ./... 2>&1 | tee {GO_TEST_LOG}'"
        ),
    ));
    tasks.push(Task::new(
        "detected.go.build",
        "Go: build",
        "go build ./...",
    ));
    tasks
}

const RUN_SCRIPTS: [&str; 4] = ["dev", "start", "serve", "preview"];
const MAX_SCRIPTS: usize = 15;

fn package_manager(files: &HashSet<String>, declared: Option<&Value>) -> &'static str {
    let named = match declared {
        Some(Value::String(text)) => text.split('@').next().unwrap_or(""),
        _ => "",
    };
    if let Some(manager) = ["pnpm", "yarn", "bun", "npm"]
        .into_iter()
        .find(|manager| *manager == named)
    {
        return manager;
    }
    if files.contains("pnpm-lock.yaml") {
        "pnpm"
    } else if files.contains("yarn.lock") {
        "yarn"
    } else if files.contains("bun.lock") || files.contains("bun.lockb") {
        "bun"
    } else {
        "npm"
    }
}

/// `npm run` needs `--` before arguments meant for the script; the others pass them straight through.
pub fn script_command(manager: &str, script: &str, args: &str) -> String {
    let base = format!("{manager} run {script}");
    if args.is_empty() {
        base
    } else if manager == "npm" {
        format!("{base} -- {args}")
    } else {
        format!("{base} {args}")
    }
}

fn is_test_script(name: &str) -> bool {
    regex!(r"^test(?-u:\b)").is_match(name)
}

/// The `test` script `npm init` writes, which only fails.
fn is_placeholder_test(name: &str, body: &str) -> bool {
    name == "test" && body.contains("no test specified")
}

fn script_rank(name: &str) -> usize {
    if let Some(rank) = RUN_SCRIPTS.iter().position(|script| *script == name) {
        rank
    } else if is_test_script(name) {
        10
    } else if regex!(r"^build(?-u:\b)").is_match(name) {
        20
    } else {
        30
    }
}

/// Test scripts also write a report Sikemux can read; Vitest runs once instead of watching.
fn reporting_args(body: &str) -> String {
    if regex!(r"(?-u:\b)vitest(?-u:\b)").is_match(body) {
        format!("--run --reporter=default --reporter=junit --outputFile.junit={VITEST_REPORT}")
    } else if regex!(r"(?-u:\b)jest(?-u:\b)").is_match(body) {
        format!("--json --outputFile={JEST_REPORT}")
    } else {
        String::new()
    }
}

/// A manifest's string scripts, in the order it lists them.
fn script_entries(scripts: Option<&Value>) -> Vec<(String, String)> {
    let Some(Value::Object(object)) = scripts else {
        return Vec::new();
    };
    object
        .iter()
        .filter_map(|(name, body)| body.as_str().map(|body| (name.clone(), body.to_owned())))
        .collect()
}

fn node_tasks(root: &Path, files: &HashSet<String>) -> Vec<Task> {
    if !files.contains("package.json") {
        return Vec::new();
    }
    let Ok(manifest) = serde_json::from_str::<Value>(&read_or_empty(&root.join("package.json")))
    else {
        return Vec::new();
    };
    let manager = package_manager(files, manifest.get("packageManager"));
    let mut scripts: Vec<(String, String)> = script_entries(manifest.get("scripts"))
        .into_iter()
        .filter(|(name, _)| FOLDER_NAME.is_match(&name.replace(':', "-")))
        .filter(|(name, body)| !is_placeholder_test(name, body))
        .collect();
    scripts.sort_by_key(|(name, _)| script_rank(name));
    scripts.truncate(MAX_SCRIPTS);
    scripts
        .into_iter()
        .map(|(name, body)| {
            let slug: String = name
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                        character
                    } else {
                        '-'
                    }
                })
                .collect();
            let label = if manager == "npm" {
                format!("npm run {name}")
            } else {
                format!("{manager} {name}")
            };
            let args = if is_test_script(&name) {
                reporting_args(&body)
            } else {
                String::new()
            };
            Task::new(
                format!("detected.node.{slug}"),
                label,
                script_command(manager, &name, &args),
            )
        })
        .collect()
}

const PYTHON_APP_FILES: [&str; 4] = ["main.py", "app.py", "app/main.py", "src/main.py"];

/// The project's own virtual environment first, then the tools that make one, then the system Python.
fn python_runner(files: &HashSet<String>) -> &'static str {
    if files.contains(".venv/") {
        ".venv/bin/python"
    } else if files.contains("venv/") {
        "venv/bin/python"
    } else if files.contains("uv.lock") {
        "uv run python"
    } else if files.contains("poetry.lock") {
        "poetry run python"
    } else {
        "python3"
    }
}

fn python_tasks(root: &Path, files: &HashSet<String>) -> Vec<Task> {
    let manifests: Vec<&str> = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "manage.py",
    ]
    .into_iter()
    .filter(|name| files.contains(*name))
    .collect();
    if manifests.is_empty() {
        return Vec::new();
    }
    let declared = manifests
        .iter()
        .map(|name| read_or_empty(&root.join(name)))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let python = python_runner(files);
    let mut tasks = Vec::new();
    if files.contains("manage.py") {
        tasks.push(Task::new(
            "detected.python.run",
            "Django: runserver",
            format!("{python} manage.py runserver"),
        ));
        tasks.push(Task::new(
            "detected.python.test",
            "Django: test",
            format!("{python} manage.py test"),
        ));
        return tasks;
    }
    if regex!(r"(?-u:\b)fastapi(?-u:\b)").is_match(&declared)
        || regex!(r"(?-u:\b)flask(?-u:\b)").is_match(&declared)
    {
        let app_pattern = regex!(r"(?mR)^([A-Za-z0-9_]+)\s*=\s*(FastAPI|Flask)\(");
        for file in PYTHON_APP_FILES {
            let source = read_or_empty(&root.join(file));
            let Some(app) = app_pattern.captures(&source) else {
                continue;
            };
            let module = file.strip_suffix(".py").unwrap_or(file).replace('/', ".");
            let name = &app[1];
            tasks.push(if &app[2] == "FastAPI" {
                Task::new(
                    "detected.python.run",
                    format!("uvicorn {module}:{name}"),
                    format!("{python} -m uvicorn {module}:{name} --reload"),
                )
            } else {
                Task::new(
                    "detected.python.run",
                    "flask run",
                    format!("{python} -m flask --app {module}:{name} run --debug"),
                )
            });
            break;
        }
    }
    if regex!(r"(?-u:\b)pytest(?-u:\b)").is_match(&declared)
        || files.contains("tests/")
        || files.contains("conftest.py")
        || files.contains("pytest.ini")
    {
        tasks.push(Task::new(
            "detected.python.test",
            "pytest",
            format!("{python} -m pytest --junitxml={PYTEST_REPORT}"),
        ));
    }
    tasks
}

const SKIPPED_FOLDERS: [&str; 6] = ["node_modules", "target", "build", "dist", "out", "vendor"];

fn files_in(dir: &Path) -> HashSet<String> {
    list(dir)
        .into_iter()
        .map(|entry| {
            if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name
            }
        })
        .collect()
}

fn tasks_in(dir: &Path, files: &HashSet<String>, above: Option<&Above>) -> Vec<Task> {
    let mut tasks = maven_tasks(dir, files);
    tasks.extend(gradle_tasks(dir, files, above));
    tasks.extend(go_tasks(dir, files));
    tasks.extend(node_tasks(dir, files));
    tasks.extend(python_tasks(dir, files));
    tasks.extend(rust_tasks(dir, files));
    tasks
}

/// Run, test and build tasks suggested by the build files at the project root and in the folders just inside it.
pub fn detected(project: &Path) -> Vec<Found> {
    let root_files = files_in(project);
    let above = Above {
        dir: project,
        files: &root_files,
    };
    let mut found: Vec<Found> = tasks_in(project, &root_files, None)
        .into_iter()
        .map(|task| Found {
            id: task.id,
            label: task.label,
            command: task.command,
            cwd: project.to_path_buf(),
            java: task.java,
        })
        .collect();
    let folders = list(project).into_iter().filter(|entry| {
        entry.is_dir
            && FOLDER_NAME.is_match(&entry.name)
            && !SKIPPED_FOLDERS.contains(&entry.name.as_str())
    });
    for folder in folders {
        let cwd = project.join(&folder.name);
        found.extend(
            tasks_in(&cwd, &files_in(&cwd), Some(&above))
                .into_iter()
                .map(|task| Found {
                    id: format!("{}:{}", task.id, folder.name),
                    label: format!("{} ({})", task.label, folder.name),
                    command: task.command,
                    cwd: cwd.clone(),
                    java: task.java,
                }),
        );
    }
    found
}

/// The command that tests the project as its root's build files suggest, the
/// same one its test task runs, for a project that names no check.
pub fn test_check(project: &Path) -> Option<String> {
    tasks_in(project, &files_in(project), None)
        .into_iter()
        .find(|task| task.id.ends_with(".test"))
        .map(|task| task.command)
}

/// A command that starts Gradle, installed or the project's own wrapper.
pub fn runs_gradle(command: &str) -> bool {
    let program = command.split(' ').next().unwrap_or("");
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    matches!(name, "gradle" | "gradlew" | "gradlew.bat")
}

pub fn is_run_task(task_id: &str) -> bool {
    let base = task_id.split(':').next().unwrap_or("");
    regex!(r"(?-u:\b)run(?-u:\b)").is_match(base)
        || regex!(r"^detected\.node\.(dev|start|serve|preview)$").is_match(base)
}

pub fn is_test_task(task_id: &str) -> bool {
    regex!(r"\.test(?-u:\b)").is_match(task_id.split(':').next().unwrap_or(""))
}

const BUILD_TOOL_NAMES: [(&str, &str); 10] = [
    ("mvn", "Maven"),
    ("gradle", "Gradle"),
    ("go", "Go"),
    ("npm", "Node.js"),
    ("pnpm", "pnpm"),
    ("yarn", "Yarn"),
    ("bun", "Bun"),
    ("python3", "Python"),
    ("uv", "uv"),
    ("poetry", "Poetry"),
];

/// The installed program a detected task's command starts with, and its name;
/// a project's own `./mvnw` or `./gradlew` needs none.
pub fn build_tool_of(command: &str) -> Option<(&'static str, &'static str)> {
    let wrapped = regex!(r"^sh -c '[^\n\r\u{2028}\u{2029}]*?(?-u:\b)(go|cargo) test(?-u:\b)")
        .captures(command);
    let program = match &wrapped {
        Some(captures) => captures.get(1).map_or("", |group| group.as_str()),
        None => command.split(' ').next().unwrap_or(""),
    };
    BUILD_TOOL_NAMES
        .into_iter()
        .find(|(tool, _)| *tool == program)
}

/// The deck's order inside the found tasks: personal first, then run tasks,
/// then test tasks, then the rest.
pub fn rank(task_id: &str) -> i32 {
    if task_id.starts_with(PERSONAL_PREFIX) {
        -1
    } else if is_run_task(task_id) {
        0
    } else if is_test_task(task_id) {
        1
    } else {
        2
    }
}

pub fn task_id_for(label: &str) -> String {
    let lower = label.to_lowercase();
    let replaced = regex!(r"[^a-z0-9._-]+").replace_all(&lower, "-");
    let slug: String = replaced.trim_matches('-').chars().take(60).collect();
    if slug.is_empty() {
        "task".to_owned()
    } else {
        slug
    }
}

fn read_object(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let Some(text) = read_text(path) else {
        return Ok(None);
    };
    match serde_json::from_str(&text).map_err(|error| error.to_string())? {
        Value::Object(object) => Ok(Some(object)),
        _ => Err(format!("{} is not a JSON object", path.display())),
    }
}

fn joined(project: &Path, part: &str) -> PathBuf {
    let part = part.replace('\\', "/");
    let part = part.trim_matches('/');
    if part.is_empty() {
        project.to_path_buf()
    } else {
        project.join(part)
    }
}

fn absolute_cwd(project: &Path, cwd: &str) -> PathBuf {
    if cwd == "." {
        project.to_path_buf()
    } else {
        joined(project, cwd)
    }
}

pub fn personal(project: &Path) -> Vec<Found> {
    let Ok(Some(stored)) = read_object(&project.join(PERSONAL_TASKS)) else {
        return Vec::new();
    };
    let Some(Value::Array(tasks)) = stored.get("tasks") else {
        return Vec::new();
    };
    tasks
        .iter()
        .filter_map(|task| {
            let field = |name: &str| task.get(name).and_then(Value::as_str);
            Some(Found {
                id: format!("{PERSONAL_PREFIX}{}", field("id")?),
                label: field("label")?.to_owned(),
                command: field("command")?.to_owned(),
                cwd: absolute_cwd(project, field("cwd")?),
                java: None,
            })
        })
        .collect()
}

/// Every task already there stays as it was, except one with the same id, which the new one replaces.
fn with_task(existing: Option<&Value>, id: &str, task: Value) -> Value {
    let mut tasks: Vec<Value> = match existing {
        Some(Value::Array(tasks)) => tasks
            .iter()
            .filter(|candidate| candidate.get("id").and_then(Value::as_str) != Some(id))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    tasks.push(task);
    Value::Array(tasks)
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    sikemux_process::files::write_atomic(path, content.as_bytes(), false)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn write_json(path: &Path, object: Map<String, Value>) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(&Value::Object(object)).map_err(|error| error.to_string())?;
    write(path, &format!("{text}\n"))
}

/// Saves a task for this person only; returns its task id.
pub fn save_personal(project: &Path, task: &AddedTask) -> Result<String, String> {
    let folder = project.join(".sikemux");
    if !folder.is_dir() {
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    write(&folder.join(".gitignore"), "*\n")?;
    let path = project.join(PERSONAL_TASKS);
    let mut stored = read_object(&path)?.unwrap_or_default();
    let id = task_id_for(&task.label);
    let entry = json!({ "id": id, "label": task.label, "command": task.command, "cwd": task.cwd });
    let tasks = with_task(stored.get("tasks"), &id, entry);
    stored.insert("tasks".to_owned(), tasks);
    write_json(&path, stored)?;
    Ok(format!("{PERSONAL_PREFIX}{id}"))
}

/// Adds a task to the project's sikemux.json, keeping everything else in it; returns its task id.
pub fn save_team(project: &Path, task: &AddedTask) -> Result<String, String> {
    let path = project.join(CONFIG_FILE);
    let mut config = read_object(&path)?.unwrap_or_else(|| {
        let mut config = Map::new();
        config.insert("version".to_owned(), json!(1));
        config
    });
    let id = task_id_for(&task.label);
    let mut entry = json!({ "id": id, "label": task.label, "command": task.command });
    if task.cwd != "." {
        entry["cwd"] = json!(task.cwd);
    }
    let tasks = with_task(config.get("tasks"), &id, entry);
    config.insert("tasks".to_owned(), tasks);
    write_json(&path, config)?;
    Ok(id)
}

/// Names the project's check command in `sikemux.json`, or takes it out when
/// `command` is empty, leaving everything else in the file as it was.
pub fn save_check(project: &Path, command: &str) -> Result<(), String> {
    let path = project.join(CONFIG_FILE);
    let mut config = read_object(&path)?.unwrap_or_else(|| {
        let mut config = Map::new();
        config.insert("version".to_owned(), json!(1));
        config
    });
    match command.trim() {
        "" => config.remove("check"),
        command => config.insert("check".to_owned(), json!(command)),
    };
    write_json(&path, config)
}

/// Takes a task the person added out of `sikemux.json` or `.sikemux/tasks.json`,
/// leaving everything else in the file as it was.
pub fn remove_saved(project: &Path, task_id: &str) -> Result<(), String> {
    if task_id.starts_with(DETECTED_PREFIX) {
        return Err("a process found in the build files cannot be removed".into());
    }
    let (path, id) = match task_id.strip_prefix(PERSONAL_PREFIX) {
        Some(id) => (project.join(PERSONAL_TASKS), id),
        None => (project.join(CONFIG_FILE), task_id),
    };
    let Some(mut stored) = read_object(&path)? else {
        return Err(format!("{} has no processes", path.display()));
    };
    let Some(Value::Array(tasks)) = stored.get_mut("tasks") else {
        return Err(format!("{} has no processes", path.display()));
    };
    let before = tasks.len();
    tasks.retain(|task| task.get("id").and_then(Value::as_str) != Some(id));
    if tasks.len() == before {
        return Err(format!("{} has no process {id}", path.display()));
    }
    write_json(&path, stored)
}
