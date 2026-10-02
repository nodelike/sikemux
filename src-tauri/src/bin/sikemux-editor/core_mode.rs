use std::path::{Path, PathBuf};

use sikemux_core::client::{probe, ClientError, CoreClient, ProbeError};

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// `sikemux core …`: the background process that owns terminals. See
/// [`sikemux_core::server::main`]. `sikemux core stop` ends a running one.
pub fn run() -> i32 {
    let mut args = std::env::args().skip(2).peekable();
    if args.peek().is_some_and(|arg| arg == "stop") {
        return stop(args.nth(1).map(PathBuf::from));
    }
    sikemux_core::server::main(args, sikemux_lib::build_identity())
}

/// Stops the core on `socket`, or this build's own, and everything it runs.
fn stop(socket: Option<PathBuf>) -> i32 {
    let Some(socket) = socket.or_else(sikemux_core::default_socket_path) else {
        eprintln!("sikemux core stop: no socket to stop");
        return 2;
    };
    match probe(&socket, PROBE_TIMEOUT) {
        Err(ProbeError::NotRunning(_)) => {
            println!("No Sikemux core is running on {}", socket.display());
            return 0;
        }
        // A core from another release cannot take the request, so it is told by signal.
        Err(ProbeError::Rejected { pid, .. }) => return terminate(pid, &socket),
        _ => {}
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("sikemux core stop: {error}");
            return 1;
        }
    };
    match runtime.block_on(shut_down(&socket)) {
        Ok(()) => {
            println!("Stopped the Sikemux core on {}", socket.display());
            0
        }
        Err(ClientError::VersionMismatch { pid, .. }) => terminate(pid, &socket),
        Err(error) => {
            eprintln!("sikemux core stop: {error}");
            1
        }
    }
}

async fn shut_down(socket: &Path) -> Result<(), ClientError> {
    let (client, _events) = CoreClient::connect(socket).await?;
    client.shutdown(true).await
}

fn terminate(pid: u32, socket: &Path) -> i32 {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        eprintln!("sikemux core stop: the core gave an impossible process id");
        return 1;
    };
    // SAFETY: kill only sends a signal; it reads and writes no memory of ours.
    let sent = unsafe { libc::kill(pid, libc::SIGTERM) };
    if sent == 0 {
        println!(
            "Stopped the Sikemux core on {} (pid {pid})",
            socket.display()
        );
        0
    } else {
        eprintln!("sikemux core stop: {}", std::io::Error::last_os_error());
        1
    }
}
