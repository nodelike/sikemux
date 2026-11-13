//! Triggers an agent sets: each, when its time comes, once or again and again, sends the
//! agent's own chat a prompt, or adds a card to its project's board, which Auto carries when it
//! is on, and stays until it is removed. A webhook trigger fires on each request to its address
//! instead of at a time. A chat's triggers pause while it is closed and go on when it is taken up
//! again.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use std::collections::VecDeque;
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
use super::chat::reset::{calendar, when, WallClock, Zone};
use super::kept::Kept;
use super::messages;
use super::wake::Wake;
use super::workspace::{Held, HeldChat};
use super::Core;
use crate::cli::protocol::HarnessRequest;

mod check;
mod ci;
mod park;

pub(crate) use ci::{ci_moved, waiting_on_ci};

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
    /// The command whose output, when it changes, is what makes a repeating trigger fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check: Option<check::Check>,
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
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

impl Trigger {
    /// Whether it waits instead of firing: its chat is closed, or the person paused it.
    fn rests(&self) -> bool {
        self.paused.is_some() || self.stopped.is_some()
    }

    /// Whether it prompts the open chat `agent_id`.
    fn prompts(&self, agent_id: &str) -> bool {
        self.board.is_none() && self.paused.is_none() && self.agent_id == agent_id
    }

    fn shown(&self) -> ChatTrigger {
        ChatTrigger {
            id: self.id,
            title: self.title.clone(),
            when: self.when.clone(),
            next_at: self.next_at.filter(|_| self.ci.is_none()),
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
        let state = Mutex::new(kept.load());
        Self {
            kept,
            state,
            wake: Wake::default(),
            seen: Mutex::default(),
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

    /// The chat's triggers, soonest first, as its clients show them.
    pub(crate) fn shown(&self, agent_id: &str) -> Vec<ChatTrigger> {
        self.shown_where(|trigger| trigger.prompts(agent_id))
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
                trigger.prompts(agent_id) && trigger.ci.is_none() && trigger.park.is_none()
            })
            .filter_map(|trigger| trigger.next_at)
            .min()
    }

    /// What the agent's chat waits on of the wakes it set itself to carry its work on: CI first,
    /// else its soonest one-time timer, which it has waited on since `rested_at`. A repeating
    /// trigger is automation and a paused one waits on nothing, so neither is a wait.
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
            });
        }
        let soonest = mine()
            .filter(|trigger| matches!(trigger.when, TriggerWhen::Once { .. }))
            .filter_map(|trigger| Some((trigger, trigger.next_at?)))
            .min_by_key(|(_, at)| *at)?;
        let (trigger, at) = soonest;
        Some(TurnWait {
            kind: TurnWaitKind::Timer,
            what: trigger.title.clone(),
            started_at: rested_at.min(at),
            expected_ms: None,
            until: Some(at),
        })
    }

    /// How many triggers prompt the open chat `agent_id`.
    pub(crate) fn count(&self, agent_id: &str) -> u32 {
        let state = self.state.locked();
        let mine = state
            .triggers
            .iter()
            .filter(|trigger| trigger.prompts(agent_id));
        mine.count() as u32
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
            if trigger.ci.is_some() {
                looking.push(trigger.id);
                trigger.next_at = Some(now + ci::LOOK_AGAIN_MS);
                return true;
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
            .filter(|trigger| trigger.board.is_none() && trigger.paused.is_none())
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
        TriggerWhen::Once { .. } | TriggerWhen::Webhook { .. } | TriggerWhen::Ci { .. } => None,
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
            if trigger.ci.is_some() {
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
                .partition(|trigger| trigger.board.is_none() && gone.contains(&trigger.agent_id));
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

async fn fire(core: Arc<Core>, firing: Firing) {
    let trigger = &firing.trigger;
    if let Some(project) = &trigger.board {
        let details = match &firing.request {
            Some(request) => format!("{}\n\n{}", trigger.prompt, fired::request_text(request)),
            None => trigger.prompt.clone(),
        };
        if let Err(error) =
            super::board::add_for_trigger(&core, project, trigger.id, &trigger.title, &details)
        {
            eprintln!(
                "sikemux core: trigger {} added no card to {project}'s board: {error}",
                trigger.id
            );
        }
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
    let note = format!(
        "Sent by trigger {} that this chat set with trigger_add, not by the person or another agent. {after}",
        trigger.id
    );
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
    let checks = {
        let state = core.triggers().state.locked();
        let trigger = state.triggers.iter().find(|trigger| trigger.id == id);
        trigger.is_some_and(|trigger| trigger.check.is_some())
    };
    match checks {
        true => check::look(core, id).await,
        false => ci::look(core, id).await,
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
    let check = match params.get("check") {
        None | Some(Value::Null) => None,
        Some(Value::String(command)) if !command.trim().is_empty() => {
            if !given(params, "every") && !given(params, "daily") {
                return Err("A check runs each time a repeating trigger comes due, so give it every or daily".into());
            }
            Some(check::Check::first(command.trim().to_owned(), request.project.clone()).await?)
        }
        Some(_) => return Err("check must be a shell command".into()),
    };
    let (rule, next_at, hook_id) = match params.get("webhook") {
        _ if park.is_some() => {
            let at = park.as_ref().map_or(now, |(_, _, at)| *at);
            (TriggerWhen::Once { at }, Some(at), None)
        }
        _ if ci.is_some() => {
            let shown = ci
                .as_ref()
                .map(|(_, shown)| shown.clone())
                .unwrap_or_default();
            (
                TriggerWhen::Ci { on: shown },
                Some(now + ci::LOOK_AGAIN_MS),
                None,
            )
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
            check,
            stopped: None,
            set: Some(learned(core, Some(&agent_id))),
            held: Vec::new(),
            dropped: 0,
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
    let mut answer = json!({ "trigger": trigger.id });
    if let Some((_, shown, at)) = &park {
        answer["note"] = format!("Once {shown} does what you wait for, {what} It says what happened, so end your turn now rather than waiting or polling. If nothing happens by {}, it fires saying it timed out.", when(*at)).into();
        return Ok(answer);
    }
    if let Some(check) = &trigger.check {
        answer["check"] = check.said().into();
    }
    match (&trigger.when, next_at) {
        (TriggerWhen::Ci { on }, _) => {
            answer["note"] = format!("Once CI on {on} ends, {what} It says how CI went, so end your turn now rather than waiting or polling.").into();
        }
        (TriggerWhen::Webhook { url }, _) => {
            answer["url"] = url.clone().into();
            answer["note"] = format!("Each time something posts to url, {what} The request comes with it, marked as untrusted. Anyone with the url can fire it, so give it only to the service that needs it.").into();
        }
        (_, Some(next)) => {
            answer["next"] = when(next).into();
            answer["nextAt"] = next.into();
            answer["note"] = match &trigger.check {
                Some(_) => format!("Each time it comes due its check runs, and only when the check says something other than the last time, {what} check holds what it says now."),
                None => format!("When it fires, {what}"),
            }
            .into();
        }
        (_, None) => {}
    }
    Ok(answer)
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
        .map(|trigger| {
            let line = RunsAlone::Trigger {
                id: trigger.id,
                title: trigger.title.clone(),
                when: trigger.when.clone(),
                next_at: trigger.next_at.filter(|_| trigger.ci.is_none()),
                set: trigger.set.clone().map(Box::new),
                paused: trigger.stopped,
            };
            (trigger.next_at, line)
        })
        .collect();
    lines.sort_by_key(|(next, _)| next.unwrap_or(u64::MAX));
    lines.into_iter().map(|(_, line)| line).collect()
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerParams {
    pub trigger: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldTriggerParams {
    pub trigger: u64,
    /// True pauses it, false resumes it.
    pub paused: bool,
}

impl super::part::Part for Triggers {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        if !waiting_on_ci(core).is_empty() {
            super::codehost::ci::start(core);
        }
        vec![
            super::part::on_wake(core, |core| &core.triggers().wake, settle),
            tokio::spawn(park::serve(core.clone())),
        ]
    }
}
