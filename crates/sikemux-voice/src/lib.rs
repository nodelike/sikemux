//! Push-to-talk dictation for every Sikemux app. Speech is recorded and transcribed by the
//! `sikemux-voice` helper, which runs NVIDIA Parakeet on the Neural Engine. Released builds
//! download it with the speech model; dev builds run their own. [`Voice`] starts the helper,
//! sends it commands as JSON lines, and puts everything it reports on the app's channel.

mod models;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use tokio::sync::{mpsc, oneshot};

use models::Missing;

/// How long a download the network paused waits before it tries again.
const OFFLINE_RETRY: std::time::Duration = std::time::Duration::from_secs(15);

const DOWNLOAD_OFFLINE: &str =
    "You're offline, so the speech model paused downloading. It carries on when you're back.";

/// The helper would otherwise honour these and fetch models from wherever they point.
const MODEL_FETCH_VARIABLES: [&str; 5] = [
    "REGISTRY_URL",
    "MODEL_REGISTRY_URL",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "HUGGINGFACEHUB_API_TOKEN",
];

/// What dictation reports: the helper's own events, then the download's and the helper's.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum VoiceEvent {
    Progress {
        stage: Stage,
        fraction: f64,
    },
    Ready,
    Listening,
    Partial {
        text: String,
    },
    Transcript {
        text: String,
    },
    Cancelled,
    Error {
        reason: Reason,
        message: String,
    },
    /// The helper stopped without the app asking it to.
    Exited,
}

/// What is being made ready while a [`VoiceEvent::Progress`] goes on.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// The speech model and the helper are downloading.
    Download,
    #[serde(other)]
    Preparing,
}

/// Why dictation failed.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// The network went away mid-download; it carries on once it is back.
    Offline,
    /// The speech model could not be fetched or loaded.
    Models,
    /// Something went wrong with one recording, and dictation still works.
    #[serde(other)]
    Other,
}

impl Reason {
    /// Dictation cannot carry on until the models are in place again.
    pub fn stops_dictation(self) -> bool {
        matches!(self, Reason::Offline | Reason::Models)
    }
}

/// What the app tells the helper.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Command<'a> {
    #[serde(rename_all = "camelCase")]
    Prepare {
        models_dir: &'a Path,
    },
    Start,
    Stop,
    Cancel,
}

struct Helper {
    child: Child,
    stdin: ChildStdin,
    stopped_on_purpose: Arc<AtomicBool>,
}

/// The model download under way, and what stops it.
struct Download {
    thread: JoinHandle<()>,
    _cancel: oneshot::Sender<()>,
}

/// Dictation for one app. Each clone drives the same helper.
#[derive(Clone)]
pub struct Voice {
    dir: PathBuf,
    events: mpsc::UnboundedSender<VoiceEvent>,
    helper: Arc<Mutex<Option<Helper>>>,
    download: Arc<Mutex<Option<Download>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    pub installed: bool,
    pub reason: Option<String>,
}

impl Voice {
    /// Dictation keeping its models in `dir` and reporting on `events`.
    pub fn new(dir: PathBuf, events: mpsc::UnboundedSender<VoiceEvent>) -> Self {
        Voice {
            dir,
            events,
            helper: Arc::default(),
            download: Arc::default(),
        }
    }

    pub fn status(&self) -> Status {
        let reason = unsupported_reason();
        Status {
            supported: reason.is_none(),
            installed: models::installed(&self.dir, local_helper().is_none()),
            reason,
        }
    }

    /// Downloads whatever of the model and helper is missing, then has the helper load the
    /// model, so the first word spoken is not kept waiting.
    pub fn prepare(&self) -> Result<(), String> {
        if let Some(reason) = unsupported_reason() {
            return Err(reason);
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let mut download = self.download.locked();
        if download
            .as_ref()
            .is_some_and(|download| !download.thread.is_finished())
        {
            return Ok(());
        }
        let (cancel, cancelled) = oneshot::channel();
        let voice = self.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(voice.fetch(cancelled)),
                Err(error) => voice.failed(
                    Reason::Models,
                    format!("Could not start the download: {error}"),
                ),
            }
        });
        *download = Some(Download {
            thread,
            _cancel: cancel,
        });
        Ok(())
    }

    /// Fetches what is missing; while the network is away it says so once, then tries
    /// again now and then until the app stops it.
    async fn fetch(&self, mut cancelled: oneshot::Receiver<()>) {
        let mut said_offline = false;
        let fetched = loop {
            let events = self.events.clone();
            let ensure = models::ensure(&self.dir, local_helper().is_none(), |fraction| {
                let _ = events.send(VoiceEvent::Progress {
                    stage: Stage::Download,
                    fraction,
                });
            });
            let fetched = tokio::select! {
                fetched = ensure => fetched,
                _ = &mut cancelled => return,
            };
            match fetched {
                Ok(()) => break Ok(()),
                Err(Missing::Failed(error)) => break Err(error),
                Err(Missing::Offline) => {}
            }
            if !std::mem::replace(&mut said_offline, true) {
                self.failed(Reason::Offline, DOWNLOAD_OFFLINE.to_owned());
            }
            tokio::select! {
                () = tokio::time::sleep(OFFLINE_RETRY) => {}
                _ = &mut cancelled => return,
            }
        };
        let sent = match fetched {
            Err(error) => Err(format!("Could not download the speech model: {error}")),
            Ok(()) => self.send(Command::Prepare {
                models_dir: &self.dir,
            }),
        };
        if let Err(message) = sent {
            self.failed(Reason::Models, message);
        }
    }

    fn failed(&self, reason: Reason, message: String) {
        let _ = self.events.send(VoiceEvent::Error { reason, message });
    }

    pub fn start(&self) -> Result<(), String> {
        self.send(Command::Start)
    }

    pub fn stop(&self) -> Result<(), String> {
        self.send(Command::Stop)
    }

    pub fn cancel(&self) -> Result<(), String> {
        self.send(Command::Cancel)
    }

    /// Stops the download and the helper, as the app quits.
    pub fn shutdown(&self) {
        self.download.locked().take();
        if let Some(mut helper) = self.helper.locked().take() {
            helper.stopped_on_purpose.store(true, Ordering::SeqCst);
            let _ = helper.child.kill();
            let _ = helper.child.wait();
        }
    }

    fn send(&self, command: Command) -> Result<(), String> {
        let mut slot = self.helper.locked();
        let running = slot
            .as_mut()
            .is_some_and(|helper| matches!(helper.child.try_wait(), Ok(None)));
        if !running {
            *slot = Some(self.spawn()?);
        }
        let helper = slot.as_mut().expect("helper was just started");
        let mut line = serde_json::to_string(&command).map_err(|error| error.to_string())?;
        line.push('\n');
        helper
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| helper.stdin.flush())
            .map_err(|error| format!("voice helper stopped listening: {error}"))
    }

    fn spawn(&self) -> Result<Helper, String> {
        let executable = helper_executable(&self.dir)
            .ok_or_else(|| "the voice helper is missing from this build".to_owned())?;
        let mut command = sikemux_process::user_environment::command(executable);
        for variable in MODEL_FETCH_VARIABLES {
            command.env_remove(variable);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start the voice helper: {error}"))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let events = self.events.clone();
        let stopped_on_purpose = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stopped_on_purpose);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<VoiceEvent>(&line) {
                    let _ = events.send(event);
                }
            }
            if !stopped.load(Ordering::SeqCst) {
                let _ = events.send(VoiceEvent::Exited);
            }
        });
        Ok(Helper {
            child,
            stdin,
            stopped_on_purpose,
        })
    }
}

/// A helper built alongside the app, as `make dev` does, rather than downloaded.
fn local_helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SIKEMUX_VOICE_EXECUTABLE") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .ok()?
        .parent()?
        .join("sikemux-voice");
    beside.is_file().then_some(beside)
}

fn helper_executable(models_dir: &Path) -> Option<PathBuf> {
    local_helper()
        .or_else(|| models::helper_is_published().then(|| models::helper_path(models_dir)))
}

#[cfg(target_os = "macos")]
fn unsupported_reason() -> Option<String> {
    use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
    let minimum = NSOperatingSystemVersion {
        majorVersion: 14,
        minorVersion: 0,
        patchVersion: 0,
    };
    if !NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(minimum) {
        return Some("Dictation needs macOS 14 or later.".into());
    }
    (local_helper().is_none() && !models::helper_is_published())
        .then(|| "This build does not include the voice helper.".into())
}

#[cfg(not(target_os = "macos"))]
fn unsupported_reason() -> Option<String> {
    Some("Dictation is only available on macOS.".into())
}
