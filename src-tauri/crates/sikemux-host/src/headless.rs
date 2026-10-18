//! The commands that run this host from a terminal: `status`, `login`,
//! `logout`, `pair`, `start`, `stop`, `restart`, `enable` and `disable`. They
//! are all a host with no window needs to join an account, let devices in and
//! keep its core running.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use sikemux_core::client::{
    ensure_running, probe, ClientError, ClientEvent, CoreClient, ProbeError,
};
use sikemux_core::protocol::{AccountLinkState, DeviceAccess, Event, PendingDevice, RemoteStatus};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::login_item::{self, CoreLaunch, IDENTIFIER};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE_POLL: Duration = Duration::from_millis(200);
/// What `status` exits with while the core is not running, as `systemctl status` does.
const NOT_RUNNING: i32 = 3;

pub const COMMANDS: &[&str] = &[
    "status", "login", "logout", "pair", "start", "stop", "restart", "enable", "disable",
];

/// Runs `command` with the arguments after it.
pub fn run(command: &str, args: &[String]) -> i32 {
    crate::install_tls_crypto();
    let result = match command {
        "status" => return status(args.iter().any(|arg| arg == "--json")),
        "login" => with_core(Work::Login),
        "logout" => with_core(Work::Logout),
        "pair" => with_core(Work::Pair),
        "start" => start().map(|_| ()),
        "stop" => stop(),
        "restart" => restart(),
        "enable" => enable(),
        "disable" => disable(),
        _ => Err(format!("no command named {command}")),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("sikemux {command}: {message}");
            1
        }
    }
}

enum Work {
    Login,
    Logout,
    Pair,
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

fn wait_until(socket: &Path, running: bool) -> bool {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    while running_pid(socket).is_some() != running {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(SETTLE_POLL);
    }
    true
}

/// Starts the core, through the service that starts it at boot when there is
/// one, so it is never a second core outside it. Answers with its socket.
fn start() -> Result<PathBuf, String> {
    let launch = launch()?;
    if let Some(pid) = running_pid(&launch.socket) {
        println!("The core is already running, pid {pid}.");
        return Ok(launch.socket);
    }
    if starts_at_boot().is_some() {
        login_item::start(IDENTIFIER);
        wait_until(&launch.socket, true);
    }
    if let Some(logs) = launch.log.parent() {
        std::fs::create_dir_all(logs).map_err(|error| error.to_string())?;
    }
    let hello = ensure_running(&launch.socket, &launch.binary, &launch.log, &launch.args)
        .map_err(|error| error.to_string())?;
    println!("Started the core, pid {}.", hello.pid);
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
            println!("The core is not running.");
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
    if !wait_until(&socket, false) {
        return Err(format!("the core, pid {pid}, is still running"));
    }
    println!("Stopped the core, pid {pid}, and everything it ran.");
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
    if !wait_until(socket, false) {
        return Err(format!("the core, pid {pid}, is still running"));
    }
    println!("Stopped the core, pid {pid}.");
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
    println!("The core now starts at boot, from {}.", path.display());
    match running_pid(&launch.socket) {
        None => {
            login_item::start(IDENTIFIER);
            wait_until(&launch.socket, true);
        }
        Some(pid) if starts_by_service(pid) => {}
        Some(pid) => println!(
            "The core running now, pid {pid}, was started another way and is not restarted after a crash; `sikemux restart` hands it over."
        ),
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
        true => println!("The core no longer starts at boot. Stop it now with `sikemux stop`."),
        false => println!("The core was not set to start at boot."),
    }
    Ok(())
}

fn with_core(work: Work) -> Result<(), String> {
    let socket = running()?;
    runtime()?.block_on(async {
        let (core, mut events) = CoreClient::connect(&socket)
            .await
            .map_err(|error| error.to_string())?;
        match work {
            Work::Login => login(&core, &mut events).await,
            Work::Logout => {
                let status = core
                    .remote_status()
                    .await
                    .map_err(|error| error.to_string())?;
                if status.owner.is_none() {
                    println!("This host is not signed in.");
                    return Ok(());
                }
                core.set_owner(None)
                    .await
                    .map_err(|error| error.to_string())?;
                println!("Signed out. Devices already paired stay paired.");
                Ok(())
            }
            Work::Pair => pair(&core, &mut events).await,
        }
    })
}

async fn login(
    core: &CoreClient,
    events: &mut UnboundedReceiver<ClientEvent>,
) -> Result<(), String> {
    let owner = core
        .remote_status()
        .await
        .map_err(|error| error.to_string())?
        .owner;
    if let Some(owner) = owner {
        println!(
            "Already signed in to {owner}. Run `sikemux logout` first to use another account."
        );
        return Ok(());
    }
    let account = crate::account::sign_in_by_code(core, |signing| {
        println!(
            "To add this host to your Sikemux account, open\n\n  {}\n\nand check the code {}, or enter it in Sikemux on your phone.",
            signing.verification_url, signing.user_code
        );
        println!("Waiting for approval…");
    })
    .await?;
    println!("Signed in to {account}.\n");
    pair(core, events).await
}

/// Asks here about every device that wants to join, until the person stops it.
async fn pair(
    core: &CoreClient,
    events: &mut UnboundedReceiver<ClientEvent>,
) -> Result<(), String> {
    let mut status = core
        .remote_status()
        .await
        .map_err(|error| error.to_string())?;
    if status.owner.is_none() {
        return Err("this host is not signed in; run `sikemux login` first".into());
    }
    println!(
        "Join this host from Sikemux on your phone, or link it on your Mac under Settings → Devices → Computers. Each device asks here first.\nPress Ctrl-C to stop waiting; `sikemux pair` waits again later."
    );
    let mut asked = HashSet::new();
    loop {
        asked.retain(|id| status.pending.iter().any(|request| &request.id == id));
        let fresh: Vec<PendingDevice> = status
            .pending
            .iter()
            .filter(|request| !asked.contains(&request.id))
            .cloned()
            .collect();
        for request in fresh {
            asked.insert(request.id.clone());
            let access = ask(request.clone()).await?;
            if let Err(error) = core
                .answer_pairing(
                    request.id,
                    access.is_some(),
                    access.unwrap_or(DeviceAccess::Watch),
                )
                .await
            {
                println!("{} could not be answered: {error}", request.name);
            }
        }
        status = next_status(events).await?;
    }
}

async fn next_status(events: &mut UnboundedReceiver<ClientEvent>) -> Result<RemoteStatus, String> {
    loop {
        match events.recv().await {
            Some(ClientEvent::Event(Event::Remote { status })) => return Ok(status),
            Some(_) => {}
            None => return Err("the core went away".into()),
        }
    }
}

async fn ask(request: PendingDevice) -> Result<Option<DeviceAccess>, String> {
    tokio::task::spawn_blocking(move || {
        let mut stdout = std::io::stdout();
        let _ = write!(
            stdout,
            "{} ({}) asks to join. Allow [f]ull control, [w]atch only, or [N]o? ",
            request.name, request.platform
        );
        let _ = stdout.flush();
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        Ok(match line.trim().to_ascii_lowercase().as_str() {
            "f" | "full" => Some(DeviceAccess::Full),
            "w" | "watch" => Some(DeviceAccess::Watch),
            _ => None,
        })
    })
    .await
    .map_err(|error| error.to_string())?
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
    let remote = hello
        .as_ref()
        .and_then(|_| runtime().ok()?.block_on(remote_status(&launch.socket)).ok());
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
    let row = |label: &str, value: String| println!("{label:<9} {value}");
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

async fn remote_status(socket: &Path) -> Result<RemoteStatus, String> {
    let (core, _events) = CoreClient::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    core.remote_status()
        .await
        .map_err(|error| error.to_string())
}
