//! Reaching the app through its CLI endpoint, starting it first when it is not running.

use super::*;

pub(super) const APP_START_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_millis(600);

pub(super) const PROBE_TIMEOUT: Duration = Duration::from_millis(900);

/// Whether the app's window is open, once the core's CLI endpoint answers.
pub fn window_open() -> Option<bool> {
    let descriptor = read_endpoint(&endpoint_to_read().ok()?).ok()?;
    probe(&descriptor).ok()
}

/// The endpoint once Sikemux's window is open, starting the app first when
/// it is not running or only its background core is.
pub(super) fn connect_or_launch(endpoint_path: &Path) -> Result<CliEndpointDescriptor, String> {
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
pub(super) fn probe(descriptor: &CliEndpointDescriptor) -> Result<bool, String> {
    let mut session = session(descriptor)?;
    session.send(&CliClientCommand::Ping {
        protocol: CLI_PROTOCOL_VERSION,
        token: descriptor.token.clone(),
    })?;
    match session.receive()? {
        CliServerResponse::Pong {
            protocol, window, ..
        } if protocol == CLI_PROTOCOL_VERSION => Ok(window),
        CliServerResponse::Error { message } => Err(message),
        _ => Err("unexpected response from Sikemux".into()),
    }
}

pub(super) fn read_endpoint(path: &Path) -> Result<CliEndpointDescriptor, String> {
    let descriptor = client::read_endpoint(path)?;
    if descriptor.protocol != CLI_PROTOCOL_VERSION {
        return Err(format!(
            "CLI protocol mismatch (CLI {CLI_PROTOCOL_VERSION}, app {}); update or restart Sikemux",
            descriptor.protocol
        ));
    }
    Ok(descriptor)
}

pub(super) fn session(descriptor: &CliEndpointDescriptor) -> Result<Session, String> {
    Session::open(descriptor, CONNECT_TIMEOUT, PROBE_TIMEOUT)
}

pub(super) fn launch_app() -> Result<(), String> {
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
