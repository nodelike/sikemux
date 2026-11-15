//! When the core puts a chat to sleep and wakes it, and starts a terminal
//! agent again. A chat sleeps once it has been idle for a while, on screen or
//! not, and off screen the oldest go first when more than a few sit idle. A
//! message, a trigger or the person's first key wakes it on its saved session.
//! A chat at work, one waiting to send again a turn the network cut short, one
//! the person keeps alive or is writing in, and one the person started whose
//! last answer they have not read, is never put to sleep. A terminal agent that
//! crashes comes back on its saved session below its last screen, unless it
//! crashes again at once.

use crate::server::activity::ActivityCore;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::Notify;

use sikemux_wire::protocol::{AgentState, Continuation, SessionId};

use super::agent::AgentActivity;
use super::{launching, session, Core, CoreResult};

/// How long a chat stays idle before it sleeps. Over half of all replies come within it, and a
/// later one loses nothing: the person's first key wakes the chat and its cache holds on resume.
const IDLE_SLEEP_MS: u64 = 2 * 60_000;
/// How many idle chats nobody looks at stay awake, so the ones switched
/// back to most come back at once.
const MAX_WARM: usize = 3;
const CHECK_EVERY: Duration = Duration::from_secs(30);
/// How long a new chat's agent runs with nothing sent before it sleeps until the person writes.
const UNSENT_SLEEP: Duration = Duration::from_secs(5 * 60);
/// A terminal agent that dies this soon after it was started again is in a
/// crash loop, and the person decides what next.
const RESUME_WINDOW: Duration = Duration::from_secs(30);
/// The person's Ctrl-C and Ctrl-\, which a CLI quitting on them reports as
/// SIGINT and SIGQUIT, or exits with 128 plus their number.
const PERSON_SIGNALS: [u32; 2] = [2, 3];
const SIGNAL_EXIT_BASE: u32 = 128;

#[derive(Default)]
pub(crate) struct Lifecycle {
    /// Agents starting, so two callers starting one start it once.
    starting: Mutex<HashSet<String>>,
    /// An agent stopped starting, whether it started or failed.
    started: Notify,
    /// When each terminal agent was last started again after a crash.
    resumed: Mutex<HashMap<String, Instant>>,
}

/// Holds an agent's place among those starting until it is dropped.
struct Starting<'a> {
    lifecycle: &'a Lifecycle,
    agent_id: String,
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.lifecycle.starting.locked().remove(&self.agent_id);
        self.lifecycle.started.notify_waiters();
    }
}

impl Lifecycle {
    fn claim(&self, agent_id: &str) -> Option<Starting<'_>> {
        let mut starting = self.starting.locked();
        starting.insert(agent_id.to_owned()).then(|| Starting {
            lifecycle: self,
            agent_id: agent_id.to_owned(),
        })
    }
}

/// Starts the agent with `start` unless `running` finds it running already,
/// waiting out another caller starting it, and answers what runs.
pub(crate) async fn start_once<T, Started>(
    core: &Core,
    agent_id: &str,
    running: impl Fn() -> Option<T>,
    start: impl FnOnce() -> Started,
) -> CoreResult<T>
where
    Started: Future<Output = CoreResult<T>>,
{
    loop {
        if let Some(running) = running() {
            return Ok(running);
        }
        let started = core.lifecycle().started.notified();
        if let Some(_starting) = core.lifecycle().claim(agent_id) {
            return match running() {
                Some(running) => Ok(running),
                None => start().await,
            };
        }
        started.await;
    }
}

/// Starts the agent's chat as its record says unless it runs already or
/// another client is starting it, and answers once it runs.
pub(crate) async fn ensure_running(
    core: &Arc<Core>,
    agent_id: &str,
    waits_for_person: bool,
) -> CoreResult<()> {
    start_once(
        core,
        agent_id,
        || core.chats().get(agent_id).map(drop),
        || async {
            launching::start_agent(core, None, agent_id, waits_for_person)
                .await
                .map(drop)
        },
    )
    .await
}

/// Puts the agent's chat to sleep unless something in it is still running,
/// keeping how it ran so it wakes on its saved conversation. Answers whether
/// it is asleep now.
pub(crate) fn sleep(core: &Core, agent_id: &str) -> bool {
    let chat = core.chats().get(agent_id);
    if let Some(chat) = &chat {
        core.workspaces()
            .note_stopping(chat.agent_id(), chat.current_now());
    }
    let stopped = core.chats().stop_if_idle(agent_id);
    if let Some(chat) = chat.filter(|_| stopped) {
        core.chats().rest(&chat);
        super::chief::chat_rested(core, agent_id, chat.feed.id());
        core.workspaces().set_asleep(agent_id, true);
        core.note_agent_asleep(agent_id);
    }
    stopped
}

/// Starts the agent of a chat the person opened or began writing in, and puts it to sleep again
/// should it sit there with nothing sent and an empty draft, so an abandoned new chat holds no
/// process. Their next key starts it again.
pub(crate) fn began_writing(core: &Arc<Core>, agent_id: &str) {
    let starts = core
        .chats()
        .get(agent_id)
        .is_none_or(|chat| chat.unstarted());
    super::chat::composing(core, agent_id);
    if !starts {
        return;
    }
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(UNSENT_SLEEP).await;
            let new = core
                .workspaces()
                .agent(&agent_id)
                .is_some_and(|agent| agent.resume_id.is_none());
            if !new || core.chats().get(&agent_id).is_none() {
                return;
            }
            let drafted = (core.drafts().chat_draft(&agent_id))
                .is_some_and(|draft| !draft.text.trim().is_empty());
            if !drafted && sleep(&core, &agent_id) {
                return;
            }
        }
    });
}

/// Puts every chat to sleep at the person's word to stop everything, ending
/// its agent and any turn it was in, so each stays listed asleep on its saved
/// conversation, here and in the next core. Every agent is held asleep until
/// the person, or an agent they set going again, opens or writes to it, so mail
/// sent as things stop wakes none.
pub(crate) fn sleep_all(core: &Core) {
    core.workspaces().hold_all();
    for chat in core.chats().stop_all() {
        core.workspaces()
            .note_stopping(chat.agent_id(), chat.current_now());
        core.chats().rest(&chat);
        super::chief::chat_rested(core, chat.agent_id(), chat.feed.id());
        core.workspaces().set_asleep(chat.agent_id(), true);
    }
    super::part::Part::flush(core.workspaces());
}

/// Puts the agent to sleep at the person's word: a chat as [`sleep`] does, and
/// a terminal agent by ending its terminal, to start again on its saved
/// session when next shown.
pub(crate) fn sleep_agent(core: &Core, agent_id: &str) -> CoreResult<()> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    if core.chats().get(agent_id).is_some() {
        return match sleep(core, agent_id) {
            true => Ok(()),
            false => Err("the agent is at work; it can sleep once its turn ends".into()),
        };
    }
    let Some(terminal) = agent.pty_id.and_then(|pty| core.session(pty)) else {
        return Ok(());
    };
    if agent.resume_id.is_none() {
        return Err("this agent has no saved session to wake on".into());
    }
    if terminal.agent.as_ref().is_some_and(AgentActivity::busy) {
        return Err("the agent is at work; it can sleep once its turn ends".into());
    }
    session::kill(core, &terminal);
    core.workspaces().edit_agent(agent_id, |agent| {
        agent.pty_id = None;
        agent.asleep = true;
    })?;
    core.note_agent_asleep(agent_id);
    Ok(())
}

/// Whether a terminal agent's process ended the way a crash does, rather than
/// by the person quitting it, Sikemux stopping it, or a program that cannot run.
fn crashed(code: Option<u32>, signal: Option<&str>, killed: bool) -> bool {
    if killed {
        return false;
    }
    if let Some(signal) = signal {
        return !signal_number(signal).is_some_and(|number| PERSON_SIGNALS.contains(&number));
    }
    match code {
        None => true,
        Some(0 | 126 | 127) => false,
        Some(code) => {
            !(code > SIGNAL_EXIT_BASE && PERSON_SIGNALS.contains(&(code - SIGNAL_EXIT_BASE)))
        }
    }
}

/// macOS names a signal "Interrupt: 2", Linux "Interrupt".
fn signal_number(signal: &str) -> Option<u32> {
    let trailing = signal.rsplit([' ', ':']).next()?.trim();
    trailing
        .parse()
        .ok()
        .or(match signal.split(':').next()?.trim() {
            "Interrupt" => Some(2),
            "Quit" => Some(3),
            _ => None,
        })
}

/// Starts a terminal agent whose terminal crashed again on its saved session,
/// below the screen it left.
pub(crate) fn terminal_exited(
    core: &Core,
    pty: SessionId,
    agent_id: &str,
    code: Option<u32>,
    signal: Option<&str>,
    killed: bool,
) {
    let Some(agent) = core.workspaces().agent(agent_id) else {
        return;
    };
    let resumable = agent.pty_id == Some(pty) && agent.resume_id.is_some() && !agent.asleep;
    if !resumable || !crashed(code, signal, killed) {
        return;
    }
    let now = Instant::now();
    let mut resumed = core.lifecycle().resumed.locked();
    resumed.retain(|_, at| now.duration_since(*at) < RESUME_WINDOW);
    if resumed.insert(agent_id.to_owned(), now).is_some() {
        return;
    }
    drop(resumed);
    let (Some(core), agent_id) = (core.arc(), agent_id.to_owned()) else {
        return;
    };
    let label = crate::agent_catalog::agent(&agent.provider)
        .map_or(agent.provider.as_str(), |entry| entry.label);
    let continues = Continuation {
        session: pty,
        note: format!("— Resuming {label} —"),
    };
    tokio::spawn(async move {
        if let Err(error) = launching::start_terminal(
            &core,
            &agent_id,
            super::spare::opening_size(&core),
            Some(continues),
        )
        .await
        {
            eprintln!("sikemux core: {agent_id} did not start again: {error}");
        }
    });
}

/// Settles the terminal agents whose terminal did not outlive the last core:
/// one with a saved session starts again on it when shown, and one without
/// has nothing left to show.
fn settle_terminals(core: &Core) {
    let gone: Vec<_> = core
        .workspaces()
        .agent_ids()
        .into_iter()
        .filter_map(|agent_id| core.workspaces().agent(&agent_id))
        .filter(|agent| agent.pty_id.is_some_and(|pty| core.session(pty).is_none()))
        .collect();
    let mut closed = Vec::new();
    for agent in gone {
        if agent.resume_id.is_some() {
            let _ = core
                .workspaces()
                .edit_agent(&agent.id, |agent| agent.pty_id = None);
        } else {
            closed.push(agent.id);
        }
    }
    if !closed.is_empty() {
        super::workspace::close_gone(core, &closed);
    }
}

/// Puts every idle chat to sleep now, on screen or not, but those the person
/// keeps alive. Answers how many slept.
pub(crate) fn sleep_idle(core: &Core) -> usize {
    core.chats()
        .ids()
        .into_iter()
        .filter(|agent_id| kept(core, agent_id) && idle(core, agent_id))
        .filter(|agent_id| sleep(core, agent_id))
        .count()
}

/// Puts to sleep the chats that have sat idle off every screen long enough.
pub(crate) async fn serve(core: Arc<Core>) {
    settle_terminals(&core);
    let began = epoch_ms();
    let mut ticker = tokio::time::interval(CHECK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = epoch_ms();
        super::chat::cap::compact_idle(&core, now);
        super::chat::cap::tell_expired(&core, now);
        for agent_id in due(&core, began, now) {
            if !super::chat::cap::compact_before_sleep(&core, &agent_id, now) {
                sleep(&core, &agent_id);
            }
        }
    }
}

/// The chats to sleep now: every one idle for [`IDLE_SLEEP_MS`], and the
/// oldest beyond [`MAX_WARM`] of the rest no client has on screen.
fn due(core: &Core, began: u64, now: u64) -> Vec<String> {
    let mut stale = Vec::new();
    let mut off_screen: Vec<(u64, String)> = Vec::new();
    for agent_id in core.chats().ids() {
        if !may_sleep(core, &agent_id) {
            continue;
        }
        let quiet = core
            .seen()
            .quiet_since(&agent_id)
            .unwrap_or(began)
            .max(began);
        if now.saturating_sub(quiet) >= IDLE_SLEEP_MS {
            stale.push(agent_id);
        } else if !core.seen().viewed(&agent_id) {
            off_screen.push((quiet, agent_id));
        }
    }
    off_screen.sort();
    let beyond_warm = off_screen.len().saturating_sub(MAX_WARM);
    stale.extend(
        off_screen
            .into_iter()
            .take(beyond_warm)
            .map(|(_, agent_id)| agent_id),
    );
    stale
}

/// Whether the chat could sleep now: it is in the workspace, the person does
/// not keep it alive, it has a conversation to wake on, nothing is drafted in
/// it, and the person has read its last answer if they started it.
fn may_sleep(core: &Core, agent_id: &str) -> bool {
    let Some(agent) = core.workspaces().agent(agent_id) else {
        return false;
    };
    let drafted =
        (core.drafts().chat_draft(agent_id)).is_some_and(|draft| !draft.text.trim().is_empty());
    let unread_by_starter = agent.lead.is_none() && core.seen().unread(agent_id);
    !agent.keep_alive
        && agent.resume_id.is_some()
        && !drafted
        && !unread_by_starter
        && idle(core, agent_id)
        && !super::chat::cap::holds_awake(core, agent_id, epoch_ms())
}

fn kept(core: &Core, agent_id: &str) -> bool {
    core.workspaces()
        .agent(agent_id)
        .is_some_and(|agent| !agent.keep_alive)
}

/// A chat waking from sleep reads as idle while it starts, since it keeps the feed it slept with,
/// so only a ready one counts. One with a background task still running is not idle, as sleep
/// would end the task with the agent, and one whose agent cannot load its conversation again
/// would wake on a new one.
fn idle(core: &Core, agent_id: &str) -> bool {
    core.chats().get(agent_id).is_some_and(|chat| {
        chat.is_ready()
            && chat.reloadable.load(Ordering::Acquire)
            && !chat.offline_failed.load(Ordering::Acquire)
            && chat.state_now() == AgentState::Idle
            && chat.feed.running_tasks() == 0
    })
}

impl super::part::Part for Lifecycle {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(serve(core.clone()))]
    }
}

#[cfg(test)]
mod tests {
    use super::crashed;

    #[test]
    fn the_person_quitting_is_no_crash() {
        assert!(!crashed(Some(0), None, false));
        for signal in ["Interrupt: 2", "Interrupt", "Quit: 3"] {
            assert!(!crashed(Some(1), Some(signal), false), "{signal}");
        }
        assert!(!crashed(Some(130), None, false));
        assert!(!crashed(Some(131), None, false));
    }

    #[test]
    fn any_other_signal_or_failing_code_is_a_crash() {
        for signal in [
            "Killed: 9",
            "Segmentation fault: 11",
            "Abort trap: 6",
            "Terminated: 15",
            "Hangup",
        ] {
            assert!(crashed(Some(1), Some(signal), false), "{signal}");
        }
        for code in [1, 2, 101, 129, 137, 139, 143] {
            assert!(crashed(Some(code), None, false), "{code}");
        }
        assert!(crashed(None, None, false));
    }

    #[test]
    fn a_cli_the_shell_could_not_run_or_one_sikemux_stopped_is_no_crash() {
        assert!(!crashed(Some(126), None, false));
        assert!(!crashed(Some(127), None, false));
        assert!(!crashed(Some(1), Some("Hangup"), true));
        assert!(!crashed(Some(143), None, true));
    }
}
