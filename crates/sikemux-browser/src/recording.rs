//! Records a tab to a video. WebKit has no screencast to tap, so the tab is
//! photographed several times a second and the pictures are encoded. Only the
//! photographs are taken on the page's own thread; encoding has its own.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
use platform::{Picture, Writer};

use crate::manager::BrowserManager;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const MAX_LENGTH: Duration = Duration::from_secs(10 * 60);
const FINISH_TIMEOUT: Duration = Duration::from_secs(40);

/// A recording in progress for one agent. It follows whichever tab the agent
/// is on, so switching tabs mid-recording keeps filming.
pub struct Session {
    path: PathBuf,
    width: usize,
    height: usize,
    stop: Arc<AtomicBool>,
    finished: mpsc::Receiver<Result<Summary, String>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct Frame {
    image: Picture,
    at: Duration,
}

struct Summary {
    frames: u64,
    seconds: f64,
}

/// Takes frames until every sender is dropped, then finishes the file.
struct Encoder {
    frames: mpsc::Sender<Frame>,
    finished: mpsc::Receiver<Result<Summary, String>>,
}

pub async fn start(
    manager: &BrowserManager,
    agent_id: &str,
    path: Option<String>,
) -> Result<Value, String> {
    if manager.recordings.locked().contains_key(agent_id) {
        return Err("this agent is already recording; stop it first".into());
    }
    let (tab_id, view) = manager
        .active_view(agent_id)
        .map_err(|_| "no browser tab is open; call browser_navigate first".to_string())?;
    let path = match path {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() || path.extension().is_none_or(|extension| extension != "mp4") {
                return Err("path must be an absolute path ending in .mp4".into());
            }
            let path = path.with_extension(platform::extension());
            if path.exists() {
                return Err(format!("{} already exists", path.display()));
            }
            path
        }
        None => {
            let folder = manager
                .host()
                .videos_dir()
                .ok_or("there is no folder for videos")?
                .join("Sikemux");
            let title = manager.page(agent_id, &tab_id).unwrap_or_default().title;
            default_path(&folder, &title)
        }
    };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let (width, height) = view
        .ask(platform::size)
        .await
        .map_err(|_| "the tab has no size to record")?;
    let (width, height) = (even(width), even(height));
    let encoder = open(&path, width, height)?;
    let stop = Arc::new(AtomicBool::new(false));
    let filming = manager.this().ok_or("the browser is closing")?;
    crate::spawn(film(
        filming,
        agent_id.to_owned(),
        encoder.frames,
        stop.clone(),
        width as f64,
    ));
    manager.recordings.locked().insert(
        agent_id.to_owned(),
        Session {
            path: path.clone(),
            width,
            height,
            stop,
            finished: encoder.finished,
        },
    );
    Ok(json!({ "recording": true, "path": path, "width": width, "height": height }))
}

pub async fn stop(manager: &BrowserManager, agent_id: &str) -> Result<Value, String> {
    let session = manager
        .recordings
        .locked()
        .remove(agent_id)
        .ok_or("nothing is recording")?;
    session.stop.store(true, Ordering::Release);
    let (path, width, height) = (session.path.clone(), session.width, session.height);
    let summary = tokio::task::spawn_blocking(move || {
        session
            .finished
            .recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| "the video did not finish in time".to_string())
    })
    .await
    .map_err(|error| error.to_string())???;
    Ok(json!({
        "path": path,
        "seconds": (summary.seconds * 10.0).round() / 10.0,
        "frames": summary.frames,
        "width": width,
        "height": height,
    }))
}

/// Photographs the agent's current tab until told to stop, the agent has no
/// tab left, or the recording reaches its length limit.
async fn film(
    manager: Arc<BrowserManager>,
    agent_id: String,
    frames: mpsc::Sender<Frame>,
    stop: Arc<AtomicBool>,
    width: f64,
) {
    let started = Instant::now();
    let mut tick = tokio::time::interval(FRAME_INTERVAL);
    while !stop.load(Ordering::Acquire) && started.elapsed() < MAX_LENGTH {
        tick.tick().await;
        let Ok((_, view)) = manager.active_view(&agent_id) else {
            break;
        };
        let frames = frames.clone();
        view.run(move |webview| {
            platform::capture(
                webview,
                width,
                Box::new(move |image| {
                    if let Some(image) = image {
                        let _ = frames.send(Frame {
                            image,
                            at: started.elapsed(),
                        });
                    }
                }),
            );
        });
    }
}

fn open(path: &Path, width: usize, height: usize) -> Result<Encoder, String> {
    let (frames, incoming) = mpsc::channel::<Frame>();
    let (ready, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("tab-recording".into())
        .spawn(move || match Writer::open(&path, width, height) {
            Ok(writer) => {
                let _ = ready.send(Ok(()));
                let _ = done.send(writer.write(incoming));
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        })
        .map_err(|error| error.to_string())?;
    started
        .recv()
        .map_err(|_| "the recorder stopped before it started".to_string())??;
    Ok(Encoder { frames, finished })
}

/// Video encoders want even sides.
fn even(side: f64) -> usize {
    ((side.round() as usize).max(2) / 2) * 2
}

fn default_path(folder: &Path, title: &str) -> PathBuf {
    let cleaned: String = title
        .chars()
        .map(|char| {
            if matches!(char, '/' | '\\' | ':') {
                '_'
            } else {
                char
            }
        })
        .collect();
    let name = cleaned.trim();
    let name = if name.is_empty() { "Recording" } else { name };
    crate::manager::unique_download_path(folder, &format!("{name}.{}", platform::extension()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sides_are_even_and_never_empty() {
        assert_eq!(even(1201.4), 1200);
        assert_eq!(even(801.0), 800);
        assert_eq!(even(0.0), 2);
    }

    #[test]
    fn a_recording_is_named_after_its_page() {
        let folder = std::env::temp_dir().join("sikemux-recording-names");
        assert_eq!(
            default_path(&folder, "Sign in: Acme/Admin"),
            folder.join(format!("Sign in_ Acme_Admin.{}", platform::extension()))
        );
        assert_eq!(
            default_path(&folder, "  "),
            folder.join(format!("Recording.{}", platform::extension()))
        );
    }
}
