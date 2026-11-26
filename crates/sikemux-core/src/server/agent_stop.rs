//! An agent stopping the turn of a worker it started, or its wait for a usage limit to reset, as
//! the person's Stop does. The worker then waits until a message may start it again: one from the
//! agent that stopped it, from a lead above that one, or from the person. Other mail goes in with
//! that message.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use tokio::time::{sleep, timeout, Instant};

use crate::cli::protocol::HarnessRequest;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::repo_state::RepoParts;
use crate::server::serving::ServeCore;
use crate::server::workspace_state::WorkspaceParts;

use super::agent_now::AgentNowCore;
use super::chat::Chat;
use super::connection::blocking;
use super::roster::{self, Listed, Reach};
use super::session::{self, Session};
use super::{hosts, Core};

/// Longer than a driver gets to end a turn before its agent is started again instead.
const STOPS_WITHIN: Duration = Duration::from_secs(14);

const ESCAPE: &[u8] = b"\x1b";

const ESCAPES_AGAIN_AFTER: Duration = Duration::from_secs(3);

const TERMINAL_LOOK: Duration = Duration::from_millis(200);

/// A line typed this long before a terminal's turn began is what began it.
const TYPED_BEFORE_MS: u64 = 5_000;

/// Stops the turns of agents the caller started, and answers once each has stopped, or says it
/// is still stopping. Each agent it may not stop is refused with its reason.
pub(crate) async fn stop(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let caller = request
        .agent_id
        .clone()
        .ok_or("agent_stop needs the calling agent's id")?;
    let wanted = roster::wanted(request, "stop")?;
    let (here, request, caller_id) = (core.clone(), request.clone(), caller.clone());
    let targets: Vec<(String, Result<Listed, String>)> = blocking(move || {
        let project = super::caller::project_of(&here, &request);
        let agents = roster::listed(&here);
        let target = |to: &String| {
            let target = roster::resolve(&here, &agents, to, &project, &caller_id)?;
            may_stop(&here, &target, &caller_id).map(|()| target)
        };
        Ok(wanted.iter().map(|to| (to.clone(), target(to))).collect())
    })
    .await
    .map_err(|error| error.to_string())?;
    let caller = &caller;
    let ends = join_all(targets.into_iter().map(|(to, target)| async move {
        let end = match target {
            Ok(target) if target.reach == Reach::Remote => {
                hosts::route::halt(core, caller, &target.agent_id)
                    .await
                    .map(|(name, over)| (super::names::at_name(&name), over))
            }
            Ok(target) => stop_here(core, &target.agent_id, caller).await.map(|over| {
                let shown = target
                    .name
                    .as_deref()
                    .map(super::names::at_name)
                    .unwrap_or(target.agent_id);
                (shown, over)
            }),
            Err(why) => Err(why),
        };
        (to, end)
    }))
    .await;
    let (mut stopped, mut stopping, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    for (to, end) in ends {
        match end {
            Ok((name, true)) => stopped.push(name),
            Ok((name, false)) => stopping.push(name),
            Err(why) => refused.push(json!({ "agent": super::names::at_name(&to), "reason": why })),
        }
    }
    let mut result = serde_json::Map::new();
    result.insert("stopped".to_owned(), stopped.into());
    if !stopping.is_empty() {
        result.insert("stillStopping".to_owned(), stopping.into());
    }
    if !refused.is_empty() {
        result.insert("refused".to_owned(), refused.into());
    }
    Ok(Value::Object(result))
}

fn may_stop(core: &Core, target: &Listed, caller: &str) -> Result<(), String> {
    if target.agent_id == caller {
        return Err("that is you; end your turn instead".into());
    }
    if target.reach == Reach::Remote || roster::started_by(core, &target.agent_id, caller) {
        return Ok(());
    }
    let name = core.names().name_or_id(&target.agent_id);
    Err(format!("@{name} is not one you started"))
}

/// Stops the turn of an agent here for `by`, which leads it, and drops the messages waiting for
/// it. Answers whether the turn is over.
pub(crate) async fn stop_here(core: &Arc<Core>, agent_id: &str, by: &str) -> Result<bool, String> {
    let name = core.names().name_or_id(agent_id);
    if core.harness().mailbox.person_waiting(agent_id) {
        return Err(format!("the person is talking to @{name}"));
    }
    let dropped = super::messages::drop_waiting(core, agent_id);
    match stop_turn(core, agent_id, &name, by).await {
        Err(_) if dropped > 0 && !core.agent_now(agent_id).mid_turn() => Ok(true),
        stopped => stopped,
    }
}

async fn stop_turn(core: &Arc<Core>, agent_id: &str, name: &str, by: &str) -> Result<bool, String> {
    let in_turn = (core.chats().get(agent_id)).is_some_and(|chat| chat.working());
    let waits = !in_turn && core.chats().resets.waits(agent_id);
    let persons = || {
        (core.chats().heard(agent_id))
            .is_some_and(|feed| feed.has_queued() || !feed.person_said().is_empty())
    };
    if (in_turn || waits) && persons() {
        return Err(format!("the person is talking to @{name}"));
    }
    if waits {
        core.inbox().stop_by(agent_id, by);
        super::chat::reset::stop(core, agent_id);
        return Ok(true);
    }
    if let Some(chat) = core.chats().get(agent_id) {
        return stop_chat(core, &chat, name, by).await;
    }
    match core.agent_session(agent_id) {
        Some(terminal) => stop_terminal(core, agent_id, &terminal, name, by).await,
        None => Err(format!("@{name} has no turn to stop")),
    }
}

async fn stop_chat(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    name: &str,
    by: &str,
) -> Result<bool, String> {
    let agent_id = chat.agent_id();
    if !chat.working() {
        return Err(format!("@{name} has no turn to stop"));
    }
    core.inbox().stop_by(agent_id, by);
    if let Err(error) = super::chat::stop_turn(chat) {
        core.inbox().may_start(agent_id);
        return Err(format!("@{name} could not be stopped: {error}"));
    }
    let _ = timeout(STOPS_WITHIN, chat.settled()).await;
    if core.agent_now(agent_id).mid_turn() {
        return Ok(false);
    }
    if core.inbox().stopped_by(agent_id).is_none() {
        return Err(format!("@{name} had just ended its turn itself"));
    }
    Ok(!core
        .chats()
        .get(agent_id)
        .is_some_and(|chat| chat.working()))
}

async fn stop_terminal(
    core: &Arc<Core>,
    agent_id: &str,
    terminal: &Arc<Session>,
    name: &str,
    by: &str,
) -> Result<bool, String> {
    let readable = (terminal.agent.as_ref()).is_some_and(|agent| agent.state().is_some());
    if !readable {
        return Err(format!(
            "Sikemux cannot read @{name}'s terminal, so it cannot stop it"
        ));
    }
    if !core.agent_now(agent_id).mid_turn() {
        return Err(format!("@{name} has no turn to stop"));
    }
    let began = core.seen().started_at(agent_id).unwrap_or_else(epoch_ms);
    let typed = core.ship().spoke_at(agent_id).unwrap_or_default();
    if typed + TYPED_BEFORE_MS >= began {
        return Err(format!("the person is talking to @{name}"));
    }
    core.inbox().stop_by(agent_id, by);
    let asked = Instant::now();
    let mut again = Some(ESCAPES_AGAIN_AFTER);
    escape(terminal);
    while core.agent_now(agent_id).mid_turn() {
        if asked.elapsed() > STOPS_WITHIN {
            core.inbox().may_start(agent_id);
            return Err(format!("@{name}'s terminal did not stop on Escape"));
        }
        if again.is_some_and(|after| asked.elapsed() > after) {
            again = None;
            escape(terminal);
        }
        sleep(TERMINAL_LOOK).await;
    }
    Ok(true)
}

fn escape(terminal: &Session) {
    session::queue_input(
        terminal,
        ESCAPE.to_vec(),
        session::Typist::Other,
        Box::new(drop),
    );
}

/// A turn of the agent began while `stopped_by` had it stopped. One nobody prompted, such as when
/// a background task of its ended, is stopped too.
pub(crate) fn began_unasked(core: &Core, agent_id: &str, stopped_by: Option<String>) {
    let Some(by) = stopped_by else { return };
    let Some(chat) = core.chats().get(agent_id).filter(|chat| chat.unprompted()) else {
        return;
    };
    core.inbox().stop_by(agent_id, &by);
    let _ = super::chat::stop_turn(&chat);
}

/// Whether a message from `from` starts the agent again once an agent stopped it.
pub(crate) fn may_start(core: &Core, from: &str, agent_id: &str) -> bool {
    let Some(by) = core.inbox().stopped_by(agent_id) else {
        return false;
    };
    let names = core.names();
    let above = |id: &str| names.leads_of(id).iter().any(|lead| lead == from);
    from == by || above(&by) || !core.agent_now(&by).open() && above(agent_id)
}
