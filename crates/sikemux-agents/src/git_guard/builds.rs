//! A build an agent runs waits for its turn on the host, which the core keeps
//! while the shim's connection to it stays open.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::client::{read_endpoint, Session};
use crate::cli::protocol::{BuildRequest, CliClientCommand, CliServerResponse};
use sikemux_process::sync::Locked;
use sikemux_process::vars;

use super::footer;

/// The build tools besides cargo the shim stands in for, so their builds take turns too.
pub(crate) const PROGRAMS: &[&str] = &["xcodebuild", "gradle", "gradlew", "swift"];
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
const COMPILING: &[&str] = &[
    "b", "build", "c", "check", "clippy", "d", "doc", "fix", "install", "rustc", "rustdoc",
];
const COMPILING_THEN_RUNNING: &[&str] = &["bench", "r", "run", "t", "test"];
const GLOBAL_OPTIONS_WITH_VALUE: &[&str] = &["--color", "--config", "--explain", "-C", "-Z"];
const BUILD_STEPS: &[&str] = &[
    "ar",
    "bash",
    "clang",
    "clang++",
    "clippy-driver",
    "git",
    "kache",
    "ld",
    "libtool",
    "rustc",
    "rustdoc",
    "sandbox-exec",
    "sccache",
    "sh",
    "xcrun",
];
/// Compiler caches a build hands rustc to, the first one installed.
const CACHES: &[&str] = &["kache", "sccache"];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LOOK_EVERY: Duration = Duration::from_secs(1);

pub(super) enum Build {
    Compiles,
    CompilesThenRuns,
}

/// What `program words` does, when it builds.
pub(super) fn of_program(program: &str, words: &[String]) -> Option<Build> {
    let asking = || words.iter().any(|word| ASKING.contains(&word.as_str()));
    match program {
        "cargo" => of_cargo(words),
        "xcodebuild" if !asking() && !xcode_tests(words) => Some(Build::Compiles),
        "gradle" | "gradlew" if !asking() && !gradle_runs(words) => Some(Build::Compiles),
        "swift" => match words.first().map(String::as_str) {
            Some("build") => Some(Build::Compiles),
            Some("test" | "run") => Some(Build::CompilesThenRuns),
            _ => None,
        },
        _ => None,
    }
}

/// Tests run on a simulator, outside the build's own processes, so their turn could
/// not end once compiling is done; they wait for none.
fn xcode_tests(words: &[String]) -> bool {
    words
        .iter()
        .any(|word| word == "test" || word == "test-without-building")
}

/// Whether gradle runs the app or keeps watching, either of which lasts until it is stopped.
fn gradle_runs(words: &[String]) -> bool {
    words.iter().any(|word| {
        let task = word.rsplit(':').next().unwrap_or(word);
        word == "-t" || word == "--continuous" || task == "run" || task.ends_with("Run")
    })
}

fn of_cargo(words: &[String]) -> Option<Build> {
    if words.iter().any(|word| word == "-h" || word == "--help") {
        return None;
    }
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if GLOBAL_OPTIONS_WITH_VALUE.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with(['-', '+']) {
            return match word.as_str() {
                word if COMPILING.contains(&word) => Some(Build::Compiles),
                word if COMPILING_THEN_RUNNING.contains(&word) => Some(Build::CompilesThenRuns),
                _ => None,
            };
        }
    }
    None
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

/// Runs `program args`, found at `real`, once the host has room for one more
/// build, and answers with its exit code.
pub(super) fn run(
    program: &str,
    real: &Path,
    args: &[OsString],
    words: &[String],
    build: Build,
) -> i32 {
    let what = std::iter::once(program.to_owned())
        .chain(words.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let nested = std::env::var_os(vars::BUILD_TURN).is_some();
    let turn = Arc::new(Mutex::new(
        (!nested).then(|| take_turn(program, what)).flatten(),
    ));
    let mut command = sikemux_process::user_environment::command(real);
    command.args(args).env(vars::BUILD_TURN, "1");
    if program == "cargo" {
        command.envs(rustc_cache()).env("PATH", path_with(real));
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{program}: {error}");
            return 126;
        }
    };
    if matches!(build, Build::CompilesThenRuns) && turn.locked().is_some() {
        let root = child.id();
        let turn = turn.clone();
        let tool = program.to_owned();
        std::thread::spawn(move || {
            while turn.locked().is_some() {
                std::thread::sleep(LOOK_EVERY);
                if program_started(root, &tool) {
                    turn.locked().take();
                }
            }
        });
    }
    let status = child.wait();
    turn.locked().take();
    match status {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!("{program}: {error}");
            1
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or_default())
}

/// Waits for the core to give the build a turn, saying so while it waits. None
/// when no core answers, and the build runs at once.
fn take_turn(program: &str, command: String) -> Option<Session> {
    let descriptor = read_endpoint(&footer::endpoint()?).ok()?;
    let mut session = Session::open(&descriptor, CONNECT_TIMEOUT, CONNECT_TIMEOUT).ok()?;
    session.wait_for_answers(None).ok()?;
    let request = BuildRequest {
        agent_id: std::env::var(vars::AGENT_ID).ok(),
        command,
        pid: Some(std::process::id()),
    };
    session
        .send(&CliClientCommand::Build {
            protocol: descriptor.protocol,
            token: descriptor.token.clone(),
            request,
        })
        .ok()?;
    let since = Instant::now();
    let mut queued = false;
    loop {
        match session.receive().ok()? {
            CliServerResponse::Queued {
                ahead,
                building,
                limit,
            } => {
                queued = true;
                eprintln!("{program}: {}", queued_line(ahead, building, limit));
            }
            CliServerResponse::Building => {
                if queued {
                    let waited = since.elapsed().as_secs();
                    eprintln!("{program}: started after {waited}s in the queue.");
                }
                return Some(session);
            }
            _ => return None,
        }
    }
}

fn queued_line(ahead: usize, building: usize, limit: usize) -> String {
    let behind = match ahead {
        0 => String::new(),
        ahead => format!(" behind {ahead} more"),
    };
    let running = match building {
        1 => "1 build is".to_owned(),
        building => format!("{building} builds are"),
    };
    format!(
        "queued{behind}: {running} running, as many as this machine takes at once ({limit}). It starts by itself when one finishes; Settings → Chats → Builds at once sets how many."
    )
}

/// Whether a program `tool` built is running under the `tool` started as
/// `root`, such as a test binary, so its compiling is over.
fn program_started(root: u32, tool: &str) -> bool {
    let below = sikemux_process::procs::descendants(root);
    let names: HashMap<u32, String> = below
        .keys()
        .filter_map(|pid| Some((*pid, sikemux_process::procs::name(*pid)?)))
        .collect();
    let is_tool =
        |pid: &u32| *pid == root || names.get(pid).is_some_and(|name| name.starts_with(tool));
    below.iter().any(|(pid, parent)| {
        is_tool(parent)
            && names.get(pid).is_some_and(|name| {
                !name.starts_with(tool)
                    && !name.starts_with("build-script")
                    && !BUILD_STEPS.contains(&name.as_str())
            })
    })
}
