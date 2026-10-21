use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use uuid::Uuid;

use sikemux_core::cli::client::{attempt, Failure};
use sikemux_core::cli::protocol::{
    CliClientCommand, CliClientHello, CliCloseReason, CliEndpointDescriptor, CliOpenRequest,
    CliOpenTarget, CliServerResponse, CliTargetKind, CLI_PROTOCOL_VERSION, MAX_CLI_RESPONSE_BYTES,
    TOOLS_REPLY_TIMEOUT,
};
use sikemux_core::waiting;
use sikemux_process::vars;

use crate::cli_paths::endpoint_to_read;

/// Starts this binary as the MCP server agents use to reach their tools, instead of the editor.
pub const TOOLS_MCP_FLAG: &str = "--tools-mcp";
const APP_START_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_millis(900);
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct OpenArgs {
    wait: bool,
    project: Option<PathBuf>,
    operands: Vec<String>,
}

pub fn run() -> i32 {
    match run_inner() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sikemux: {message}");
            1
        }
    }
}

fn run_inner() -> Result<i32, String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print!("{}", overview());
        return Ok(0);
    }
    let command = args[0].as_str();
    let wants_help = args[1..].iter().any(|arg| arg == "-h" || arg == "--help");
    match command {
        "--version" | "-V" | "version" => {
            println!("sikemux {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        "--help" | "-h" | "help" => {
            match args.get(1) {
                Some(topic) => print!("{}", command_help(topic).ok_or_else(|| unknown(topic))?),
                None => print!("{}", overview()),
            }
            return Ok(0);
        }
        _ if wants_help && command_help(command).is_some() => {
            print!("{}", command_help(command).unwrap_or_default());
            return Ok(0);
        }
        "tool" => return execute_tool(&args[1..]),
        "doctor" => return doctor(),
        #[cfg(unix)]
        _ if crate::headless::COMMANDS.contains(&command) => {
            return Ok(crate::headless::run(command, &args[1..]));
        }
        "open" => {
            args.remove(0);
        }
        word if looks_like_command(word) => return Err(unknown_word(word)),
        _ => {}
    }
    let Some(open) = parse_open_args(args)? else {
        return Ok(0);
    };
    execute_open(open)
}

/// What `sikemux help` prints. The docs' CLI page is generated from it.
const HELP: &str = r#"Usage: sikemux <command> [options]
       sikemux <path[:line[:column]]>...

This host
  status       Show the core, its account and devices      [--json]
  login        Sign this host in to your Sikemux account
  logout       Take this host off your account
  plugins      Sign this host in to a plugin's service       [login PLUGIN]
  start        Start the core
  stop         Stop the core and everything it runs
  restart      Restart the core, as an update needs
  enable       Start the core at boot
  disable      Stop starting the core at boot

Files and projects
  open         Open files, or folders as projects, in the app
               (the same as `sikemux <path>`)

Troubleshooting
  doctor       Show the latest hang report
  tool         Call one of Sikemux's agent tools
  core         Run the core in the foreground, as the service does

  help [command], -h, --help    Show help
  version, -V, --version        Show the version

To open a file named like a command, write ./name.
"#;

fn overview() -> String {
    format!("Sikemux {}\n\n{HELP}", env!("CARGO_PKG_VERSION"))
}

fn command_help(command: &str) -> Option<&'static str> {
    Some(match command {
        "status" => "Usage: sikemux status [--json]\n\nShows whether the core is running, whether it starts at boot, the account this\nhost is signed in to and its devices. Exits with 3 while the core is not running.\n--json prints the same as one JSON object.\n",
        "login" => "Usage: sikemux login\n\nSigns this host in to your Sikemux account without a browser: it prints a code to\nenter at app.sikemux.com or in Sikemux on your phone. Then your devices and hosts\non the account reach it with nothing more to set up.\n",
        "logout" => "Usage: sikemux logout\n\nTakes this host off your account. Devices already paired stay paired.\n",
        "plugins" => "Usage: sikemux plugins login PLUGIN\n\nSigns this host in to a plugin's service, such as linear, jira, bitbucket or vercel,\nso its agents can use it. It prints a link to open in a browser on any machine. A\nbrowser on another machine then lands on a page that does not load; paste that\npage's address back here to finish.\n",
        "start" => "Usage: sikemux start\n\nStarts the core, through the service that starts it at boot when there is one.\n",
        "stop" => "Usage: sikemux stop\n\nStops the core and the terminals and agents it runs. A core set to start at boot\nstays stopped until `sikemux start` or the next boot.\n",
        "restart" => "Usage: sikemux restart\n\nStops the core and starts it again, so an updated sikemux takes over. Its terminals\nend; chats pick up where they were.\n",
        "enable" => "Usage: sikemux enable\n\nStarts the core at boot and again if it crashes: a systemd user service on Linux,\na LaunchAgent on macOS. On Linux, `loginctl enable-linger` keeps it running\nafter you log out.\n",
        "disable" => "Usage: sikemux disable\n\nStops starting the core at boot. The core keeps running until `sikemux stop`.\n",
        "open" => "Usage: sikemux open [--wait] [--project DIR] <path[:line[:column]]>...\n       sikemux <path[:line[:column]]>...\n\nOpens files, or folders as projects, in the app, starting it when needed.\nLINE and COLUMN are one-based.\n\n  -w, --wait         Wait until every opened file tab is closed\n  -p, --project DIR  Open files that belong to no project in this one\n",
        "doctor" => "Usage: sikemux doctor\n\nShows the latest report the app wrote when its window stopped answering, and\nwhere the rest of it is. It reads them from disk, so it works while the app is stuck.\n",
        "tool" => "Usage: sikemux tool METHOD [JSON_PARAMS]\n\nCalls one of the tools Sikemux gives agents, such as browser_navigate, as the\nagent in this terminal.\n",
        "core" => "Usage: sikemux core [--socket PATH] [--cli-endpoint PATH] [--data-dir PATH]\n\nRuns the core in the foreground. The service and the app start it this way; to\nmanage it, use `sikemux start`, `stop` and `status` instead.\n",
        _ => return None,
    })
}

/// A bare lowercase word that is not a file here was meant as a command.
fn looks_like_command(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        && !Path::new(word).exists()
}

fn unknown_word(word: &str) -> String {
    format!("no command or file named {word}; run `sikemux help` for the commands")
}

fn unknown(word: &str) -> String {
    format!("no command named {word}; run `sikemux help` for the list")
}

fn parse_open_args(args: Vec<String>) -> Result<Option<OpenArgs>, String> {
    let mut wait = false;
    let mut project = None;
    let mut operands = Vec::new();
    let mut literal = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if literal {
            operands.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => literal = true,
            "-w" | "--wait" => wait = true,
            "-p" | "--project" => {
                let value = iter.next().ok_or("--project requires a directory")?;
                project = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print!("{}", command_help("open").unwrap_or_default());
                return Ok(None);
            }
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            _ => operands.push(arg),
        }
    }
    if operands.is_empty() {
        return Err("open requires at least one path".into());
    }
    Ok(Some(OpenArgs {
        wait,
        project,
        operands,
    }))
}

fn execute_open(args: OpenArgs) -> Result<i32, String> {
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?;
    let explicit_project = args
        .project
        .map(|path| resolve_existing(&path, &cwd))
        .transpose()?
        .map(|path| {
            if path.is_dir() {
                Ok(path)
            } else {
                Err("--project must name a directory".to_string())
            }
        })
        .transpose()?;

    let mut targets = Vec::with_capacity(args.operands.len());
    for (index, operand) in args.operands.iter().enumerate() {
        targets.push(resolve_target(
            operand,
            &cwd,
            explicit_project.as_deref(),
            index,
        )?);
    }
    if args.wait
        && targets
            .iter()
            .all(|target| target.kind == CliTargetKind::Directory)
    {
        return Err("--wait requires at least one file target".into());
    }

    let endpoint_path = endpoint_to_read()?;
    let descriptor = connect_or_launch(&endpoint_path)?;
    let request = CliOpenRequest {
        id: format!("request-{}", Uuid::new_v4().simple()),
        cwd: cwd.to_string_lossy().into_owned(),
        wait: args.wait,
        targets,
    };
    let command = CliClientCommand::Open {
        protocol: CLI_PROTOCOL_VERSION,
        token: descriptor.token.clone(),
        request,
    };
    let mut reader = session(&descriptor)?;
    reader
        .get_ref()
        .set_read_timeout(Some(ACCEPT_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    write_command(reader.get_mut(), &command)?;
    let accepted = read_response(&mut reader)?;
    let mut failed = false;
    match accepted {
        CliServerResponse::Accepted {
            failed: failures, ..
        } => {
            for failure in failures {
                eprintln!("sikemux: {}", failure.message);
                failed = true;
            }
        }
        CliServerResponse::Error { message } => return Err(message),
        _ => return Err("unexpected response from Sikemux".into()),
    }
    reader
        .get_mut()
        .set_read_timeout(None)
        .map_err(|error| format!("cannot configure CLI wait: {error}"))?;

    if args.wait {
        match read_response(&mut reader)? {
            CliServerResponse::Closed {
                reason: CliCloseReason::TabsClosed,
                ..
            } => {}
            CliServerResponse::Closed {
                reason: CliCloseReason::AppExit,
                ..
            } => return Err("Sikemux exited before the editor tabs were closed".into()),
            CliServerResponse::Error { message } => return Err(message),
            _ => return Err("unexpected wait response from Sikemux".into()),
        }
    }
    Ok(i32::from(failed))
}

/// Whether the app's window is open, once the core's CLI endpoint answers.
pub fn window_open() -> Option<bool> {
    let descriptor = read_endpoint(&endpoint_to_read().ok()?).ok()?;
    probe(&descriptor).ok()
}

/// Prints the most recent hang autopsy and where to read the rest of it.
///
/// This reads the bundles straight off disk rather than asking the app, so it
/// still answers while the app is the thing that is stuck.
fn doctor() -> Result<i32, String> {
    let directory = crate::hang_reports::autopsy_dir().ok_or("home directory is unavailable")?;
    print!(
        "{}",
        doctor_report(&directory, &crate::hang_reports::list_reports(&directory))
    );
    Ok(0)
}

fn doctor_report(directory: &Path, reports: &[crate::hang_reports::HangReportSummary]) -> String {
    let mut out = format!(
        "Sikemux {}\nAutopsies {}\n",
        env!("CARGO_PKG_VERSION"),
        directory.display()
    );
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

fn resolve_target(
    operand: &str,
    cwd: &Path,
    explicit_project: Option<&Path>,
    index: usize,
) -> Result<CliOpenTarget, String> {
    let expanded = sikemux_process::paths::expand_home(operand)
        .to_string_lossy()
        .into_owned();
    let raw_path = absolute_candidate(Path::new(&expanded), cwd);
    let (path, line, column) = if raw_path.exists() {
        (raw_path, None, None)
    } else {
        let (path_part, line, column) = split_location(&expanded)?;
        (absolute_candidate(Path::new(path_part), cwd), line, column)
    };
    let path = fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let kind = if metadata.is_file() {
        CliTargetKind::File
    } else if metadata.is_dir() {
        CliTargetKind::Directory
    } else {
        return Err(format!(
            "{} is not a regular file or directory",
            path.display()
        ));
    };
    if kind == CliTargetKind::Directory && (line.is_some() || column.is_some()) {
        return Err(format!(
            "{} is a directory and cannot have a line or column",
            path.display()
        ));
    }
    let project_root = explicit_project
        .map(Path::to_path_buf)
        .unwrap_or_else(|| infer_project_root(&path, kind, cwd));
    Ok(CliOpenTarget {
        id: format!("target-{index}-{}", Uuid::new_v4().simple()),
        kind,
        path: path.to_string_lossy().into_owned(),
        project_root: project_root.to_string_lossy().into_owned(),
        line,
        column,
    })
}

fn resolve_existing(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let expanded = sikemux_process::paths::expand_home(&path.to_string_lossy());
    let path = absolute_candidate(&expanded, cwd);
    fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn absolute_candidate(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn split_location(raw: &str) -> Result<(&str, Option<u32>, Option<u32>), String> {
    let Some((before_last, last)) = raw.rsplit_once(':') else {
        return Ok((raw, None, None));
    };
    let Ok(last_number) = last.parse::<u32>() else {
        return Ok((raw, None, None));
    };
    if last_number == 0 {
        return Err("line and column positions are one-based and must be greater than zero".into());
    }
    if let Some((path, line)) = before_last.rsplit_once(':') {
        if let Ok(line_number) = line.parse::<u32>() {
            if line_number == 0 {
                return Err(
                    "line and column positions are one-based and must be greater than zero".into(),
                );
            }
            return Ok((path, Some(line_number - 1), Some(last_number - 1)));
        }
    }
    Ok((before_last, Some(last_number - 1), None))
}

fn infer_project_root(path: &Path, kind: CliTargetKind, cwd: &Path) -> PathBuf {
    if let Some(root) = git_root(path, kind) {
        return root;
    }
    if path.starts_with(cwd) && cwd.is_dir() {
        return cwd.to_path_buf();
    }
    enclosing_folder(path, kind).to_path_buf()
}

/// The project a path opened from outside a shell belongs to: its repository,
/// or else the folder itself, or the folder a file sits in.
pub fn project_root(path: &Path, kind: CliTargetKind) -> PathBuf {
    git_root(path, kind).unwrap_or_else(|| enclosing_folder(path, kind).to_path_buf())
}

fn git_root(path: &Path, kind: CliTargetKind) -> Option<PathBuf> {
    enclosing_folder(path, kind)
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

fn enclosing_folder(path: &Path, kind: CliTargetKind) -> &Path {
    if kind == CliTargetKind::Directory {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

/// The project a command run here works in: the one Sikemux started this
/// shell or agent in, else the repository around the current folder.
pub fn current_project() -> Result<String, String> {
    if let Some(project) = env::var(vars::PROJECT)
        .ok()
        .filter(|project| !project.is_empty())
    {
        return Ok(project);
    }
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    Ok(project_root(&cwd, CliTargetKind::Directory)
        .to_string_lossy()
        .into_owned())
}

/// The endpoint once Sikemux's window is open, starting the app first when
/// it is not running or only its background core is.
fn connect_or_launch(endpoint_path: &Path) -> Result<CliEndpointDescriptor, String> {
    let open = || {
        read_endpoint(endpoint_path)
            .ok()
            .filter(|descriptor| probe(descriptor) == Ok(true))
    };
    if let Some(descriptor) = open() {
        return Ok(descriptor);
    }
    launch_app()?;
    let socket = sikemux_core::default_socket_path().ok_or("home directory is unavailable")?;
    waiting::wait_until(&socket, Instant::now() + APP_START_TIMEOUT, open)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Sikemux did not become ready within 15 seconds".into())
}

/// Whether the window is open, once the endpoint answers.
fn probe(descriptor: &CliEndpointDescriptor) -> Result<bool, String> {
    let mut reader = session(descriptor)?;
    write_command(
        reader.get_mut(),
        &CliClientCommand::Ping {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token.clone(),
        },
    )?;
    match read_response(&mut reader)? {
        CliServerResponse::Pong {
            protocol, window, ..
        } if protocol == CLI_PROTOCOL_VERSION => Ok(window),
        CliServerResponse::Error { message } => Err(message),
        _ => Err("unexpected response from Sikemux".into()),
    }
}

fn read_endpoint(path: &Path) -> Result<CliEndpointDescriptor, String> {
    let file = fs::File::open(path).map_err(|_| "Sikemux is not running".to_string())?;
    let descriptor: CliEndpointDescriptor =
        serde_json::from_reader(file).map_err(|_| "Sikemux CLI endpoint is invalid".to_string())?;
    if descriptor.protocol != CLI_PROTOCOL_VERSION {
        return Err(format!(
            "CLI protocol mismatch (CLI {CLI_PROTOCOL_VERSION}, app {}); update or restart Sikemux",
            descriptor.protocol
        ));
    }
    Ok(descriptor)
}

fn connect(descriptor: &CliEndpointDescriptor) -> Result<TcpStream, String> {
    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, descriptor.port).into(),
        Duration::from_millis(600),
    )
    .map_err(|_| "Sikemux is not running".to_string())?;
    stream
        .set_write_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    Ok(stream)
}

/// Connects and makes the app prove it holds the endpoint token before
/// anything is sent that a program squatting on a stale port should not see.
fn session(descriptor: &CliEndpointDescriptor) -> Result<BufReader<TcpStream>, String> {
    let stream = connect(descriptor)?;
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    let mut reader = BufReader::new(stream);
    let nonce = sikemux_core::cli::auth::new_nonce();
    write_command(
        reader.get_mut(),
        &CliClientHello::Hello {
            protocol: CLI_PROTOCOL_VERSION,
            nonce: nonce.clone(),
        },
    )?;
    let expected =
        sikemux_core::cli::auth::server_proof(&descriptor.token, descriptor.port, &nonce);
    match read_response(&mut reader) {
        Ok(CliServerResponse::Hello { proof })
            if sikemux_core::cli::auth::same_secret(&proof, &expected) =>
        {
            Ok(reader)
        }
        _ => Err(STALE_ENDPOINT.into()),
    }
}

/// A connection to the core's tool endpoint at `endpoint` collecting the
/// plugin stream `ticket` names, which carries the stream's events line by line.
pub fn open_plugin_stream(endpoint: &Path, ticket: String) -> Result<TcpStream, String> {
    let descriptor = read_endpoint(endpoint)?;
    let mut reader = session(&descriptor)?;
    if !reader.buffer().is_empty() {
        return Err("Sikemux sent more than its proof".into());
    }
    write_command(
        reader.get_mut(),
        &CliClientCommand::PluginStream {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token,
            ticket,
        },
    )?;
    let stream = reader.into_inner();
    stream
        .set_read_timeout(None)
        .and_then(|()| stream.set_write_timeout(None))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    Ok(stream)
}

const STALE_ENDPOINT: &str =
    "the program on Sikemux's CLI port is not Sikemux; the endpoint file may be stale";

fn write_command(stream: &mut TcpStream, command: &impl serde::Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, command).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn read_response(reader: &mut BufReader<TcpStream>) -> Result<CliServerResponse, String> {
    let mut frame = Vec::new();
    reader
        .by_ref()
        .take(MAX_CLI_RESPONSE_BYTES + 1)
        .read_until(b'\n', &mut frame)
        .map_err(|error| format!("CLI connection failed: {error}"))?;
    if frame.is_empty() {
        return Err("Sikemux closed the CLI connection".into());
    }
    if frame.len() as u64 > MAX_CLI_RESPONSE_BYTES {
        return Err("Sikemux sent an oversized CLI response".into());
    }
    serde_json::from_slice(&frame).map_err(|_| "Sikemux sent an invalid CLI response".into())
}

fn launch_app() -> Result<(), String> {
    if let Some(executable) = env::var_os("SIKEMUX_APP_EXECUTABLE") {
        sikemux_process::user_environment::command(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot launch Sikemux: {error}"))?;
        return Ok(());
    }
    let current =
        env::current_exe().map_err(|error| format!("cannot locate Sikemux CLI: {error}"))?;
    let current = fs::canonicalize(&current).unwrap_or(current);

    #[cfg(windows)]
    if let Some(directory) = current.parent() {
        let pointer = directory.join(".sikemux-app-executable");
        if let Ok(value) = fs::read_to_string(pointer) {
            let executable = PathBuf::from(value.trim());
            if executable.is_file() && executable != current {
                sikemux_process::user_environment::command(executable)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| format!("cannot launch Sikemux: {error}"))?;
                return Ok(());
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = sikemux_process::paths::app_bundle_of_core(&current) {
            sikemux_process::user_environment::command("/usr/bin/open")
                .arg(bundle)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("cannot launch Sikemux: {error}"))?;
            return Ok(());
        }
    }

    let executable = current
        .parent()
        .ok_or("cannot locate the Sikemux application")?
        .join(if cfg!(windows) {
            "sikemux.exe"
        } else {
            "sikemux"
        });
    sikemux_process::user_environment::command(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot launch Sikemux: {error}"))?;
    Ok(())
}

fn execute_tool(args: &[String]) -> Result<i32, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("usage: sikemux tool METHOD [JSON_PARAMS]".into());
    }
    let params = args
        .get(1)
        .map(|value| serde_json::from_str(value))
        .transpose()
        .map_err(|_| "invalid JSON params")?
        .unwrap_or(serde_json::json!({}));
    let agent_id = env::var(vars::AGENT_ID).ok();
    let value = attempt(
        &endpoint_to_read()?,
        &current_project()?,
        agent_id.as_deref(),
        &args[0],
        &params,
        TOOLS_REPLY_TIMEOUT,
    )
    .map_err(|(Failure::NotSent(message) | Failure::Sent(message))| message)?;
    println!("{value}");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_hello_with(token: &'static str) -> CliEndpointDescriptor {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut hello = String::new();
            BufReader::new(&stream).read_line(&mut hello).unwrap();
            let Ok(CliClientHello::Hello { nonce, .. }) = serde_json::from_str(&hello) else {
                panic!("the client did not open with a hello: {hello}");
            };
            let proof = sikemux_core::cli::auth::server_proof(token, port, &nonce);
            let mut reply = serde_json::to_vec(&CliServerResponse::Hello { proof }).unwrap();
            reply.push(b'\n');
            (&stream).write_all(&reply).unwrap();
        });
        CliEndpointDescriptor {
            protocol: CLI_PROTOCOL_VERSION,
            pid: 1,
            port,
            token: "endpoint-token".into(),
            version: "test".into(),
        }
    }

    #[test]
    fn only_a_server_holding_the_endpoint_token_gets_a_session() {
        assert!(session(&answer_hello_with("endpoint-token")).is_ok());
        assert_eq!(
            session(&answer_hello_with("squatter-token"))
                .err()
                .as_deref(),
            Some(STALE_ENDPOINT)
        );
    }

    #[test]
    fn nested_open_help_is_a_successful_control_path() {
        assert!(parse_open_args(vec!["--help".into()]).unwrap().is_none());
    }

    #[test]
    fn parses_one_based_locations_into_zero_based_positions() {
        assert_eq!(
            split_location("src/main.rs:42:8").unwrap(),
            ("src/main.rs", Some(41), Some(7))
        );
        assert_eq!(
            split_location("src/main.rs:42").unwrap(),
            ("src/main.rs", Some(41), None)
        );
        assert_eq!(
            split_location(r"C:\repo\main.rs:42:8").unwrap(),
            (r"C:\repo\main.rs", Some(41), Some(7))
        );
        assert_eq!(
            split_location("notes:today.md").unwrap(),
            ("notes:today.md", None, None)
        );
        assert!(split_location("src/main.rs:0").is_err());
    }

    #[test]
    fn exact_colon_filename_wins_before_location_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report:12");
        fs::write(&path, "ok").unwrap();
        let target = resolve_target("report:12", dir.path(), None, 0).unwrap();
        assert_eq!(Path::new(&target.path), fs::canonicalize(path).unwrap());
        assert_eq!(target.line, None);
    }

    #[test]
    fn nearest_git_root_becomes_project_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let file = dir.path().join("src/main.rs");
        fs::write(&file, "fn main() {}").unwrap();
        let target = resolve_target(
            file.to_str().unwrap(),
            dir.path().join("src").as_path(),
            None,
            0,
        )
        .unwrap();
        assert_eq!(
            Path::new(&target.project_root),
            fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn doctor_names_the_latest_hang_and_where_its_stacks_are() {
        let reports = [
            crate::hang_reports::HangReportSummary {
                directory: "/autopsy/hang-20260918-142233-471".to_owned(),
                report: "/autopsy/hang-20260918-142233-471/report.json".to_owned(),
                captured_at: "2026-09-18T14:22:33.471Z".to_owned(),
                captured_at_ms: 1_789_000_000_000,
                delay_ms: 4_200,
                summary: "ui froze for 4.2s · focus editor".to_owned(),
                stacks: vec!["/autopsy/hang-20260918-142233-471/webcontent.sample.txt".to_owned()],
            },
            crate::hang_reports::HangReportSummary {
                directory: "/autopsy/hang-20260918-140000-000".to_owned(),
                report: "/autopsy/hang-20260918-140000-000/report.json".to_owned(),
                captured_at: "2026-09-18T14:00:00.000Z".to_owned(),
                captured_at_ms: 1_788_000_000_000,
                delay_ms: 2_500,
                summary: "ui froze for 2.5s".to_owned(),
                stacks: Vec::new(),
            },
        ];

        let rendered = doctor_report(Path::new("/autopsy"), &reports);
        assert!(rendered.contains("Latest hang 2026-09-18T14:22:33.471Z"));
        assert!(rendered.contains("ui froze for 4.2s · focus editor"));
        assert!(rendered.contains("report /autopsy/hang-20260918-142233-471/report.json"));
        assert!(rendered.contains("stack  /autopsy/hang-20260918-142233-471/webcontent.sample.txt"));
        assert!(rendered.contains("Earlier hangs"));
        assert!(rendered.contains("2026-09-18T14:00:00.000Z ui froze for 2.5s"));
    }
}
