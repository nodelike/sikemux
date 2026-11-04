//! Actions agents take that leave this machine, held for the person. An agent
//! pushing, merging, deploying, publishing or writing to production is stopped
//! there, and the person is asked in the inbox. Allowing one lets exactly that
//! action run once, until the end of the turn in which the agent hears of it.
//! Where the person lets agents land work, pushes and merges go through.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::inbox::{ShipHold, ShipKind};

use super::board::{self, auto::Landing};
use super::{messages, plugins, Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::ship::{Ship, ToolShip};

/// More than an agent waits on at once; past this its oldest hold goes.
const MAX_HOLDS_PER_AGENT: usize = 16;
/// The person's answers remembered, across every agent, while the core runs.
const MAX_ANSWERED: usize = 256;
const DATABASE: &str = "sikemux.database";

#[derive(Default)]
pub(crate) struct Ships {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next: u64,
    holds: Vec<Hold>,
    allowed: Vec<Allowed>,
    answered: VecDeque<Answered>,
}

/// A held action the person answered.
#[derive(Clone)]
pub(crate) struct Answered {
    pub agent_id: String,
    pub what: String,
    pub allowed: bool,
    pub at: u64,
}

/// An agent's action waiting on the person.
struct Hold {
    id: u64,
    agent_id: String,
    project: String,
    ship: Ship,
    cwd: PathBuf,
    at: u64,
}

/// An action the person allowed the agent to take once.
struct Allowed {
    agent_id: String,
    ship: Ship,
    cwd: PathBuf,
    /// The agent has heard it may, so the approval ends with its turn.
    heard: bool,
}

/// What waits on the person, as the inbox lists it.
pub(crate) struct Waiting {
    pub hold: ShipHold,
    pub agent_id: String,
    pub project: String,
    pub at: u64,
}

impl Ships {
    /// The actions waiting on the person, oldest first.
    pub(crate) fn waiting(&self) -> Vec<Waiting> {
        (self.state.locked().holds.iter())
            .map(|hold| Waiting {
                hold: ShipHold {
                    id: hold.id.to_string(),
                    kind: hold.ship.kind,
                    what: hold.ship.what.clone(),
                },
                agent_id: hold.agent_id.clone(),
                project: hold.project.clone(),
                at: hold.at,
            })
            .collect()
    }

    /// What the person answered of the actions held since the core started, oldest first.
    pub(crate) fn answered(&self) -> Vec<Answered> {
        self.state.locked().answered.iter().cloned().collect()
    }

    fn remember(&self, hold: &Hold, allowed: bool) {
        let mut state = self.state.locked();
        if state.answered.len() >= MAX_ANSWERED {
            state.answered.pop_front();
        }
        state.answered.push_back(Answered {
            agent_id: hold.agent_id.clone(),
            what: hold.ship.what.clone(),
            allowed,
            at: epoch_ms(),
        });
    }

    /// The agent began a turn, in which what it heard it may do holds.
    pub(crate) fn turn_started(&self, agent_id: &str) {
        for allowed in
            (self.state.locked().allowed.iter_mut()).filter(|allowed| allowed.agent_id == agent_id)
        {
            allowed.heard = true;
        }
    }

    /// The agent's turn ended, and with it what it was allowed in it.
    pub(crate) fn turn_ended(&self, agent_id: &str) {
        (self.state.locked().allowed)
            .retain(|allowed| allowed.agent_id != agent_id || !allowed.heard);
    }
}

/// Whether the agent `agent_id` may take `ship` from `cwd` now: once the person
/// allowed exactly that, or where they let agents land work, for a push or a
/// merge. Otherwise it is held, and the agent is told why.
pub(crate) fn check(core: &Core, agent_id: &str, ship: Ship, cwd: &Path) -> Result<(), String> {
    let project = core.workspaces.project_of(cwd);
    let lands = matches!(ship.kind, ShipKind::Push | ShipKind::Merge)
        && board::landing(core, &project) == Landing::Core;
    if lands {
        return Ok(());
    }
    let said = format!(
        "Held for the person: `{}` {}, so Sikemux asked them first. Go on with other work; you will hear when they allow it, and then run exactly this again.",
        ship.what,
        leaves(ship.kind)
    );
    {
        let mut state = core.ship.state.locked();
        let same =
            |agent: &str, held: &Ship, at: &Path| agent == agent_id && *held == ship && at == cwd;
        if let Some(at) = (state.allowed.iter())
            .position(|allowed| same(&allowed.agent_id, &allowed.ship, &allowed.cwd))
        {
            state.allowed.remove(at);
            return Ok(());
        }
        if state
            .holds
            .iter()
            .any(|hold| same(&hold.agent_id, &hold.ship, &hold.cwd))
        {
            return Err(said);
        }
        let held = state
            .holds
            .iter()
            .filter(|hold| hold.agent_id == agent_id)
            .count();
        if held >= MAX_HOLDS_PER_AGENT {
            if let Some(oldest) = state
                .holds
                .iter()
                .position(|hold| hold.agent_id == agent_id)
            {
                state.holds.remove(oldest);
            }
        }
        state.next += 1;
        let id = state.next;
        state.holds.push(Hold {
            id,
            agent_id: agent_id.to_owned(),
            project,
            ship,
            cwd: cwd.to_owned(),
            at: epoch_ms(),
        });
    }
    core.agents_changed();
    Err(said)
}

fn leaves(kind: ShipKind) -> &'static str {
    match kind {
        ShipKind::Push => "pushes commits off this machine",
        ShipKind::Merge => "merges a pull request",
        ShipKind::Deploy => "deploys",
        ShipKind::Publish => "publishes a release",
        ShipKind::ProductionWrite => "can change a production database",
    }
}

/// What the shim and the hook ask before an action leaving this machine runs:
/// whether it may, and if not, what the agent is told. A program no agent of
/// this core runs, as the person's own shell does, is never held.
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let Some(agent_id) = request.agent_id.clone() else {
        return Ok(json!({ "allowed": true }));
    };
    let kind: ShipKind = serde_json::from_value(request.params["kind"].clone())
        .map_err(|_| "kind must be what the action does")?;
    let what = (request.params["what"].as_str())
        .ok_or("what must be the action's command")?
        .to_owned();
    let cwd = PathBuf::from(&request.project);
    Ok(match check(core, &agent_id, Ship { kind, what }, &cwd) {
        Ok(()) => json!({ "allowed": true }),
        Err(said) => json!({ "allowed": false, "said": said }),
    })
}

/// Whether an agent's call to the Sikemux tool `name` may run now, as
/// [`check`] says for what it does.
pub(crate) async fn check_tool(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    arguments: &Value,
) -> Result<(), String> {
    let (Some(agent_id), Some(when)) = (&request.agent_id, crate::ship::of_tool(name)) else {
        return Ok(());
    };
    let kind = match when {
        ToolShip::Always(kind) => kind,
        ToolShip::ProductionDatabase => {
            let asked = plugins::call(core, DATABASE, "agentWritesProduction", arguments.clone())
                .await
                .map_err(|failure| plugins::explain(&failure))?;
            if asked != true {
                return Ok(());
            }
            ShipKind::ProductionWrite
        }
    };
    let what = format!("{name} {arguments}");
    check(
        core,
        agent_id,
        Ship { kind, what },
        Path::new(&request.project),
    )
}

#[derive(Deserialize, Serialize)]
pub struct HoldParams {
    pub hold: String,
}

/// The person allows the held action once: the agent is told it may run it
/// again, which then runs until the end of the turn in which it hears this.
pub(crate) async fn allow(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    core.ship.remember(&hold, true);
    let text = format!(
        "The person allowed `{}` once. Run exactly that again now; it may run once, until this turn ends.",
        hold.ship.what
    );
    core.ship.state.locked().allowed.push(Allowed {
        agent_id: hold.agent_id.clone(),
        ship: hold.ship,
        cwd: hold.cwd,
        heard: false,
    });
    let told = messages::for_person(core, hold.agent_id.clone(), text).await;
    let heard =
        told.is_ok_and(|told| matches!(told["delivery"].as_str(), Some("steered" | "newTurn")));
    if heard {
        core.ship.turn_started(&hold.agent_id);
    }
    Ok(())
}

/// The person refuses the held action, and the agent is told not to take it.
pub(crate) async fn refuse(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    core.ship.remember(&hold, false);
    let text = format!(
        "The person did not allow `{}`. Do not run it; if your work needed it, say so in your answer.",
        hold.ship.what
    );
    let _ = messages::for_person(core, hold.agent_id, text).await;
    Ok(())
}

fn take(core: &Core, id: &str) -> CoreResult<Hold> {
    let hold = {
        let mut state = core.ship.state.locked();
        let at = (state.holds.iter())
            .position(|hold| hold.id.to_string() == id)
            .ok_or("that action no longer waits on you")?;
        state.holds.remove(at)
    };
    core.agents_changed();
    Ok(hold)
}
