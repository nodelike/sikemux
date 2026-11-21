//! Triggers an agent sets: each, when its time comes, once or again and again, sends the
//! agent's own chat a prompt, or adds a card to its project's board, which Auto carries when it
//! is on, and stays until it is removed. A webhook trigger fires on each request to its address
//! instead of at a time, and one on a mod's collection when a document there is added or changed.
//! A chat's triggers pause while it is closed and go on when it is taken up
//! again. The core also watches CI after each push an agent makes, with no trigger_add.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::triggers::{
    AddTriggerParams, HoldTriggerParams, TriggerOn, TriggerParams, TriggerWakes,
};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::trigger as fired;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{ChatTrigger, FiredRequest, RequestHeader, TriggerFired, TriggerWhen};
use sikemux_view::memory::Learned;
use sikemux_view::playbook::RunsAlone;
use sikemux_view::turn::{TurnWait, TurnWaitKind};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::board::goal::learned;
use super::board::Subject;
use super::chat::reset::{calendar, when, WallClock, Zone};
use super::kept::Kept;
use super::messages;
use super::wake::Wake;
use super::workspace::{Held, HeldChat};
use super::Core;
use crate::cli::protocol::HarnessRequest;

mod check;
mod ci;
mod github;
mod on_mod;
mod park;
mod rundeck;

pub(crate) use ci::{ci_moved, waiting_on_ci};
pub(crate) use on_mod::{written as mod_written, Written as ModWritten};

const FILE: &str = "triggers.json";
const MAX_PER_AGENT: usize = 20;
const MAX_PER_BOARD: usize = 20;
const MAX_TRIGGERS: usize = 512;
const MAX_TITLE_CHARS: usize = 120;
const MAX_PROMPT_CHARS: usize = 8192;
const MIN_EVERY_SECONDS: u64 = 60;
const MAX_AHEAD_SECONDS: u64 = 366 * 24 * 60 * 60;
/// A repeating trigger whose time passed while the host was off fires once
/// when the core comes back within this long, and otherwise waits for its next time.
const LATE_GRACE_MS: u64 = 15 * 60 * 1000;
/// The most of a webhook request's body a prompt carries.
const MAX_REQUEST_BYTES: usize = 24 * 1024;
/// How many webhook requests are remembered, so one the server sends again is not taken twice.
const SEEN_REQUESTS: usize = 512;
/// How many requests a webhook trigger keeps while its chat is closed; past that the oldest go.
const MAX_HELD_REQUESTS: usize = 10;
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Trigger {
    id: u64,
    /// The chat that set it, which its prompt goes to unless it adds cards.
    agent_id: String,
    /// The project whose board it adds a card to when it fires, rather than prompting the chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    board: Option<String>,
    title: String,
    prompt: String,
    when: TriggerWhen,
    next_at: Option<u64>,
    /// The webhook on the accounts server whose requests fire it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook_id: Option<String>,
    /// The closed chat it waits for, to go on in whichever agent takes that chat up again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paused: Option<HeldChat>,
    /// The CI it fires once it ends; its `next_at` is only when it looks again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ci: Option<ci::CiWatch>,
    /// The run or agent it fires on once it ends or does what it waits for; its time is only
    /// when it gives up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    park: Option<park::Park>,
    /// The Rundeck run it fires on once it ends; its time is only when it looks again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rundeck: Option<rundeck::RundeckRun>,
    /// The command whose output, when it changes, is what makes a repeating trigger fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check: Option<check::Check>,
    /// The mod's collection whose changed documents fire it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    watch: Option<on_mod::Watch>,
    /// When the person paused it; it skips its times until they resume it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopped: Option<u64>,
    /// The agent that set it up, in which chat turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set: Option<Learned>,
    /// Requests to its webhook that came while its chat was closed, oldest first, sent once the
    /// chat is taken up again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    held: Vec<FiredRequest>,
    /// How many requests came while its chat was closed past the most it keeps.
    #[serde(default, skip_serializing_if = "is_zero")]
    dropped: u32,
    /// Its webhook has been called by GitHub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    from_github: bool,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

impl Trigger {
    /// Whether it waits instead of firing: its chat is closed, or the person paused it.
    fn rests(&self) -> bool {
        self.paused.is_some() || self.stopped.is_some()
    }

    fn waits_in_line(&self) -> bool {
        self.park.as_ref().is_some_and(park::Park::waits_in_line)
    }

    /// Whether it prompts the open chat `agent_id`. The core's watch after a push is none of the
    /// chat's own, so it neither keeps the chat open nor counts as a wait the chat set.
    fn prompts(&self, agent_id: &str) -> bool {
        self.board.is_none()
            && self.paused.is_none()
            && self.agent_id == agent_id
            && !self.after_push()
    }

    fn after_push(&self) -> bool {
        self.ci.as_ref().is_some_and(ci::CiWatch::after_push)
    }

    fn ends_with_chat(&self) -> bool {
        self.board.is_none() && self.paused.is_none() && !self.after_push()
    }

    fn shown(&self) -> ChatTrigger {
        ChatTrigger {
            id: self.id,
            title: self.title.clone(),
            when: self.when.clone(),
            next_at: self.shown_next_at(),
        }
    }

    /// Its row on the project's Automation page.
    fn line(&self) -> RunsAlone {
        RunsAlone::Trigger {
            id: self.id,
            title: self.title.clone(),
            when: self.when.clone(),
            next_at: self.shown_next_at(),
            set: self.set.clone().map(Box::new),
            paused: self.stopped,
            from_github: self.from_github,
        }
    }

    /// Whether the person set it up rather than an agent.
    fn by_person(&self) -> bool {
        self.set.as_ref().is_some_and(|set| set.agent.is_none())
    }

    /// When it is shown to fire next: never for CI, and for a Rundeck run when it gives up.
    fn shown_next_at(&self) -> Option<u64> {
        match (&self.ci, &self.rundeck) {
            (Some(_), _) => None,
            (_, Some(run)) => Some(run.gives_up_at),
            _ => self.next_at,
        }
    }
}

struct Due {
    firing: Vec<Firing>,
    /// Triggers that look at what they wait on before firing.
    looking: Vec<u64>,
    /// Triggers given a new next time, or ended, without firing now.
    moved: Vec<Trigger>,
    next: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    next_id: u64,
    triggers: Vec<Trigger>,
}

pub(crate) struct Triggers {
    kept: Kept,
    state: Mutex<Saved>,
    pub(crate) wake: Wake,
    seen: Mutex<VecDeque<String>>,
    watched: Mutex<HashSet<String>>,
    /// Mod triggers gathering a burst of changes before they fire.
    gathering: Mutex<HashSet<u64>>,
}

/// A trigger whose time came, and whether it was due long before the core could send it.
struct Firing {
    trigger: Trigger,
    late: Option<Late>,
    last: bool,
    /// The webhook request that fired it.
    request: Option<FiredRequest>,
}

/// Why a trigger fires after its time, and when that was.
enum Late {
    HostOff(u64),
    ChatClosed(u64),
    /// A webhook request that came while the chat was closed, and how many more came and were
    /// dropped.
    Held(u32),
}

impl Triggers {
    pub(crate) fn new(data_dir: Option<&std::path::Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let saved: Saved = kept.load();
        let watched = Mutex::new(on_mod::watched(&saved));
        Self {
            kept,
            state: Mutex::new(saved),
            wake: Wake::default(),
            seen: Mutex::default(),
            watched,
            gathering: Mutex::default(),
        }
    }

    /// Whether the webhook request is new, remembering it.
    fn first_time(&self, delivery_id: &str) -> bool {
        let mut seen = self.seen.locked();
        if seen.iter().any(|seen| seen == delivery_id) {
            return false;
        }
        if seen.len() >= SEEN_REQUESTS {
            seen.pop_front();
        }
        seen.push_back(delivery_id.to_owned());
        true
    }

    /// The chat's triggers, soonest first, as its clients show them. A wait in line is left
    /// out, since the chat's waiting line shows it.
    pub(crate) fn shown(&self, agent_id: &str) -> Vec<ChatTrigger> {
        self.shown_where(|trigger| trigger.prompts(agent_id) && !trigger.waits_in_line())
    }

    /// The triggers that add cards to the project's board, soonest first.
    pub(crate) fn on_board(&self, project: &str) -> Vec<ChatTrigger> {
        self.shown_where(|trigger| trigger.board.as_deref() == Some(project))
    }

    fn shown_where(&self, keep: impl Fn(&Trigger) -> bool) -> Vec<ChatTrigger> {
        let state = self.state.locked();
        let mut shown: Vec<ChatTrigger> = state
            .triggers
            .iter()
            .filter(|trigger| keep(trigger))
            .map(Trigger::shown)
            .collect();
        shown.sort_by_key(|trigger| trigger.next_at.unwrap_or(u64::MAX));
        shown
    }

    /// When the soonest trigger that prompts the agent's chat fires at a time. A CI trigger
    /// fires when its CI ends, so its time is only when it looks again.
    pub(crate) fn next_at(&self, agent_id: &str) -> Option<u64> {
        let state = self.state.locked();
        state
            .triggers
            .iter()
            .filter(|trigger| {
                trigger.prompts(agent_id)
                    && trigger.ci.is_none()
                    && trigger.park.is_none()
                    && trigger.rundeck.is_none()
            })
            .filter_map(|trigger| trigger.next_at)
            .min()
    }

    /// What the agent's chat waits on of the wakes it set itself to carry its work on: CI first,
    /// else its soonest one-time timer, which it has waited on since `rested_at`, else a
    /// repeating trigger with a check that has not fired yet. A repeating trigger without one,
    /// or whose check has fired, is automation and a paused one waits on nothing, so neither is
    /// a wait.
    pub(crate) fn wait_of(
        &self,
        agent_id: &str,
        rested_at: u64,
        expected: impl Fn(&str) -> Option<u64>,
    ) -> Option<TurnWait> {
        let state = self.state.locked();
        let mine = || {
            (state.triggers.iter()).filter(|trigger| trigger.prompts(agent_id) && !trigger.rests())
        };
        if let Some(trigger) = mine().find(|trigger| trigger.ci.is_some()) {
            return trigger.ci.as_ref().map(|ci| TurnWait {
                kind: TurnWaitKind::Ci,
                what: trigger.title.clone(),
                started_at: ci.since(),
                expected_ms: expected(&ci.waits_on()),
                until: None,
                place: None,
                by: None,
                agent: None,
                agents: Vec::new(),
            });
        }
        if let Some((trigger, park)) =
            mine().find_map(|trigger| Some((trigger, trigger.park.as_ref()?)))
        {
            return Some(TurnWait {
                kind: park.kind(),
                what: trigger.title.clone(),
                started_at: park.since,
                expected_ms: park.waits_on().and_then(|on| expected(&on)),
                until: None,
                place: None,
                by: None,
                agent: park.agent().map(str::to_owned),
                agents: park.agent().map(str::to_owned).into_iter().collect(),
            });
        }
        let soonest = mine()
            .filter(|trigger| matches!(trigger.when, TriggerWhen::Once { .. }))
            .filter_map(|trigger| Some((trigger, trigger.next_at?)))
            .min_by_key(|(_, at)| *at);
        let Some((trigger, at)) = soonest else {
            let trigger = mine()
                .filter(|trigger| trigger.check.as_ref().is_some_and(|check| !check.fired()))
                .min_by_key(|trigger| trigger.set.as_ref().map(|set| set.at))?;
            return Some(TurnWait {
                kind: TurnWaitKind::Watch,
                what: trigger.title.clone(),
                started_at: trigger.set.as_ref().map_or(rested_at, |set| set.at),
                expected_ms: None,
                until: None,
                place: None,
                by: None,
                agent: None,
                agents: Vec::new(),
            });
        };
        Some(TurnWait {
            kind: TurnWaitKind::Timer,
            what: trigger.title.clone(),
            started_at: rested_at.min(at),
            expected_ms: None,
            until: Some(at),
            place: None,
            by: None,
            agent: None,
            agents: Vec::new(),
        })
    }

    /// How many triggers prompt the open chat `agent_id`.
    pub(crate) fn count(&self, agent_id: &str) -> u32 {
        let state = self.state.locked();
        let mine = state
            .triggers
            .iter()
            .filter(|trigger| trigger.prompts(agent_id) && !trigger.waits_in_line());
        mine.count() as u32
    }

    /// The titles of the triggers that prompt the open chat `agent_id`.
    pub(crate) fn titles(&self, agent_id: &str) -> Vec<String> {
        (self.state.locked().triggers.iter())
            .filter(|trigger| trigger.prompts(agent_id) && !trigger.waits_in_line())
            .map(|trigger| trigger.title.clone())
            .collect()
    }

    /// Whether any trigger waits for a closed chat.
    fn any_paused(&self) -> bool {
        let state = self.state.locked();
        state
            .triggers
            .iter()
            .any(|trigger| trigger.paused.is_some())
    }

    /// The closed chats paused triggers wait for, each with its triggers soonest first.
    pub(crate) fn held(&self) -> Held {
        let state = self.state.locked();
        let mut held: Held = Vec::new();
        for trigger in &state.triggers {
            let Some(chat) = &trigger.paused else {
                continue;
            };
            match held.iter_mut().find(|(kept, _)| kept.id() == chat.id()) {
                Some((_, triggers)) => triggers.push(trigger.shown()),
                None => held.push((chat.clone(), vec![trigger.shown()])),
            }
        }
        for (_, triggers) in &mut held {
            triggers.sort_by_key(|trigger| trigger.next_at.unwrap_or(u64::MAX));
        }
        held
    }

    fn change<T>(&self, change: impl FnOnce(&mut Saved) -> T) -> T {
        let mut state = self.state.locked();
        let out = change(&mut state);
        self.kept.save(&*state);
        *self.watched.locked() = on_mod::watched(&state);
        drop(state);
        self.wake.now();
        out
    }

    /// Takes out the triggers whose time has come, moving repeating ones on
    /// to their next time.
    fn take_due(&self, now: u64) -> Due {
        let mut state = self.state.locked();
        let mut firing = Vec::new();
        let mut looking = Vec::new();
        let mut moved = Vec::new();
        let mut changed = false;
        state.triggers.retain_mut(|trigger| {
            let Some(due) = trigger.next_at.filter(|due| *due <= now) else {
                return true;
            };
            if trigger.rests() {
                return true;
            }
            changed = true;
            if let Some(watch) = &trigger.ci {
                looking.push(trigger.id);
                trigger.next_at = Some(now + watch.look_again_ms());
                return true;
            }
            if let Some(run) = &trigger.rundeck {
                if now < run.gives_up_at {
                    looking.push(trigger.id);
                    trigger.next_at = Some((now + rundeck::LOOK_AGAIN_MS).min(run.gives_up_at));
                    return true;
                }
                let mut timed_out = trigger.clone();
                timed_out.prompt = format!("{}\n\n{}", trigger.prompt, run.timed_out(now));
                firing.push(Firing {
                    trigger: timed_out,
                    late: None,
                    last: true,
                    request: None,
                });
                return false;
            }
            if let Some(park) = &trigger.park {
                let mut timed_out = trigger.clone();
                timed_out.prompt = format!("{}\n\n{}", trigger.prompt, park.timed_out(now));
                firing.push(Firing {
                    trigger: timed_out,
                    late: None,
                    last: true,
                    request: None,
                });
                return false;
            }
            let late = now.saturating_sub(due) > LATE_GRACE_MS;
            let next = next_after(&trigger.when, due, now);
            let last = next.is_none();
            if trigger.check.is_some() {
                looking.push(trigger.id);
                moved.push(trigger.clone());
            } else if !late || last {
                firing.push(Firing {
                    trigger: trigger.clone(),
                    late: late.then_some(Late::HostOff(due)),
                    last,
                    request: None,
                });
            } else {
                moved.push(trigger.clone());
            }
            trigger.next_at = next;
            next.is_some()
        });
        if changed {
            self.kept.save(&*state);
        }
        let next = state
            .triggers
            .iter()
            .filter(|trigger| !trigger.rests())
            .filter_map(|trigger| trigger.next_at)
            .min();
        Due {
            firing,
            looking,
            moved,
            next,
        }
    }

    fn agents(&self) -> Vec<String> {
        let mut agents: Vec<String> = self
            .state
            .locked()
            .triggers
            .iter()
            .filter(|trigger| trigger.ends_with_chat())
            .map(|trigger| trigger.agent_id.clone())
            .collect();
        agents.sort();
        agents.dedup();
        agents
    }
}

/// When a trigger that was due at `due` fires next after `now`; none for one that fired its
/// only time.
pub(crate) fn next_after(when: &TriggerWhen, due: u64, now: u64) -> Option<u64> {
    match when {
        TriggerWhen::Once { .. }
        | TriggerWhen::Webhook { .. }
        | TriggerWhen::Ci { .. }
        | TriggerWhen::Mod { .. } => None,
        TriggerWhen::Every { seconds } => {
            let step = seconds * 1000;
            let behind = now.saturating_sub(due) / step + 1;
            Some(due + behind * step)
        }
        TriggerWhen::Daily { minute, weekdays } => next_daily(*minute, weekdays, now),
    }
}

/// The first time after `now` that the host's clock reads `minute` past midnight on one of
/// `weekdays`, or on any day when there are none.
fn next_daily(minute: u32, weekdays: &[u8], now: u64) -> Option<u64> {
    let clock = WallClock {
        zone: Zone::Host,
        date: None,
        hour: (minute / 60) as i32,
        minute: (minute % 60) as i32,
    };
    (0..=7)
        .filter_map(|days_later| clock.on_day(now, days_later))
        .filter(|at| *at > now)
        .find(|at| {
            weekdays.is_empty()
                || calendar(*at, Zone::Host)
                    .is_some_and(|tm| weekdays.contains(&(tm.tm_wday as u8)))
        })
}

/// Pauses the triggers of a chat that is closing, until it is taken up again. A chat with no
/// session to take up never comes back, so its triggers go.
pub(crate) fn pause(core: &Core, agent_id: &str, chat: Option<HeldChat>) {
    if core.triggers().count(agent_id) == 0 {
        return;
    }
    let gone = core.triggers().change(|state| {
        let mine = |trigger: &Trigger| trigger.prompts(agent_id);
        if let Some(chat) = &chat {
            for trigger in state.triggers.iter_mut().filter(|trigger| mine(trigger)) {
                trigger.paused = Some(chat.clone());
            }
            return Vec::new();
        }
        let (gone, kept) = std::mem::take(&mut state.triggers)
            .into_iter()
            .partition(|trigger| mine(trigger));
        state.triggers = kept;
        gone
    });
    gone.iter().for_each(|trigger| unhook(core, trigger));
    core.changed(What::View);
}

/// Gives the paused triggers of each chat taken up again to the agent holding it now. A
/// repeating one skips the times that passed while it was closed; a one-off that came due
/// fires once.
pub(crate) fn resume(core: &Arc<Core>) {
    if !core.triggers().any_paused() {
        return;
    }
    let holders = core.workspaces().holders();
    let now = epoch_ms();
    let (resumed, missed, held) = core.triggers().change(|state| {
        let (mut resumed, mut missed, mut held) = (Vec::new(), Vec::new(), Vec::new());
        state.triggers.retain_mut(|trigger| {
            let chat = trigger.paused.as_ref().map(HeldChat::id);
            let Some(holder) = chat.and_then(|chat| holders.get(&chat)) else {
                return true;
            };
            trigger.agent_id = holder.clone();
            trigger.paused = None;
            let dropped = std::mem::take(&mut trigger.dropped);
            let requests = std::mem::take(&mut trigger.held);
            held.extend(
                requests
                    .into_iter()
                    .enumerate()
                    .map(|(index, request)| Firing {
                        trigger: trigger.clone(),
                        late: Some(Late::Held(if index == 0 { dropped } else { 0 })),
                        last: false,
                        request: Some(request),
                    }),
            );
            resumed.push(trigger.clone());
            if trigger.watch.is_some() {
                return true;
            }
            if trigger.ci.is_some() || trigger.rundeck.is_some() {
                trigger.next_at = Some(now);
                return true;
            }
            if trigger.park.is_some() {
                return true;
            }
            let Some(due) = trigger.next_at.filter(|due| *due <= now) else {
                return true;
            };
            trigger.next_at = next_after(&trigger.when, due, now);
            if trigger.next_at.is_some() {
                return true;
            }
            missed.push(Firing {
                trigger: trigger.clone(),
                late: Some(Late::ChatClosed(due)),
                last: true,
                request: None,
            });
            false
        });
        (resumed, missed, held)
    });
    for trigger in &resumed {
        show(core, trigger);
    }
    if resumed.iter().any(|trigger| trigger.park.is_some()) {
        core.harness().changed();
    }
    for trigger in &resumed {
        if trigger.watch.as_ref().is_some_and(on_mod::Watch::waiting) {
            on_mod::gather(core, trigger.id);
        }
    }
    for firing in missed {
        tokio::spawn(fire(core.clone(), firing));
    }
    if !held.is_empty() {
        let core = core.clone();
        tokio::spawn(async move {
            for firing in held {
                fire(core.clone(), firing).await;
            }
        });
    }
}

/// Sends what came due, closes the triggers of chats that are gone, and waits for the next.
pub(crate) fn settle(core: &Arc<Core>) {
    let gone: Vec<String> = core
        .triggers()
        .agents()
        .into_iter()
        .filter(|agent_id| core.workspaces().closed(agent_id))
        .collect();
    if !gone.is_empty() {
        let closed = core.triggers().change(|state| {
            let (closed, kept) = std::mem::take(&mut state.triggers)
                .into_iter()
                .partition(|trigger| trigger.ends_with_chat() && gone.contains(&trigger.agent_id));
            state.triggers = kept;
            closed
        });
        closed.iter().for_each(|trigger| unhook(core, trigger));
    }
    let Due {
        firing,
        looking,
        moved,
        next,
    } = core.triggers().take_due(epoch_ms());
    if let Some(next) = next {
        core.triggers().wake.at(next);
    }
    for id in looking {
        tokio::spawn(look(core.clone(), id));
    }
    for trigger in firing.iter().map(|firing| &firing.trigger).chain(&moved) {
        show(core, trigger);
    }
    for firing in firing {
        tokio::spawn(fire(core.clone(), firing));
    }
}

/// Puts a firing of `trigger` on `project`'s board, on the card of its `subject` when it has one,
/// and takes it on to that card's agent.
async fn on_board(
    core: Arc<Core>,
    trigger: &Trigger,
    project: &str,
    request: Option<&FiredRequest>,
    subject: Option<Subject>,
) {
    let details = match request {
        Some(request) => format!("{}\n\n{}", trigger.prompt, fired::request_text(request)),
        None => trigger.prompt.clone(),
    };
    let here = core.remote().core_id().unwrap_or_default();
    let fired = (trigger.id, here.as_str());
    let title = &trigger.title;
    match super::board::add_for_trigger(&core, project, fired, title, &details, subject) {
        Ok(went) => super::board::hand_off::carry(core.clone(), project.to_owned(), went).await,
        Err(error) => eprintln!(
            "sikemux core: trigger {} added no card to {project}'s board: {error}",
            trigger.id
        ),
    }
}

async fn fire(core: Arc<Core>, firing: Firing) {
    let trigger = &firing.trigger;
    if let Some(project) = &trigger.board {
        on_board(
            core.clone(),
            trigger,
            project,
            firing.request.as_ref(),
            None,
        )
        .await;
        return;
    }
    let mut prompt = trigger.prompt.clone();
    match firing.late {
        Some(Late::HostOff(due)) => prompt.push_str(&format!(
            "\n\n(This was due {}, while this host was off.)",
            when(due)
        )),
        Some(Late::ChatClosed(due)) => prompt.push_str(&format!(
            "\n\n(This was due {}, while this chat was closed.)",
            when(due)
        )),
        Some(Late::Held(0)) => prompt.push_str("\n\n(This request came while this chat was closed.)"),
        Some(Late::Held(dropped)) => prompt.push_str(&format!(
            "\n\n(This request came while this chat was closed. {dropped} earlier {} came too and {} dropped, as a closed chat keeps only the last {MAX_HELD_REQUESTS}.)",
            if dropped == 1 { "one" } else { "ones" },
            if dropped == 1 { "was" } else { "were" },
        )),
        None => {}
    }
    let text = fired::text(&TriggerFired {
        title: trigger.title.clone(),
        prompt,
        request: firing.request.clone(),
    });
    let after = match (firing.last, &firing.request) {
        (true, _) => "It will not fire again.".to_owned(),
        (false, Some(_)) => format!(
            "It fires on each request to its address; trigger_remove {} stops it.",
            trigger.id
        ),
        (false, None) if trigger.watch.is_some() => format!(
            "It fires again on the next changes; trigger_remove {} stops it.",
            trigger.id
        ),
        (false, None) => core
            .triggers()
            .shown(&trigger.agent_id)
            .iter()
            .find(|shown| shown.id == trigger.id)
            .and_then(|shown| shown.next_at)
            .map_or_else(String::new, |next| {
                format!(
                    "It fires again {}; trigger_remove {} stops it.",
                    when(next),
                    trigger.id
                )
            }),
    };
    let note = match trigger.by_person() {
        true => format!(
            "Sent by trigger {} that the person set up for this chat, not typed by them now. {after}",
            trigger.id
        ),
        false => format!(
            "Sent by trigger {} that this chat set with trigger_add, not by the person or another agent. {after}",
            trigger.id
        ),
    };
    if let Err(error) = messages::from_trigger(
        &core,
        trigger.agent_id.clone(),
        text,
        note.trim().to_owned(),
    )
    .await
    {
        eprintln!(
            "sikemux core: trigger {} did not reach chat {}: {error}",
            trigger.id, trigger.agent_id
        );
    }
}

/// Has the trigger `id` look at what it waits on, and fire if that calls for it.
async fn look(core: Arc<Core>, id: u64) {
    let (checks, rundeck) = {
        let state = core.triggers().state.locked();
        let trigger = state.triggers.iter().find(|trigger| trigger.id == id);
        trigger.map_or((false, false), |trigger| {
            (trigger.check.is_some(), trigger.rundeck.is_some())
        })
    };
    match (checks, rundeck) {
        (true, _) => check::look(core, id).await,
        (_, true) => rundeck::look(core, id).await,
        _ => ci::look(core, id).await,
    }
}

/// Tells the clients of the trigger's chat or board what triggers it has now, and every
/// device's view when the chat's next one fires.
fn show(core: &Core, trigger: &Trigger) {
    core.changed(What::Read(super::changed::Topic::Playbook));
    super::hosts::route::rows_moved(core);
    if let Some(project) = &trigger.board {
        super::board::triggers_changed(core, project);
        return;
    }
    if let Some(feed) = core.chats().heard(&trigger.agent_id) {
        feed.set_triggers(core.triggers().shown(&trigger.agent_id));
    }
    core.changed(What::View);
}

pub(crate) async fn add(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = crate::server::caller::agent_of(request, "trigger_add")?;
    let params = &request.params;
    let title = text(params, "title", MAX_TITLE_CHARS)?;
    let prompt = text(params, "prompt", MAX_PROMPT_CHARS)?;
    let board = match params.get("board") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(crate::server::caller::project_of(core, request)),
        Some(_) => return Err("board must be a boolean".into()),
    };
    let now = epoch_ms();
    let ci = match params.get("ci") {
        None | Some(Value::Null) => None,
        Some(Value::String(on)) if !on.trim().is_empty() => {
            if board.is_some()
                || given(params, "webhook")
                || TIMES.iter().any(|key| given(params, key))
            {
                return Err("A CI trigger prompts this chat once CI ends, so give it no time, webhook or board".into());
            }
            let (watch, shown) = ci::watch(core, &request.project, on.trim()).await?;
            if let ci::Ci::Ended(said, _) = ci::ended(core, &watch).await? {
                return Ok(
                    json!({ "ended": said, "note": "It already ended, so no trigger was set." }),
                );
            }
            Some((watch, shown))
        }
        Some(_) => return Err("ci must be a commit, a branch or a CI run's address or id".into()),
    };
    let park = match park::asked(params) {
        false => None,
        true => {
            if ci.is_some()
                || board.is_some()
                || given(params, "webhook")
                || given(params, "check")
                || TIMES.iter().any(|key| given(params, key))
            {
                return Err("A process or agent trigger prompts this chat once what it waits on happens, so give it only until and timeout, no time, ci, check, webhook or board".into());
            }
            let timeout = park::timeout(params)?;
            match park::set(core, request).await? {
                park::Parked::Already(said) => {
                    return Ok(
                        json!({ "happened": said, "note": "It already happened, so no trigger was set." }),
                    )
                }
                park::Parked::Waits(park, shown) => Some((park, shown, now + timeout * 1000)),
            }
        }
    };
    let rundeck = match params.get("rundeck") {
        None | Some(Value::Null) => None,
        Some(on) => {
            if ci.is_some()
                || park.is_some()
                || board.is_some()
                || given(params, "webhook")
                || given(params, "check")
                || TIMES.iter().any(|key| given(params, key))
            {
                return Err("A Rundeck trigger prompts this chat once its run ends, so give it only timeout, no time, ci, process, agent, check, webhook or board".into());
            }
            let gives_up_at = now + park::timeout(params)? * 1000;
            match rundeck::asked(core, on, now, gives_up_at).await? {
                rundeck::Asked::Ended(said) => {
                    return Ok(
                        json!({ "ended": said, "note": "It already ended, so no trigger was set." }),
                    )
                }
                rundeck::Asked::Waits(run, shown) => Some((run, shown)),
            }
        }
    };
    let check = match params.get("check") {
        None | Some(Value::Null) => None,
        Some(Value::String(command)) if !command.trim().is_empty() => {
            if !given(params, "every") && !given(params, "daily") {
                return Err("A check runs each time a repeating trigger comes due, so give it every or daily".into());
            }
            let dir = board.clone().unwrap_or_else(|| request.project.clone());
            Some(check::Check::first(command.trim().to_owned(), dir).await?)
        }
        Some(_) => return Err("check must be a shell command".into()),
    };
    let watch = match params.get("mod") {
        None | Some(Value::Null) => None,
        Some(Value::String(on)) => {
            if ci.is_some()
                || park.is_some()
                || given(params, "webhook")
                || given(params, "check")
                || TIMES.iter().any(|key| given(params, key))
            {
                return Err("A mod trigger fires when a document in its collection is added or changed, so give it no time, ci, process, agent, check or webhook".into());
            }
            Some(on_mod::Watch::asked(core, on.trim(), params.get("where"))?)
        }
        Some(_) => {
            return Err(
                "mod must be a mod's id and one of its collections, such as posts/drafts".into(),
            )
        }
    };
    if watch.is_none() && given(params, "where") {
        return Err("where goes with mod".into());
    }
    let (rule, next_at, hook_id) = match params.get("webhook") {
        _ if watch.is_some() => {
            let on = watch.as_ref().map(on_mod::Watch::on).unwrap_or_default();
            (TriggerWhen::Mod { on }, None, None)
        }
        _ if park.is_some() => {
            let at = park.as_ref().map_or(now, |(_, _, at)| *at);
            (TriggerWhen::Once { at }, Some(at), None)
        }
        _ if rundeck.is_some() => {
            let at = rundeck.as_ref().map_or(now, |(run, _)| run.gives_up_at);
            (
                TriggerWhen::Once { at },
                Some(now + rundeck::LOOK_AGAIN_MS),
                None,
            )
        }
        _ if ci.is_some() => {
            let (look_again, shown) = (ci.as_ref())
                .map(|(watch, shown)| (watch.look_again_ms(), shown.clone()))
                .unwrap_or_default();
            (TriggerWhen::Ci { on: shown }, Some(now + look_again), None)
        }
        Some(Value::Bool(true)) => {
            if TIMES.iter().any(|key| given(params, key)) {
                return Err("A webhook trigger fires on each request, so give it no time".into());
            }
            let hook = super::hooks::register_hook(core)
                .await
                .map_err(|error| format!("the webhook could not be made: {error}"))?;
            (
                TriggerWhen::Webhook { url: hook.url },
                None,
                Some(hook.hook_id),
            )
        }
        None | Some(Value::Null) | Some(Value::Bool(false)) => {
            let (rule, next_at) = when_of(params, now)?;
            (rule, Some(next_at), None)
        }
        Some(_) => return Err("webhook must be a boolean".into()),
    };
    let added = core.triggers().change(|state| {
        let (own, most, whose) = match &board {
            Some(project) => (
                state
                    .triggers
                    .iter()
                    .filter(|trigger| trigger.board.as_ref() == Some(project))
                    .count(),
                MAX_PER_BOARD,
                "This project's board",
            ),
            None => (
                state
                    .triggers
                    .iter()
                    .filter(|trigger| trigger.prompts(&agent_id))
                    .count(),
                MAX_PER_AGENT,
                "This chat",
            ),
        };
        if own >= most {
            return Err(format!(
                "{whose} already has {most} triggers; remove one with trigger_remove first"
            ));
        }
        if state.triggers.len() >= MAX_TRIGGERS {
            return Err(format!(
                "This host already holds {MAX_TRIGGERS} triggers, so it takes no more"
            ));
        }
        state.next_id += 1;
        let trigger = Trigger {
            id: state.next_id,
            agent_id: agent_id.clone(),
            board,
            title,
            prompt,
            when: rule,
            next_at,
            hook_id: hook_id.clone(),
            paused: None,
            ci: ci.map(|(watch, _)| watch),
            park: park.clone().map(|(park, _, _)| park),
            rundeck: rundeck.clone().map(|(run, _)| run),
            check,
            watch: watch.clone(),
            stopped: None,
            set: Some(learned(core, Some(&agent_id))),
            held: Vec::new(),
            dropped: 0,
            from_github: false,
        };
        state.triggers.push(trigger.clone());
        Ok(trigger)
    });
    let trigger = match added {
        Ok(trigger) => trigger,
        Err(error) => {
            if let Some(hook_id) = hook_id {
                let core = core.clone();
                tokio::spawn(async move { drop(super::hooks::remove_hook(&core, &hook_id).await) });
            }
            return Err(error);
        }
    };
    show(core, &trigger);
    if trigger.ci.is_some() {
        super::codehost::ci::start(core);
    }
    let what = match &trigger.board {
        Some(_) => "it adds a card to this project's board, titled as the trigger with its prompt as the details, which Auto carries when it is on. It stays after this chat closes.",
        None => "it sends this chat its prompt, as a message you read like any other.",
    };
    let mut answer = serde_json::Map::new();
    answer.insert("trigger".into(), trigger.id.into());
    if let Some((_, shown, at)) = &park {
        answer.insert("note".into(), format!("Once {shown} does what you wait for, {what} It says what happened, so end your turn now rather than waiting or polling. If nothing happens by {}, it fires saying it timed out.", when(*at)).into());
        return Ok(Value::Object(answer));
    }
    if let Some((_, shown)) = &rundeck {
        answer.insert("note".into(), format!("Once {shown} ends, {what} It says how the run ended, so end your turn now rather than waiting or polling. If it has not ended by {}, it fires saying it timed out.", when(trigger.shown_next_at().unwrap_or(now))).into());
        return Ok(Value::Object(answer));
    }
    if let Some(check) = &trigger.check {
        answer.insert("check".into(), check.said().into());
    }
    match (&trigger.when, next_at) {
        (TriggerWhen::Ci { on }, _) => {
            answer.insert("note".into(), format!("Once CI on {on} ends, {what} It says how CI went, so end your turn now rather than waiting or polling.").into());
        }
        (TriggerWhen::Mod { on }, _) => {
            answer.insert("note".into(), format!("Each time anyone but you adds or changes documents in {on}{}, {what} The documents are named, a few seconds of changes at once; read them with mod_read.", match watch.as_ref().is_some_and(on_mod::Watch::filtered) {
                true => " that match where",
                false => "",
            }).into());
        }
        (TriggerWhen::Webhook { url }, _) => {
            answer.insert("url".into(), url.clone().into());
            answer.insert("note".into(), format!("Each time something posts to url, {what} The request comes with it, marked as untrusted. Anyone with the url can fire it, so give it only to the service that needs it.").into());
        }
        (_, Some(next)) => {
            answer.insert("next".into(), when(next).into());
            answer.insert("nextAt".into(), next.into());
            answer.insert("note".into(), match &trigger.check {
                Some(_) => format!("Each time it comes due its check runs, and only when the check says something other than the last time, {what} check holds what it says now."),
                None => format!("When it fires, {what}"),
            }
            .into());
        }
        (_, None) => {}
    }
    Ok(Value::Object(answer))
}

/// Sets up a trigger the person made, which tells an open chat its prompt or starts a new chat in
/// a project each time it fires. Answers its row, with the address of a webhook.
pub(crate) async fn add_own(
    core: &Arc<Core>,
    params: AddTriggerParams,
) -> Result<RunsAlone, String> {
    let prompt = params.prompt.trim().to_owned();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "What it sends must be 1 to {MAX_PROMPT_CHARS} characters"
        ));
    }
    let title = own_title(params.title.as_deref(), &prompt);
    let (agent_id, board) = match params.wakes {
        TriggerWakes::Chat { agent } => {
            if core.names().get(&agent).is_none() || core.workspaces().closed(&agent) {
                return Err("That chat is not open".into());
            }
            (agent, None)
        }
        TriggerWakes::Project { project } => (
            String::new(),
            Some(super::board::project_at(core, &project)?),
        ),
    };
    let now = epoch_ms();
    let (rule, next_at, hook_id) = match params.when {
        TriggerOn::Webhook => {
            let hook = super::hooks::register_hook(core)
                .await
                .map_err(|error| format!("the webhook could not be made: {error}"))?;
            (
                TriggerWhen::Webhook { url: hook.url },
                None,
                Some(hook.hook_id),
            )
        }
        on => {
            let (rule, at) = own_when(on, now)?;
            (rule, Some(at), None)
        }
    };
    let added = core.triggers().change(|state| {
        let (own, most) = match &board {
            Some(project) => (
                (state.triggers.iter())
                    .filter(|trigger| trigger.board.as_ref() == Some(project))
                    .count(),
                MAX_PER_BOARD,
            ),
            None => (
                (state.triggers.iter())
                    .filter(|trigger| trigger.prompts(&agent_id))
                    .count(),
                MAX_PER_AGENT,
            ),
        };
        if own >= most {
            return Err(format!("It already has {most} triggers; remove one first"));
        }
        if state.triggers.len() >= MAX_TRIGGERS {
            return Err(format!("This host already holds {MAX_TRIGGERS} triggers"));
        }
        state.next_id += 1;
        let trigger = Trigger {
            id: state.next_id,
            agent_id,
            board,
            title,
            prompt,
            when: rule,
            next_at,
            hook_id: hook_id.clone(),
            paused: None,
            ci: None,
            park: None,
            rundeck: None,
            check: None,
            watch: None,
            stopped: None,
            set: Some(learned(core, None)),
            held: Vec::new(),
            dropped: 0,
            from_github: false,
        };
        state.triggers.push(trigger.clone());
        Ok(trigger)
    });
    let trigger = match added {
        Ok(trigger) => trigger,
        Err(error) => {
            if let Some(hook_id) = hook_id {
                drop(super::hooks::remove_hook(core, &hook_id).await);
            }
            return Err(error);
        }
    };
    show(core, &trigger);
    Ok(trigger.line())
}

/// The title the person gave, or else the first line of what it sends, cut to fit.
fn own_title(given: Option<&str>, prompt: &str) -> String {
    let title = given
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| prompt.lines().next().unwrap_or_default().trim());
    match title.chars().count() > MAX_TITLE_CHARS {
        true => {
            let cut: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
            format!("{}…", cut.trim_end())
        }
        false => title.to_owned(),
    }
}

/// The time rule the person chose, and when it first fires.
fn own_when(on: TriggerOn, now: u64) -> Result<(TriggerWhen, u64), String> {
    let latest = now + MAX_AHEAD_SECONDS * 1000;
    match on {
        TriggerOn::Every { seconds } => {
            if !(MIN_EVERY_SECONDS..=MAX_AHEAD_SECONDS).contains(&seconds) {
                return Err("It must repeat at most once a minute and at least once a year".into());
            }
            Ok((TriggerWhen::Every { seconds }, now + seconds * 1000))
        }
        TriggerOn::Daily { minute, weekdays } => {
            if minute >= 24 * 60
                || weekdays
                    .iter()
                    .any(|day| usize::from(*day) >= WEEKDAYS.len())
            {
                return Err("That is not a time of day on chosen weekdays".into());
            }
            let mut weekdays = weekdays;
            weekdays.sort_unstable();
            weekdays.dedup();
            if weekdays.len() == WEEKDAYS.len() {
                weekdays.clear();
            }
            let at = next_daily(minute, &weekdays, now)
                .ok_or("That time does not come on this host's clock")?;
            Ok((TriggerWhen::Daily { minute, weekdays }, at))
        }
        TriggerOn::Once { at } => {
            if at <= now || at > latest {
                return Err("It must fire later than now and within a year".into());
            }
            Ok((TriggerWhen::Once { at }, at))
        }
        TriggerOn::Webhook => Err("A webhook fires on each request, not at a time".into()),
    }
}

pub(crate) fn list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = crate::server::caller::agent_of(request, "trigger_list")?;
    let project = crate::server::caller::project_of(core, request);
    let state = core.triggers().state.locked();
    let mut triggers: Vec<&Trigger> = state
        .triggers
        .iter()
        .filter(|trigger| yours(trigger, &agent_id, &project))
        .collect();
    triggers.sort_by_key(|trigger| trigger.next_at.unwrap_or(u64::MAX));
    let listed: Vec<Value> = triggers
        .into_iter()
        .map(|trigger| {
            json!({
                "trigger": trigger.id,
                "title": trigger.title,
                "prompt": trigger.prompt,
                "when": trigger.when,
                "board": trigger.board.is_some(),
                "next": trigger.next_at.map(when),
                "nextAt": trigger.next_at,
            })
        })
        .collect();
    Ok(json!({ "triggers": listed }))
}

pub(crate) fn remove_by_agent(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = crate::server::caller::agent_of(request, "trigger_remove")?;
    let project = crate::server::caller::project_of(core, request);
    let id = request
        .params
        .get("trigger")
        .and_then(Value::as_u64)
        .ok_or("trigger must be the number trigger_add or trigger_list gave")?;
    if remove_where(core, |trigger| {
        trigger.id == id && yours(trigger, &agent_id, &project)
    })
    .is_none()
    {
        return Err(format!(
            "Neither this chat nor its project's board has trigger {id}; trigger_list shows them"
        ));
    }
    Ok(json!({ "removed": id }))
}

/// A trigger of the agent's own chat, or of its project's board.
fn yours(trigger: &Trigger, agent_id: &str, project: &str) -> bool {
    match &trigger.board {
        Some(board) => board == project,
        None => trigger.prompts(agent_id),
    }
}

/// Pauses trigger `id` at the person's word, or resumes it. A repeating one skips the times
/// that passed while it was paused; a one-off that came due fires once it is resumed.
pub(crate) fn hold(core: &Arc<Core>, params: HoldTriggerParams) -> bool {
    let now = epoch_ms();
    let held = core.triggers().change(|state| {
        let trigger = state
            .triggers
            .iter_mut()
            .find(|trigger| trigger.id == params.trigger)?;
        match params.paused {
            true => trigger.stopped = trigger.stopped.or(Some(now)),
            false if trigger.stopped.take().is_some() => {
                if let Some(due) = trigger.next_at.filter(|due| *due <= now) {
                    trigger.next_at = next_after(&trigger.when, due, now).or(Some(now));
                }
            }
            false => {}
        }
        Some(trigger.clone())
    });
    let Some(trigger) = held else {
        return false;
    };
    show(core, &trigger);
    true
}

/// What runs in `project` with nobody asking: the triggers its chats and its board set,
/// soonest first.
pub(crate) fn for_playbook(core: &Core, project: &str) -> Vec<RunsAlone> {
    let state = core.triggers().state.locked();
    let mut lines: Vec<(Option<u64>, RunsAlone)> = state
        .triggers
        .iter()
        .filter(|trigger| match &trigger.board {
            Some(board) => board == project,
            None => core
                .names()
                .get(&trigger.agent_id)
                .is_some_and(|named| named.project == project),
        })
        .map(|trigger| (trigger.next_at, trigger.line()))
        .collect();
    lines.sort_by_key(|(next, _)| next.unwrap_or(u64::MAX));
    lines.into_iter().map(|(_, line)| line).collect()
}

/// Parks `agent_id` on its heavy command `command`, run in `cwd` of `project`, until its turn
/// on the host comes, replacing a park on the same command it had.
pub(crate) fn park_in_line(
    core: &Arc<Core>,
    agent_id: &str,
    project: &str,
    cwd: &str,
    command: &str,
) {
    line_left(core, agent_id, cwd, command);
    let now = epoch_ms();
    let at = now + park::DEFAULT_TIMEOUT_SECONDS * 1000;
    let park = park::Park::in_line(core, project, cwd, command);
    let trigger = core.triggers().change(|state| {
        state.next_id += 1;
        let trigger = Trigger {
            id: state.next_id,
            agent_id: agent_id.to_owned(),
            board: None,
            title: format!("{command} in line"),
            prompt: format!("Your turn for `{command}` came on this host."),
            when: TriggerWhen::Once { at },
            next_at: Some(at),
            hook_id: None,
            paused: None,
            ci: None,
            park: Some(park),
            rundeck: None,
            check: None,
            watch: None,
            stopped: None,
            set: Some(learned(core, Some(agent_id))),
            held: Vec::new(),
            dropped: 0,
            from_github: false,
        };
        state.triggers.push(trigger.clone());
        trigger
    });
    show(core, &trigger);
}

/// Parks `agent_id` until the thing `name` of `project`, shown as `label`, is free for it,
/// replacing a park on the same thing it had.
pub(crate) fn park_for_thing(
    core: &Arc<Core>,
    agent_id: &str,
    project: &str,
    name: &str,
    label: &str,
) {
    let mine = |trigger: &Trigger| {
        trigger.agent_id == agent_id
            && (trigger.park.as_ref()).is_some_and(|park| park.is_thing(project, name))
    };
    remove_where(core, mine);
    let at = epoch_ms() + park::DEFAULT_TIMEOUT_SECONDS * 1000;
    let park = park::Park::for_thing(core, project, name);
    let trigger = core.triggers().change(|state| {
        state.next_id += 1;
        let trigger = Trigger {
            id: state.next_id,
            agent_id: agent_id.to_owned(),
            board: None,
            title: label.to_owned(),
            prompt: format!("{label} is free for you now."),
            when: TriggerWhen::Once { at },
            next_at: Some(at),
            hook_id: None,
            paused: None,
            ci: None,
            park: Some(park),
            rundeck: None,
            check: None,
            watch: None,
            stopped: None,
            set: Some(learned(core, Some(agent_id))),
            held: Vec::new(),
            dropped: 0,
            from_github: false,
        };
        state.triggers.push(trigger.clone());
        trigger
    });
    show(core, &trigger);
}

/// Wakes `agent_id`, parked on the thing `name` of `project`, now that it is free for it.
pub(crate) fn in_use_freed(
    core: &Arc<Core>,
    agent_id: &str,
    project: &str,
    name: &str,
    label: &str,
) {
    let parked = |trigger: &Trigger| {
        trigger.agent_id == agent_id
            && (trigger.park.as_ref()).is_some_and(|park| park.is_thing(project, name))
    };
    let Some(mut trigger) = remove_where(core, parked) else {
        return;
    };
    trigger.prompt = format!(
        "{label} is free for you now. Call hold with \"{name}\" to take it: Sikemux keeps it for you {} s, then hands it to the next in line.",
        super::line::ROOM_KEPT_MS / 1000
    );
    tokio::spawn(fire(
        core.clone(),
        Firing {
            trigger,
            late: None,
            last: true,
            request: None,
        },
    ));
}

/// Wakes `agent_id`, parked on its heavy command `command` run in `cwd`, now that its turn came.
pub(crate) fn line_turn_came(core: &Arc<Core>, agent_id: &str, cwd: &str, command: &str) {
    let parked = |trigger: &Trigger| {
        trigger.agent_id == agent_id
            && (trigger.park.as_ref()).is_some_and(|park| park.is_line(cwd, command))
    };
    let Some(mut trigger) = remove_where(core, parked) else {
        return;
    };
    trigger.prompt = format!(
        "Your turn for `{command}` came on this host. Run it again now, in {cwd}: the host keeps room for it for {} s, then gives the room to the next in line.",
        super::line::ROOM_KEPT_MS / 1000
    );
    tokio::spawn(fire(
        core.clone(),
        Firing {
            trigger,
            late: None,
            last: true,
            request: None,
        },
    ));
}

/// Takes away the park `agent_id` had on its heavy command `command` run in `cwd`, which it
/// runs again now.
pub(crate) fn line_left(core: &Arc<Core>, agent_id: &str, cwd: &str, command: &str) {
    remove_where(core, |trigger: &Trigger| {
        trigger.agent_id == agent_id
            && (trigger.park.as_ref()).is_some_and(|park| park.is_line(cwd, command))
    });
}

/// Removes the triggers on the mod `mod_id`'s collections, as the mod goes.
pub(crate) fn mod_removed(core: &Arc<Core>, mod_id: &str) {
    if !core.triggers().watched.locked().contains(mod_id) {
        return;
    }
    let on_it = |trigger: &Trigger| {
        trigger
            .watch
            .as_ref()
            .is_some_and(|watch| watch.is_on(mod_id))
    };
    let gone: Vec<Trigger> = core.triggers().change(|state| {
        let (gone, kept) = std::mem::take(&mut state.triggers)
            .into_iter()
            .partition(on_it);
        state.triggers = kept;
        gone
    });
    gone.iter().for_each(|trigger| show(core, trigger));
}

/// Commits `agents` made went to the remote from `dir`, with `tip` the newest, so the core
/// watches the CI on it for them.
pub(crate) fn pushed(core: &super::Core, dir: &std::path::Path, agents: &[String], tip: &str) {
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    let (dir, agents, tip) = (
        dir.to_string_lossy().into_owned(),
        agents.to_vec(),
        tip.to_owned(),
    );
    runtime.spawn(async move {
        if let Some(watch) = ci::after_push(&core, dir, agents, tip).await {
            watch_after_push(&core, watch);
        }
    });
}

/// Keeps one watch per pushed run, and one per agent: a newer push of an agent's stands for
/// the CI of its older ones.
fn watch_after_push(core: &Arc<Core>, watch: ci::CiWatch) {
    let now = epoch_ms();
    core.triggers().change(|state| {
        let mut same_run = None;
        for (at, trigger) in state.triggers.iter_mut().enumerate() {
            let Some(other) = trigger.ci.as_mut().filter(|other| other.after_push()) else {
                continue;
            };
            match other.same_run(&watch) {
                true => same_run = Some(at),
                false if other.same_repo(&watch) => other.forget(watch.pushed_by()),
                false => {}
            }
        }
        match (same_run, watch.pushed_by().first().cloned()) {
            (Some(at), _) => {
                let same = state.triggers.get_mut(at).and_then(|same| same.ci.as_mut());
                if let Some(other) = same {
                    other.join(watch.pushed_by());
                }
            }
            (None, Some(agent_id)) if state.triggers.len() < MAX_TRIGGERS => {
                state.next_id += 1;
                state.triggers.push(Trigger {
                    id: state.next_id,
                    agent_id,
                    board: None,
                    title: format!("CI after the push of {}", watch.shown()),
                    prompt: String::new(),
                    when: TriggerWhen::Ci { on: watch.shown() },
                    next_at: Some(now + watch.look_again_ms()),
                    hook_id: None,
                    paused: None,
                    ci: Some(watch),
                    park: None,
                    rundeck: None,
                    check: None,
                    watch: None,
                    stopped: None,
                    set: None,
                    held: Vec::new(),
                    dropped: 0,
                    from_github: false,
                });
            }
            (None, _) => {}
        }
        state.triggers.retain(|trigger| {
            !(trigger.ci.as_ref()).is_some_and(|ci| ci.after_push() && ci.pushed_by().is_empty())
        });
    });
    core.changed(What::Read(super::changed::Topic::Playbook));
    super::codehost::ci::start(core);
}

/// Removes trigger `id` at the person's word, saying whether there was one.
pub(crate) fn remove(core: &Arc<Core>, id: u64) -> bool {
    remove_where(core, |trigger| trigger.id == id).is_some()
}

fn remove_where(core: &Arc<Core>, matches: impl Fn(&Trigger) -> bool) -> Option<Trigger> {
    let removed = core.triggers().change(|state| {
        let at = state.triggers.iter().position(&matches)?;
        Some(state.triggers.remove(at))
    })?;
    unhook(core, &removed);
    show(core, &removed);
    Some(removed)
}

/// Puts the chat's triggers in its feed, as it starts or wakes.
pub(crate) fn seed(core: &Core, feed: &super::chat::feed::Feed, agent_id: &str) {
    feed.set_triggers(core.triggers().shown(agent_id));
}

fn text(params: &Value, key: &str, max: usize) -> Result<String, String> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be text"))?;
    if value.chars().count() > max {
        return Err(format!("{key} must be at most {max} characters"));
    }
    Ok(value.to_owned())
}

/// Takes a gone trigger's webhook off the accounts server, so its address stops taking requests.
fn unhook(core: &Core, trigger: &Trigger) {
    if let (Some(hook_id), Some(core)) = (trigger.hook_id.clone(), core.arc()) {
        tokio::spawn(async move {
            if let Err(error) = super::hooks::remove_hook(&core, &hook_id).await {
                eprintln!("sikemux core: webhook {hook_id} was not taken off the accounts server: {error}");
            }
        });
    }
}

const TIMES: [&str; 4] = ["in", "at", "every", "daily"];

fn given(params: &Value, key: &str) -> bool {
    params.get(key).is_some_and(|value| !value.is_null())
}

/// The one time rule the call names, and when it first fires.
fn when_of(params: &Value, now: u64) -> Result<(TriggerWhen, u64), String> {
    let given: Vec<&str> = TIMES.into_iter().filter(|key| given(params, key)).collect();
    let [key] = given.as_slice() else {
        return Err("Give exactly one of in, at, every or daily, or webhook: true".into());
    };
    let value = &params[*key];
    let latest = now + MAX_AHEAD_SECONDS * 1000;
    match *key {
        "in" => {
            let seconds = seconds(value, "in")?;
            let at = now + seconds * 1000;
            Ok((TriggerWhen::Once { at }, at))
        }
        "at" => {
            let at = value
                .as_str()
                .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok())
                .ok_or("at must be a time such as 2026-10-28T09:30:00+05:30")?;
            let at = u64::try_from(at.unix_timestamp())
                .ok()
                .and_then(|seconds| seconds.checked_mul(1000))
                .filter(|at| *at > now)
                .ok_or("at must be later than now")?;
            if at > latest {
                return Err("at must be within a year".into());
            }
            Ok((TriggerWhen::Once { at }, at))
        }
        "every" => {
            let seconds = seconds(value, "every")?;
            Ok((TriggerWhen::Every { seconds }, now + seconds * 1000))
        }
        _ => {
            let (minute, weekdays) = daily(value, params.get("weekdays"))?;
            let at = next_daily(minute, &weekdays, now)
                .ok_or("That time does not come on this host's clock")?;
            Ok((TriggerWhen::Daily { minute, weekdays }, at))
        }
    }
}

fn seconds(value: &Value, key: &str) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|seconds| (MIN_EVERY_SECONDS..=MAX_AHEAD_SECONDS).contains(seconds))
        .ok_or_else(|| {
            format!("{key} must be a whole number of seconds from {MIN_EVERY_SECONDS} to a year")
        })
}

fn daily(value: &Value, weekdays: Option<&Value>) -> Result<(u32, Vec<u8>), String> {
    let minute = value
        .as_str()
        .and_then(|text| text.split_once(':'))
        .and_then(|(hour, minute)| Some((hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?)))
        .filter(|(hour, minute)| *hour < 24 && *minute < 60)
        .map(|(hour, minute)| hour * 60 + minute)
        .ok_or("daily must be a time on this host's clock such as 09:30")?;
    let mut days = Vec::new();
    for day in weekdays.and_then(Value::as_array).into_iter().flatten() {
        let day = day
            .as_str()
            .map(str::to_lowercase)
            .and_then(|day| WEEKDAYS.iter().position(|known| day.starts_with(known)))
            .ok_or("weekdays must be days such as mon, tue or sat")?;
        days.push(day as u8);
    }
    days.sort_unstable();
    days.dedup();
    if days.len() == WEEKDAYS.len() {
        days.clear();
    }
    Ok((minute, days))
}

/// A request one of this host's webhooks received, as its sender made it.
pub(crate) struct HookDelivery {
    pub hook_id: String,
    pub delivery_id: String,
    /// Milliseconds since the Unix epoch.
    pub received_at: u64,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub query: String,
    pub body: Vec<u8>,
}

/// Takes a webhook's request: fires its trigger with the request, once however often the server
/// sends it. A request for a trigger that is gone is taken and dropped, and its webhook removed.
/// One for a trigger paused with its chat waits for the chat to be taken up again, up to the most
/// it keeps; one for a trigger the person paused is dropped.
pub(crate) fn delivered(core: &Arc<Core>, delivery: HookDelivery) -> Result<(), String> {
    if !core.triggers().first_time(&delivery.delivery_id) {
        return Ok(());
    }
    let trigger = core
        .triggers()
        .state
        .locked()
        .triggers
        .iter()
        .find(|trigger| trigger.hook_id.as_deref() == Some(delivery.hook_id.as_str()))
        .cloned();
    let Some(trigger) = trigger else {
        let core = core.clone();
        tokio::spawn(
            async move { drop(super::hooks::remove_hook(&core, &delivery.hook_id).await) },
        );
        return Ok(());
    };
    let from_github =
        (delivery.headers.iter()).any(|(name, _)| name.eq_ignore_ascii_case("x-github-event"));
    if from_github && !trigger.from_github {
        core.triggers().change(|state| {
            if let Some(kept) = (state.triggers.iter_mut()).find(|kept| kept.id == trigger.id) {
                kept.from_github = true;
            }
        });
        core.changed(What::Read(super::changed::Topic::Playbook));
    }
    if trigger.paused.is_some() && trigger.stopped.is_none() {
        let request = fired_request(delivery);
        core.triggers().change(|state| {
            let Some(kept) = (state.triggers.iter_mut()).find(|kept| kept.id == trigger.id) else {
                return;
            };
            kept.held.push(request);
            if kept.held.len() > MAX_HELD_REQUESTS {
                kept.held.remove(0);
                kept.dropped += 1;
            }
        });
        return Ok(());
    }
    if trigger.rests() {
        return Ok(());
    }
    if let Some(project) = trigger.board.clone() {
        let subject = match github::read(&delivery.headers, &delivery.body) {
            github::Read::Quiet => return Ok(()),
            github::Read::About(subject) => subject,
            github::Read::Elsewhere => None,
        };
        let request = fired_request(delivery);
        let core = core.clone();
        tokio::spawn(
            async move { on_board(core, &trigger, &project, Some(&request), subject).await },
        );
        return Ok(());
    }
    let firing = Firing {
        trigger,
        late: None,
        last: false,
        request: Some(fired_request(delivery)),
    };
    tokio::spawn(fire(core.clone(), firing));
    Ok(())
}

/// The request as its trigger carries it, with as much of its body as fits in a prompt.
fn fired_request(delivery: HookDelivery) -> FiredRequest {
    let mut body = String::from_utf8_lossy(&delivery.body).into_owned();
    if body.len() > MAX_REQUEST_BYTES {
        let mut cut = MAX_REQUEST_BYTES;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        body.push_str("\n[the rest of the body was cut]");
    }
    FiredRequest {
        method: delivery.method,
        received: when(delivery.received_at),
        query: delivery.query,
        headers: delivery
            .headers
            .into_iter()
            .map(|(name, value)| RequestHeader { name, value })
            .collect(),
        body,
        body_tree: None,
    }
}

impl super::part::Part for Triggers {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        if !waiting_on_ci(core).is_empty() {
            super::codehost::ci::start(core);
        }
        on_mod::gather_waiting(core);
        vec![
            super::part::on_wake(core, |core| &core.triggers().wake, settle),
            tokio::spawn(park::serve(core.clone())),
        ]
    }
}
