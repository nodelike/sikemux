//! Chats stopped by a usage limit, each waiting for the limit to reset to
//! send its stopped prompt again. Each waits on a timer of its own, and the
//! list is kept on disk so a core that restarts goes on waiting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::account;
use crate::agent_accounts::usage;
use crate::protocol::{ChatEventKind, ChatLaunch};

use super::super::remote::unix_ms;
use super::super::{workspace, Core};
use super::rebind::Replay;
use super::{Chat, ChatCommand};

const FILE: &str = "limit-resumes.json";
const MAX_WAITING: usize = 256;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Waiting {
    agent_id: String,
    /// When the limit resets, in Unix milliseconds.
    at: u64,
    prompt: Replay,
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
pub(crate) struct Resets {
    path: Option<PathBuf>,
    slots: Mutex<HashMap<String, Slot>>,
    tickets: AtomicU64,
}

impl Resets {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join(FILE));
        let saved: Vec<Waiting> = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
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
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    pub(crate) fn arm_saved(&self, core: &Arc<Core>) {
        for slot in self.lock().values_mut() {
            if let Slot::Waiting { waiting, timer } = slot {
                *timer = Some(arm(core, waiting));
            }
        }
    }

    /// Stops the chat waiting, saying whether it was.
    pub(crate) fn cancel(&self, agent_id: &str) -> bool {
        let mut slots = self.lock();
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

    /// The chat's stopped prompt, once its limit has reset.
    fn take_due(&self, agent_id: &str) -> Option<Replay> {
        let mut slots = self.lock();
        let due = matches!(
            slots.get(agent_id),
            Some(Slot::Waiting { waiting, .. }) if waiting.at <= unix_ms()
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

/// Waits for the chat's usage limit to reset to send `prompt` again, when
/// `resets_at` or the account's usage says when that is.
pub(super) fn schedule(
    chat: &Arc<Chat>,
    launch: &ChatLaunch,
    prompt: Replay,
    resets_at: Option<u64>,
) {
    let Some(core) = chat.core.upgrade() else {
        return;
    };
    let resets = &core.chats.resets;
    let ticket = resets.tickets.fetch_add(1, Ordering::Relaxed);
    {
        let mut slots = resets.lock();
        if slots.len() >= MAX_WAITING && !slots.contains_key(chat.agent_id()) {
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
        let resets = &core.chats.resets;
        let mut slots = resets.lock();
        let current = slots.get(chat.agent_id());
        if !matches!(current, Some(Slot::Looking { ticket: own }) if *own == ticket) {
            return;
        }
        let Some(at) = at.filter(|at| *at > unix_ms()) else {
            slots.remove(chat.agent_id());
            return;
        };
        let waiting = Waiting {
            agent_id: chat.agent_id().to_owned(),
            at,
            prompt,
        };
        let timer = Some(arm(&core, &waiting));
        slots.insert(chat.agent_id().to_owned(), Slot::Waiting { waiting, timer });
        resets.save(&slots);
        drop(slots);
        say(&chat, resume_notice(&launch.provider, at));
    });
}

/// Stops the chat waiting for its limit to reset, as something sent to it
/// takes the place of its stopped prompt.
pub(super) fn cancel(chat: &Chat) {
    if let Some(core) = chat.core.upgrade() {
        core.chats.resets.cancel(chat.agent_id());
    }
}

/// The chat's stopped prompt, when its limit reset while it was not running.
pub(super) fn take_due(chat: &Chat) -> Option<Replay> {
    chat.core.upgrade()?.chats.resets.take_due(chat.agent_id())
}

/// Stops the chat waiting at the person's word, and says so in it.
pub(crate) fn cancel_by_person(core: &Core, agent_id: &str) {
    if core.chats.resets.cancel(agent_id) {
        if let Some(chat) = core.chats.get(agent_id) {
            say(
                &chat,
                json!({
                    "sessionUpdate": "notice",
                    "severity": "info",
                    "title": "This chat will not resume when the limit resets",
                    "resumesAt": null,
                }),
            );
        }
    }
}

fn arm(core: &Arc<Core>, waiting: &Waiting) -> tokio::task::AbortHandle {
    let wait = Duration::from_millis(waiting.at.saturating_sub(unix_ms()));
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
    let resets = &core.chats.resets;
    if let Some(chat) = core.chats.get(&agent_id) {
        if let Some(prompt) = resets.take_due(&agent_id) {
            let _ = chat.send(ChatCommand::Resume { prompt });
        }
        return;
    }
    if core.workspaces.closed(&agent_id) {
        resets.cancel(&agent_id);
        return;
    }
    if let Err(error) = workspace::wake_chat(core, None, agent_id.clone()).await {
        eprintln!("sikemux core: chat {agent_id} waits to resume until it next starts: {error}");
    }
}

/// When the account the chat runs on can be used again, from its usage.
async fn reset_from_usage(core: &Core, launch: &ChatLaunch) -> Option<u64> {
    let provider = launch.provider.as_str();
    let folder = account::directory_variable(provider)
        .and_then(|variable| launch.env.get(variable))
        .map(String::as_str);
    let program = core
        .workspaces
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
    json!({
        "sessionUpdate": "notice",
        "severity": "info",
        "title": format!("Resumes {} when your {agent} limit resets", when(at)),
        "description": "Sikemux sends your last message again then. Sending anything to this chat first cancels it.",
        "resumesAt": at,
    })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A day an agent writes out, such as "Oct 17" or "Oct 17th", as its month from 1 and day.
pub(super) fn month_day(text: &str) -> Option<(i32, i32)> {
    let (name, day) = text.split_once(' ')?;
    let month = MONTHS.iter().position(|known| *known == name)? as i32 + 1;
    let day = day
        .trim_end_matches(char::is_alphabetic)
        .parse::<i32>()
        .ok()?;
    (1..=31).contains(&day).then_some((month, day))
}

/// `at`, in Unix milliseconds, as this host's clock reads it: "at 15:40"
/// today, else "on 18 Oct at 15:40".
fn when(at: u64) -> String {
    let (Some(then), Some(now)) = (calendar(at, Zone::Host), calendar(unix_ms(), Zone::Host))
    else {
        return String::new();
    };
    let clock = format!("at {:02}:{:02}", then.tm_hour, then.tm_min);
    if (then.tm_year, then.tm_yday) == (now.tm_year, now.tm_yday) {
        return clock;
    }
    let month = MONTHS
        .get(then.tm_mon as usize)
        .copied()
        .unwrap_or_default();
    format!("on {} {month} {clock}", then.tm_mday)
}

/// The zone a reset time an agent writes out is in.
#[derive(Clone, Copy)]
pub(super) enum Zone {
    Host,
    Utc,
}

/// A reset time as an agent writes it out, to the minute.
pub(super) struct WallClock {
    pub zone: Zone,
    /// Year, month from 1 and day, when it names a day; the year defaults to this one.
    pub date: Option<(Option<i32>, i32, i32)>,
    pub hour: i32,
    pub minute: i32,
}

impl WallClock {
    /// In Unix milliseconds: on its day, else the next time the clock reads
    /// it. `None` for a day already past.
    pub(super) fn unix_ms(&self) -> Option<u64> {
        let now = unix_ms();
        match self.on_day(now, 0)? {
            at if at > now => Some(at),
            _ if self.date.is_some() => None,
            _ => self.on_day(now, 1).filter(|at| *at > now),
        }
    }

    fn on_day(&self, now: u64, days_later: i32) -> Option<u64> {
        let mut tm = calendar(now, self.zone)?;
        if let Some((year, month, day)) = self.date {
            if let Some(year) = year {
                tm.tm_year = year - 1900;
            }
            tm.tm_mon = month - 1;
            tm.tm_mday = day;
        }
        tm.tm_mday += days_later;
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (self.hour, self.minute, 0, -1);
        // SAFETY: `tm` is a live value for the length of the call.
        let at = unsafe {
            match self.zone {
                Zone::Host => libc::mktime(&mut tm),
                Zone::Utc => libc::timegm(&mut tm),
            }
        };
        u64::try_from(at).ok()?.checked_mul(1000)
    }
}

fn calendar(at: u64, zone: Zone) -> Option<libc::tm> {
    let seconds = (at / 1000) as libc::time_t;
    // SAFETY: `tm` is plain data, for which all zeroes is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers point at live values for the length of the call.
    let filled = unsafe {
        match zone {
            Zone::Host => libc::localtime_r(&seconds, &mut tm),
            Zone::Utc => libc::gmtime_r(&seconds, &mut tm),
        }
    };
    (!filled.is_null()).then_some(tm)
}
