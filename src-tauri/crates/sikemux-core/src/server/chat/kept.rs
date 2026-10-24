//! Every chat the core runs, written down whenever it changes, so a core that
//! starts after this one died, or after the host restarted, takes each chat
//! up again by itself and carries on a turn that was cut short.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::user_environment::shell_var;
use tokio::sync::watch;

use crate::launch::providers::{self, ROUTED_ENV};
use crate::protocol::ChatLaunch;

use super::super::callers;
use super::super::Core;
use super::rebind::Replay;
use super::Origin;

/// More chats than anyone runs at once; past this the rest are not read back.
const MAX_KEPT: usize = 1024;
const UNREADABLE: &str = "unreadable";

/// A chat as it is written down. Its launch holds no provider keys and none
/// of the person's shell variables; those are read again as it starts.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeptChat {
    pub launch: ChatLaunch,
    /// Variables of the launch whose values come from the person's shell.
    pub from_shell: Vec<String>,
    /// Variables of the launch that hold model providers' keys.
    pub keys: Vec<String>,
    #[serde(flatten)]
    pub origin: Origin,
    /// The feed whose history file holds everything the chat said.
    pub feed: String,
    pub agent: Option<KeptAgent>,
    /// A turn was at work, on `prompt` when one was sent for it.
    pub working: bool,
    pub prompt: Option<Replay>,
    /// The prompt a lost network stopped, which goes again.
    pub offline: Option<Replay>,
    /// Whether a new agent process can load the chat's session.
    pub reloadable: bool,
    /// Its agent never started: it waits for the person, as a new chat does.
    #[serde(default)]
    pub unstarted: bool,
}

/// The agent's process, which a core that died leaves running.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptAgent {
    pub pid: u32,
    pub started: u64,
}

impl KeptAgent {
    pub(crate) fn of(pid: u32) -> Option<Self> {
        callers::start_time(pid).map(|started| Self { pid, started })
    }

    /// Stops what an earlier core's agent still runs, so it does not work on
    /// the session beside the agent started in its place.
    pub(crate) fn stop(self) {
        if callers::start_time(self.pid) == Some(self.started) {
            super::native::kill_group(self.pid);
        }
    }
}

impl KeptChat {
    /// Takes the keys and the person's shell variables out of the launch,
    /// keeping only their names.
    fn seal(&mut self) {
        let keys = providers::key_names();
        for (name, value) in std::mem::take(&mut self.launch.env) {
            if !value.is_empty() && shell_var(&name).as_deref() == Some(value.as_str()) {
                note(&mut self.from_shell, name);
            } else if keys.contains(name.as_str()) {
                note(&mut self.keys, name);
            } else {
                self.launch.env.insert(name, value);
            }
        }
    }

    /// Whether the chat holds anything to take up: a session, or a turn to send again.
    pub(crate) fn begun(&self) -> bool {
        self.launch.resume_id.is_some()
            || self.working
            || self.prompt.is_some()
            || self.offline.is_some()
    }

    /// The launch as it ran, its keys read again from the Keychain and its
    /// shell variables from the person's shell. Blocks.
    pub(crate) fn unsealed(&self, core: &Core) -> Result<ChatLaunch, String> {
        let mut launch = self.launch.clone();
        for name in &self.from_shell {
            if let Some(value) = shell_var(name) {
                launch.env.insert(name.clone(), value);
            }
        }
        if !self.keys.is_empty() {
            let routed = launch.env.get(ROUTED_ENV).cloned();
            let wired = core.workspaces.wiring(
                &launch.provider,
                routed.as_deref(),
                launch.model.as_deref(),
            )?;
            for name in &self.keys {
                if let Some(value) = wired.env.get(name) {
                    launch.env.insert(name.clone(), value.clone());
                }
            }
        }
        Ok(launch)
    }
}

fn note(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

fn path(dir: &Path, feed: &str) -> PathBuf {
    dir.join(format!("{feed}.json"))
}

fn save(path: &Path, chat: &KeptChat) {
    let written = serde_json::to_vec(chat)
        .map_err(std::io::Error::from)
        .and_then(|bytes| sikemux_process::files::write_atomic(path, &bytes, true));
    if let Err(error) = written {
        eprintln!(
            "sikemux core: chat {} was not written down: {error}",
            chat.launch.agent_id
        );
    }
}

pub(crate) fn forget(dir: &Path, feed: &str) {
    let _ = std::fs::remove_file(path(dir, feed));
}

/// Every chat an earlier core wrote down. One that cannot be read is set aside.
pub(crate) fn read(dir: &Path) -> Vec<KeptChat> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut chats = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let read = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<KeptChat>(&bytes).map_err(|error| error.to_string())
            });
        match read {
            Ok(chat) if chats.len() < MAX_KEPT => chats.push(chat),
            Ok(chat) => eprintln!(
                "sikemux core: chat {} is past the most kept and was not taken up again",
                chat.launch.agent_id
            ),
            Err(error) => set_aside(&path, &error),
        }
    }
    chats
}

/// Moves a chat that cannot be read into a folder beside it that no core
/// reads chats from, so what it held is still there to look at.
fn set_aside(path: &Path, why: &str) {
    let moved =
        unreadable_place(path).and_then(|aside| std::fs::rename(path, &aside).map(|()| aside));
    eprintln!(
        "sikemux core: a chat written down at {} could not be read ({why}); {}",
        path.display(),
        match moved {
            Ok(aside) => format!("it was kept as {}", aside.display()),
            Err(error) => format!("it could not be moved aside: {error}"),
        }
    );
}

/// Where what `path` held is kept once it cannot be read: a folder beside it
/// that nothing reads from.
pub(super) fn unreadable_place(path: &Path) -> std::io::Result<PathBuf> {
    let folder = path.with_file_name(UNREADABLE);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Ok(folder.join(format!("{}-{name}", epoch_ms())))
}

/// Writes a chat down again under `feed`, as a core taking it up names it.
pub(crate) fn moved(dir: &Path, chat: &KeptChat, feed: &str) {
    let mut moved = chat.clone();
    moved.feed = feed.to_owned();
    save(&path(dir, feed), &moved);
    forget(dir, &chat.feed);
}

#[derive(Clone, PartialEq)]
enum Note {
    Nothing,
    Keep(Box<KeptChat>),
    /// The chat is over.
    Forget,
    /// Another core goes on with the chat.
    Leave,
}

/// Writes one chat down, the latest of what it is told, off the async
/// threads and in order.
pub(crate) struct Keeper {
    notes: watch::Sender<Note>,
}

impl Keeper {
    /// Writes the chat whose feed is `feed` down in `dir`.
    pub(crate) fn new(dir: Option<&Path>, feed: &str) -> Self {
        let (notes, read) = watch::channel(Note::Nothing);
        if let Some(dir) = dir {
            tokio::spawn(write(path(dir, feed), read));
        }
        Self { notes }
    }

    pub(crate) fn keep(&self, chat: KeptChat) {
        let next = Note::Keep(Box::new(chat));
        self.notes.send_if_modified(|note| {
            let changed = matches!(note, Note::Nothing | Note::Keep(_)) && *note != next;
            if changed {
                *note = next;
            }
            changed
        });
    }

    pub(crate) fn forget(&self) {
        self.notes.send_replace(Note::Forget);
    }

    pub(crate) fn leave(&self) {
        self.notes.send_replace(Note::Leave);
    }
}

async fn write(path: PathBuf, mut notes: watch::Receiver<Note>) {
    while notes.changed().await.is_ok() {
        let note = notes.borrow_and_update().clone();
        let path = path.clone();
        let goes_on = tokio::task::spawn_blocking(move || match note {
            Note::Nothing => true,
            Note::Keep(mut chat) => {
                chat.seal();
                save(&path, &chat);
                true
            }
            Note::Forget => {
                let _ = std::fs::remove_file(&path);
                false
            }
            Note::Leave => false,
        })
        .await;
        if !matches!(goes_on, Ok(true)) {
            return;
        }
    }
}

/// How a chat's turns go, as its loop knows them.
pub(crate) struct Progress<'a> {
    /// The prompt last sent.
    pub prompt: Option<&'a Replay>,
    /// The prompt the last turn failed on, kept to send again.
    pub failed: Option<&'a Replay>,
    pub reloadable: bool,
}

impl Progress<'_> {
    /// Before the chat's loop has said anything.
    pub(crate) const STARTING: Progress<'static> = Progress {
        prompt: None,
        failed: None,
        reloadable: true,
    };
}
