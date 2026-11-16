//! While remote access is on, the background core starts when the person logs
//! in, so a paired device reaches this host after a restart without Sikemux
//! being opened. A LaunchAgent on macOS, a systemd user service on Linux, or a
//! Run entry on Windows runs the same command the app starts the core with;
//! turning remote access off removes it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sikemux_client::client::{ClientError, CoreHello};
use sikemux_corekit::client::ensure_running;
use sikemux_corekit::client::probe;
use sikemux_corekit::client::wait_for_core;

#[cfg(target_os = "macos")]
use launchd as service;
#[cfg(windows)]
use run_entry as service;
#[cfg(windows)]
pub use run_entry::{keep_core_up, start_at_sign_in, START_CORE_ARG};
#[cfg(all(unix, not(target_os = "macos")))]
use systemd as service;

const SERVICE_PROBE: Duration = Duration::from_secs(1);
#[cfg(any(target_os = "macos", windows))]
const SERVICE_START: Duration = Duration::from_secs(5);
/// How long systemd may take to bring the core up, which the first start
/// after an update can take a while to do.
#[cfg(all(unix, not(target_os = "macos")))]
const SERVICE_START: Duration = Duration::from_secs(60);

/// How the app starts the core, which the login item repeats.
#[derive(Clone)]
pub struct CoreLaunch {
    pub binary: PathBuf,
    pub socket: PathBuf,
    pub log: PathBuf,
    pub args: Vec<OsString>,
}

impl CoreLaunch {
    #[cfg(unix)]
    fn command(&self) -> impl Iterator<Item = String> + '_ {
        [
            self.binary.as_os_str(),
            "core".as_ref(),
            "--socket".as_ref(),
            self.socket.as_os_str(),
        ]
        .into_iter()
        .chain(self.args.iter().map(OsString::as_os_str))
        .map(|arg| arg.to_string_lossy().into_owned())
    }
}

pub fn label(identifier: &str) -> String {
    format!("{identifier}.core")
}

/// The app's identifier, with `.dev` for dev builds.
pub const IDENTIFIER: &str = sikemux_process::names::APP_IDENTIFIER;

/// What the core is started with after its socket, by the app or a login item.
pub fn core_args(data_dir: Option<PathBuf>) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(endpoint) = sikemux_corekit::cli::endpoint::default_endpoint_path() {
        args.extend([OsString::from("--cli-endpoint"), endpoint.into()]);
    }
    if let Some(data_dir) = data_dir {
        args.extend([OsString::from("--data-dir"), data_dir.into()]);
    }
    args
}

/// How this binary starts the core on a host set up from a terminal, with the
/// folders the app would give it.
pub fn headless_launch() -> Option<CoreLaunch> {
    let home = sikemux_process::paths::home()?;
    let data = data_dir()?;
    let logs = if cfg!(target_os = "macos") {
        home.join("Library/Logs").join(IDENTIFIER)
    } else {
        data.join("logs")
    };
    Some(CoreLaunch {
        binary: real_exe()?,
        socket: sikemux_corekit::default_socket_path()?,
        log: logs.join("core.log"),
        args: core_args(Some(data)),
    })
}

pub use sikemux_process::paths::data_dir;

/// This program, its links followed. Windows has none to follow, and its canonical form
/// starts `\\?\`, which the sign-in entry and the app's own launch do not.
fn real_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    match cfg!(windows) {
        true => Some(exe),
        false => sikemux_process::files::real_path(exe).ok(),
    }
}

/// True when the file was written.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(true)
}

#[cfg(unix)]
fn quiet(program: &str, args: &[&str]) {
    let _ = sikemux_process::user_environment::command(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Installs or removes the login item to match `enabled`. Either takes effect
/// at the next login; the core running now, with its terminals and agents,
/// keeps running. On Windows an installed one also keeps the core up from now on.
pub fn sync(identifier: &str, launch: Option<&CoreLaunch>, enabled: bool) {
    if !enabled {
        if let Err(error) = forget(identifier) {
            eprintln!("Sikemux could not stop starting its core at login: {error}");
        }
        return;
    }
    let Some(launch) = launch else {
        return;
    };
    match install(identifier, launch) {
        Err(error) => eprintln!("Sikemux could not start its core at login: {error}"),
        #[cfg(windows)]
        Ok(_) => run_entry::keep_watching(&label(identifier)),
        #[cfg(not(windows))]
        Ok(_) => {}
    }
}

/// Where the login item is, whether or not it is installed.
pub fn path(identifier: &str) -> Option<PathBuf> {
    service::path(&label(identifier))
}

/// The core `launch` describes, started first when none runs: through the
/// login item when there is one, so it is never a second core beside it, and
/// directly when there is none. Under systemd the service alone starts it: a
/// core started beside a slow service makes the service's own one stop, and
/// the host's core then runs outside it. A LaunchAgent already loaded cannot
/// be started again, so on macOS the core starts directly when it did not.
pub fn start_core(identifier: &str, launch: &CoreLaunch) -> Result<CoreHello, ClientError> {
    #[cfg(windows)]
    if sikemux_trust::service::installed() {
        if let Ok(hello) = probe(&launch.socket, SERVICE_PROBE) {
            return Ok(hello);
        }
        if let Some(logs) = launch.log.parent() {
            std::fs::create_dir_all(logs).map_err(|error| ClientError::Core(error.to_string()))?;
        }
        sikemux_trust::service::start_core(&launch.socket, &launch.log, &launch.args).map_err(
            |error| {
                ClientError::Core(format!(
                    "the Sikemux service did not start the core: {error}"
                ))
            },
        )?;
        return wait_for_core(&launch.socket, SERVICE_START);
    }
    if path(identifier).is_some_and(|path| path.exists()) {
        if let Ok(hello) = probe(&launch.socket, SERVICE_PROBE) {
            return Ok(hello);
        }
        start(identifier).map_err(ClientError::Core)?;
        let started = wait_for_core(&launch.socket, SERVICE_START);
        #[cfg(any(target_os = "macos", windows))]
        if let Ok(hello) = started {
            return Ok(hello);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        return started.map_err(|error| match error {
            ClientError::StartTimeout(_) => ClientError::Core(format!(
                "its service did not start the core within {}s; see `systemctl --user status {}` and {}",
                SERVICE_START.as_secs(),
                service::unit(&label(identifier)),
                launch.log.display()
            )),
            other => other,
        });
    }
    ensure_running(&launch.socket, &launch.binary, &launch.log, &launch.args)
}

pub fn install(identifier: &str, launch: &CoreLaunch) -> std::io::Result<PathBuf> {
    let label = label(identifier);
    let path = service::path(&label).ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    // systemd and launchd open the log themselves and never create its folder.
    if let Some(logs) = launch.log.parent() {
        std::fs::create_dir_all(logs)?;
    }
    let path_env = crate::environment::child_path();
    let contents = service::render(&label, launch, path_env.as_deref());
    if write_if_changed(&path, &contents)? {
        service::installed(&label);
    }
    Ok(path)
}

/// Starts the installed login item now rather than at the next login.
pub fn start(identifier: &str) -> Result<(), String> {
    let label = label(identifier);
    match service::path(&label) {
        Some(path) => service::start(&label, &path),
        None => Ok(()),
    }
}

/// Removes the login item without stopping the core it started.
fn forget(identifier: &str) -> std::io::Result<()> {
    let label = label(identifier);
    let Some(path) = service::path(&label).filter(|path| path.exists()) else {
        return Ok(());
    };
    service::forgotten(&label);
    std::fs::remove_file(&path)
}

/// Makes sure nothing starts the core again once it is stopped, until it is started again.
/// Only Windows keeps its core up from a process of its own.
pub fn stop_restarting(identifier: &str) {
    #[cfg(windows)]
    run_entry::stop_restarting(&label(identifier));
    #[cfg(not(windows))]
    let _ = identifier;
}

/// Stops and removes the login item. False when there was none.
pub fn remove(identifier: &str) -> std::io::Result<bool> {
    let label = label(identifier);
    let Some(path) = service::path(&label).filter(|path| path.exists()) else {
        return Ok(false);
    };
    service::stop(&label);
    std::fs::remove_file(&path)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
mod launchd {
    use std::path::PathBuf;

    use super::{quiet, CoreLaunch};

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn string(value: impl AsRef<std::ffi::OsStr>) -> String {
        format!(
            "<string>{}</string>",
            escape(&value.as_ref().to_string_lossy())
        )
    }

    pub(super) fn path(label: &str) -> Option<PathBuf> {
        Some(
            sikemux_process::paths::home()?
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
        )
    }

    /// Started at login, and again only if it crashed, so Quit and Stop
    /// Everything or a second core finding one running stays stopped. Without
    /// `Interactive`, launchd runs it at background priority, and so every
    /// shell it starts.
    pub(super) fn render(label: &str, launch: &CoreLaunch, path_env: Option<&str>) -> String {
        let arguments: Vec<String> = launch.command().map(string).collect();
        let environment = path_env
            .map(|path| {
                format!(
                    "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    {}\n  </dict>\n",
                    string(path)
                )
            })
            .unwrap_or_default();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  {label}
  <key>ProgramArguments</key>
  <array>
    {arguments}
  </array>
{environment}  <key>ProcessType</key>
  <string>Interactive</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  {log}
  <key>StandardErrorPath</key>
  {log}
</dict>
</plist>
"#,
            label = string(label),
            arguments = arguments.join("\n    "),
            log = string(&launch.log),
        )
    }

    /// launchd reads the folder itself at the next login.
    pub(super) fn installed(_label: &str) {}

    pub(super) fn forgotten(_label: &str) {}

    pub(super) fn start(_label: &str, path: &std::path::Path) -> Result<(), String> {
        // SAFETY: getuid takes no arguments, cannot fail and touches no memory.
        let uid = unsafe { libc::getuid() };
        quiet(
            "/bin/launchctl",
            &["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()],
        );
        Ok(())
    }

    pub(super) fn stop(label: &str) {
        // SAFETY: getuid takes no arguments, cannot fail and touches no memory.
        let uid = unsafe { libc::getuid() };
        quiet(
            "/bin/launchctl",
            &["bootout", &format!("gui/{uid}/{label}")],
        );
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod systemd {
    use std::path::{Path, PathBuf};

    use super::{quiet, CoreLaunch};

    pub(super) fn unit(label: &str) -> String {
        format!("{label}.service")
    }

    /// systemd expands `%` in every value.
    fn quote(text: &str) -> String {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        format!("\"{escaped}\"")
    }

    pub(super) fn path(label: &str) -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| Some(sikemux_process::paths::home()?.join(".config")))?;
        Some(config.join("systemd/user").join(unit(label)))
    }

    /// Restarted only after a crash, so Quit and Stop Everything or a second
    /// core finding one running stays stopped.
    /// The service is the only thing that starts this core, so it never
    /// stops for being idle.
    pub(super) fn render(_label: &str, launch: &CoreLaunch, path_env: Option<&str>) -> String {
        let command: Vec<String> = launch
            .command()
            .chain(["--no-idle-exit".to_owned()])
            .map(|arg| quote(&arg.replace('$', "$$")))
            .collect();
        let environment = path_env
            .map(|path| format!("Environment={}\n", quote(&format!("PATH={path}"))))
            .unwrap_or_default();
        format!(
            "[Unit]
Description=Sikemux core

[Service]
ExecStart={command}
{environment}Delegate=memory
Restart=on-failure
RestartSec=2
StandardOutput=append:{log}
StandardError=append:{log}

[Install]
WantedBy=default.target
",
            command = command.join(" "),
            log = launch.log.to_string_lossy().replace('%', "%%"),
        )
    }

    /// A user service starts at login; only lingering starts it at boot, which
    /// a host nobody logs in to needs.
    pub(super) fn installed(label: &str) {
        quiet("systemctl", &["--user", "daemon-reload"]);
        quiet("systemctl", &["--user", "enable", &unit(label)]);
        let lingering = std::env::var("USER")
            .is_ok_and(|user| Path::new("/var/lib/systemd/linger").join(user).exists());
        if !lingering {
            eprintln!(
                "Sikemux's core now starts when you log in. To start it at boot, run: loginctl enable-linger"
            );
        }
    }

    pub(super) fn start(label: &str, _path: &Path) -> Result<(), String> {
        let output = sikemux_process::user_environment::command("systemctl")
            .args(["--user", "start", &unit(label)])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("could not run systemctl: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(format!(
            "systemctl --user start {} failed: {}",
            unit(label),
            if said.is_empty() {
                output.status.to_string()
            } else {
                said
            }
        ))
    }

    pub(super) fn stop(label: &str) {
        quiet("systemctl", &["--user", "disable", "--now", &unit(label)]);
    }

    pub(super) fn forgotten(label: &str) {
        quiet("systemctl", &["--user", "disable", &unit(label)]);
    }
}

/// A console program started at sign-in opens a console window, so the Run entry starts the
/// app, which has none, with [`START_CORE_ARG`]. The app starts the core's own program
/// detached with the same argument, and that process keeps the core up: it starts the core
/// again whenever it ends with an error, as launchd and systemd do. The file the entry names
/// holds how to start it.
#[cfg(windows)]
mod run_entry {
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};
    use sikemux_corekit::client::{ensure_running, frozen_request, spawn_detached};
    use sikemux_corekit::frozen::FrozenRequest;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    };
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, TerminateProcess, WaitForSingleObject, INFINITE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    use super::CoreLaunch;

    pub const START_CORE_ARG: &str = "--start-core";

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APP: &str = "sikemux.exe";
    const STOP_WAIT: Duration = Duration::from_secs(10);
    /// A core that ends sooner than this after it started is failing to run, not crashing.
    const SHORT_RUN: Duration = Duration::from_secs(60);
    /// How many short runs in a row the core is given before it is left down.
    const SHORT_RUNS: u32 = 5;
    const RETRY_WAIT: Duration = Duration::from_secs(1);
    const WATCH_END_WAIT: Duration = Duration::from_secs(5);

    #[derive(Serialize, Deserialize)]
    struct Saved {
        binary: PathBuf,
        socket: PathBuf,
        log: PathBuf,
        args: Vec<OsString>,
    }

    fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain([0]).collect()
    }

    pub(super) fn path(label: &str) -> Option<PathBuf> {
        Some(super::data_dir()?.join(format!("{label}.json")))
    }

    /// The core's own environment at sign-in is the person's, so it needs no PATH of its own.
    pub(super) fn render(_label: &str, launch: &CoreLaunch, _path_env: Option<&str>) -> String {
        serde_json::to_string_pretty(&Saved {
            binary: launch.binary.clone(),
            socket: launch.socket.clone(),
            log: launch.log.clone(),
            args: launch.args.clone(),
        })
        .unwrap_or_default()
    }

    fn read(path: &Path) -> Option<Saved> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// The app sits beside the core in the install folder.
    pub(super) fn installed(label: &str) {
        let Some(path) = path(label) else {
            return;
        };
        let Some(saved) = read(&path) else {
            return;
        };
        let app = saved.binary.with_file_name(APP);
        let command = format!(
            "\"{}\" {START_CORE_ARG} \"{}\"",
            app.display(),
            path.display()
        );
        let value = wide(&command);
        // SAFETY: every string is nul-terminated UTF-16 that lives for the call, and the size
        // is the value's length in bytes, its nul included.
        let set = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                wide(label).as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if set != 0 {
            eprintln!(
                "Sikemux could not start its core at sign-in: {}",
                std::io::Error::from_raw_os_error(set as i32)
            );
        }
    }

    pub(super) fn forgotten(label: &str) {
        // SAFETY: both strings are nul-terminated UTF-16 that live for the call.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(label).as_ptr()) };
    }

    /// Starts the process that keeps the core up, which starts the core. Where the Sikemux
    /// service is installed, the service starts the core and keeps it up instead.
    pub(super) fn start(_label: &str, path: &Path) -> Result<(), String> {
        let saved = read(path).ok_or_else(|| format!("{} cannot be read", path.display()))?;
        if let Some(logs) = saved.log.parent() {
            std::fs::create_dir_all(logs).map_err(|error| error.to_string())?;
        }
        if sikemux_trust::service::installed() {
            return sikemux_trust::service::start_core(&saved.socket, &saved.log, &saved.args)
                .map(drop)
                .map_err(|error| format!("the Sikemux service did not start the core: {error}"));
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&saved.log)
            .map_err(|error| error.to_string())?;
        let mut command = sikemux_process::user_environment::command(&saved.binary);
        command.arg(START_CORE_ARG).arg(path);
        spawn_detached(command, log).map_err(|error| error.to_string())
    }

    /// Stops the core it started, with everything running in it, and leaves sign-in alone.
    pub(super) fn stop(label: &str) {
        forgotten(label);
        let Some(path) = path(label) else {
            return;
        };
        end_watch(&path);
        if let Some(saved) = read(&path) {
            let _ = frozen_request(&saved.socket, &FrozenRequest::StopEverything, STOP_WAIT);
        }
    }

    /// Ends the process keeping the core up, so a core stopped next stays down.
    pub(super) fn stop_restarting(label: &str) {
        if let Some(path) = path(label) {
            end_watch(&path);
        }
    }

    /// Starts the process that keeps the core up when none runs, such as after `stop`, an
    /// update, or a core that kept failing to run.
    pub(super) fn keep_watching(label: &str) {
        let Some(path) = path(label) else {
            return;
        };
        if watch_file(&path).exists() {
            return;
        }
        if let Err(error) = start(label, &path) {
            eprintln!("Sikemux could not keep its core up: {error}");
        }
    }

    /// What the app runs when started with [`START_CORE_ARG`] and `file`. Returns its exit code.
    pub fn start_at_sign_in(file: &Path) -> i32 {
        match start("", file) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("Sikemux could not start its core at sign-in: {error}");
                1
            }
        }
    }

    /// What the core's program runs when started with [`START_CORE_ARG`] and `file`: it starts
    /// the core and starts it again each time it ends with an error, until it ends cleanly, keeps
    /// failing to run, or `file` is gone. A second one leaves the first to do it.
    pub fn keep_core_up(file: &Path) -> i32 {
        let Some(_watching) = watch(file) else {
            return match read(file)
                .map(|saved| ensure_running(&saved.socket, &saved.binary, &saved.log, &saved.args))
            {
                Some(Err(error)) => {
                    eprintln!("Sikemux could not start its core: {error}");
                    1
                }
                _ => 0,
            };
        };
        let mut short_runs = 0;
        loop {
            let Some(saved) = read(file) else {
                return 0;
            };
            let started = Instant::now();
            let exit = ensure_running(&saved.socket, &saved.binary, &saved.log, &saved.args)
                .map_err(|error| error.to_string())
                .and_then(|hello| exit_code(hello.pid));
            if exit == Ok(0) {
                return 0;
            }
            short_runs = match started.elapsed() < SHORT_RUN {
                true => short_runs + 1,
                false => 1,
            };
            let ended = match exit {
                Ok(code) => format!("ended with {code:#x}"),
                Err(error) => error,
            };
            if short_runs >= SHORT_RUNS {
                eprintln!("Sikemux's core {ended}, {short_runs} times in a row; it is left down");
                return 1;
            }
            eprintln!("Sikemux's core {ended}; starting it again");
            std::thread::sleep(RETRY_WAIT * short_runs);
        }
    }

    fn watch_file(file: &Path) -> PathBuf {
        file.with_extension("watch")
    }

    /// Holds the file that says which process keeps the core up, naming this one. Windows
    /// deletes it once this process ends, however it ends. None while another holds it.
    fn watch(file: &Path) -> Option<File> {
        let mut watching = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .open(watch_file(file))
            .ok()?;
        write!(watching, "{}", std::process::id()).ok()?;
        Some(watching)
    }

    fn end_watch(file: &Path) {
        let watching = watch_file(file);
        let mut pid = String::new();
        let read = File::open(&watching).and_then(|mut opened| opened.read_to_string(&mut pid));
        let Some(pid) = read.ok().and_then(|_| pid.trim().parse::<u32>().ok()) else {
            return;
        };
        // SAFETY: the handle is checked before use and closed after.
        unsafe {
            let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return;
            }
            TerminateProcess(process, 0);
            CloseHandle(process);
        }
        let deadline = Instant::now() + WATCH_END_WAIT;
        while watching.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits for the process `pid` to end, and answers with its exit code.
    fn exit_code(pid: u32) -> Result<u32, String> {
        // SAFETY: the handle is checked before use and closed after, and `code` is writable.
        unsafe {
            let process = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if process.is_null() {
                return Err(format!(
                    "could not be watched: {}",
                    std::io::Error::last_os_error()
                ));
            }
            WaitForSingleObject(process, INFINITE);
            let mut code = 0;
            let read = GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            match read {
                0 => Err(format!(
                    "ended, how is unknown: {}",
                    std::io::Error::last_os_error()
                )),
                _ => Ok(code),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn launch() -> CoreLaunch {
        CoreLaunch {
            binary: "/Applications/Sikemux.app/Contents/MacOS/sikemux-host".into(),
            socket: "/Users/me/.config/sikemux/core.sock".into(),
            log: "/Users/me/Library/Logs/com.nodelike.sikemux/core.log".into(),
            args: vec![
                "--data-dir".into(),
                "/Users/me/Library/Application Support/A&B".into(),
            ],
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_agent_runs_the_core_the_way_the_app_does() {
        let plist = service::render(
            "com.nodelike.sikemux.core",
            &launch(),
            Some("/usr/bin:/bin"),
        );
        assert!(plist.contains("<string>com.nodelike.sikemux.core</string>"));
        assert!(plist.contains(
            "<string>/Applications/Sikemux.app/Contents/MacOS/sikemux-host</string>\n    <string>core</string>\n    <string>--socket</string>\n    <string>/Users/me/.config/sikemux/core.sock</string>\n    <string>--data-dir</string>"
        ));
        assert!(plist.contains("Application Support/A&amp;B"));
        assert!(plist.contains("<key>PATH</key>\n    <string>/usr/bin:/bin</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
    }

    #[test]
    fn an_unchanged_agent_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/x.plist");
        write_if_changed(&path, "one").unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_if_changed(&path, "one").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        write_if_changed(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    }

    #[cfg(windows)]
    fn run_value(label: &str) -> Option<String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
        };
        let wide = |text: &str| {
            std::ffi::OsStr::new(text)
                .encode_wide()
                .chain([0])
                .collect::<Vec<u16>>()
        };
        let mut value = vec![0u16; 4096];
        let mut size = (value.len() * 2) as u32;
        // SAFETY: the strings are nul-terminated UTF-16 and `size` is the buffer's length in bytes.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(r"Software\Microsoft\Windows\CurrentVersion\Run").as_ptr(),
                wide(label).as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                value.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (read == 0).then(|| String::from_utf16_lossy(&value[..size as usize / 2 - 1]))
    }

    #[cfg(windows)]
    #[test]
    fn signing_in_runs_the_app_beside_the_core_until_removed() {
        let identifier = "com.nodelike.sikemux.login-item-test";
        let dir = tempfile::tempdir().unwrap();
        let launch = CoreLaunch {
            binary: dir.path().join("sikemux-host.exe"),
            socket: dir.path().join("core.sock"),
            log: dir.path().join("logs").join("core.log"),
            args: Vec::new(),
        };
        let file = install(identifier, &launch).unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(saved.contains("sikemux-host.exe"));
        assert_eq!(
            run_value(&label(identifier)),
            Some(format!(
                "\"{}\" {START_CORE_ARG} \"{}\"",
                dir.path().join("sikemux.exe").display(),
                file.display()
            ))
        );
        assert!(remove(identifier).unwrap());
        assert_eq!(run_value(&label(identifier)), None);
        assert!(!file.exists());
    }
}
