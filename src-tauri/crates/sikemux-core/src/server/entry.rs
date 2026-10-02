use std::collections::HashMap;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::Duration;

use crate::protocol::frozen::{UpgradeInfo, UPGRADE_INFO_ARG};
use crate::protocol::{BuildIdentity, SessionId};

use super::handover::{self, Recovery};
use super::{ServerConfig, ServerError, DEFAULT_IDLE_EXIT};

const USAGE: &str = "usage: sikemux core [--socket <path>] [--cli-endpoint <path>] [--data-dir <path>] [--idle-exit-secs N] | --upgrade-info";

#[derive(Default)]
struct CoreArgs {
    socket: Option<PathBuf>,
    idle_exit: Option<Duration>,
    cli_endpoint: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    upgrade_info: bool,
    resume: Option<PathBuf>,
    listener_fd: Option<RawFd>,
    lock_fd: Option<RawFd>,
    tools_fd: Option<RawFd>,
    sessions: Vec<(SessionId, RawFd, Option<u32>)>,
}

fn number<T: std::str::FromStr>(value: Option<String>, flag: &str) -> Result<T, String> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("{flag} needs a whole number"))
}

fn handed_over_session(value: Option<String>) -> Result<(SessionId, RawFd, Option<u32>), String> {
    let value = value.ok_or("--session needs id:fd:pid")?;
    let mut parts = value.split(':');
    let mut next = || parts.next().ok_or("--session needs id:fd:pid");
    let id = next()?.parse().map_err(|_| "--session has a bad id")?;
    let fd = next()?
        .parse()
        .map_err(|_| "--session has a bad descriptor")?;
    let pid = match next()? {
        "-" => None,
        pid => Some(pid.parse().map_err(|_| "--session has a bad pid")?),
    };
    Ok((id, fd, pid))
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<CoreArgs, String> {
    let mut parsed = CoreArgs::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                parsed.socket = Some(args.next().ok_or("--socket needs a path")?.into());
            }
            "--cli-endpoint" => {
                parsed.cli_endpoint =
                    Some(args.next().ok_or("--cli-endpoint needs a path")?.into());
            }
            "--data-dir" => {
                parsed.data_dir = Some(args.next().ok_or("--data-dir needs a path")?.into());
            }
            "--idle-exit-secs" => {
                parsed.idle_exit = Some(Duration::from_secs(number(
                    args.next(),
                    "--idle-exit-secs",
                )?));
            }
            "--idle-exit-ms" => {
                parsed.idle_exit = Some(Duration::from_millis(number(
                    args.next(),
                    "--idle-exit-ms",
                )?));
            }
            "--resume" => {
                parsed.resume = Some(args.next().ok_or("--resume needs a directory")?.into());
            }
            "--listen-fd" => parsed.listener_fd = Some(number(args.next(), "--listen-fd")?),
            "--lock-fd" => parsed.lock_fd = Some(number(args.next(), "--lock-fd")?),
            "--tools-fd" => parsed.tools_fd = Some(number(args.next(), "--tools-fd")?),
            "--session" => parsed.sessions.push(handed_over_session(args.next())?),
            flag if flag == UPGRADE_INFO_ARG => parsed.upgrade_info = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(parsed)
}

/// `sikemux core …`: runs the background core in the foreground until it is
/// shut down or sits idle, publishing agents' tool endpoint at
/// `--cli-endpoint`, by default where the CLI looks for it. `args` are the
/// ones after `core`. Returns the exit code.
pub fn main(args: impl Iterator<Item = String>, build: BuildIdentity) -> i32 {
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("sikemux core: {message}");
            eprintln!("{USAGE}");
            return 2;
        }
    };
    if args.upgrade_info {
        return match serde_json::to_string(&UpgradeInfo::of(&build)) {
            Ok(info) => {
                println!("{info}");
                0
            }
            Err(error) => {
                eprintln!("sikemux core: {error}");
                1
            }
        };
    }
    if let Some(directory) = args.resume {
        let mut recovery = Recovery {
            directory,
            socket: args.socket,
            listener_fd: args.listener_fd,
            lock_fd: args.lock_fd,
            tools_fd: args.tools_fd,
            cli_endpoint: args.cli_endpoint,
            data_dir: args.data_dir,
            idle_exit: args.idle_exit.unwrap_or(DEFAULT_IDLE_EXIT),
            sessions: args.sessions,
            inherited: HashMap::new(),
        };
        // Taken before anything opens a descriptor of its own.
        recovery.inherited = recovery
            .listed_fds()
            .into_iter()
            .filter_map(|fd| Some((fd, handover::file_type(fd)?)))
            .collect();
        std::thread::spawn(sikemux_pty::user_shell::warm_login_shell_environment);
        return exit_code(run_runtime(handover::resume(recovery, build)));
    }
    let Some(socket) = args.socket.or_else(crate::default_socket_path) else {
        eprintln!("sikemux core: HOME is not set, so pass --socket");
        return 2;
    };
    std::thread::spawn(sikemux_pty::user_shell::warm_login_shell_environment);
    exit_code(super::run(ServerConfig {
        socket,
        idle_exit: args.idle_exit.unwrap_or(DEFAULT_IDLE_EXIT),
        cli_endpoint: args
            .cli_endpoint
            .or_else(crate::cli::endpoint::default_endpoint_path),
        data_dir: args.data_dir,
        build,
        remote_direct_only: false,
    }))
}

fn run_runtime(
    work: impl std::future::Future<Output = Result<(), ServerError>>,
) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sikemux-core")
        .build()?;
    let result = runtime.block_on(work);
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

fn exit_code(result: Result<(), ServerError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error @ ServerError::AlreadyRunning { .. }) => {
            eprintln!("sikemux core: {error}");
            0
        }
        Err(error) => {
            eprintln!("sikemux core: {error}");
            1
        }
    }
}
