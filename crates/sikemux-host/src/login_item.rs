//! While remote access is on, the background core starts when the person logs
//! in, so a paired device reaches this host after a restart without Sikemux
//! being opened. A LaunchAgent on macOS, or a systemd user service elsewhere,
//! runs the same command the app starts the core with; turning remote access
//! off removes it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sikemux_core::client::{ensure_running, probe, wait_for_core, ClientError, CoreHello};

#[cfg(target_os = "macos")]
use launchd as service;
#[cfg(not(target_os = "macos"))]
use systemd as service;

const SERVICE_PROBE: Duration = Duration::from_secs(1);
#[cfg(target_os = "macos")]
const SERVICE_START: Duration = Duration::from_secs(5);
/// How long systemd may take to bring the core up, which the first start
/// after an update can take a while to do.
#[cfg(not(target_os = "macos"))]
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
pub const IDENTIFIER: &str = if cfg!(debug_assertions) {
    "com.nodelike.sikemux.dev"
} else {
    "com.nodelike.sikemux"
};

/// What the core is started with after its socket, by the app or a login item.
pub fn core_args(data_dir: Option<PathBuf>) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(endpoint) = sikemux_core::cli::endpoint::default_endpoint_path() {
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
    let (data, logs) = if cfg!(target_os = "macos") {
        (
            home.join("Library/Application Support").join(IDENTIFIER),
            home.join("Library/Logs").join(IDENTIFIER),
        )
    } else {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
            .join(IDENTIFIER);
        (data.clone(), data.join("logs"))
    };
    Some(CoreLaunch {
        binary: std::env::current_exe().ok()?.canonicalize().ok()?,
        socket: sikemux_core::default_socket_path()?,
        log: logs.join("core.log"),
        args: core_args(Some(data)),
    })
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

fn quiet(program: &str, args: &[&str]) {
    let _ = sikemux_process::user_environment::command(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Installs or removes the login item to match `enabled`. Either takes effect
/// at the next login; the core running now, with its terminals and agents,
/// keeps running.
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
    if let Err(error) = install(identifier, launch) {
        eprintln!("Sikemux could not start its core at login: {error}");
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
    if path(identifier).is_some_and(|path| path.exists()) {
        if let Ok(hello) = probe(&launch.socket, SERVICE_PROBE) {
            return Ok(hello);
        }
        start(identifier).map_err(ClientError::Core)?;
        let started = wait_for_core(&launch.socket, SERVICE_START);
        #[cfg(target_os = "macos")]
        if let Ok(hello) = started {
            return Ok(hello);
        }
        #[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
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
{environment}Restart=on-failure
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
}
