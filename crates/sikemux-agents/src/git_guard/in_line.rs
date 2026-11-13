//! A heavy command an agent runs waits for its turn on the host, which the core keeps while the
//! shim's connection to it stays open. The shim still starts the command itself. When the same
//! command already runs, it plays that run's output instead; when the wait is long and the
//! agent typed the command, it says so and ends, to be woken when its turn comes.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::client::{read_endpoint, Session};
use crate::cli::protocol::{BuildNote, BuildRequest, CliClientCommand, CliServerResponse};
use sikemux_process::sync::Locked;
use sikemux_process::vars;

use super::footer;
use super::heavy::{path_with, rustc_cache, Heavy};

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
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "fish"];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LOOK_EVERY: Duration = Duration::from_secs(1);
const FOLLOW_EVERY: Duration = Duration::from_millis(100);
/// How long a waiting command tries to reach a core that went away before starting alone.
const CORE_GONE_AFTER: Duration = Duration::from_secs(30);
/// How far apart waiting commands start once no core answers.
const ALONE_APART: Duration = Duration::from_secs(20);
/// How long a command that ended by a signal waits to hear whether Sikemux stopped it, and why.
const STOP_TOLD_WITHIN: Duration = Duration::from_secs(1);
/// The exit code of a command sent away to come back when its turn comes.
const SENT_AWAY: i32 = 75;

/// What the core said to do.
enum Turn {
    /// Start it, telling the core through `session` when there is one, and copying its output
    /// to `log` when given.
    Go {
        session: Option<Session>,
        log: Option<PathBuf>,
    },
    /// It is not started now, for the reason given.
    SentAway(String),
    /// The same command ran for another caller and ended with this code.
    Followed(i32),
}

/// Runs `program args`, found at `real`, once its turn comes, and answers with its exit code.
pub(super) fn run(
    program: &str,
    real: &Path,
    args: &[OsString],
    words: &[String],
    cwd: &Path,
    heavy: Heavy,
) -> i32 {
    let what = std::iter::once(program.to_owned())
        .chain(words.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let agent = std::env::var(vars::AGENT_ID).ok();
    let task = std::env::var(vars::TASK_EXECUTION_ID).ok();
    let request = BuildRequest {
        may_come_back: agent.is_some() && task.is_none(),
        started_by: started_by(),
        agent_id: agent.clone(),
        command: what,
        pid: Some(std::process::id()),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        key: Some(heavy.key),
        before_commit: heavy.before_commit,
        watched: heavy.watched,
        // SAFETY: isatty only asks about the descriptor.
        copies_output: unsafe { libc::isatty(libc::STDOUT_FILENO) } == 0,
        asked_at: Some(sikemux_process::clock::epoch_ms()),
        execution_id: task,
        needs: heavy.needs.clone(),
    };
    let turn = match std::env::var_os(vars::BUILD_TURN) {
        Some(_) => Turn::Go {
            session: None,
            log: None,
        },
        None => take_turn(program, request),
    };
    let (session, log) = match turn {
        Turn::SentAway(said) => {
            eprintln!("{program}: {said}");
            return SENT_AWAY;
        }
        Turn::Followed(code) => return code,
        Turn::Go { session, log } => (session, log),
    };
    let mut command = match utility_priority().filter(|_| agent.is_some()) {
        Some((lower, with)) => {
            let mut command = sikemux_process::user_environment::command(lower);
            command.args(with).arg(real);
            command
        }
        None => sikemux_process::user_environment::command(real),
    };
    command.args(args).env(vars::BUILD_TURN, "1");
    if program == "cargo" {
        command.envs(rustc_cache()).env("PATH", path_with(real));
    }
    if log.is_some() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{program}: {error}");
            return 126;
        }
    };
    let copying = log.and_then(|log| copy_output(&mut child, &log));
    let session = Arc::new(Mutex::new(session));
    let stopped: Arc<Mutex<Option<String>>> = Arc::default();
    if let Some(reading) = session.locked().as_ref().and_then(|s| s.try_clone().ok()) {
        let stopped = stopped.clone();
        std::thread::spawn(move || told_stop(reading, &stopped));
    }
    if heavy.then_runs && session.locked().is_some() {
        let root = child.id();
        let session = session.clone();
        let tool = program.to_owned();
        std::thread::spawn(move || loop {
            std::thread::sleep(LOOK_EVERY);
            if program_started(root, &tool) {
                if let Some(session) = session.locked().as_mut() {
                    let _ = session.send(&BuildNote::Released);
                }
                return;
            }
        });
    }
    let status = child.wait();
    for copier in copying.into_iter().flatten() {
        let _ = copier.join();
    }
    let code = match status {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!("{program}: {error}");
            1
        }
    };
    if code > 128 && session.locked().is_some() {
        let until = Instant::now() + STOP_TOLD_WITHIN;
        while stopped.locked().is_none() && Instant::now() < until {
            std::thread::sleep(FOLLOW_EVERY);
        }
    }
    if let Some(why) = stopped.locked().take() {
        eprintln!("{program}: Sikemux stopped it: {why}.");
    }
    if let Some(mut session) = session.locked().take() {
        let _ = session.send(&BuildNote::Exited { code });
        session.close();
    }
    code
}

/// Copies what the child writes to this process's own output and to `log`, from threads of
/// their own.
fn copy_output(
    child: &mut std::process::Child,
    log: &Path,
) -> Option<Vec<std::thread::JoinHandle<()>>> {
    let file = std::fs::OpenOptions::new().append(true).open(log).ok()?;
    let file = Arc::new(Mutex::new(file));
    let mut copiers = Vec::new();
    if let Some(out) = child.stdout.take() {
        let file = file.clone();
        copiers.push(std::thread::spawn(move || {
            copy(out, std::io::stdout(), &file);
        }));
    }
    if let Some(err) = child.stderr.take() {
        copiers.push(std::thread::spawn(move || {
            copy(err, std::io::stderr(), &file);
        }));
    }
    Some(copiers)
}

fn copy(mut from: impl Read, mut to: impl Write, log: &Mutex<std::fs::File>) {
    let mut buffer = [0u8; 8192];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let _ = to.write_all(&buffer[..read]);
                let _ = to.flush();
                let _ = log.locked().write_all(&buffer[..read]);
            }
        }
    }
}

/// Says why the core stopped the command, when it does.
fn told_stop(mut session: Session, why: &Mutex<Option<String>>) {
    while let Ok(answer) = session.receive() {
        if let CliServerResponse::Stopped { why: said } = answer {
            *why.locked() = Some(said);
        }
    }
}

/// The process that started this command, past the shell that ran it if one did, with when it
/// started. The core sends the command away only when that is the agent's own program, so a
/// command a script, a make file or a package runner started waits where it is.
fn started_by() -> Option<(u32, u64)> {
    let parent = std::os::unix::process::parent_id();
    let shell = sikemux_process::procs::name(parent)
        .is_some_and(|name| SHELLS.contains(&name.trim_start_matches('-')));
    let by = match shell {
        true => sikemux_process::procs::parent(parent)?.0,
        false => parent,
    };
    Some((by, sikemux_process::procs::started(by)?))
}

/// What starts a program at utility priority, on systems that have it, so an agent's heavy
/// command never competes with what the person is doing.
fn utility_priority() -> Option<(&'static str, &'static [&'static str])> {
    const TASKPOLICY: &str = "/usr/sbin/taskpolicy";
    (cfg!(target_os = "macos") && Path::new(TASKPOLICY).exists())
        .then_some((TASKPOLICY, &["-c", "utility"][..]))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or_default())
}

/// Waits for the core to give the command its turn, saying so while it waits. With no core at
/// all it runs at once; when the core goes away while it waits, it asks again in its old place,
/// and starts alone if none answers for a while.
fn take_turn(program: &str, request: BuildRequest) -> Turn {
    let since = Instant::now();
    let mut told = false;
    let mut lost: Option<Instant> = None;
    loop {
        let Some(mut session) = open() else {
            match lost {
                None => {
                    return Turn::Go {
                        session: None,
                        log: None,
                    }
                }
                Some(at) if at.elapsed() >= CORE_GONE_AFTER => {
                    eprintln!("{program}: no Sikemux answers, so it starts without its turn.");
                    start_alone();
                    return Turn::Go {
                        session: None,
                        log: None,
                    };
                }
                Some(_) => {
                    std::thread::sleep(LOOK_EVERY);
                    continue;
                }
            }
        };
        if ask(&mut session, &request).is_err() {
            lost.get_or_insert_with(Instant::now);
            continue;
        }
        loop {
            match session.receive() {
                Ok(CliServerResponse::Queued {
                    ahead,
                    building,
                    why,
                }) => {
                    if !told {
                        eprintln!("{program}: {}", in_line(ahead, building, &why));
                    }
                    told = true;
                }
                Ok(CliServerResponse::Building { log }) => {
                    if told {
                        let waited = since.elapsed().as_secs();
                        eprintln!("{program}: started after {waited}s in line.");
                    }
                    return Turn::Go {
                        session: Some(session),
                        log: log.map(PathBuf::from),
                    };
                }
                Ok(CliServerResponse::Parked { said }) => return Turn::SentAway(said),
                Ok(CliServerResponse::Joined { log }) => {
                    eprintln!(
                        "{program}: the same command already runs here, so this shows its output."
                    );
                    return Turn::Followed(follow(session, Path::new(&log), program));
                }
                Ok(CliServerResponse::Ended { code }) => {
                    return Turn::Followed(code.unwrap_or(1));
                }
                Ok(_) => {
                    return Turn::Go {
                        session: None,
                        log: None,
                    }
                }
                Err(_) => {
                    lost = Some(Instant::now());
                    break;
                }
            }
        }
    }
}

fn open() -> Option<Session> {
    let descriptor = read_endpoint(&footer::endpoint()?).ok()?;
    let session = Session::open(&descriptor, CONNECT_TIMEOUT, CONNECT_TIMEOUT).ok()?;
    session.wait_for_answers(None).ok()?;
    Some(session)
}

fn ask(session: &mut Session, request: &BuildRequest) -> Result<(), String> {
    let descriptor = read_endpoint(&footer::endpoint().ok_or("no core")?)?;
    session.send(&CliClientCommand::Build {
        protocol: descriptor.protocol,
        token: descriptor.token,
        request: request.clone(),
    })
}

/// Plays the output of the run another caller started, as it comes, and answers with how it
/// ended.
fn follow(mut session: Session, log: &Path, program: &str) -> i32 {
    let ended: Arc<Mutex<Option<Option<i32>>>> = Arc::default();
    {
        let ended = ended.clone();
        std::thread::spawn(move || {
            let code = loop {
                match session.receive() {
                    Ok(CliServerResponse::Ended { code }) => break code,
                    Ok(_) => {}
                    Err(_) => break None,
                }
            };
            *ended.locked() = Some(code);
        });
    }
    let Ok(mut file) = std::fs::File::open(log) else {
        eprintln!("{program}: the output of the run it follows is gone.");
        return 1;
    };
    let mut out = std::io::stdout();
    let mut buffer = [0u8; 8192];
    loop {
        let done = ended.locked().is_some();
        match file.read(&mut buffer) {
            Ok(0) | Err(_) if done => break,
            Ok(0) | Err(_) => std::thread::sleep(FOLLOW_EVERY),
            Ok(read) => {
                let _ = out.write_all(&buffer[..read]);
                let _ = out.flush();
            }
        }
    }
    let code = ended.locked().flatten();
    code.unwrap_or_else(|| {
        eprintln!("{program}: the run it followed ended without saying how.");
        1
    })
}

/// Starts no sooner than a while after the last command that started alone, so commands that
/// lost their core do not all start at once.
fn start_alone() {
    let marker = std::env::temp_dir().join("sikemux-line-alone");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)
    else {
        return;
    };
    use std::os::fd::AsRawFd;
    // SAFETY: flock only locks the open file, which lives until this returns.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    let since = std::fs::metadata(&marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok());
    if let Some(since) = since.filter(|since| *since < ALONE_APART) {
        std::thread::sleep(ALONE_APART - since);
    }
    let _ = file.set_modified(std::time::SystemTime::now());
}

fn in_line(ahead: usize, running: usize, why: &str) -> String {
    let behind = match ahead {
        0 => String::new(),
        ahead => format!(" behind {ahead} more"),
    };
    let running = match running {
        1 => "1 heavy command is".to_owned(),
        running => format!("{running} heavy commands are"),
    };
    format!(
        "in line{behind}: {running} running on this host, and {why}. It starts by itself when its turn comes; Settings → Chats → Heavy commands at once sets how many run together."
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
