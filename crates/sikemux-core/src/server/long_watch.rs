//! Tells whoever hears about an agent, once per watch, that it has waited over two hours on a
//! repeating check it set, which has no end of its own.

use std::collections::HashMap;
use std::sync::Arc;

use sikemux_view::turn::{TurnPhase, TurnState, TurnWaitKind};

use super::Core;

const LONG_MS: u64 = 2 * 60 * 60 * 1000;

pub(crate) struct Long {
    agent: String,
    to: String,
    text: String,
}

/// The agents whose watch has gone on long now, and when the next one falls due. `told` keeps,
/// per agent, the start of the watch it was last told about.
pub(crate) fn read(
    core: &Core,
    turns: &HashMap<String, TurnState>,
    told: &mut HashMap<String, u64>,
    now: u64,
) -> (Vec<Long>, Option<u64>) {
    told.retain(|agent, _| turns.contains_key(agent));
    let mut long = Vec::new();
    let mut next: Option<u64> = None;
    for (agent_id, turn) in turns {
        let Some(wait) = (turn.wait.as_ref())
            .filter(|wait| turn.state == TurnPhase::Waiting && wait.kind == TurnWaitKind::Watch)
        else {
            continue;
        };
        if told.get(agent_id) == Some(&wait.started_at) {
            continue;
        }
        let due = wait.started_at + LONG_MS;
        if now < due {
            next = Some(next.map_or(due, |next| next.min(due)));
            continue;
        }
        let Some(to) = super::roster::hears_of(core, agent_id)
            .agent()
            .map(str::to_owned)
        else {
            continue;
        };
        told.insert(agent_id.clone(), wait.started_at);
        long.push(Long {
            agent: agent_id.clone(),
            to,
            text: format!(
                "It has watched for \"{}\" for {} hours. If that is not coming, tell it to remove the trigger and what to do instead.",
                wait.what,
                (now - wait.started_at) / (60 * 60 * 1000)
            ),
        });
    }
    (long, next)
}

pub(crate) fn tell(core: &Arc<Core>, long: Vec<Long>) {
    for long in long {
        super::messages::report_from_sikemux(
            core,
            long.agent,
            long.to,
            long.text,
            "has waited long on a repeating check it set, and this is a note from Sikemux, sent once per watch.",
        );
    }
}
