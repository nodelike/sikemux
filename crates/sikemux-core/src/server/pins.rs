//! Pins the person leaves on a desk's page. Each goes word for word to the agent whose desk it
//! is, stays open until that agent's next turn ends, and is answered with what it said. They
//! live per project in a file of lines, each the whole pin after a change, the last one counting.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::pins::{
    Pin, PinAfterParams, PinBy, PinNote, PinNoteParams, PinParams, PinState, PlacePinParams,
    PlacedPin,
};
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

use super::chat::calls::{prompt_chat, PromptChatParams};
use super::{Core, CoreError, CoreResult, What};
use crate::git::Git;

const FOLDER: &str = "pins";
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// How long a closed pin stays in the file before a new one drops it.
const ANSWERED_KEPT_MS: u64 = 7 * 24 * 60 * 60 * 1000;

pub(crate) struct Pins {
    folder: Option<PathBuf>,
    by_id: std::sync::Mutex<HashMap<String, Pin>>,
}

impl Pins {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        let mut by_id = HashMap::new();
        for pin in folder.as_deref().map(read_all).unwrap_or_default() {
            by_id.insert(pin.id.clone(), pin);
        }
        let now = epoch_ms();
        by_id.retain(|_, pin: &mut Pin| {
            pin.state != PinState::Closed || now.saturating_sub(pin.since) < ANSWERED_KEPT_MS
        });
        Self {
            folder,
            by_id: std::sync::Mutex::new(by_id),
        }
    }

    fn file(&self, project: &str) -> Option<PathBuf> {
        let digest = hex::encode(Sha256::digest(project.as_bytes()));
        let stem = digest.get(..16).unwrap_or(&digest);
        Some(self.folder.as_ref()?.join(format!("{stem}.jsonl")))
    }

    /// Keeps `pin` as it now stands, in memory and in its project's file.
    fn keep(&self, pin: Pin) {
        let mut by_id = self.by_id.locked();
        by_id.insert(pin.id.clone(), pin.clone());
        let Some(path) = self.file(&pin.project) else {
            return;
        };
        let full = std::fs::metadata(&path).is_ok_and(|file| file.len() > MAX_FILE_BYTES);
        let lines: Vec<&Pin> = match full {
            true => (by_id.values())
                .filter(|kept| kept.project == pin.project && kept.state != PinState::Closed)
                .collect(),
            false => vec![&pin],
        };
        let written = (|| -> std::io::Result<()> {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true);
            match full {
                true => options.write(true).truncate(true),
                false => options.append(true),
            };
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&path)?;
            for each in lines {
                let mut line = serde_json::to_vec(each)?;
                line.push(b'\n');
                file.write_all(&line)?;
            }
            Ok(())
        })();
        if let Err(error) = written {
            eprintln!("sikemux core: a pin was not kept: {error}");
        }
    }

    fn get(&self, id: &str) -> CoreResult<Pin> {
        (self.by_id.locked().get(id).cloned()).ok_or_else(|| CoreError::from("That pin is gone"))
    }
}

/// Every file's lines, oldest first, so each pin's last line wins.
fn read_all(folder: &Path) -> Vec<Pin> {
    let Ok(listed) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    (listed.flatten())
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .flat_map(|bytes| {
            (bytes.split(|byte| *byte == b'\n'))
                .filter_map(|line| serde_json::from_slice::<Pin>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The pins every client draws: the open and the answered ones, with whether their agent is at
/// work on them now.
pub(crate) fn shown(core: &Core) -> Vec<Pin> {
    let mut pins: Vec<Pin> = (core.pins.by_id.locked().values())
        .filter(|pin| pin.state != PinState::Closed)
        .cloned()
        .collect();
    for pin in &mut pins {
        pin.working = pin.state == PinState::Open
            && core
                .seen
                .started_at(&pin.agent_id)
                .is_some_and(|started| started >= pin.since);
    }
    pins.sort_by_key(|pin| pin.since);
    pins
}

/// The person places a pin: it is kept, and goes to the agent as their message.
pub(crate) async fn place(core: &Arc<Core>, params: PlacePinParams) -> CoreResult<PlacedPin> {
    let named = core.names.get(&params.agent_id);
    let cwd = (core
        .chats
        .get(&params.agent_id)
        .map(|chat| chat.launch.cwd.clone()))
    .or_else(|| (core.workspaces.agent(&params.agent_id)).map(|agent| agent.cwd.clone()));
    let project = (named.as_ref().map(|named| named.project.clone()))
        .or_else(|| cwd.as_ref().map(|cwd| core.workspaces.project_of(cwd)))
        .ok_or_else(|| CoreError::from("That agent is gone"))?;
    let commit = cwd.and_then(|cwd| {
        Git::quick(&cwd)
            .trimmed(&["rev-parse", "--short", "HEAD"])
            .ok()
    });
    let now = epoch_ms();
    let pin = Pin {
        id: format!("pin-{}", uuid::Uuid::new_v4().simple()),
        project,
        agent_id: params.agent_id.clone(),
        agent_name: named.map(|named| named.name),
        url: params.url.clone(),
        title: params.title.clone(),
        width: params.width,
        height: params.height,
        label: params.label.clone(),
        selector: params.selector.clone(),
        before: params.before.clone(),
        after: None,
        commit,
        notes: vec![PinNote {
            by: PinBy::Person,
            words: params.note.clone(),
            at: now,
        }],
        state: PinState::Open,
        reopened: false,
        since: now,
        working: false,
    };
    send(core, &pin, &params.note, Some(&params.html)).await?;
    let id = pin.id.clone();
    core.pins.keep(pin);
    core.changed(What::View);
    Ok(PlacedPin { id })
}

/// Another note on a pin opens it again and sends it to the same agent.
pub(crate) async fn note(core: &Arc<Core>, params: PinNoteParams) -> CoreResult<()> {
    let words = params.words.trim().to_owned();
    if words.is_empty() {
        return Err(CoreError::from("Say what is still off"));
    }
    let mut pin = core.pins.get(&params.id)?;
    let now = epoch_ms();
    pin.reopened |= pin.state != PinState::Open;
    pin.state = PinState::Open;
    pin.since = now;
    pin.after = None;
    pin.notes.push(PinNote {
        by: PinBy::Person,
        words: words.clone(),
        at: now,
    });
    send(core, &pin, &words, None).await?;
    core.pins.keep(pin);
    core.changed(What::View);
    Ok(())
}

/// The person says the spot looks right: the pin leaves the page.
pub(crate) fn looks_right(core: &Core, params: PinParams) -> CoreResult<()> {
    let mut pin = core.pins.get(&params.id)?;
    pin.state = PinState::Closed;
    pin.since = epoch_ms();
    core.pins.keep(pin);
    core.changed(What::View);
    Ok(())
}

/// The picture of an answered pin's spot, which the app takes once the turn has ended.
pub(crate) fn after(core: &Core, params: PinAfterParams) -> CoreResult<()> {
    let mut pin = core.pins.get(&params.id)?;
    if pin.state != PinState::Answered {
        return Ok(());
    }
    pin.after = Some(params.after);
    core.pins.keep(pin);
    core.changed(What::View);
    Ok(())
}

/// An agent's turn ended: each pin sent before that turn began is answered with what it said. A
/// pin that came in while the turn ran waits for the turn after.
pub(crate) fn turn_ended(core: &Core, agent_id: &str) {
    let Some(started) = core.seen.started_at(agent_id) else {
        return;
    };
    let open: Vec<Pin> = (core.pins.by_id.locked().values())
        .filter(|pin| {
            pin.agent_id == agent_id && pin.state == PinState::Open && pin.since <= started
        })
        .cloned()
        .collect();
    if open.is_empty() {
        return;
    }
    let said = core
        .chats
        .get(agent_id)
        .and_then(|chat| chat.feed.last_answer());
    let now = epoch_ms();
    for mut pin in open {
        pin.state = PinState::Answered;
        pin.since = now;
        if let Some(said) = said.clone() {
            pin.notes.push(PinNote {
                by: PinBy::Agent,
                words: said,
                at: now,
            });
        }
        core.pins.keep(pin);
    }
    core.changed(What::View);
}

/// Sends the pin's line and the person's words to its agent as their message, with where the
/// spot is for the agent alone.
async fn send(core: &Arc<Core>, pin: &Pin, words: &str, html: Option<&str>) -> CoreResult<()> {
    let line = format!("📌 {} · {} · {} wide", pin.label, pin.title, pin.width);
    let mut about = format!(
        "This is a pin the person placed on {} at {} × {}, on the element `{}`.",
        pin.url, pin.width, pin.height, pin.selector
    );
    if let Some(html) = html.filter(|html| !html.is_empty()) {
        about.push_str(&format!("\n\nIts markup: {html}"));
    }
    if let Some(commit) = &pin.commit {
        about.push_str(&format!("\n\nThe page ran from commit {commit}."));
    }
    if let Some(before) = &pin.before {
        about.push_str(&format!("\n\nA picture of the spot: {before}"));
    }
    about.push_str(
        "\n\nA pin gives direction only. Drive the page yourself to check the change before you say it is done.",
    );
    let mut params = PromptChatParams::new(pin.agent_id.clone(), format!("{line}\n{words}"));
    params.context.push(ChatContext {
        uri: format!("{FOR_AGENT_URI}pin/{}", pin.id),
        title: format!("Pin {}", pin.label),
        text: about,
    });
    prompt_chat(core, params).await
}
