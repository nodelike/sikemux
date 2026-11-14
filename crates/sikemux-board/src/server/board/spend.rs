//! Tells the person, as a money call already settled, each time a chat's reported spend passes
//! another step of its project's spend amount. It never stops an agent. A chat first seen past a
//! step, as after a restart, starts counting from there.

use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAsker, CallKind, CallOutcome, CallResult};

use crate::server::person_calls::{self, Raise};
use crate::server::Core;

/// The step the person hears at when they set none for the project, in dollars.
const STEP_USD: u32 = 50;

/// Tells of each chat that passed another step since it was last looked at.
pub fn settle(core: &Core) {
    let mut passed = Vec::new();
    {
        let state = core.boards().state.locked();
        let mut told = core.boards().spend_told.locked();
        let mut seen = HashMap::new();
        for chat in core.chats().all() {
            let agent = chat.agent_id().to_owned();
            let Some(cost) = chat.feed.cost_usd() else {
                continue;
            };
            let Some(project) = core.names().get(&agent).map(|named| named.project) else {
                continue;
            };
            let step = (state.boards.get(&project))
                .and_then(|board| board.settings.spend_ceiling)
                .unwrap_or(STEP_USD)
                .max(1);
            let steps = (cost / f64::from(step)) as u32;
            if told.get(&agent).is_some_and(|before| steps > *before) {
                passed.push((agent.clone(), project, cost));
            }
            seen.insert(agent, steps);
        }
        *told = seen;
    }
    for (agent, project, cost) in passed {
        let raise = Raise {
            project,
            kind: CallKind::Money,
            asker: CallAsker {
                name: core.names().name_or_id(&agent),
                cli: core.agent_now(&agent).cli().unwrap_or_else(|| "agent".to_owned()),
                agent_id: agent,
                chief: false,
            },
            question: format!("This chat has spent ${cost:.0} so far"),
            context: Some("Nothing was stopped; this only tells you.".to_owned()),
            options: Vec::new(),
            pick: None,
            against: None,
            evidence: Vec::new(),
            action: None,
            brief: None,
            place: None,
            key: None,
            yes: None,
        };
        let outcome = CallOutcome {
            result: CallResult::Heard,
            healthy: None,
            roll_back: false,
            why: None,
            at: epoch_ms(),
        };
        person_calls::told(core, raise, outcome);
    }
}
