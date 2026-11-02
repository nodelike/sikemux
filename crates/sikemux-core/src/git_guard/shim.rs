//! The `git` agents Sikemux starts find first on their PATH: this binary,
//! linked under that name. It refuses what [`super`] refuses and runs the real
//! git for everything else, whichever harness or shell the agent runs it from.
//!
//! Grok and Hermes rebuild PATH from the person's login profile, which puts
//! the system's git first, so they also check each shell command before it
//! runs: this binary again, linked as the hook their harnesses call.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::vars;

use super::footer;

/// The folder of a core's data folder agents find this shim in first.
const AGENT_BIN: &str = "agent-bin";
/// The name the hook is linked under, outside the agents' PATH.
const HOOK: &str = "sikemux-git-hook";
/// Given to the hook where the shim is first on PATH, so it checks only what
/// leaves this machine past the shim.
const SHIP_ONLY: &str = "ship";
/// macOS has no `timeout`, which agents reach for out of habit.
const TIMEOUT: &str = "timeout";

static FOLDER: OnceLock<PathBuf> = OnceLock::new();
static HOOKS_FOLDER: OnceLock<PathBuf> = OnceLock::new();
static ENDPOINT: OnceLock<PathBuf> = OnceLock::new();
static GROK_PLUGIN: OnceLock<PathBuf> = OnceLock::new();
static HOOK_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Links this binary as `git` in a folder of `data_dir`, for agents started
/// from now on to find first, as the hook, with the plugin Grok chats load it
/// from, and as what puts the footer on their commits, which asks the core at
/// `endpoint`.
pub(crate) fn install(data_dir: &Path, endpoint: Option<&Path>) {
    let folder = data_dir.join(AGENT_BIN);
    if folder.to_string_lossy().contains(':') {
        return;
    }
    if let Err(error) = link(&folder, TIMEOUT) {
        eprintln!("sikemux core: agents have no timeout command: {error}");
    }
    crate::agent_shell::install(data_dir);
    match link(&folder, "git") {
        Ok(()) => {
            for program in crate::ship::PROGRAMS {
                if let Err(error) = link(&folder, program) {
                    eprintln!("sikemux core: agents' {program} commands go unchecked: {error}");
                }
            }
            let _ = FOLDER.set(folder);
        }
        Err(error) => eprintln!("sikemux core: agents' git commands go unchecked: {error}"),
    }
    match install_hook(data_dir) {
        Ok(plugin) => {
            let _ = GROK_PLUGIN.set(plugin);
            let _ = HOOK_DATA_DIR.set(data_dir.to_path_buf());
        }
        Err(error) => eprintln!("sikemux core: Grok's and Hermes's git go unchecked: {error}"),
    }
    let hooks = footer::folder(data_dir);
    match link(&hooks, footer::LINK) {
        Ok(()) => {
            let _ = HOOKS_FOLDER.set(hooks);
            if let Some(endpoint) = endpoint {
                let _ = ENDPOINT.set(endpoint.to_path_buf());
            }
        }
        Err(error) => eprintln!("sikemux core: agents' commits go without a footer: {error}"),
    }
}

fn link(folder: &Path, name: &str) -> std::io::Result<()> {
    let binary = std::env::current_exe()?;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(name);
    if std::fs::read_link(&path).is_ok_and(|target| target == binary) {
        return Ok(());
    }
    let fresh = folder.join(format!("{name}.{}", std::process::id()));
    let _ = std::fs::remove_file(&fresh);
    std::os::unix::fs::symlink(&binary, &fresh)?;
    std::fs::rename(&fresh, &path)
}

fn hook_folder(data_dir: &Path) -> PathBuf {
    data_dir.join("agent-hook")
}

fn install_hook(data_dir: &Path) -> std::io::Result<PathBuf> {
    let folder = hook_folder(data_dir);
    link(&folder, HOOK)?;
    let plugin = folder.join("sikemux");
    let hooks = json!({ "hooks": grok_hooks(&quoted(&folder.join(HOOK))) });
    sikemux_process::files::write_atomic(
        &plugin.join("hooks").join("hooks.json"),
        hooks.to_string().as_bytes(),
        false,
    )?;
    Ok(plugin)
}

/// The data folder the hook was linked in, and the command a harness runs it
/// with. None before [`install`].
pub(super) fn hook() -> Option<(&'static Path, String)> {
    let data_dir = HOOK_DATA_DIR.get()?;
    Some((data_dir, quoted(&hook_folder(data_dir).join(HOOK))))
}

/// Grok's `hooks` table with the hook on every shell command.
pub(super) fn grok_hooks(command: &str) -> Value {
    json!({
        "PreToolUse": [{
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": command }],
        }],
    })
}

/// The settings a Claude Code chat starts with to check each shell command for
/// what leaves this machine, which a program it runs by its full path would
/// get past the shim with. None before [`install`].
pub(crate) fn claude_settings() -> Option<String> {
    let (_, command) = hook()?;
    Some(json!({ "hooks": grok_hooks(&format!("{command} {SHIP_ONLY}")) }).to_string())
}

/// The plugin a Grok chat loads the hook from. None before [`install`].
pub(crate) fn grok_plugin() -> Option<&'static Path> {
    GROK_PLUGIN.get().map(PathBuf::as_path)
}

/// `path` as one word for `sh -c`, and for Hermes, which splits a hook's
/// command the way a shell would.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// The variables an agent runs with to find the shim first: `path`, or the
/// person's PATH, with the shim's folder ahead of it, and the real git. None
/// before [`install`].
pub(crate) fn environment(path: Option<&str>) -> Vec<(String, String)> {
    let Some(folder) = FOLDER.get() else {
        return Vec::new();
    };
    let folder = folder.to_string_lossy();
    let path = path
        .map(str::to_owned)
        .or_else(|| sikemux_process::user_environment::var("PATH"));
    let path = match path.filter(|path| !path.is_empty()) {
        Some(path) => format!("{folder}:{path}"),
        None => folder.into_owned(),
    };
    let mut env = vec![("PATH".to_owned(), path)];
    env.extend(crate::agent_shell::environment());
    if let Some(git) = sikemux_process::user_environment::git_program() {
        env.push((
            vars::REAL_GIT.to_owned(),
            git.to_string_lossy().into_owned(),
        ));
    }
    if let Some(hooks) = HOOKS_FOLDER.get() {
        env.push((
            footer::HOOKS.to_owned(),
            hooks.to_string_lossy().into_owned(),
        ));
    }
    if let Some(endpoint) = ENDPOINT.get() {
        env.push((
            vars::CLI_ENDPOINT.to_owned(),
            endpoint.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// Has `command`, a `git commit` the core runs in `dir`, put the footer on
/// through the hooks an agent's own commit runs. Nothing before [`install`].
pub(crate) fn sign_commit(command: &mut Command, dir: &Path) {
    let (Some(hooks), Some(git)) = (
        HOOKS_FOLDER.get(),
        sikemux_process::user_environment::git_program(),
    ) else {
        return;
    };
    let options = ["-C".to_owned(), dir.to_string_lossy().into_owned()];
    footer::prepare(command, hooks, &git, &options, &[]);
    if let Some(endpoint) = ENDPOINT.get() {
        command.env(vars::CLI_ENDPOINT, endpoint);
    }
}

/// `path` without the folder agents find the shim in, which a process started
/// from an agent's shell inherits. None when it has none.
pub(crate) fn path_without_shim(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split(':').collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !is_shim_folder(Path::new(part)))
        .collect();
    (kept.len() != parts.len()).then(|| kept.join(":"))
}

fn is_shim_folder(folder: &Path) -> bool {
    folder.file_name().is_some_and(|name| name == AGENT_BIN) && folder.join("git").is_symlink()
}

/// The shim's folder, for a shell to put first on PATH again after its
/// profile has run.
pub(crate) fn folder() -> Option<&'static Path> {
    FOLDER.get().map(PathBuf::as_path)
}

/// What `provider` is started with for its commands to find the shim. Codex
/// runs them in a login shell, whose profile puts the system's git ahead of
/// it, unless told not to.
pub(crate) fn arguments(provider: &str) -> &'static [&'static str] {
    match crate::agent_catalog::agent(provider) {
        Some(agent) if FOLDER.get().is_some() => agent.git_shim_args,
        _ => &[],
    }
}

/// Runs as `git` or as the hook when this binary was started under either
/// name, and answers with its exit code. None under any other name.
pub fn run_if_git() -> Option<i32> {
    let mut args = std::env::args_os();
    let name = args.next()?;
    match Path::new(&name).file_name()?.to_str()? {
        "git" => Some(run(args.collect())),
        HOOK => Some(run_hook(args.next().is_some_and(|arg| arg == SHIP_ONLY))),
        TIMEOUT => Some(crate::timeout::run(args.collect())),
        footer::LINK => Some(footer::run(args.collect())),
        program => {
            let program = crate::ship::PROGRAMS
                .iter()
                .find(|known| **known == program)?;
            Some(run_program(program, args.collect()))
        }
    }
}

/// Checks the shell command in the tool call a harness writes to stdin, in
/// Claude's hook form, which Grok and Hermes share: for what the git guard
/// refuses unless `ship_only`, and for what leaves this machine. Exit code 2
/// refuses it, with the reason on stderr.
fn run_hook(ship_only: bool) -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_str::<Value>(&input) else {
        return 0;
    };
    let tool = &event["tool_input"];
    let (Some(command), Some(cwd)) = (tool["command"].as_str(), event["cwd"].as_str()) else {
        return 0;
    };
    let cwd = ["workdir", "cwd"]
        .iter()
        .find_map(|key| tool[*key].as_str())
        .map_or_else(|| PathBuf::from(cwd), |dir| Path::new(cwd).join(dir));
    if let Some(refusal) = (!ship_only)
        .then(|| super::refusal(command, &cwd, &others))
        .flatten()
    {
        eprintln!("{refusal}");
        return 2;
    }
    let extra = project_ships(&cwd);
    let looks = match ship_only {
        true => crate::ship::Looks::PastTheShim,
        false => crate::ship::Looks::Every,
    };
    match crate::ship::of_command(command, &extra, looks).and_then(|ship| held(&ship, &cwd)) {
        Some(held) => {
            eprintln!("{held}");
            2
        }
        None => 0,
    }
}

/// The commands the project `cwd` is in lists in its sikemux.json as leaving
/// this machine, such as `make deploy`.
fn project_ships(cwd: &Path) -> Vec<String> {
    let project = cwd
        .ancestors()
        .find(|dir| dir.join(crate::tasks::config::CONFIG_FILE).is_file());
    match project.map(crate::tasks::config::load) {
        Some(crate::tasks::config::ConfigLoad::Valid { config, .. }) => config.ship,
        _ => Vec::new(),
    }
}

/// What the agent is told when the core holds `ship`, run in `cwd`, for the
/// person to allow first. None when it may run now.
fn held(ship: &crate::ship::Ship, cwd: &Path) -> Option<String> {
    let endpoint = footer::endpoint()?;
    let asked = crate::cli::client::attempt(
        &endpoint,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::SHIP_CHECK_METHOD,
        &json!({ "kind": ship.kind, "what": ship.what }),
        GUARD_WAIT,
    );
    match asked {
        Ok(answer) if answer["allowed"] == true => None,
        Ok(answer) => Some(answer["said"].as_str().unwrap_or_default().to_owned()),
        Err(_) => Some(format!(
            "Sikemux could not ask whether `{}` may leave this machine. Run it again in a moment.",
            ship.what
        )),
    }
}

/// Runs `program`, which the shim stands in for, unless what it does leaves
/// this machine and the core holds it for the person.
fn run_program(program: &str, args: Vec<OsString>) -> i32 {
    let words: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if let Some(kind) = crate::ship::of_program(program, &words, &|_| None) {
        let ship = crate::ship::Ship {
            kind,
            what: std::iter::once(program.to_owned())
                .chain(words)
                .collect::<Vec<_>>()
                .join(" "),
        };
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(held) = held(&ship, &cwd) {
            eprintln!("{held}");
            return 1;
        }
    }
    let Some(real) = real(program) else {
        eprintln!("{program}: Sikemux found no {program} to run on PATH");
        return 127;
    };
    let error = sikemux_process::user_environment::command(&real)
        .args(&args)
        .exec();
    eprintln!("{program}: {}: {error}", real.display());
    126
}

fn run(args: Vec<OsString>) -> i32 {
    let Some(git) = real_git() else {
        eprintln!("git: Sikemux found no git to run on PATH");
        return 127;
    };
    let words: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let private_index = std::env::var_os("GIT_INDEX_FILE").is_some();
    let real = || sikemux_process::user_environment::command(&git);
    if let Some(refusal) = super::refused(&words, private_index, &real, &others) {
        eprintln!("{refusal}");
        return 1;
    }
    let alias = |name: &str| alias_of(&git, name);
    if let Some(kind) = crate::ship::of_program("git", &words, &alias) {
        let ship = crate::ship::Ship {
            kind,
            what: std::iter::once("git".to_owned())
                .chain(words.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
        };
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(held) = held(&ship, &cwd) {
            eprintln!("{held}");
            return 1;
        }
    }
    let mut command = sikemux_process::user_environment::command(&git);
    let subcommand = super::git_args(&words);
    match subcommand.first().map(String::as_str) {
        Some("commit" | "merge") => {
            if let Some(folder) = std::env::var_os(footer::HOOKS) {
                let options = &words[..words.len() - subcommand.len()];
                footer::prepare(
                    &mut command,
                    Path::new(&folder),
                    &git,
                    options,
                    &subcommand[1..],
                );
            }
        }
        Some("rebase") => footer::rewrite_notes(&mut command),
        _ => {}
    }
    let error = command.args(&args).exec();
    eprintln!("git: {}: {error}", git.display());
    126
}

/// The other agents whose work a git command in `dir` would touch, as the
/// core that started this agent knows them.
fn others(dir: &Path, paths: &[String]) -> Option<Vec<String>> {
    let answer = crate::cli::client::attempt(
        &footer::endpoint()?,
        &dir.to_string_lossy(),
        None,
        crate::cli::protocol::GIT_GUARD_METHOD,
        &json!({ "paths": paths }),
        GUARD_WAIT,
    )
    .ok()?;
    serde_json::from_value(answer.get("others")?.clone()).ok()
}

const GUARD_WAIT: Duration = Duration::from_secs(10);

/// What the git alias `name` stands for, as git in this folder reads it.
fn alias_of(git: &Path, name: &str) -> Option<String> {
    let output = sikemux_process::user_environment::command(git)
        .args(["config", "--get", &format!("alias.{name}")])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether `path` is this shim rather than the program it stands in for.
fn is_shim(path: &Path) -> bool {
    static SHIM: OnceLock<Option<PathBuf>> = OnceLock::new();
    let shim = SHIM.get_or_init(|| {
        std::env::current_exe()
            .and_then(|exe| exe.canonicalize())
            .ok()
    });
    path.canonicalize().ok() == *shim
}

/// The program `name` this shim stands in for, the first on PATH that is not
/// the shim itself.
fn real(name: &str) -> Option<PathBuf> {
    sikemux_process::user_environment::find_executables(name)
        .into_iter()
        .find(|found| !is_shim(found))
}

/// The git this shim stands in for, never the shim itself.
fn real_git() -> Option<PathBuf> {
    std::env::var_os(vars::REAL_GIT)
        .map(PathBuf::from)
        .filter(|git| git.is_file() && !is_shim(git))
        .or_else(|| real("git"))
}
