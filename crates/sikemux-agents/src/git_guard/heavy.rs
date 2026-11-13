//! Which commands are heavy, such as a build or a test suite: the built-in list and the
//! project's own, which wait for their turn on the host.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::tasks::config::HeavyCommand;
use sikemux_process::vars;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Waits for its turn from its first run.
    Queued,
    /// Runs at once while the core measures it, and waits in a project only once it was
    /// seen to take a lot of memory there.
    Watched,
}

/// A command the line knows without being told.
struct BuiltIn {
    /// The program and its first words.
    command: &'static str,
    kind: Kind,
    /// It builds a program and then runs it, and gives its turn up once that starts.
    then_runs: bool,
    /// Words that keep it out of the line wherever they stand. `*x` is any word ending in x.
    unless: &'static [&'static [&'static str]],
    /// The program's options that take the next word as their value.
    valued: &'static [&'static str],
}

/// What `xcodebuild` and `gradle` are asked that only reads, and builds nothing.
const ASKING: &[&str] = &[
    "-h",
    "-help",
    "--help",
    "-v",
    "-version",
    "--version",
    "-list",
    "-showsdks",
    "-showBuildSettings",
    "-showdestinations",
    "-showTestPlans",
    "--status",
    "--stop",
];
const CARGO_HELP: &[&str] = &["-h", "--help"];
const CARGO_VALUED: &[&str] = &["--color", "--config", "--explain", "-C", "-Z"];
/// Tests run on a simulator, outside the build's own processes, so their turn could not end
/// once compiling is done.
const XCODE_TESTS: &[&str] = &["test", "test-without-building"];
/// Gradle running the app or watching, either of which lasts until it is stopped.
const GRADLE_RUNS: &[&str] = &["-t", "--continuous", "run", "*:run", "*Run"];

const fn cargo(command: &'static str, then_runs: bool) -> BuiltIn {
    BuiltIn {
        command,
        kind: Kind::Queued,
        then_runs,
        unless: &[CARGO_HELP],
        valued: CARGO_VALUED,
    }
}

const fn plain(command: &'static str, kind: Kind, then_runs: bool) -> BuiltIn {
    BuiltIn {
        command,
        kind,
        then_runs,
        unless: &[],
        valued: &[],
    }
}

const BUILT_IN: &[BuiltIn] = &[
    cargo("cargo build", false),
    cargo("cargo b", false),
    cargo("cargo check", false),
    cargo("cargo c", false),
    cargo("cargo clippy", false),
    cargo("cargo doc", false),
    cargo("cargo d", false),
    cargo("cargo fix", false),
    cargo("cargo install", false),
    cargo("cargo rustc", false),
    cargo("cargo rustdoc", false),
    cargo("cargo run", true),
    cargo("cargo r", true),
    cargo("cargo test", true),
    cargo("cargo t", true),
    cargo("cargo bench", true),
    BuiltIn {
        command: "xcodebuild",
        kind: Kind::Queued,
        then_runs: false,
        unless: &[ASKING, XCODE_TESTS],
        valued: &[],
    },
    BuiltIn {
        command: "gradle",
        kind: Kind::Queued,
        then_runs: false,
        unless: &[ASKING, GRADLE_RUNS],
        valued: &[],
    },
    BuiltIn {
        command: "gradlew",
        kind: Kind::Queued,
        then_runs: false,
        unless: &[ASKING, GRADLE_RUNS],
        valued: &[],
    },
    plain("swift build", Kind::Queued, false),
    plain("swift test", Kind::Queued, true),
    plain("swift run", Kind::Queued, true),
    plain("go build", Kind::Watched, false),
    plain("go test", Kind::Watched, false),
    plain("go vet", Kind::Watched, false),
    plain("mvn", Kind::Watched, false),
    plain("tsc", Kind::Watched, false),
    plain("vitest", Kind::Watched, false),
    plain("jest", Kind::Watched, false),
    plain("pytest", Kind::Watched, false),
];

/// Programs a project's `heavy` list may not have the shim stand in for, since nearly every
/// command runs them.
const NEVER_STOOD_IN: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "env", "exec", "sudo", "nice", "nohup", "time",
];

/// Compiler caches a build hands rustc to, the first one installed.
const CACHES: &[&str] = &["kache", "sccache"];

/// The programs of the built-in list, which the shim always stands in for.
pub(crate) fn built_in_programs() -> impl Iterator<Item = &'static str> {
    let mut seen = Vec::new();
    BUILT_IN.iter().filter_map(move |built_in| {
        let program = program_of(built_in.command);
        (!seen.contains(&program)).then(|| {
            seen.push(program);
            program
        })
    })
}

/// The programs `heavy` names beyond the built-in list, which the shim may stand in for.
pub(crate) fn listed_programs(heavy: &[HeavyCommand]) -> Vec<String> {
    let mut programs: Vec<String> = Vec::new();
    for entry in heavy.iter().filter(|entry| !entry.light) {
        let program = program_of(&entry.command);
        let plain = !program.contains('/') && !program.is_empty();
        if plain
            && !NEVER_STOOD_IN.contains(&program)
            && !built_in_programs().any(|known| known == program)
            && !programs.iter().any(|known| known == program)
        {
            programs.push(program.to_owned());
        }
    }
    programs
}

fn program_of(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or_default()
}

/// A heavy command, as the shim found it.
pub(super) struct Heavy {
    /// The command it matched, which the core measures it by.
    pub(super) key: String,
    pub(super) before_commit: bool,
    pub(super) watched: bool,
    pub(super) then_runs: bool,
}

/// Whether `program words`, run in `cwd`, is heavy: in the project's `heavy` list there, or in
/// the built-in one unless the project keeps it out.
pub(super) fn of_program(program: &str, words: &[String], cwd: &Path) -> Option<Heavy> {
    let valued = BUILT_IN
        .iter()
        .find(|built_in| program_of(built_in.command) == program)
        .map_or(&[][..], |built_in| built_in.valued);
    let listed = crate::tasks::config::heavy_near(cwd);
    let matching = |command: &str| matches(command, program, words, valued);
    let project = listed.as_deref().unwrap_or_default();
    if project
        .iter()
        .any(|entry| entry.light && matching(&entry.command))
    {
        return None;
    }
    if listed.as_ref().is_some_and(Vec::is_empty) {
        return None;
    }
    let entry = project
        .iter()
        .filter(|entry| !entry.light && matching(&entry.command))
        .max_by_key(|entry| entry.command.split_whitespace().count());
    let built_in = BUILT_IN.iter().find(|built_in| {
        matching(built_in.command) && !built_in.unless.iter().any(|unless| has_any(words, unless))
    });
    let key = match (entry, built_in) {
        (Some(entry), _) => entry.command.clone(),
        (None, Some(built_in)) => built_in.command.to_owned(),
        (None, None) => return None,
    };
    let before_commit = match std::env::var(vars::PRIORITY).as_deref() {
        Ok("final") => true,
        Ok("normal") => false,
        _ => entry.is_some_and(|entry| entry.before_commit),
    };
    Some(Heavy {
        key,
        before_commit,
        watched: entry.is_none() && built_in.is_some_and(|built_in| built_in.kind == Kind::Watched),
        then_runs: built_in.is_some_and(|built_in| built_in.then_runs),
    })
}

/// Whether `command`, a program and its first words, is what `program words` runs: the same
/// program, then the same first words past its options.
fn matches(command: &str, program: &str, words: &[String], valued: &[&str]) -> bool {
    let mut wanted = command.split_whitespace();
    if wanted.next() != Some(program) {
        return false;
    }
    let mut given = leading_words(words, valued);
    wanted.all(|word| given.next() == Some(word))
}

/// The words of a command that are not its options or their values.
fn leading_words<'a>(words: &'a [String], valued: &'a [&str]) -> impl Iterator<Item = &'a str> {
    let mut words = words.iter();
    std::iter::from_fn(move || loop {
        let word = words.next()?;
        if valued.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with(['-', '+']) {
            return Some(word.as_str());
        }
    })
}

fn has_any(words: &[String], patterns: &[&str]) -> bool {
    words.iter().any(|word| {
        patterns
            .iter()
            .any(|pattern| match pattern.strip_prefix('*') {
                Some(end) => word.ends_with(end),
                None => word == pattern,
            })
    })
}

/// Rustup's cargo when the project in `cwd` pins its Rust, since another cargo
/// first on PATH, such as Homebrew's, would not follow the pin.
pub(super) fn rustup_cargo(cwd: &Path) -> Option<PathBuf> {
    let pinned = cwd.ancestors().any(|dir| {
        ["rust-toolchain.toml", "rust-toolchain"]
            .iter()
            .any(|file| dir.join(file).is_file())
    });
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| sikemux_process::paths::home().map(|home| home.join(".cargo")))?;
    let cargo = home.join("bin").join("cargo");
    (pinned && cargo.is_file()).then_some(cargo)
}

/// PATH with `cargo`'s folder first, so the rustc and tools it calls are its own
/// toolchain's rather than another Rust's that comes first, such as Homebrew's.
pub(super) fn path_with(cargo: &Path) -> OsString {
    let folders = cargo.parent().map(Path::to_path_buf).into_iter().chain(
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .collect::<Vec<_>>(),
    );
    std::env::join_paths(folders).unwrap_or_default()
}

/// The compiler cache to hand rustc to, unless the build names its own.
pub(crate) fn rustc_cache() -> Option<(&'static str, PathBuf)> {
    if std::env::var_os("RUSTC_WRAPPER").is_some()
        || sikemux_process::user_environment::var("RUSTC_WRAPPER").is_some()
    {
        return None;
    }
    CACHES.iter().find_map(|cache| {
        sikemux_process::user_environment::find_executables(cache)
            .into_iter()
            .next()
            .map(|path| ("RUSTC_WRAPPER", path))
    })
}
