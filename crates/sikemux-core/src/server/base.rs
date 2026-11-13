//! What the core gives the modules it stands on to call: each thing they ask of the features
//! built over them, as those features do it.

use sikemux_wire::protocol::{AgentState, Event, SessionId};

use super::above::Above;
use super::access::{Needs, Program};
use super::activity::{Activity, ActivityCore};
use super::device_view::DeviceViewCore;
use super::notify::Signal;
use super::parts::Parts;
use super::session::Session;
use super::spawn::{self, Ending};
use super::{calls, harness, hosts, lifecycle, messages, requests, ssh, tasks, upgrade, Core};

pub(super) fn above() -> Above {
    Above {
        terminal_colors: |core| core.look().terminal_colors(),
        ports_changed: |core| core.ports().changed(),
        ssh_moved: ssh::moved,
        ssh_connected: |core, alias| hosts::probe::ask(core, alias, hosts::probe::Why::Connected),
        task_ended,
        agent_exited,
        agent_moved,
        view_due: |core| core.device_view().due.notify_one(),
        agents_moved,
        view_moved,
        told: |core, event| {
            if let Some(signal) = signal_of(event) {
                core.notify().send(signal);
            }
        },
        frozen_request: |core, peer, payload, writer| {
            Box::pin(upgrade::answer(core, peer, payload, writer))
        },
        send_device_view: |core, client| core.send_device_view(client),
        run_requests: |core, client, queue| Box::pin(requests::run(core, client, queue)),
        admit: requests::admit,
        write_to_program: requests::write_to_program,
    }
}

pub(super) fn access_of(method: &str) -> (Needs, Program) {
    let row = calls::row(method);
    (row.needs, row.program)
}

fn task_ended(core: &Core, id: SessionId, code: Option<u32>, signal: Option<String>) {
    if let Some(failed) = core.harness().session_exited(id, code, signal) {
        harness::tell_agent_run_failed(core, failed);
    }
    tasks::ended(core, id);
}

fn agent_exited(
    core: &Core,
    session: &Session,
    code: Option<u32>,
    signal: Option<&str>,
    mid_turn: bool,
) {
    let Some(agent) = session.agent.as_ref() else {
        return;
    };
    lifecycle::terminal_exited(
        core,
        session.id,
        agent.agent_id(),
        code,
        signal,
        session.is_killed(),
    );
    let failed = match (code, signal) {
        (_, Some(signal)) => Some(format!("It stopped from signal {signal}.")),
        (Some(code), None) if code != 0 => Some(format!("It exited with code {code}.")),
        _ => None,
    }
    .filter(|_| !session.is_killed());
    if let Some(reason) = &failed {
        core.note_activity(agent.agent_id(), Activity::Problem(reason));
    }
    core.note_agent_gone(agent.agent_id());
    if mid_turn || !session.is_killed() {
        spawn::terminal_turn_ended(
            core,
            agent.agent_id(),
            Ending::Gone {
                reason: failed,
                mid_turn,
                asked: session.is_killed(),
            },
        );
    }
}

fn agent_moved(
    core: &Core,
    agent_id: &str,
    next: AgentState,
    busy: bool,
    typed: bool,
    last_output_ms: u64,
) {
    let activity = match next {
        AgentState::Working if busy => Some(Activity::Working),
        AgentState::Working => Some(Activity::Started),
        AgentState::Blocked => Some(Activity::NeedsYou),
        AgentState::Idle if busy => Some(Activity::Finished { cancelled: false }),
        _ => None,
    };
    if let Some(activity) = activity {
        core.note_activity(agent_id, activity);
    }
    match next {
        AgentState::Working if typed => messages::prompt_taken(core, agent_id),
        AgentState::Blocked => messages::prompt_taken(core, agent_id),
        AgentState::Idle | AgentState::Stopped if busy => {
            messages::prompt_settled(core, agent_id, last_output_ms)
        }
        _ => {}
    }
    if next == AgentState::Idle && busy {
        spawn::terminal_turn_ended(core, agent_id, Ending::Finished);
    }
}

fn agents_moved(core: &Core) {
    core.harness().mailbox.wake.now();
    core.harness().questions.wake.now();
    core.boards().wake.now();
    core.updates().chats_changed();
}

fn view_moved(core: &Core) {
    core.device_view().stale.notify_one();
    core.hosts().copies.wake.now();
    hosts::route::rows_moved(core);
    core.notify().inbox_moved();
}

fn signal_of(event: &Event) -> Option<Signal> {
    match event {
        Event::Attention { attention } => Some(Signal::Attention(Box::new(attention.clone()))),
        Event::AttentionCleared { id, agent_id } => Some(Signal::Cleared {
            id: id.clone(),
            agent_id: agent_id.clone(),
        }),
        Event::AgentState(state) => Some(Signal::AgentState {
            agent_id: state.agent_id.clone(),
            state: state.state,
        }),
        _ => None,
    }
}
