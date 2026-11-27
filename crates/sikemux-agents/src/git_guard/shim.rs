//! The `git` agents Sikemux starts find first on their PATH: this binary,
//! linked under that name. It refuses what [`super`] refuses and runs the real
//! git for everything else, whichever harness or shell the agent runs it from.
//!
//! Grok and Hermes rebuild PATH from the person's login profile, which puts
//! the system's git first, so they also check each shell command before it
//! runs: this binary again, linked as the hook their harnesses call.

use sikemux_process::command::Command;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::vars;

use super::{credential, footer, identity};

/// The folder of a core's data folder agents find this shim in first.
const AGENT_BIN: &str = "agent-bin";
/// The name the hook is linked under, outside the agents' PATH.
const HOOK: &str = "sikemux-git-hook";
/// Given to the hook where the shim is first on PATH, so it checks only what
/// leaves this machine past the shim. Only Claude chats still running from
/// before the core checked their commands itself pass it.
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
pub fn install(data_dir: &Path, endpoint: Option<&Path>) {
    let folder = data_dir.join(AGENT_BIN);
    if folder
        .to_string_lossy()
        .contains(crate::os::PATH_LIST_SEPARATOR)
    {
        return;
    }
    if let Err(error) = link(&folder, TIMEOUT) {
        eprintln!("sikemux core: agents have no timeout command: {error}");
    }
    crate::agent_shell::install(data_dir);
    match link(&folder, "git") {
        Ok(()) => {
            let built_in = super::heavy::built_in_programs();
            for program in crate::ship::PROGRAMS.iter().copied().chain(built_in) {
                if let Err(error) = link(&folder, program) {
                    eprintln!("sikemux core: agents' {program} commands go unchecked: {error}");
                }
            }
            unlink_others(&folder);
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
            if let Some(endpoint) = endpoint {
                let _ = ENDPOINT.set(endpoint.to_path_buf());
                match link(&hooks, identity::HELPER) {
                    Ok(()) => identity::install(&hooks, endpoint),
                    Err(error) => eprintln!(
                        "sikemux core: agents' git uses the person's credentials everywhere: {error}"
                    ),
                }
            }
            let _ = HOOKS_FOLDER.set(hooks);
        }
        Err(error) => eprintln!("sikemux core: agents' commits go without a footer: {error}"),
    }
}

fn link(folder: &Path, name: &str) -> std::io::Result<()> {
    let binary = std::env::current_exe()?;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(program_file(name));
    if std::fs::read_link(&path).is_ok_and(|target| target == binary)
        || crate::os::is_hard_link_of(&path, &binary)
    {
        return Ok(());
    }
    let fresh = folder.join(format!("{name}.{}", std::process::id()));
    let _ = std::fs::remove_file(&fresh);
    crate::os::symlink(&binary, &fresh)?;
    std::fs::rename(&fresh, &path)
}

fn program_file(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
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

/// The plugin a Grok chat loads the hook from. None before [`install`].
pub(crate) fn grok_plugin() -> Option<&'static Path> {
    GROK_PLUGIN.get().map(PathBuf::as_path)
}

/// `path` as one word for `sh -c`, and for Hermes, which splits a hook's
/// command the way a shell would.
fn quoted(path: &Path) -> String {
    sikemux_process::single_quoted(&path.to_string_lossy())
}

/// The variables an agent runs with to find the shim first: `path`, or the
/// person's PATH, with the shim's folder ahead of it, and the real git. None
/// before [`install`].
pub fn environment(path: Option<&str>) -> Vec<(String, String)> {
    let Some(folder) = FOLDER.get() else {
        return Vec::new();
    };
    let folder = folder.to_string_lossy();
    let path = path
        .map(str::to_owned)
        .or_else(|| sikemux_process::user_environment::var("PATH"));
    let path = match path.filter(|path| !path.is_empty()) {
        Some(path) => format!("{folder}{}{path}", crate::os::PATH_LIST_SEPARATOR),
        None => folder.into_owned(),
    };
    let mut env = vec![("PATH".to_owned(), path)];
    crate::android_sdk::extend(&mut env);
    env.extend(crate::agent_shell::environment());
    env.extend(super::heavy::rustc_cache());
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
pub fn sign_commit(command: &mut Command, dir: &Path) {
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
pub fn path_without_shim(path: &str) -> Option<String> {
    let separator = crate::os::PATH_LIST_SEPARATOR;
    let parts: Vec<&str> = path.split(separator).collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !is_shim_folder(Path::new(part)))
        .collect();
    (kept.len() != parts.len()).then(|| kept.join(&separator.to_string()))
}

fn is_shim_folder(folder: &Path) -> bool {
    folder.file_name().is_some_and(|name| name == AGENT_BIN)
        && crate::os::is_link(&folder.join(program_file("git")))
}

/// The shim's folder, for a shell to put first on PATH again after its
/// profile has run.
pub fn folder() -> Option<&'static Path> {
    FOLDER.get().map(PathBuf::as_path)
}

/// What `provider` is started with for its commands to find the shim. Codex
/// runs them in a login shell, whose profile puts the system's git ahead of
/// it, unless told not to.
pub fn arguments(provider: &str) -> &'static [&'static str] {
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
    let name = Path::new(&name).file_name()?.to_str()?;
    let name = name
        .strip_suffix(std::env::consts::EXE_SUFFIX)
        .unwrap_or(name);
    let ours = matches!(
        name,
        "git" | HOOK | TIMEOUT | footer::LINK | identity::HELPER
    ) || stands_in_for(name)
        || linked_as(name);
    if !ours {
        return None;
    }
    die_on_closed_pipe();
    Some(match name {
        "git" => run(args.collect()),
        HOOK => run_hook(args.next().is_some_and(|arg| arg == SHIP_ONLY)),
        TIMEOUT => crate::timeout::run(args.collect()),
        footer::LINK => footer::run(args.collect()),
        identity::HELPER => credential::run(args.collect()),
        program => run_program(program, args.collect()),
    })
}

/// Rust ignores SIGPIPE, so a message written after the caller closed its end
/// panicked; the programs the shim stands in for just stop.
fn die_on_closed_pipe() {
    #[cfg(unix)]
    // SAFETY: restoring the default action of a signal has no other effect.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Whether this build checks `program` on every host, whatever its projects list.
fn stands_in_for(program: &str) -> bool {
    crate::ship::PROGRAMS.contains(&program)
        || super::heavy::built_in_programs().any(|known| known == program)
}

/// Whether this binary is linked as `program` in the shim's folder on PATH: for a project's
/// `heavy` list, or by a build that stood in for more, whose link must still run the program.
fn linked_as(program: &str) -> bool {
    !program.starts_with("sikemux")
        && std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .any(|folder| {
                is_shim_folder(&folder) && crate::os::is_link(&folder.join(program_file(program)))
            })
}

/// Removes from the shim's folder the links to programs this build stands in for no longer,
/// so agents run those straight from the person's PATH again.
fn unlink_others(folder: &Path) {
    let listed = listed_programs(folder);
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(name) = name.strip_suffix(std::env::consts::EXE_SUFFIX) else {
            continue;
        };
        let kept = name == "git"
            || name == TIMEOUT
            || stands_in_for(name)
            || listed.iter().any(|listed| listed == name);
        if !kept && crate::os::is_link(&entry.path()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The file in the shim's folder that lists the programs projects' `heavy` lists named, which
/// the shim stands in for beside the built-in ones.
const LISTED: &str = "line.json";

/// Has the shim stand in for the programs the `heavy` list of the project at `dir` names,
/// for agents started from now on. Nothing before [`install`].
pub fn stand_in_for_heavy(dir: &Path) {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Some(folder) = FOLDER.get() else {
        return;
    };
    let Some(heavy) = crate::tasks::config::heavy_near(dir) else {
        return;
    };
    let wanted = super::heavy::listed_programs(&heavy);
    if wanted.is_empty() {
        return;
    }
    let _writing = WRITING.lock();
    let mut listed = listed_programs(folder);
    let new: Vec<String> = wanted
        .into_iter()
        .filter(|program| !listed.contains(program))
        .collect();
    if new.is_empty() {
        return;
    }
    for program in new {
        match link(folder, &program) {
            Ok(()) => listed.push(program),
            Err(error) => eprintln!(
                "sikemux core: agents' {program} commands do not wait their turn: {error}"
            ),
        }
    }
    let table = json!({ "programs": listed }).to_string();
    if let Err(error) =
        sikemux_process::files::write_atomic(&folder.join(LISTED), table.as_bytes(), false)
    {
        eprintln!("sikemux core: could not list the heavy commands agents run: {error}");
    }
}

fn listed_programs(folder: &Path) -> Vec<String> {
    std::fs::read(folder.join(LISTED))
        .ok()
        .and_then(|table| serde_json::from_slice::<Value>(&table).ok())
        .and_then(|table| serde_json::from_value(table.get("programs")?.clone()).ok())
        .unwrap_or_default()
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
    let Some((command, cwd)) = hooked_command(&event) else {
        return 0;
    };
    if let Some(refusal) = (!ship_only)
        .then(|| super::refusal(command, &cwd, &others, &in_project))
        .flatten()
    {
        eprintln!("{refusal}");
        return 2;
    }
    let looks = match ship_only {
        true => crate::ship::Looks::PastTheShim,
        false => crate::ship::Looks::Every,
    };
    let ships = ships_of(command, looks, &cwd, &|name| std::env::var(name).ok());
    let held = ships
        .iter()
        .find_map(|ship| match asked(ship, &cwd, false) {
            Asked::Held(said) => Some(said),
            Asked::Goes { .. } => None,
        });
    match held {
        Some(held) => {
            eprintln!("{held}");
            2
        }
        None => 0,
    }
}

/// The shell command of a tool call a harness is about to run, in Claude's
/// hook form, and the folder it runs in.
pub fn hooked_command(event: &Value) -> Option<(&str, PathBuf)> {
    let tool = &event["tool_input"];
    let (command, cwd) = (tool["command"].as_str()?, event["cwd"].as_str()?);
    let cwd = ["workdir", "cwd"]
        .iter()
        .find_map(|key| tool[*key].as_str())
        .map_or_else(|| PathBuf::from(cwd), |dir| Path::new(cwd).join(dir));
    Some((command, cwd))
}

/// Every command of the shell command line `command`, run in `cwd` under
/// `env`, that leaves this machine, the project's own such commands among them.
pub fn ships_of(
    command: &str,
    looks: crate::ship::Looks,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<crate::ship::Ship> {
    crate::ship::of_command(command, &project_ships(cwd), looks, cwd, env)
}

/// The commands the project `cwd` is in lists in its sikemux.json as leaving
/// this machine, such as `make deploy`.
pub fn project_ships(cwd: &Path) -> Vec<String> {
    let project = cwd
        .ancestors()
        .find(|dir| dir.join(crate::tasks::config::CONFIG_FILE).is_file());
    match project.map(crate::tasks::config::load) {
        Some(crate::tasks::config::ConfigLoad::Valid { config, .. }) => config.ship,
        _ => Vec::new(),
    }
}

/// What the core says of `ship`, run in `cwd`.
enum Asked {
    /// It may run now; the core wants to hear how it ended when it names a `went`.
    Goes { went: Option<String> },
    /// What the agent is told when it holds it for the person to allow first.
    Held(String),
}

/// Asks the core about `ship`. `follows` when this program sees it end and can
/// say how it went.
fn asked(ship: &crate::ship::Ship, cwd: &Path, follows: bool) -> Asked {
    let endpoint = sikemux_process::paths::handed_cli_endpoint();
    asked_at(endpoint.as_deref(), ship, cwd, follows)
}

fn asked_at(endpoint: Option<&Path>, ship: &crate::ship::Ship, cwd: &Path, follows: bool) -> Asked {
    let Some(endpoint) = endpoint else {
        return Asked::Held(format!(
            "Sikemux could not ask whether `{}` may leave this machine: {} is not set.",
            ship.what,
            vars::CLI_ENDPOINT
        ));
    };
    let answer = crate::cli::client::attempt(
        endpoint,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::SHIP_CHECK_METHOD,
        &json!({ "kind": ship.kind, "what": ship.what, "target": ship.target, "follows": follows }),
        GUARD_WAIT,
    );
    match answer {
        Ok(answer) if answer.get("allowed").and_then(Value::as_bool) == Some(true) => Asked::Goes {
            went: answer
                .get("went")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        Ok(answer) => Asked::Held(
            answer
                .get("said")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        Err(_) => Asked::Held(format!(
            "Sikemux could not ask whether `{}` may leave this machine. Run it again in a moment.",
            ship.what
        )),
    }
}

fn tell_ran(went: &str, cwd: &Path, exit: Option<i32>) {
    if let Some(endpoint) = footer::endpoint() {
        let _ = crate::cli::client::attempt(
            &endpoint,
            &cwd.to_string_lossy(),
            None,
            crate::cli::protocol::SHIP_RAN_METHOD,
            &json!({ "went": went, "exit": exit }),
            GUARD_WAIT,
        );
    }
}

/// Runs `program`, which the shim stands in for, unless what it does leaves
/// this machine and the core holds it for the person.
fn run_program(program: &str, args: Vec<OsString>) -> i32 {
    let words: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut watched = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_default();
    let env = |name: &str| std::env::var(name).ok();
    for ship in crate::ship::of_invocations(program, &words, &env, &cwd) {
        match asked(&ship, &cwd, true) {
            Asked::Held(said) => {
                eprintln!("{said}");
                return 1;
            }
            Asked::Goes { went } => watched.extend(went),
        }
    }
    let rustup = (program == "cargo")
        .then(|| super::heavy::rustup_cargo(&cwd))
        .flatten();
    let Some(real) = rustup.or_else(|| real(program)) else {
        eprintln!("{program}: Sikemux found no {program} to run on PATH");
        return 127;
    };
    if program == "gh" {
        if let Some(code) = gh_run(&words, &cwd) {
            return code;
        }
    }
    let as_agents = (program == "gh")
        .then(|| gh_as_agents(&words, &cwd))
        .flatten();
    let command = |real: &Path| {
        let mut command = sikemux_process::user_environment::command(real);
        if let Some(gh) = &as_agents {
            gh.apply(&mut command);
        }
        command
    };
    if let (Some(gh), ["pr", "merge", rest @ ..]) = (&as_agents, words_of(&words).as_slice()) {
        let code = gh_merge(command(&real), gh, rest, &cwd);
        for went in watched {
            tell_ran(&went, &cwd, Some(code));
        }
        return code;
    }
    if !watched.is_empty() {
        let status = command(&real).args(&args).status();
        let code = status
            .as_ref()
            .ok()
            .and_then(std::process::ExitStatus::code);
        for went in watched {
            tell_ran(&went, &cwd, code);
        }
        return code.unwrap_or(1);
    }
    if let Some(heavy) = super::heavy::of_program(program, &words, &cwd) {
        return super::in_line::run(program, &real, &args, &words, &cwd, heavy);
    }
    let mut command = command(&real);
    if program == "cargo" {
        command.env("PATH", super::heavy::path_with(&real));
    }
    let error = crate::os::exec(command.args(&args));
    eprintln!("{program}: {}: {error}", real.display());
    126
}

/// What `gh` runs with in a repository whose remote has an identity of the agents' own: a
/// token for that repository alone, and none of the person's.
struct GhAsAgents {
    remote: String,
    token: String,
    config: PathBuf,
}

impl GhAsAgents {
    fn apply(&self, command: &mut Command) {
        for name in vars::PERSON_GITHUB_TOKENS {
            command.env_remove(name);
        }
        command
            .env("GH_TOKEN", &self.token)
            .env("GH_CONFIG_DIR", &self.config);
    }
}

/// A token for the repository `gh` with `words` acts on in `cwd`, when its remote has an
/// identity of the agents' own.
fn gh_as_agents(words: &[String], cwd: &Path) -> Option<GhAsAgents> {
    let folder = PathBuf::from(std::env::var_os(footer::HOOKS)?);
    let remote = identity::acting_remote(&folder, || {
        match gh_repo_named(words).or_else(|| std::env::var("GH_REPO").ok()) {
            Some(repo) => vec![gh_repo_url(&repo)],
            None => gh_remote_urls(cwd),
        }
    })?;
    let answer = credential::ask(&footer::endpoint()?, &remote)?;
    let token = answer.get("password")?.as_str()?.to_owned();
    let config = folder.join("gh");
    std::fs::create_dir_all(&config).ok()?;
    Some(GhAsAgents {
        remote,
        token,
        config,
    })
}

fn words_of(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).collect()
}

/// Hands `gh run list`, `view` and `watch` to the core, which answers from the CI it holds for
/// every agent rather than each one asking GitHub; none when the real `gh` should run instead.
fn gh_run(words: &[String], cwd: &Path) -> Option<i32> {
    let ["run", "list" | "ls" | "view" | "watch", ..] = words_of(words).as_slice() else {
        return None;
    };
    let mut args = Vec::new();
    let mut filter = None;
    let mut given = words.iter();
    while let Some(word) = given.next() {
        match word.split_once('=') {
            Some(("--jq" | "-q", value)) => filter = Some(value.to_owned()),
            _ if word == "--jq" || word == "-q" => filter = Some(given.next()?.clone()),
            _ => args.push(word.clone()),
        }
    }
    let jq = match &filter {
        Some(_) if args.iter().any(|arg| arg.starts_with("--json")) => Some(real("jq")?),
        Some(_) => return None,
        None => None,
    };
    let answer = crate::cli::client::attempt(
        &footer::endpoint()?,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::GH_RUN_METHOD,
        &json!({ "args": args }),
        Duration::from_secs(60),
    )
    .ok()?;
    if answer.get("pass").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let code = answer
        .get("code")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
        .unwrap_or(0);
    if let Some(said) = answer.get("err").and_then(Value::as_str) {
        eprintln!("{said}");
        return Some(code);
    }
    let out = answer
        .get("out")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (Some(jq), Some(filter)) = (jq, filter) else {
        println!("{out}");
        return Some(code);
    };
    let mut child = sikemux_process::user_environment::command(&jq)
        .args(["-r", &filter])
        .stdin(sikemux_process::command::Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        drop(stdin.write_all(out.as_bytes()));
    }
    let filtered = child.wait().ok()?.code().unwrap_or(1);
    Some(if filtered == 0 { code } else { filtered })
}

/// Hands `gh pr merge <args>` to the core, which merges as the agents' own identity, since no
/// token an agent holds may write the repository. `gh` itself, through `view`, finds the pull
/// request the arguments or the current branch name.
fn gh_merge(mut view: Command, gh: &GhAsAgents, args: &[&str], cwd: &Path) -> i32 {
    let mut pull = Vec::new();
    let (mut method, mut sha, mut delete) = (None, None, false);
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--merge" | "-m" => method = Some("merge"),
            "--squash" | "-s" => method = Some("squash"),
            "--rebase" | "-r" => method = Some("rebase"),
            "--delete-branch" | "-d" => delete = true,
            "--match-head-commit" => sha = args.next().map(|sha| sha.to_string()),
            "-R" | "--repo" => pull.extend(["--repo", args.next().copied().unwrap_or_default()]),
            _ if arg.starts_with("--match-head-commit=") => {
                sha = arg.split_once('=').map(|(_, sha)| sha.to_owned());
            }
            _ if arg.starts_with("--repo=") || (arg.starts_with("-R") && arg.len() > 2) => {
                pull.push(arg);
            }
            _ if !arg.starts_with('-') && !pull.iter().any(|kept| !kept.starts_with('-')) => {
                pull.push(arg);
            }
            _ => {
                eprintln!(
                    "gh: Sikemux merges as the agents' own identity and takes only a pull request, --merge, --squash, --rebase, --match-head-commit and --delete-branch, not {arg}"
                );
                return 1;
            }
        }
    }
    let viewed = view
        .args(["pr", "view"])
        .args(&pull)
        .args(["--json", "number,headRefName,headRefOid"])
        .stderr(sikemux_process::command::Stdio::inherit())
        .output();
    let Some(found) = viewed
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
    else {
        return 1;
    };
    let Some(number) = found.get("number").and_then(Value::as_u64) else {
        return 1;
    };
    let mut params = serde_json::Map::from_iter([
        ("remote".to_owned(), json!(gh.remote)),
        ("number".to_owned(), json!(number)),
    ]);
    if let Some(method) = method {
        params.insert("method".to_owned(), json!(method));
    }
    if let Some(sha) = sha {
        params.insert("sha".to_owned(), json!(sha));
    }
    if delete {
        let field = |key: &str| found.get(key).cloned().unwrap_or(Value::Null);
        params.insert("deleteBranch".to_owned(), field("headRefName"));
        params.insert("head".to_owned(), field("headRefOid"));
    }
    let params = Value::Object(params);
    let Some(endpoint) = footer::endpoint() else {
        return 1;
    };
    let answer = crate::cli::client::attempt(
        &endpoint,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::GH_MERGE_METHOD,
        &params,
        Duration::from_secs(120),
    );
    match answer {
        Ok(answer) => {
            eprintln!(
                "{}",
                answer
                    .get("said")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            0
        }
        Err(
            crate::cli::client::Failure::NotSent(said) | crate::cli::client::Failure::Sent(said),
        ) => {
            eprintln!("gh: {said}");
            1
        }
    }
}

fn gh_repo_named(words: &[String]) -> Option<String> {
    let mut words = words.iter().take_while(|word| *word != "--");
    while let Some(word) = words.next() {
        if word == "-R" || word == "--repo" {
            return words.next().cloned();
        }
        if let Some(repo) = word
            .strip_prefix("--repo=")
            .or_else(|| word.strip_prefix("-R"))
        {
            return Some(repo.to_owned());
        }
    }
    None
}

/// `repo` as gh takes it, `OWNER/REPO`, `HOST/OWNER/REPO` or an address, as an address.
fn gh_repo_url(repo: &str) -> String {
    if repo.contains("://") || repo.contains('@') {
        return repo.to_owned();
    }
    match repo.split('/').count() {
        2 => format!("https://github.com/{repo}"),
        _ => format!("https://{repo}"),
    }
}

/// The addresses of the remotes in `cwd`, the one gh picks first: the one set as its default,
/// then `upstream`, `github`, `origin`.
fn gh_remote_urls(cwd: &Path) -> Vec<String> {
    let Some(git) = real_git() else {
        return Vec::new();
    };
    let output = sikemux_process::user_environment::command(&git)
        .current_dir(cwd)
        .args(["config", "--get-regexp", r"^remote\..*\.(url|gh-resolved)$"])
        .stdin(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut remotes: Vec<(&str, &str)> = Vec::new();
    let mut resolved = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("remote.") else {
            continue;
        };
        if let Some(name) = rest.strip_suffix(".url") {
            remotes.push((name, value));
        } else if let Some(name) = rest.strip_suffix(".gh-resolved") {
            resolved = Some(name);
        }
    }
    let rank = |name: &str| match name {
        _ if Some(name) == resolved => 0,
        "upstream" => 1,
        "github" => 2,
        "origin" => 3,
        _ => 4,
    };
    remotes.sort_by_key(|(name, _)| rank(name));
    remotes
        .into_iter()
        .take(1)
        .map(|(_, url)| url.to_owned())
        .collect()
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
    if let Some(refusal) = super::refused(&words, private_index, &real, &others, &in_project) {
        eprintln!("{refusal}");
        return 1;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let alias = |global: &[String], name: &str| alias_of(&git, global, name);
    for ship in crate::ship::of_git_invocation(&words, &not_ours, &cwd, &alias) {
        if let Asked::Held(said) = asked(&ship, &cwd, false) {
            eprintln!("{said}");
            return 1;
        }
    }
    let mut command = sikemux_process::user_environment::command(&git);
    let subcommand = super::git_args(&words);
    if let Some(folder) = std::env::var_os(footer::HOOKS) {
        let options = words
            .get(..words.len() - subcommand.len())
            .unwrap_or_default();
        let folder = Path::new(&folder);
        sikemux_git::add_config(&mut command, &identity::agent_settings(folder));
        match subcommand.first().map(String::as_str) {
            Some("commit" | "merge") => {
                let rest = subcommand.get(1..).unwrap_or_default();
                footer::prepare(&mut command, folder, &git, options, rest);
            }
            Some("rebase" | "cherry-pick" | "pull") => {
                footer::prepare_remaking(&mut command, folder, &git, options);
            }
            _ => {}
        }
    }
    let error = crate::os::exec(command.args(&args));
    eprintln!("git: {}: {error}", git.display());
    126
}

/// The environment variable `name`, but for git settings that are all ones this shim handed the
/// git that runs this one.
fn not_ours(name: &str) -> Option<String> {
    let said = std::env::var(name).ok()?;
    if name != "GIT_CONFIG_COUNT" {
        return Some(said);
    }
    let ours = std::env::var_os(footer::HOOKS)
        .map(|folder| identity::agent_settings(Path::new(&folder)))
        .unwrap_or_default();
    let given = |part: &str, at: usize| std::env::var(format!("GIT_CONFIG_{part}_{at}")).ok();
    let all_ours = said.parse().is_ok_and(|count: usize| {
        (0..count).all(|at| {
            given("KEY", at)
                .zip(given("VALUE", at))
                .is_some_and(|setting| ours.contains(&setting))
        })
    });
    (!all_ours).then_some(said)
}

/// The other agents whose work a git command in `dir` would touch, as the
/// core that started this agent knows them.
fn others(dir: &Path, paths: &[String]) -> Option<Vec<String>> {
    // Git that a build or its tests run, such as in a test's temporary repository, is the
    // build's own and not the agent's.
    if std::env::var_os(vars::AGENT_ID).is_none() || std::env::var_os(vars::BUILD_TURN).is_some() {
        return Some(Vec::new());
    }
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

/// Whether `dir` is in a project checkout the core that started this agent manages. Git run
/// outside every agent's processes, or by a build, is never in one.
fn in_project(dir: &Path) -> Option<bool> {
    if std::env::var_os(vars::AGENT_ID).is_none() || std::env::var_os(vars::BUILD_TURN).is_some() {
        return Some(false);
    }
    let answer = crate::cli::client::attempt(
        &footer::endpoint()?,
        &dir.to_string_lossy(),
        None,
        crate::cli::protocol::GIT_GUARD_METHOD,
        &json!({ "in_project": true }),
        GUARD_WAIT,
    )
    .ok()?;
    answer.get("in_project")?.as_bool()
}

const GUARD_WAIT: Duration = Duration::from_secs(10);

/// What the git alias `name` stands for, as git in this folder reads it under `global`.
fn alias_of(git: &Path, global: &[String], name: &str) -> Option<String> {
    let output = sikemux_process::user_environment::command(git)
        .args(global)
        .args(["config", "--get", &format!("alias.{name}")])
        .stdin(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null())
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
            .and_then(sikemux_process::files::real_path)
            .ok()
    });
    sikemux_process::files::real_path(path).ok() == *shim
        || shim
            .as_deref()
            .is_some_and(|shim| crate::os::is_hard_link_of(path, shim))
}

/// The program `name` this shim stands in for, the first on PATH that is not
/// the shim itself.
fn real(name: &str) -> Option<PathBuf> {
    sikemux_process::user_environment::find_executables(name)
        .into_iter()
        .find(|found| !is_shim(found))
}

/// The git this shim stands in for, never the shim itself.
pub(super) fn real_git() -> Option<PathBuf> {
    std::env::var_os(vars::REAL_GIT)
        .map(PathBuf::from)
        .filter(|git| git.is_file() && !is_shim(git))
        .or_else(|| real("git"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_to_a_program_no_longer_stood_in_for_is_removed() {
        let top = tempfile::tempdir().unwrap();
        let program = top.path().join("program");
        std::fs::write(&program, "").unwrap();
        let folder = top.path().join(AGENT_BIN);
        std::fs::create_dir(&folder).unwrap();
        for name in ["git", TIMEOUT, "gh", "cargo", "npm", "curl", "tsc"] {
            crate::os::symlink(&program, &folder.join(program_file(name))).unwrap();
        }
        std::fs::write(folder.join(LISTED), r#"{"programs":["tsc"]}"#).unwrap();

        unlink_others(&folder);

        let mut left: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut kept: Vec<String> = ["cargo", "gh", "git", TIMEOUT, "tsc"]
            .map(program_file)
            .into_iter()
            .chain([LISTED.to_owned()])
            .collect();
        kept.sort();
        assert_eq!(left, kept);
    }

    #[test]
    fn shim_fails_closed_without_cli_endpoint() {
        let ship = crate::ship::Ship {
            kind: sikemux_view::inbox::ShipKind::Deploy,
            what: "vercel --prod".to_owned(),
            target: Some(crate::ship::Target::Vercel { production: true }),
        };
        let result = asked_at(None, &ship, Path::new("/nowhere"), true);
        assert!(matches!(result, Asked::Held(said) if said.contains(vars::CLI_ENDPOINT)));
    }
}
