//! On Linux each `sikemux://` link a browser opens, and each file opened from the file
//! manager, starts the app again. A second start hands what it was opened with to the app
//! already running and quits, so one window answers them all, as the Mac's does.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use url::Url;

/// How long a running app waits on a second start to say what it was opened with.
const HANDING: Duration = Duration::from_secs(2);

/// Says whether this start is the app. When another is already running it is given the links
/// and paths this one was opened with, or brought forward when there are none, and this
/// start ends; otherwise this one listens for later starts and sends everything to `opened`.
pub fn take_over(opened: UnboundedSender<Vec<String>>) -> bool {
    let handed = handed_here();
    let Some(socket) = socket() else {
        if !handed.is_empty() {
            let _ = opened.send(handed);
        }
        return true;
    };
    if let Ok(mut running) = UnixStream::connect(&socket) {
        if running.write_all(handed.join("\n").as_bytes()).is_ok() {
            return false;
        }
    }
    let _ = std::fs::remove_file(&socket);
    if let Some(folder) = socket.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(listener) = UnixListener::bind(&socket) {
        let opened = opened.clone();
        let _ = std::thread::Builder::new()
            .name("handoff".into())
            .spawn(move || {
                for start in listener.incoming().flatten() {
                    let _ = start.set_read_timeout(Some(HANDING));
                    let links = BufReader::new(start)
                        .lines()
                        .map_while(Result::ok)
                        .filter(|link| !link.is_empty())
                        .collect();
                    if opened.send(links).is_err() {
                        return;
                    }
                }
            });
    }
    if !handed.is_empty() {
        let _ = opened.send(handed);
    }
    true
}

/// The links and files this start was opened with, the files as `file:` URLs.
fn handed_here() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter_map(|arg| {
            if arg.starts_with("sikemux:") || arg.starts_with("file:") {
                return Some(arg);
            }
            let path = std::fs::canonicalize(&arg).ok()?;
            Url::from_file_path(path).ok().map(String::from)
        })
        .collect()
}

/// In the session's own folder, which only this user can reach; without one, every start is
/// its own app.
fn socket() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    Some(runtime.join(crate::core::IDENTIFIER).join("app.sock"))
}
