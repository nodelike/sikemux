//! The chats that can be opened again, asleep or closed or woken since, each with the history
//! file it keeps, so a core that starts after this one shows them as they were.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_serve::server::kept::Kept;

use super::feed::Reach;
use crate::agent_catalog::AgentKind;
use crate::launch::PermissionMode;
use sikemux_wire::protocol::ChatStart;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shelved {
    /// The name of the history file in the chat's history folder.
    pub feed: String,
    /// The agent's session the history follows.
    pub session: String,
    pub provider: String,
    pub cwd: PathBuf,
    /// The folder the agent keeps its account and sessions in.
    pub account: Option<PathBuf>,
    pub permission_mode: PermissionMode,
    pub claude: bool,
    /// The session the chat last ran, as clients take it up.
    pub start: Option<ChatStart>,
    /// How far into the agent's own record of the session the history reaches.
    pub reach: Option<Reach>,
    /// The agent's own record of the session; once it is gone, so is the chat from Recent.
    pub transcript: Option<PathBuf>,
    /// The chat woke on this history and runs, or ran when its core stopped, so its history
    /// reaches past `reach`.
    pub running: bool,
    pub bytes: u64,
    /// When a client last read the chat, in Unix milliseconds.
    pub read_at: u64,
}

impl Shelved {
    /// The agent's own record of the session, where Sikemux reads that agent's records.
    pub fn find_transcript(&self) -> Option<PathBuf> {
        let kind = AgentKind::from_name(&self.provider)?;
        let root = match &self.account {
            Some(account) => account.clone(),
            None => crate::sessions::roots::account_root(kind, None, &BTreeMap::new())?,
        };
        let cwd = self.cwd.to_string_lossy();
        crate::sessions::transcript::find(kind, &root, Some(&cwd), &self.session)
    }

    fn agent_keeps_records(&self) -> bool {
        AgentKind::from_name(&self.provider)
            .is_some_and(|kind| kind.ops().transcript_said().is_some())
    }
}

pub struct Shelf {
    file: Kept,
    kept: Mutex<HashMap<String, Shelved>>,
}

impl Default for Shelf {
    fn default() -> Self {
        Self {
            file: Kept::new(None, "kept.json"),
            kept: Mutex::default(),
        }
    }
}

impl Shelf {
    /// What an earlier core kept in `history`, less the chats whose file is gone.
    pub fn load(history: Option<&Path>) -> Self {
        let file = Kept::new(history, "kept.json");
        let mut kept: HashMap<String, Shelved> = file.load();
        let listed = kept.len();
        if let Some(history) = history {
            kept.retain(|_, shelved| history.join(format!("{}.jsonl", shelved.feed)).is_file());
        }
        let shelf = Self {
            file,
            kept: Mutex::new(kept),
        };
        if shelf.kept.locked().len() != listed {
            shelf.save();
        }
        shelf
    }

    pub fn get(&self, agent_id: &str) -> Option<Shelved> {
        self.kept.locked().get(agent_id).cloned()
    }

    /// Lists `shelved` for the agent, answering what it listed before.
    pub fn put(&self, agent_id: &str, shelved: Shelved) -> Option<Shelved> {
        let before = self.kept.locked().insert(agent_id.to_owned(), shelved);
        self.save();
        before
    }

    pub fn take(&self, agent_id: &str) -> Option<Shelved> {
        let taken = self.kept.locked().remove(agent_id);
        if taken.is_some() {
            self.save();
        }
        taken
    }

    /// The agent's chat woke on the history it kept, and writes on past where it reached.
    pub fn woke(&self, agent_id: &str) {
        if let Some(shelved) = self.kept.locked().get_mut(agent_id) {
            shelved.running = true;
        }
        self.save();
    }

    pub fn read(&self, agent_id: &str) {
        if let Some(shelved) = self.kept.locked().get_mut(agent_id) {
            shelved.read_at = epoch_ms();
        }
    }

    pub fn holds(&self, agent_id: &str) -> bool {
        self.kept.locked().contains_key(agent_id)
    }

    pub fn feeds(&self) -> Vec<String> {
        (self.kept.locked().values())
            .map(|shelved| shelved.feed.clone())
            .collect()
    }

    /// The chat read least lately while all those kept hold more than `most` bytes.
    pub fn over(&self, most: u64) -> Option<String> {
        let kept = self.kept.locked();
        let bytes: u64 = kept.values().map(|shelved| shelved.bytes).sum();
        (bytes > most)
            .then(|| kept.iter().min_by_key(|(_, shelved)| shelved.read_at))
            .flatten()
            .map(|(agent_id, _)| agent_id.clone())
    }

    /// The chats whose agent keeps no record of their session, or no longer does, so they are
    /// not in Recent. A record not found before is looked for again.
    pub fn gone_from_recent(&self) -> Vec<String> {
        let mut kept = self.kept.locked();
        let mut gone = Vec::new();
        for (agent_id, shelved) in kept.iter_mut() {
            if shelved.transcript.as_deref().is_some_and(Path::exists) {
                continue;
            }
            shelved.transcript = shelved.find_transcript();
            if shelved.transcript.is_none() && shelved.agent_keeps_records() {
                gone.push(agent_id.clone());
            }
        }
        gone
    }

    /// Waits until the list is on disk.
    pub fn flush(&self) {
        self.file.flush();
    }

    fn save(&self) {
        self.file.save_soon(&*self.kept.locked());
    }
}
