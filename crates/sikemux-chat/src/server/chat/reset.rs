//! Chats stopped by a usage limit, each waiting for the limit to reset to
//! send its stopped prompt again. Each waits on a timer of its own, and the
//! list is kept on disk so a core that restarts goes on waiting.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
use sikemux_fold::update::notice;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::agent_accounts::usage;
use sikemux_fold::ChatEventKind;
use sikemux_wire::protocol::ChatLaunch;

use super::super::Core;
use super::kept;
use super::rebind::Replay;
use super::{Chat, ChatCommand};
pub use sikemux_corekit::wall_clock::{calendar, when, WallClock, Zone};
use sikemux_process::clock::epoch_ms;

const FILE: &str = "limit-resumes.json";
const MAX_WAITING: usize = 256;
/// How long a chat woken early by a reading waits before another can wake it early again, so a
/// reading that shows room the account does not have cannot keep waking it.
const EARLY_AGAIN_AFTER_MS: u64 = 15 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Waiting {
    agent_id: String,
    /// When the limit resets, in Unix milliseconds.
    at: u64,
    prompt: Replay,
    /// When it began waiting, or this core started: only a reading taken after it can wake it
    /// before the reset.
    #[serde(skip, default = "epoch_ms")]
    since: u64,
}

enum Slot {
    /// The reset time is being looked up; `ticket` tells a lookup that the
    /// chat was sent something meanwhile.
    Looking { ticket: u64 },
    Waiting {
        waiting: Waiting,
        timer: Option<tokio::task::AbortHandle>,
    },
}

#[derive(Default)]
pub struct LimitResumes {
    path: Option<PathBuf>,
    slots: Mutex<HashMap<String, Slot>>,
    tickets: AtomicU64,
    /// When each chat a reading woke early was woken.
    woken_early: Mutex<HashMap<String, u64>>,
}

impl LimitResumes {
    pub fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join(FILE));
        let saved = path.as_deref().map(read_saved).unwrap_or_default();
        let slots = saved
            .into_iter()
            .take(MAX_WAITING)
            .map(|waiting| {
                let agent_id = waiting.agent_id.clone();
                let timer = None;
                (agent_id, Slot::Waiting { waiting, timer })
            })
            .collect();
        Self {
            path,
            slots: Mutex::new(slots),
            tickets: AtomicU64::new(0),
            woken_early: Mutex::default(),
        }
    }

    fn save(&self, slots: &HashMap<String, Slot>) {
        let Some(path) = &self.path else {
            return;
        };
        let waiting: Vec<&Waiting> = slots
            .values()
            .filter_map(|slot| match slot {
                Slot::Waiting { waiting, .. } => Some(waiting),
                Slot::Looking { .. } => None,
            })
            .collect();
        let written = serde_json::to_vec(&waiting)
            .map_err(std::io::Error::from)
            .and_then(|bytes| sikemux_process::files::write_atomic(path, &bytes, true));
        if let Err(error) = written {
            eprintln!("sikemux core: the chats waiting for a usage limit were not saved: {error}");
        }
    }

    /// Starts the timers of the chats an earlier core left waiting.
    pub fn arm_saved(&self, core: &Arc<Core>) {
        for slot in self.slots.locked().values_mut() {
            if let Slot::Waiting { waiting, timer } = slot {
                *timer = Some(arm(core, waiting));
            }
        }
    }

    /// When the chat sends its stopped prompt again, once its usage limit resets.
    pub fn resumes_at(&self, agent_id: &str) -> Option<u64> {
        match self.slots.locked().get(agent_id)? {
            Slot::Waiting { waiting, .. } => Some(waiting.at),
            Slot::Looking { .. } => None,
        }
    }

    /// Stops the chat waiting, saying whether it was.
    pub fn cancel(&self, agent_id: &str) -> bool {
        let mut slots = self.slots.locked();
        let Some(slot) = slots.remove(agent_id) else {
            return false;
        };
        let Slot::Waiting { timer, .. } = slot else {
            return false;
        };
        if let Some(timer) = timer {
            timer.abort();
        }
        self.save(&slots);
        true
    }

    /// The chats a reading shows room for, each made due at once.
    fn due_with_room(&self, core: &Core) -> Vec<String> {
        let now = epoch_ms();
        let waiting: Vec<(String, u64)> = (self.slots.locked().values())
            .filter_map(|slot| match slot {
                Slot::Waiting { waiting, .. } if waiting.at > now => {
                    Some((waiting.agent_id.clone(), waiting.since))
                }
                _ => None,
            })
            .collect();
        if waiting.is_empty() {
            return Vec::new();
        }
        let mut early = self.woken_early.locked();
        early.retain(|_, woken| now.saturating_sub(*woken) < EARLY_AGAIN_AFTER_MS);
        let roomy: Vec<String> = (waiting.into_iter())
            .filter(|(agent_id, _)| !early.contains_key(agent_id))
            .filter(|(agent_id, since)| has_room(core, agent_id, *since, now))
            .map(|(agent_id, _)| agent_id)
            .collect();
        let mut slots = self.slots.locked();
        let due: Vec<String> = (roomy.into_iter())
            .filter(|agent_id| match slots.get_mut(agent_id) {
                Some(Slot::Waiting { waiting, timer }) => {
                    waiting.at = now;
                    if let Some(timer) = timer.take() {
                        timer.abort();
                    }
                    true
                }
                _ => false,
            })
            .collect();
        if !due.is_empty() {
            self.save(&slots);
        }
        early.extend(due.iter().map(|agent_id| (agent_id.clone(), now)));
        due
    }

    /// The chat's stopped prompt, once its limit has reset.
    fn take_due(&self, agent_id: &str) -> Option<Replay> {
        let mut slots = self.slots.locked();
        let due = matches!(
            slots.get(agent_id),
            Some(Slot::Waiting { waiting, .. }) if waiting.at <= epoch_ms()
        );
        if !due {
            return None;
        }
        let Some(Slot::Waiting { waiting, .. }) = slots.remove(agent_id) else {
            return None;
        };
        self.save(&slots);
        Some(waiting.prompt)
    }
}

/// The chats an earlier core left waiting. Those that cannot be read are
/// left out, and a copy of the file is kept so nothing they held is lost.
fn read_saved(path: &Path) -> Vec<Waiting> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let entries = match serde_json::from_slice::<Vec<Value>>(&bytes) {
        Ok(entries) => entries,
        Err(error) => {
            keep_unreadable(path, &error.to_string());
            return Vec::new();
        }
    };
    let mut unreadable = None;
    let waiting = entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value(entry)
                .map_err(|error| unreadable.get_or_insert(error.to_string()).clone())
                .ok()
        })
        .collect();
    if let Some(why) = unreadable {
        keep_unreadable(path, &why);
    }
    waiting
}

fn keep_unreadable(path: &Path, why: &str) {
    let copied =
        kept::unreadable_place(path).and_then(|aside| std::fs::copy(path, &aside).map(|_| aside));
    eprintln!(
        "sikemux core: {} holds a chat waiting for a usage limit that could not be read ({why}); {}",
        path.display(),
        match copied {
            Ok(aside) => format!("a copy of it was kept as {}", aside.display()),
            Err(error) => format!("it could not be copied aside: {error}"),
        }
    );
}

/// Waits for the chat's usage limit to reset to send `prompt` again, when
/// `resets_at` or the account's usage says when that is. When it can't, the
/// failed turn waits on the person instead.
pub fn schedule(chat: &Arc<Chat>, launch: &ChatLaunch, prompt: Replay, resets_at: Option<u64>) {
    let Some(core) = chat.core.upgrade() else {
        return;
    };
    let resets = &core.chats().resets;
    let ticket = resets.tickets.fetch_add(1, Ordering::Relaxed);
    {
        let mut slots = resets.slots.locked();
        if slots.len() >= MAX_WAITING && !slots.contains_key(chat.agent_id()) {
            drop(slots);
            not_retrying(&core, chat);
            return;
        }
        if let Some(Slot::Waiting {
            timer: Some(timer), ..
        }) = slots.insert(chat.agent_id().to_owned(), Slot::Looking { ticket })
        {
            timer.abort();
        }
    }
    let (chat, launch) = (chat.clone(), launch.clone());
    tokio::spawn(async move {
        let at = match resets_at {
            None => reset_from_usage(&core, &launch).await,
            known => known,
        };
        let resets = &core.chats().resets;
        let mut slots = resets.slots.locked();
        let current = slots.get(chat.agent_id());
        if !matches!(current, Some(Slot::Looking { ticket: own }) if *own == ticket) {
            return;
        }
        let Some(at) = at.filter(|at| *at > epoch_ms()) else {
            slots.remove(chat.agent_id());
            drop(slots);
            not_retrying(&core, &chat);
            return;
        };
        let waiting = Waiting {
            agent_id: chat.agent_id().to_owned(),
            at,
            prompt,
            since: epoch_ms(),
        };
        let timer = Some(arm(&core, &waiting));
        slots.insert(chat.agent_id().to_owned(), Slot::Waiting { waiting, timer });
        resets.save(&slots);
        drop(slots);
        say(&chat, resume_notice(&launch.provider, at));
    });
}

fn not_retrying(core: &Core, chat: &Chat) {
    (core.chat_above().inbox_not_retrying)(core, chat.agent_id());
    (core.chat_above().board_stopped_waiting)(core, chat.agent_id());
    core.changed(What::View);
}

/// Stops the chat waiting for its limit to reset, as something sent to it
/// takes the place of its stopped prompt.
pub fn cancel(chat: &Chat) {
    if let Some(core) = chat.core.upgrade() {
        core.chats().resets.cancel(chat.agent_id());
    }
}

/// The chat's stopped prompt, when its limit reset while it was not running.
pub fn take_due(chat: &Chat) -> Option<Replay> {
    chat.core
        .upgrade()?
        .chats()
        .resets
        .take_due(chat.agent_id())
}

/// Stops the chat waiting at the person's word, and says so in it.
pub fn cancel_by_person(core: &Core, agent_id: &str) {
    if core.chats().resets.cancel(agent_id) {
        if let Some(chat) = core.chats().get(agent_id) {
            let mut update = notice(
                "info",
                "This chat will not resume when the limit resets",
                None,
            );
            update["resumesAt"] = Value::Null;
            say(&chat, update);
        }
    }
}

/// Resumes each waiting chat as soon as a reading shows room on an account it may run on, rather
/// than at the reset. One whose own account is still full moves at its next limit, as a chat at
/// work does.
pub fn wake_on_readings(core: &Arc<Core>) -> tokio::task::JoinHandle<()> {
    let mut landed = usage::readings_landed();
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        while landed.changed().await.is_ok() {
            let Some(core) = core.upgrade() else {
                return;
            };
            for agent_id in core.chats().resets.due_with_room(&core) {
                let core = core.clone();
                tokio::spawn(async move { resume(&core, agent_id).await });
            }
        }
    })
}

/// Whether a reading taken after `since` shows room on the chat's own account, or with smart
/// accounts on, on another it may move to.
fn has_room(core: &Core, agent_id: &str, since: u64, now: u64) -> bool {
    let (Some(agent), Ok(takers)) = (
        core.workspaces().agent(agent_id),
        super::accounts(core, agent_id),
    ) else {
        return false;
    };
    let book = core.workspaces().accounts().unwrap_or_default();
    let roomy = |env: &BTreeMap<String, String>| {
        let folder = crate::launch::named_account(&agent.provider, env);
        usage::last_reading(&agent.provider, folder).is_some_and(|reading| {
            let windows = &reading.usage.windows;
            reading.read_at > since
                && !windows.is_empty()
                && windows.iter().all(|window| window.used_at(now) < 100.0)
        })
    };
    let own = takers
        .current
        .as_ref()
        .map_or_else(BTreeMap::new, |account| account.env.clone());
    roomy(&own)
        || (book.smart
            && (takers.others.iter())
                .filter(|other| book.allowed_in(&agent.cwd, &other.id))
                .any(|other| roomy(&other.env)))
}

fn arm(core: &Arc<Core>, waiting: &Waiting) -> tokio::task::AbortHandle {
    let wait = Duration::from_millis(waiting.at.saturating_sub(epoch_ms()));
    let (core, agent_id) = (Arc::downgrade(core), waiting.agent_id.clone());
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        if let Some(core) = core.upgrade() {
            resume(&core, agent_id).await;
        }
    })
    .abort_handle()
}

/// Sends the chat its stopped prompt again. A chat not running is woken, and
/// takes the prompt up as it starts; one that cannot be woken now takes it up
/// whenever it next starts.
async fn resume(core: &Arc<Core>, agent_id: String) {
    let resets = &core.chats().resets;
    if let Some(chat) = core.chats().get(&agent_id) {
        if let Some(prompt) = resets.take_due(&agent_id) {
            let _ = chat.send(ChatCommand::Resume { prompt });
        }
        return;
    }
    if core.workspaces().closed(&agent_id) {
        resets.cancel(&agent_id);
        return;
    }
    if let Err(error) = (core.chat_above().wake_chat)(core, agent_id.clone()).await {
        eprintln!("sikemux core: chat {agent_id} waits to resume until it next starts: {error}");
    }
}

/// When the account the chat runs on can be used again, from its usage.
async fn reset_from_usage(core: &Core, launch: &ChatLaunch) -> Option<u64> {
    let provider = launch.provider.as_str();
    let folder = crate::launch::named_account(provider, &launch.env);
    let program = core
        .workspaces()
        .account_spec(provider, folder)
        .program()
        .await
        .ok()?;
    usage::usage(provider, &program, folder)
        .await
        .ok()?
        .resets_at()
}

fn say(chat: &Chat, update: Value) {
    let Some(start) = chat.feed.start() else {
        return;
    };
    chat.emit(
        ChatEventKind::SessionUpdate,
        json!({ "sessionId": start.session_id, "update": update }),
    );
}

fn resume_notice(provider: &str, at: u64) -> Value {
    let agent = crate::agent_catalog::AGENTS
        .iter()
        .find(|agent| agent.kind == provider)
        .map_or(provider, |agent| agent.label);
    let mut update = notice(
        "info",
        &format!("Resumes {} when your {agent} limit resets", when(at)),
        Some("Sikemux sends your last message again then. Sending anything to this chat first cancels it."),
    );
    update["resumesAt"] = json!(at);
    update
}
