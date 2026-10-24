//! The commands that run this host from a terminal: `status`, `login`,
//! `logout`, `start`, `stop`, `restart`, `enable` and `disable`. They are all
//! a host with no window needs to join an account and keep its core running.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use sikemux_core::client::{
    probe, wait_for_core, ClientError, ClientEvent, CoreClient, ProbeError,
};
use sikemux_core::protocol::{AccountLinkState, Event, RemoteStatus, Request, Response};
use sikemux_core::waiting;

use crate::login_item::{self, CoreLaunch, IDENTIFIER};
use crate::style;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
/// What `status` exits with while the core is not running, as `systemctl status` does.
const NOT_RUNNING: i32 = 3;

pub const COMMANDS: &[&str] = &[
    "status", "login", "logout", "start", "stop", "restart", "enable", "disable", "plugins",
];

/// Runs `command` with the arguments after it.
pub fn run(command: &str, args: &[String]) -> i32 {
    crate::install_tls_crypto();
    let result = match command {
        "status" => return status(args.iter().any(|arg| arg == "--json")),
        "login" => with_core(Work::Login),
        "logout" => with_core(Work::Logout),
        "start" => start().map(|_| ()),
        "stop" => stop(),
        "restart" => restart(),
        "enable" => enable(),
        "disable" => disable(),
        "plugins" => plugins(args),
        _ => Err(format!("no command named {command}")),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("  {} {message}", style::red("✗"));
            1
        }
    }
}

enum Work {
    Login,
    Logout,
}

fn launch() -> Result<CoreLaunch, String> {
    login_item::headless_launch().ok_or_else(|| "HOME is not set".into())
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
}

fn starts_at_boot() -> Option<PathBuf> {
    login_item::path(IDENTIFIER).filter(|path| path.exists())
}

fn running_pid(socket: &Path) -> Option<u32> {
    probe(socket, PROBE_TIMEOUT).ok().map(|hello| hello.pid)
}

/// Whether the core at `socket` stopped within the time a core takes to settle.
fn stopped(socket: &Path) -> bool {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    waiting::wait_until(socket, deadline, || {
        running_pid(socket).is_none().then_some(())
    })
    .is_ok_and(|stopped| stopped.is_some())
}

/// Starts the core, through the service that starts it at boot when there is
/// one, so it is never a second core outside it. Answers with its socket.
fn start() -> Result<PathBuf, String> {
    let launch = launch()?;
    if let Some(pid) = running_pid(&launch.socket) {
        style::done("The core is already running", &format!("pid {pid}"));
        return Ok(launch.socket);
    }
    if let Some(logs) = launch.log.parent() {
        std::fs::create_dir_all(logs).map_err(|error| error.to_string())?;
    }
    let hello = login_item::start_core(IDENTIFIER, &launch).map_err(|error| error.to_string())?;
    style::done("Started the core", &format!("pid {}", hello.pid));
    Ok(launch.socket)
}

/// The core's socket, starting the core first when none runs.
fn running() -> Result<PathBuf, String> {
    let socket = launch()?.socket;
    if running_pid(&socket).is_some() {
        return Ok(socket);
    }
    start()
}

/// Stops the core and everything it runs. A core from another release cannot
/// take the request, so it is told by signal.
fn stop() -> Result<(), String> {
    let socket = launch()?.socket;
    let pid = match probe(&socket, PROBE_TIMEOUT) {
        Err(ProbeError::NotRunning(_)) => {
            style::note("The core is not running.");
            return Ok(());
        }
        Err(ProbeError::Rejected { pid, .. }) => return terminate(pid, &socket),
        Err(ProbeError::Unanswered(error)) => return Err(error),
        Ok(hello) => hello.pid,
    };
    match runtime()?.block_on(shut_down(&socket)) {
        Ok(()) => {}
        Err(ClientError::VersionMismatch { pid, .. }) => return terminate(pid, &socket),
        Err(error) => return Err(error.to_string()),
    }
    if !stopped(&socket) {
        return Err(format!("the core, pid {pid}, is still running"));
    }
    style::done(
        "Stopped the core",
        &format!("pid {pid}, and everything it ran"),
    );
    Ok(())
}

async fn shut_down(socket: &Path) -> Result<(), ClientError> {
    let (client, _events) = CoreClient::connect(socket).await?;
    client.shutdown(true).await
}

fn terminate(pid: u32, socket: &Path) -> Result<(), String> {
    let id = libc::pid_t::try_from(pid).map_err(|_| "the core gave an impossible process id")?;
    // SAFETY: kill only sends a signal; it reads and writes no memory of ours.
    if unsafe { libc::kill(id, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if !stopped(socket) {
        return Err(format!("the core, pid {pid}, is still running"));
    }
    style::done("Stopped the core", &format!("pid {pid}"));
    Ok(())
}

/// What an update needs: the new binary only takes over once the core restarts.
fn restart() -> Result<(), String> {
    stop()?;
    start().map(|_| ())
}

fn enable() -> Result<(), String> {
    let launch = launch()?;
    let path = login_item::install(IDENTIFIER, &launch).map_err(|error| error.to_string())?;
    style::done("The core starts at boot", &path.display().to_string());
    match running_pid(&launch.socket) {
        None => {
            login_item::start(IDENTIFIER)?;
            wait_for_core(&launch.socket, SETTLE_TIMEOUT).map_err(|error| error.to_string())?;
        }
        Some(pid) if starts_by_service(pid) => {}
        Some(pid) => style::note(&format!(
            "The core running now, pid {pid}, was started another way; `sikemux restart` hands it to the service."
        )),
    }
    Ok(())
}

/// Whether the service started the core with this pid. Only systemd can say;
/// a LaunchAgent's core is the app's own while the app runs.
fn starts_by_service(pid: u32) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    sikemux_process::user_environment::command("systemctl")
        .args(["--user", "show", "-p", "MainPID", "--value"])
        .arg(format!("{}.service", login_item::label(IDENTIFIER)))
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == pid.to_string())
}

fn disable() -> Result<(), String> {
    match login_item::remove(IDENTIFIER).map_err(|error| error.to_string())? {
        true => {
            style::done("The core no longer starts at boot", "");
            style::note("It keeps running until `sikemux stop`.");
        }
        false => style::note("The core was not set to start at boot."),
    }
    Ok(())
}

fn with_core(work: Work) -> Result<(), String> {
    let socket = running()?;
    runtime()?.block_on(async {
        let (core, _events) = CoreClient::connect(&socket)
            .await
            .map_err(|error| error.to_string())?;
        match work {
            Work::Login => login(&core).await,
            Work::Logout => {
                let status = core
                    .remote_status()
                    .await
                    .map_err(|error| error.to_string())?;
                if status.owner.is_none() {
                    style::note("This host is not signed in.");
                    return Ok(());
                }
                core.set_owner(None)
                    .await
                    .map_err(|error| error.to_string())?;
                style::done("Signed out", "devices already paired stay paired");
                Ok(())
            }
        }
    })
}

async fn login(core: &CoreClient) -> Result<(), String> {
    let owner = core
        .remote_status()
        .await
        .map_err(|error| error.to_string())?
        .owner;
    if let Some(owner) = owner {
        style::done("Already signed in", &owner);
        style::note("To use another account, run `sikemux logout` first.");
        return Ok(());
    }
    let account = crate::account::sign_in_by_code(core, |signing| {
        println!(
            "\n  Open this link to add the host to your account:\n\n    {}\n\n  and enter the code {}, or enter it in Sikemux on your phone.\n",
            style::accent(&signing.verification_url),
            style::bold(&signing.user_code)
        );
        style::note("Waiting for approval…");
    })
    .await?;
    style::done("Signed in", &account);
    style::note(
        "Your devices and hosts on this account reach it now: it shows in Sikemux on each of them.",
    );
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    running: bool,
    pid: Option<u32>,
    version: Option<String>,
    socket: PathBuf,
    starts_at_boot: Option<PathBuf>,
    /// Whether the app's window is open, when the app runs at all.
    window: Option<bool>,
    account: Option<String>,
    /// The host's public key, which the account and its devices know it by.
    core_id: Option<String>,
    /// The host's own name, which it signs in to the account under.
    name: Option<String>,
    account_connection: Option<&'static str>,
    remote_access: Option<bool>,
    devices: usize,
    connected: usize,
    waiting: usize,
}

fn status(json: bool) -> i32 {
    let Ok(launch) = launch() else {
        eprintln!("sikemux status: HOME is not set");
        return 1;
    };
    let hello = probe(&launch.socket, PROBE_TIMEOUT).ok();
    let about = hello
        .as_ref()
        .and_then(|_| runtime().ok()?.block_on(about(&launch.socket)).ok());
    let (remote, name) = about.map_or((None, None), |(remote, name)| (Some(remote), Some(name)));
    let status = Status {
        running: hello.is_some(),
        pid: hello.as_ref().map(|hello| hello.pid),
        version: hello.as_ref().map(|hello| hello.build.version.clone()),
        socket: launch.socket,
        starts_at_boot: starts_at_boot(),
        window: hello
            .as_ref()
            .and_then(|_| crate::cli_client::window_open()),
        account: remote.as_ref().and_then(|remote| remote.owner.clone()),
        core_id: remote
            .as_ref()
            .map(|remote| remote.core_id.clone())
            .filter(|id| !id.is_empty()),
        name,
        account_connection: remote
            .as_ref()
            .and_then(|remote| remote.account.as_ref())
            .map(|link| match link.state {
                AccountLinkState::Connecting => "connecting",
                AccountLinkState::Live => "connected",
                AccountLinkState::Offline => "offline, trying again",
                AccountLinkState::Removed => "taken off the account",
            }),
        remote_access: remote.as_ref().map(|remote| remote.enabled),
        devices: remote.as_ref().map_or(0, |remote| remote.devices.len()),
        connected: remote.as_ref().map_or(0, |remote| remote.connected.len()),
        waiting: remote.as_ref().map_or(0, |remote| remote.pending.len()),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
    } else {
        print_status(&status);
    }
    if status.running {
        0
    } else {
        NOT_RUNNING
    }
}

fn print_status(status: &Status) {
    let row =
        |label: &str, value: String| println!("  {} {value}", style::dim(&format!("{label:<9}")));
    match (status.pid, &status.version) {
        (Some(pid), Some(version)) => row("Core", format!("running, pid {pid}, version {version}")),
        _ => row("Core", "not running; start it with `sikemux start`".into()),
    }
    row(
        "At boot",
        status
            .starts_at_boot
            .as_ref()
            .map_or("no; turn it on with `sikemux enable`".into(), |path| {
                format!("yes, from {}", path.display())
            }),
    );
    if !status.running {
        return;
    }
    match status.window {
        Some(true) => row("App", "window open".into()),
        Some(false) if cfg!(target_os = "macos") => row("App", "window closed".into()),
        _ => {}
    }
    row(
        "Account",
        match (&status.account, status.account_connection) {
            (Some(account), Some(connection)) => format!("{account}, {connection}"),
            (Some(account), None) => account.clone(),
            (None, _) => "not signed in; sign in with `sikemux login`".into(),
        },
    );
    if let Some(enabled) = status.remote_access {
        row(
            "Devices",
            format!(
                "{} paired, {} connected, {} waiting; remote access {}",
                status.devices,
                status.connected,
                status.waiting,
                if enabled { "on" } else { "off" }
            ),
        );
    }
}

fn plugins(args: &[String]) -> Result<(), String> {
    let [verb, plugin] = args else {
        return Err("usage: sikemux plugins login <plugin>".into());
    };
    if verb != "login" {
        return Err(format!("no plugins command named {verb}"));
    }
    let plugin = match plugin.contains('.') {
        true => plugin.clone(),
        false => format!("sikemux.{plugin}"),
    };
    let socket = running()?;
    runtime()?.block_on(plugin_login(&socket, &plugin))
}

/// Signs the host in to a plugin's service through a browser on any machine.
/// A browser on this host comes back to the core by itself; one elsewhere
/// lands on a page that does not load, whose address is pasted back here.
async fn plugin_login(socket: &Path, plugin: &str) -> Result<(), String> {
    const STREAM: u64 = 1;
    let (core, mut events) = CoreClient::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    core.call(
        "pluginSignIn",
        json!({ "stream": STREAM, "plugin": plugin }),
    )
    .await
    .map_err(|error| error.to_string())?;
    let (pasted_into, mut pasted) = tokio::sync::mpsc::unbounded_channel();
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else {
                    return Err("the core ended the sign-in".into());
                };
                let ClientEvent::Event(Event::Stream { stream: STREAM, items, done, error }) = event else {
                    continue;
                };
                for url in items.iter().filter_map(|item| item["url"].as_str()) {
                    println!(
                        "\n  Open this link in a browser on any machine to sign in:\n\n    {}\n\n  If the browser then lands on a page that does not load, paste its address here.\n",
                        style::accent(url)
                    );
                    style::note("Waiting for the sign-in…");
                    let pasted_into = pasted_into.clone();
                    std::thread::spawn(move || {
                        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                            if pasted_into.send(line).is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(error) = error {
                    return Err(error["message"].as_str().unwrap_or("the sign-in failed").to_owned());
                }
                if done {
                    style::done("Signed in", plugin);
                    return Ok(());
                }
            },
            Some(landed) = pasted.recv() => {
                if landed.trim().is_empty() {
                    continue;
                }
                if let Err(error) = core.call("signInCallback", json!({ "target": landed })).await {
                    eprintln!("  {} {error}", style::red("✗"));
                }
            }
        }
    }
}

/// What the core says of remote access, and the host's own name.
async fn about(socket: &Path) -> Result<(RemoteStatus, String), String> {
    let (core, _events) = CoreClient::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    let remote = core
        .remote_status()
        .await
        .map_err(|error| error.to_string())?;
    match core
        .request(Request::Host)
        .await
        .map_err(|error| error.to_string())?
    {
        Response::Host { host } => Ok((remote, host.name)),
        _ => Err("the core answered with something else".into()),
    }
}
