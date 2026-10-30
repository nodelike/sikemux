use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use uuid::Uuid;

use sikemux_core::cli::client::{self, attempt, Failure, Session};
use sikemux_core::cli::protocol::{
    CliClientCommand, CliCloseReason, CliEndpointDescriptor, CliOpenRequest, CliOpenTarget,
    CliServerResponse, CliTargetKind, CLI_PROTOCOL_VERSION, TOOLS_REPLY_TIMEOUT,
};
use sikemux_core::waiting;
use sikemux_process::vars;

use crate::cli_paths::endpoint_to_read;

mod doctor;
mod endpoint;
mod open;

use doctor::*;
pub use endpoint::window_open;
use endpoint::*;
use open::*;
pub use open::{current_project, project_root};

/// Starts this binary as the MCP server agents use to reach their tools, instead of the editor.
pub const TOOLS_MCP_FLAG: &str = "--tools-mcp";
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
  update       Update the core to the newest release       [--on|--off]

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
        "update" => "Usage: sikemux update [--on | --off]\n\nOn a Linux host, installs the newest signed release on its channel, stable or\nnightly, and restarts the core into it, keeping its terminals and chats. The\nhost also does this by itself once no chat is in a turn. Only you can turn that\noff or back on, so agents cannot: from the host's ⋯ in Sikemux, or with\n`sudo sikemux update --off` or `--on`. While updates are off, nothing installs,\nnot even with `sikemux update`.\n",
        "open" => "Usage: sikemux open [--wait] [--project DIR] <path[:line[:column]]>...\n       sikemux <path[:line[:column]]>...\n\nOpens files, or folders as projects, in the app, starting it when needed.\nLINE and COLUMN are one-based.\n\n  -w, --wait         Wait until every opened file tab is closed\n  -p, --project DIR  Open files that belong to no project in this one\n",
        "doctor" => "Usage: sikemux doctor\n\nShows the latest reports Sikemux wrote when its window stopped answering or a\nprocess panicked, and where the rest of them are. It reads them from disk, so it\nworks while the app is stuck or gone.\n",
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
    use std::io::{BufRead, BufReader, Write};
    use std::net::Ipv4Addr;

    use sikemux_core::cli::client::STALE_ENDPOINT;
    use sikemux_core::cli::protocol::CliClientHello;

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

        let rendered = doctor_report(Path::new("/autopsy"), &[], &reports);
        assert!(rendered.contains("Latest hang 2026-09-18T14:22:33.471Z"));
        assert!(rendered.contains("ui froze for 4.2s · focus editor"));
        assert!(rendered.contains("report /autopsy/hang-20260918-142233-471/report.json"));
        assert!(rendered.contains("stack  /autopsy/hang-20260918-142233-471/webcontent.sample.txt"));
        assert!(rendered.contains("Earlier hangs"));
        assert!(rendered.contains("2026-09-18T14:00:00.000Z ui froze for 2.5s"));
    }
}
