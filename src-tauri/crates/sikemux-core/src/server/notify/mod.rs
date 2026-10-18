//! Tells paired phones, through the account's push service, when an agent
//! needs the person, finishes a long turn, runs into a problem, or when a
//! device asks to pair. Each phone chose what it hears about and gave this
//! host a key to seal it with (see [`crate::push`]); the host decides
//! everything else, since it knows most.

mod presence;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::accounts::live::Push;
use crate::accounts::protocol::PushKind;
use crate::protocol::{Attention, NotifyPrefs, NotifyWhen, PendingDevice};
use crate::push::{self, Notification, NotificationCategory, NotificationKind, MAX_PLAINTEXT};

pub(crate) use presence::{Presence, SystemPresence};

use super::access::{self, Needs, Peer};
use super::remote::{unix_ms, NotifyTarget};
use super::Core;

/// How often the host looks whether the person left, to send what waited
/// for that.
const TICK: Duration = Duration::from_secs(15);
const FINISHED_AFTER_MS: u64 = 30_000;
const FINISHED_GAP_MS: u64 = 5 * 60_000;
const PROBLEM_GAP_MS: u64 = 60_000;
const PERMISSION_LIFE_MS: u64 = 60 * 60_000;
const INPUT_LIFE_MS: u64 = 30 * 60_000;
const FINISHED_LIFE_MS: u64 = 2 * 60_000;
const PROBLEM_LIFE_MS: u64 = 10 * 60_000;
const CLEAR_LIFE_MS: u64 = 60 * 60_000;
const MAX_TITLE_CHARS: usize = 100;
const MAX_BODY_CHARS: usize = 180;
const MAX_DETAIL_CHARS: usize = 120;
const MAX_OPTION_ID: usize = 128;
/// A terminal agent's state while it waits for the person to type.
const NEEDS_INPUT: &str = "blocked";

/// Something that happened on this host that a phone may want to hear of.
#[derive(Clone, Debug)]
pub(crate) enum Signal {
    Attention(Box<Attention>),
    Cleared {
        id: String,
        agent_id: String,
    },
    AgentState {
        agent_id: String,
        state: String,
    },
    TurnStarted {
        agent_id: String,
    },
    TurnCompleted {
        agent_id: String,
        cancelled: bool,
    },
    Problem {
        agent_id: String,
        message: String,
    },
    /// An agent's question now waits on the person.
    Question {
        id: String,
        agent_id: String,
        name: String,
        text: String,
        at: u64,
    },
    QuestionCleared {
        id: String,
        agent_id: String,
    },
    /// A device waits for someone to let it pair.
    Pairing(PendingDevice),
    PairingSettled {
        id: String,
    },
}

/// A question waiting on the person, as a phone is told of it.
#[derive(Clone, Debug)]
struct Asked {
    name: String,
    text: String,
    at: u64,
}

#[derive(Default)]
pub(crate) struct Notifier {
    signals: Mutex<Option<mpsc::UnboundedSender<Signal>>>,
    state: Mutex<State>,
    presence: Mutex<Option<Arc<dyn Presence>>>,
    host_name: OnceLock<String>,
}

#[derive(Default)]
struct State {
    away: bool,
    /// When the person last went away.
    left_at: Option<u64>,
    /// Cards a phone was sent and that are still showing, by phone and card.
    alerted: HashSet<(String, String)>,
    turns: HashMap<String, u64>,
    finished: HashMap<(String, String), u64>,
    problems: HashMap<(String, String), u64>,
    waiting_for_input: HashSet<String>,
    /// Permission requests still waiting, by agent and request.
    pending: HashMap<(String, String), Attention>,
    /// Agents' questions still waiting on the person, by agent and question.
    questions: HashMap<(String, String), Asked>,
}

impl Notifier {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Hands `signal` to the notifier's task. Dropped when it is not running.
    pub(crate) fn send(&self, signal: Signal) {
        if let Ok(signals) = self.signals.lock() {
            if let Some(signals) = signals.as_ref() {
                let _ = signals.send(signal);
            }
        }
    }

    fn presence(&self) -> Arc<dyn Presence> {
        let mut presence = self
            .presence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        presence
            .get_or_insert_with(|| Arc::new(SystemPresence))
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn set_presence(&self, presence: Arc<dyn Presence>) {
        *self
            .presence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(presence);
    }

    /// When the person last went away, and whether they still are.
    pub(crate) fn absence(&self) -> (Option<u64>, bool) {
        let state = self.lock();
        (state.left_at, state.away)
    }

    fn host_name(&self) -> String {
        self.host_name
            .get()
            .cloned()
            .unwrap_or_else(|| "your computer".into())
    }
}

/// Handles signals in order as they come, and watches for the person leaving.
pub(crate) async fn run(core: Arc<Core>) {
    let (signals, mut received) = mpsc::unbounded_channel();
    if let Ok(mut installed) = core.notify.signals.lock() {
        *installed = Some(signals);
    }
    let named = tokio::task::spawn_blocking(|| super::host::info().name).await;
    if let Ok(name) = named {
        let _ = core.notify.host_name.set(name);
    }
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            signal = received.recv() => match signal {
                Some(signal) => {
                    noted(&core, away_now(&core).await, unix_ms());
                    apply(&core, signal, unix_ms());
                }
                None => return,
            },
            _ = ticker.tick() => noted(&core, away_now(&core).await, unix_ms()),
        }
    }
}

/// Asking macOS whether the screen is locked can take seconds the first time,
/// so it runs off the threads that serve connections.
async fn away_now(core: &Core) -> bool {
    let presence = core.notify.presence();
    tokio::task::spawn_blocking(move || presence.away())
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

#[cfg(test)]
pub(crate) fn look_around(core: &Core, now: u64) {
    noted(core, core.notify.presence().away().unwrap_or(true), now);
}

/// Notes whether the person is here, and once they leave, sends what waited
/// for that: permission requests and agents still waiting for input.
fn noted(core: &Core, away: bool, now: u64) {
    let left = {
        let mut state = core.notify.lock();
        let left = away && !state.away;
        state.away = away;
        if left {
            state.left_at = Some(now);
        }
        left
    };
    if !left {
        return;
    }
    let pending: Vec<Attention> = core.notify.lock().pending.values().cloned().collect();
    for attention in pending {
        if let Some(draft) = permission(core, &attention) {
            alert_all(core, &draft, now);
        }
    }
    let questions: Vec<((String, String), Asked)> = core
        .notify
        .lock()
        .questions
        .iter()
        .map(|(key, asked)| (key.clone(), asked.clone()))
        .collect();
    for ((agent_id, id), asked) in questions {
        if let Some(draft) = question(core, &agent_id, &id, &asked) {
            alert_all(core, &draft, now);
        }
    }
    let waiting: Vec<String> = core
        .notify
        .lock()
        .waiting_for_input
        .iter()
        .cloned()
        .collect();
    for agent_id in waiting {
        if let Some(draft) = needs_input(core, &agent_id, now) {
            alert_all(core, &draft, now);
        }
    }
}

#[cfg(test)]
pub(crate) fn handle(core: &Core, signal: Signal, now: u64) {
    look_around(core, now);
    apply(core, signal, now);
}

fn apply(core: &Core, signal: Signal, now: u64) {
    match signal {
        Signal::Attention(attention) => {
            core.notify.lock().pending.insert(
                (attention.agent_id.clone(), attention.id.clone()),
                (*attention).clone(),
            );
            if let Some(draft) = permission(core, &attention) {
                alert_all(core, &draft, now);
            }
        }
        Signal::Cleared { id, agent_id } => {
            core.notify
                .lock()
                .pending
                .remove(&(agent_id.clone(), id.clone()));
            clear_all(
                core,
                &agent_id,
                &permission_card(&agent_id, &id),
                Some(id),
                now,
            );
        }
        Signal::AgentState { agent_id, state } => {
            let was_waiting = {
                let mut notify = core.notify.lock();
                if state == NEEDS_INPUT {
                    !notify.waiting_for_input.insert(agent_id.clone())
                } else {
                    notify.waiting_for_input.remove(&agent_id)
                }
            };
            if state == NEEDS_INPUT && !was_waiting {
                if let Some(draft) = needs_input(core, &agent_id, now) {
                    alert_all(core, &draft, now);
                }
            } else if state != NEEDS_INPUT && was_waiting {
                clear_all(core, &agent_id, &input_card(&agent_id), None, now);
            }
        }
        Signal::Question {
            id,
            agent_id,
            name,
            text,
            at,
        } => {
            let asked = Asked { name, text, at };
            core.notify
                .lock()
                .questions
                .insert((agent_id.clone(), id.clone()), asked.clone());
            if let Some(draft) = question(core, &agent_id, &id, &asked) {
                alert_all(core, &draft, now);
            }
        }
        Signal::QuestionCleared { id, agent_id } => {
            core.notify
                .lock()
                .questions
                .remove(&(agent_id.clone(), id.clone()));
            clear_all(core, &agent_id, &question_card(&agent_id, &id), None, now);
        }
        Signal::Pairing(device) => alert_all(core, &pairing(core, &device), now),
        Signal::PairingSettled { id } => clear_all(core, "", &pairing_card(&id), None, now),
        Signal::TurnStarted { agent_id } => {
            core.notify.lock().turns.insert(agent_id, now);
        }
        Signal::TurnCompleted {
            agent_id,
            cancelled,
        } => {
            let started = core.notify.lock().turns.remove(&agent_id);
            let long =
                started.is_some_and(|started| now.saturating_sub(started) >= FINISHED_AFTER_MS);
            if long && !cancelled && !core.harness.questions.waits_on_answer(&agent_id) {
                if let Some(draft) = finished(core, &agent_id, now) {
                    alert_all(core, &draft, now);
                }
            }
        }
        Signal::Problem { agent_id, message } => {
            if core.notify.lock().waiting_for_input.remove(&agent_id) {
                clear_all(core, &agent_id, &input_card(&agent_id), None, now);
            }
            if let Some(draft) = problem(core, &agent_id, &message, now) {
                alert_all(core, &draft, now);
            }
        }
    }
}

fn permission_card(agent_id: &str, request_id: &str) -> String {
    format!("permission|{agent_id}|{request_id}")
}

fn question_card(agent_id: &str, id: &str) -> String {
    format!("question|{agent_id}|{id}")
}

fn input_card(agent_id: &str) -> String {
    format!("input|{agent_id}")
}

fn pairing_card(id: &str) -> String {
    format!("pairing|{id}")
}

/// A notification before it is addressed to a phone.
#[derive(Clone, Debug)]
struct Draft {
    kind: NotificationKind,
    card: String,
    agent_id: String,
    /// A chat, rather than an agent in a terminal.
    chat: bool,
    provider: String,
    chat_title: Option<String>,
    title: String,
    body: String,
    detail: Option<String>,
    request_id: Option<String>,
    allow_option_id: Option<String>,
    reject_option_id: Option<String>,
    at: u64,
    expires_at: u64,
}

/// What the person would call an agent and where it works.
struct Named {
    chat: bool,
    provider: String,
    /// Its name, or its CLI's for an agent without one.
    agent: String,
    title: Option<String>,
    project: Option<String>,
}

fn provider_name(core: &Core, provider: &str) -> String {
    if let Some(agent) = crate::agent_catalog::agent(provider) {
        return agent.label.into();
    }
    core.workspaces
        .view()
        .launchers
        .into_iter()
        .find(|launcher| launcher.provider == provider)
        .map(|launcher| launcher.label)
        .unwrap_or_else(|| {
            let mut chars = provider.chars();
            chars.next().map_or_else(
                || "An agent".into(),
                |first| first.to_uppercase().chain(chars).collect(),
            )
        })
}

fn project_name(core: &Core, project: &str) -> String {
    core.workspaces
        .view()
        .projects
        .into_iter()
        .find(|known| known.id == project || known.path == Path::new(project))
        .map(|known| known.name)
        .unwrap_or_else(|| folder_name(Path::new(project)))
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn name(core: &Core, agent_id: &str) -> Option<Named> {
    if let Some(chat) = core.chats.get(agent_id) {
        let title = core
            .chat_infos()
            .into_iter()
            .find(|info| info.agent_id == agent_id)
            .and_then(|info| info.title);
        return Some(Named {
            chat: true,
            provider: chat.provider().to_owned(),
            agent: core
                .names
                .name(agent_id)
                .unwrap_or_else(|| provider_name(core, chat.provider())),
            title,
            project: Some(project_name(core, &chat.launch.cwd.to_string_lossy())),
        });
    }
    let session = core.agent_session(agent_id)?;
    let owner = session.owner();
    let provider = owner.agent_type.clone().unwrap_or_default();
    Some(Named {
        chat: false,
        agent: core
            .names
            .name(agent_id)
            .unwrap_or_else(|| provider_name(core, &provider)),
        provider,
        title: core.workspaces.title(agent_id),
        project: owner
            .project
            .as_deref()
            .map(|project| project_name(core, project)),
    })
}

pub(crate) fn clip(text: &str, limit: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() <= limit {
        return line.to_owned();
    }
    let mut clipped: String = line.chars().take(limit.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

impl Named {
    /// "Fix the tests on MacBook Pro": the chat, or its project, and the host.
    fn whereabouts(&self, core: &Core, prefer_title: bool) -> String {
        let place = if prefer_title {
            self.title.clone().or_else(|| self.project.clone())
        } else {
            self.project.clone().or_else(|| self.title.clone())
        };
        let host = core.notify.host_name();
        match place {
            Some(place) => format!("{place} on {host}"),
            None => format!("On {host}"),
        }
    }

    fn draft(
        &self,
        kind: NotificationKind,
        card: String,
        agent_id: &str,
        at: u64,
        life: u64,
    ) -> Draft {
        Draft {
            kind,
            card,
            agent_id: agent_id.to_owned(),
            chat: self.chat,
            provider: self.provider.clone(),
            chat_title: self
                .title
                .as_deref()
                .map(|title| clip(title, MAX_TITLE_CHARS)),
            title: String::new(),
            body: String::new(),
            detail: None,
            request_id: None,
            allow_option_id: None,
            reject_option_id: None,
            at,
            expires_at: at + life,
        }
    }
}

fn option_id(id: &Option<String>) -> Option<String> {
    id.clone().filter(|id| id.len() <= MAX_OPTION_ID)
}

fn permission(core: &Core, attention: &Attention) -> Option<Draft> {
    let named = name(core, &attention.agent_id)?;
    let mut draft = named.draft(
        NotificationKind::Permission,
        permission_card(&attention.agent_id, &attention.id),
        &attention.agent_id,
        attention.at,
        PERMISSION_LIFE_MS,
    );
    draft.title = clip(
        &format!("{} needs permission", named.agent),
        MAX_TITLE_CHARS,
    );
    draft.body = clip(&named.whereabouts(core, false), MAX_BODY_CHARS);
    let choices = &attention.choices;
    draft.detail = choices
        .title
        .as_deref()
        .map(|title| clip(title, MAX_DETAIL_CHARS));
    draft.request_id = Some(attention.id.clone());
    draft.allow_option_id = option_id(&choices.allow_once);
    draft.reject_option_id = option_id(&choices.reject);
    Some(draft)
}

fn question(core: &Core, agent_id: &str, id: &str, asked: &Asked) -> Option<Draft> {
    let named = name(core, agent_id)?;
    let mut draft = named.draft(
        NotificationKind::Input,
        question_card(agent_id, id),
        agent_id,
        asked.at,
        PERMISSION_LIFE_MS,
    );
    draft.title = clip(&format!("{} asks you", asked.name), MAX_TITLE_CHARS);
    draft.body = clip(&asked.text, MAX_BODY_CHARS);
    draft.detail = Some(clip(&named.whereabouts(core, true), MAX_DETAIL_CHARS));
    Some(draft)
}

fn needs_input(core: &Core, agent_id: &str, now: u64) -> Option<Draft> {
    let named = name(core, agent_id)?;
    let mut draft = named.draft(
        NotificationKind::Input,
        input_card(agent_id),
        agent_id,
        now,
        INPUT_LIFE_MS,
    );
    draft.title = clip(
        &format!("{} needs your input", named.agent),
        MAX_TITLE_CHARS,
    );
    draft.body = clip(&named.whereabouts(core, true), MAX_BODY_CHARS);
    Some(draft)
}

fn finished(core: &Core, agent_id: &str, now: u64) -> Option<Draft> {
    let named = name(core, agent_id)?;
    let mut draft = named.draft(
        NotificationKind::Finished,
        format!("finished|{agent_id}"),
        agent_id,
        now,
        FINISHED_LIFE_MS,
    );
    draft.title = clip(&format!("{} finished", named.agent), MAX_TITLE_CHARS);
    draft.body = clip(&named.whereabouts(core, true), MAX_BODY_CHARS);
    Some(draft)
}

fn problem(core: &Core, agent_id: &str, message: &str, now: u64) -> Option<Draft> {
    let named = name(core, agent_id)?;
    let mut draft = named.draft(
        NotificationKind::Problem,
        format!("problem|{agent_id}"),
        agent_id,
        now,
        PROBLEM_LIFE_MS,
    );
    draft.title = clip(
        &format!("{} ran into a problem", named.agent),
        MAX_TITLE_CHARS,
    );
    draft.body = clip(&named.whereabouts(core, true), MAX_BODY_CHARS);
    draft.detail = Some(clip(message, MAX_DETAIL_CHARS)).filter(|detail| !detail.is_empty());
    Some(draft)
}

fn pairing(core: &Core, device: &PendingDevice) -> Draft {
    let name = if device.name.is_empty() {
        "A phone"
    } else {
        &device.name
    };
    Draft {
        kind: NotificationKind::Pairing,
        card: pairing_card(&device.id),
        agent_id: String::new(),
        chat: false,
        provider: String::new(),
        chat_title: None,
        title: clip(&format!("{name} wants to connect"), MAX_TITLE_CHARS),
        body: clip(
            &format!("To {}, from your Sikemux account", core.notify.host_name()),
            MAX_BODY_CHARS,
        ),
        detail: None,
        request_id: None,
        allow_option_id: None,
        reject_option_id: None,
        at: unix_ms(),
        expires_at: device.expires_at,
    }
}

fn wants(
    prefs: &NotifyPrefs,
    kind: NotificationKind,
    agent_id: &str,
    away: bool,
    now: u64,
) -> bool {
    let kind_on = match kind {
        NotificationKind::Permission | NotificationKind::Input | NotificationKind::Pairing => {
            prefs.needs_you
        }
        NotificationKind::Finished => prefs.finished,
        NotificationKind::Problem => prefs.problems,
        NotificationKind::Clear => true,
    };
    let when = match prefs.when {
        NotifyWhen::Always => true,
        NotifyWhen::Away => away,
        NotifyWhen::Off => false,
    };
    let muted = prefs
        .muted
        .iter()
        .any(|mute| mute.agent_id == agent_id && mute.until.is_none_or(|until| until > now));
    kind_on && when && !muted
}

/// Only phones that may let a device in hear that one asks.
fn answers_pairing(core: &Core, phone: &str) -> bool {
    let peer = Peer::Device {
        id: phone.to_owned(),
    };
    access::permit(&peer, core.remote.standing_of(phone), Needs::Person).is_ok()
}

/// The phone has this agent open on screen, so it already sees it.
fn watching(core: &Core, phone: &str, draft: &Draft) -> bool {
    let chat = draft
        .chat
        .then(|| core.chats.get(&draft.agent_id))
        .flatten();
    let session = (!draft.chat)
        .then(|| core.agent_session(&draft.agent_id))
        .flatten();
    core.clients().iter().any(|client| {
        client.peer.is_device(phone)
            && client.in_front()
            && (chat
                .as_ref()
                .is_some_and(|chat| chat.feed.has_subscriber(client.id))
                || session
                    .as_ref()
                    .is_some_and(|session| client.is_subscribed(session.id)))
    })
}

fn address(draft: &Draft, host: &str, host_name: &str, target: &NotifyTarget) -> Option<String> {
    let collapse_id = target.key.collapse_id(&draft.card);
    let category = match draft.kind {
        NotificationKind::Permission
            if draft.allow_option_id.is_some() && draft.reject_option_id.is_some() =>
        {
            Some(NotificationCategory::Permission)
        }
        NotificationKind::Permission | NotificationKind::Input | NotificationKind::Pairing => {
            Some(NotificationCategory::NeedsYou)
        }
        NotificationKind::Finished => Some(NotificationCategory::Finished),
        NotificationKind::Problem => Some(NotificationCategory::Problem),
        NotificationKind::Clear => None,
    };
    let url = if draft.kind == NotificationKind::Pairing {
        format!("sikemux://device/{host}/pairing")
    } else if draft.chat {
        format!("sikemux://device/{host}/chat/{}", draft.agent_id)
    } else {
        format!("sikemux://device/{host}")
    };
    let mut notification = Notification {
        v: 1,
        kind: draft.kind,
        channel: draft.kind.channel(),
        category,
        collapse_id,
        thread: format!("{host}/{}", draft.agent_id),
        host_key: host.to_owned(),
        host_name: clip(host_name, MAX_TITLE_CHARS),
        agent_id: draft.agent_id.clone(),
        provider: clip(&draft.provider, MAX_TITLE_CHARS),
        chat_title: draft.chat_title.clone(),
        title: draft.title.clone(),
        body: draft.body.clone(),
        detail: draft.detail.clone(),
        url,
        request_id: draft.request_id.clone(),
        allow_option_id: draft.allow_option_id.clone(),
        reject_option_id: draft.reject_option_id.clone(),
        at: draft.at,
        expires_at: draft.expires_at,
    };
    let mut plaintext = serde_json::to_vec(&notification).ok()?;
    if plaintext.len() > MAX_PLAINTEXT {
        notification.detail = None;
        notification.chat_title = None;
        notification.body = clip(&notification.body, 60);
        plaintext = serde_json::to_vec(&notification).ok()?;
    }
    push::seal(&target.key, host, &target.device_id, &plaintext)
}

fn alert_all(core: &Core, draft: &Draft, now: u64) {
    let Some((host, targets)) = core.remote.notify_targets() else {
        return;
    };
    let away = core.notify.lock().away;
    let host_name = core.notify.host_name();
    let outbox = core.remote.outbox();
    for target in targets {
        let phone = target.device_id.clone();
        let card = (phone.clone(), draft.card.clone());
        if !wants(&target.prefs, draft.kind, &draft.agent_id, away, now) {
            continue;
        }
        if draft.kind == NotificationKind::Pairing && !answers_pairing(core, &phone) {
            continue;
        }
        {
            let state = core.notify.lock();
            let once = matches!(
                draft.kind,
                NotificationKind::Permission | NotificationKind::Input | NotificationKind::Pairing
            );
            if once && state.alerted.contains(&card) {
                continue;
            }
            let pair = (phone.clone(), draft.agent_id.clone());
            let recent = |last: Option<&u64>, gap: u64| {
                last.is_some_and(|last| now.saturating_sub(*last) < gap)
            };
            if draft.kind == NotificationKind::Finished
                && recent(state.finished.get(&pair), FINISHED_GAP_MS)
            {
                continue;
            }
            if draft.kind == NotificationKind::Problem
                && recent(state.problems.get(&pair), PROBLEM_GAP_MS)
            {
                continue;
            }
        }
        if watching(core, &phone, draft) {
            continue;
        }
        let Some(blob) = address(draft, &host, &host_name, &target) else {
            continue;
        };
        outbox.post(Push {
            to: phone.clone(),
            kind: PushKind::Alert,
            collapse_id: target.key.collapse_id(&draft.card),
            blob,
            expires_at: draft.expires_at,
        });
        let mut state = core.notify.lock();
        let pair = (phone, draft.agent_id.clone());
        match draft.kind {
            NotificationKind::Finished => {
                state.finished.insert(pair, now);
            }
            NotificationKind::Problem => {
                state.problems.insert(pair, now);
            }
            _ => {}
        }
        state.alerted.insert(card);
    }
}

/// Takes the card off every phone that was sent it: withdrawn if it has not
/// gone out yet, otherwise followed by a `clear`.
fn clear_all(core: &Core, agent_id: &str, card: &str, request_id: Option<String>, now: u64) {
    let Some((host, targets)) = core.remote.notify_targets() else {
        return;
    };
    let host_name = core.notify.host_name();
    let outbox = core.remote.outbox();
    for target in targets {
        let shown = (target.device_id.clone(), card.to_owned());
        if !core.notify.lock().alerted.remove(&shown) {
            continue;
        }
        let collapse_id = target.key.collapse_id(card);
        if outbox.withdraw(&target.device_id, &collapse_id) {
            continue;
        }
        let draft = Draft {
            kind: NotificationKind::Clear,
            card: card.to_owned(),
            agent_id: agent_id.to_owned(),
            chat: request_id.is_some(),
            provider: String::new(),
            chat_title: None,
            title: String::new(),
            body: String::new(),
            detail: None,
            request_id: request_id.clone(),
            allow_option_id: None,
            reject_option_id: None,
            at: now,
            expires_at: now + CLEAR_LIFE_MS,
        };
        let Some(blob) = address(&draft, &host, &host_name, &target) else {
            continue;
        };
        outbox.post(Push {
            to: target.device_id.clone(),
            kind: PushKind::Clear,
            collapse_id,
            blob,
            expires_at: draft.expires_at,
        });
    }
}
