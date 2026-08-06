use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::cli_protocol::{
    CliClientCommand, CliCloseReason, CliEndpointDescriptor, CliOpenRequest, CliOpenTarget,
    CliServerResponse, CliTargetKind, CLI_PROTOCOL_VERSION, MAX_CLI_FRAME_BYTES,
};

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
    let invoked_as_editor = env::args()
        .next()
        .and_then(|value| {
            PathBuf::from(value)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .is_some_and(|stem| stem.eq_ignore_ascii_case("sikemux-editor"));

    if args.is_empty() {
        if invoked_as_editor {
            return Err("editor invocation requires at least one file".into());
        }
        print_help();
        return Ok(0);
    }
    if matches!(args[0].as_str(), "--version" | "-V" | "version") {
        println!("sikemux {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if matches!(args[0].as_str(), "--help" | "-h" | "help") {
        print_help();
        return Ok(0);
    }
    if args[0] == "status" {
        return status();
    }

    if args[0] == "open" {
        args.remove(0);
    }
    let Some(open) = parse_open_args(args, invoked_as_editor)? else {
        return Ok(0);
    };
    execute_open(open)
}

fn print_help() {
    println!(
        "Sikemux CLI {}\n\nUSAGE:\n  sikemux open [--wait] [--project DIR] <PATH[:LINE[:COLUMN]]>...\n  sikemux <PATH[:LINE[:COLUMN]]>...\n  sikemux status\n  sikemux --version\n\nOPTIONS:\n  -w, --wait         Wait until every opened file tab is closed\n  -p, --project DIR  Route unowned files into this project\n  -h, --help         Print help\n  -V, --version      Print version\n\nLINE and COLUMN are one-based. Existing directories open as projects.",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_open_args(args: Vec<String>, invoked_as_editor: bool) -> Result<Option<OpenArgs>, String> {
    let mut wait = invoked_as_editor;
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
                print_help();
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

    let endpoint_path = endpoint_path()?;
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
    let mut stream = connect(&descriptor)?;
    stream
        .set_read_timeout(Some(ACCEPT_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    write_command(&mut stream, &command)?;
    let mut reader = BufReader::new(stream);
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

fn status() -> Result<i32, String> {
    let endpoint_path = endpoint_path()?;
    let descriptor = read_endpoint(&endpoint_path)?;
    let mut stream = connect(&descriptor)?;
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI connection: {error}"))?;
    write_command(
        &mut stream,
        &CliClientCommand::Ping {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token,
        },
    )?;
    let mut reader = BufReader::new(stream);
    match read_response(&mut reader)? {
        CliServerResponse::Pong { version, .. } => {
            println!("Sikemux {version} is running");
            Ok(0)
        }
        CliServerResponse::Error { message } => Err(message),
        _ => Err("unexpected response from Sikemux".into()),
    }
}

fn resolve_target(
    operand: &str,
    cwd: &Path,
    explicit_project: Option<&Path>,
    index: usize,
) -> Result<CliOpenTarget, String> {
    let expanded = expand_home(operand)?;
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
    let expanded = expand_home(&path.to_string_lossy())?;
    let path = absolute_candidate(Path::new(&expanded), cwd);
    fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn expand_home(raw: &str) -> Result<String, String> {
    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .ok_or("home directory is unavailable")?;
        let suffix = raw.strip_prefix('~').unwrap_or_default();
        return Ok(format!("{}{}", PathBuf::from(home).display(), suffix));
    }
    Ok(raw.into())
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
    let start = if kind == CliTargetKind::Directory {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    for ancestor in start.ancestors() {
        if ancestor.join(".git").exists() {
            return ancestor.to_path_buf();
        }
    }
    if path.starts_with(cwd) && cwd.is_dir() {
        return cwd.to_path_buf();
    }
    start.to_path_buf()
}

fn endpoint_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("SIKEMUX_CLI_ENDPOINT") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .ok_or("home directory is unavailable")?;
    Ok(PathBuf::from(home)
        .join(".config/sikemux")
        .join(if cfg!(debug_assertions) {
            "cli.dev.json"
        } else {
            "cli.json"
        }))
}

fn connect_or_launch(endpoint_path: &Path) -> Result<CliEndpointDescriptor, String> {
    if let Ok(descriptor) = read_endpoint(endpoint_path) {
        if probe(&descriptor).is_ok() {
            return Ok(descriptor);
        }
    }
    launch_app()?;
    let started = Instant::now();
    let mut delay = Duration::from_millis(40);
    while started.elapsed() < APP_START_TIMEOUT {
        if let Ok(descriptor) = read_endpoint(endpoint_path) {
            if probe(&descriptor).is_ok() {
                return Ok(descriptor);
            }
        }
        thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_millis(400));
    }
    Err("Sikemux did not become ready within 15 seconds".into())
}

fn probe(descriptor: &CliEndpointDescriptor) -> Result<(), String> {
    let mut stream = connect(descriptor)?;
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| format!("cannot configure CLI probe: {error}"))?;
    write_command(
        &mut stream,
        &CliClientCommand::Ping {
            protocol: CLI_PROTOCOL_VERSION,
            token: descriptor.token.clone(),
        },
    )?;
    let mut reader = BufReader::new(stream);
    match read_response(&mut reader)? {
        CliServerResponse::Pong { protocol, .. } if protocol == CLI_PROTOCOL_VERSION => Ok(()),
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

fn write_command(stream: &mut TcpStream, command: &CliClientCommand) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, command).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn read_response(reader: &mut BufReader<TcpStream>) -> Result<CliServerResponse, String> {
    let mut frame = Vec::new();
    reader
        .by_ref()
        .take(MAX_CLI_FRAME_BYTES + 1)
        .read_until(b'\n', &mut frame)
        .map_err(|error| format!("CLI connection failed: {error}"))?;
    if frame.is_empty() {
        return Err("Sikemux closed the CLI connection".into());
    }
    if frame.len() as u64 > MAX_CLI_FRAME_BYTES {
        return Err("Sikemux sent an oversized CLI response".into());
    }
    serde_json::from_slice(&frame).map_err(|_| "Sikemux sent an invalid CLI response".into())
}

fn launch_app() -> Result<(), String> {
    if let Some(executable) = env::var_os("SIKEMUX_APP_EXECUTABLE") {
        Command::new(executable)
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
                Command::new(executable)
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
        if let Some(bundle) = current
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .filter(|path| path.extension().is_some_and(|extension| extension == "app"))
        {
            Command::new("/usr/bin/open")
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
    Command::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot launch Sikemux: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_open_help_is_a_successful_control_path() {
        assert!(parse_open_args(vec!["--help".into()], false)
            .unwrap()
            .is_none());
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
}
