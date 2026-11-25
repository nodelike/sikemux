use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use sikemux_calls::ship::HoldParams;
use sikemux_plugin_api::vault::as_caller;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    CallAsker, CallKind, CallOption, CallOutcome, CallPlace, CallResult, CallState, PersonCall,
};
use sikemux_view::inbox::ShipKind;

use super::gate::{explain, with_reason};
use super::{Allowed, Deploy, Hold, Permit, ToolCall, MAX_HOLDS_PER_AGENT};
use crate::git::Git;
use crate::server::changed::{ChangedCore, What};
use crate::server::person_calls;
use crate::server::places::Judged;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{Core, CoreResult};
use crate::ship::Ship;

/// Holds `ship` for the person, unless they allowed exactly it or a deploy to its place
/// already, and tells the agent why.
pub(super) fn hold(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: Ship,
    cwd: &Path,
    call: Option<ToolCall>,
    judged: Option<Judged>,
) -> Result<Option<Deploy>, String> {
    let then = match call {
        Some(_) => "Do not call it again: once they allow it, Sikemux runs it and sends you what it answered.",
        None => "You will hear when they allow it, and then run exactly this again.",
    };
    let why = judged.as_ref().and_then(|judged| judged.why.clone());
    let place = (judged.as_ref())
        .and_then(|judged| judged.place.as_ref())
        .map(|place| place.name().to_owned());
    let mut said = format!(
        "Held for the person: `{}` {}, so Sikemux asked them first: they answer it on its card in your chat in Sikemux, on the notch, or in your chat on their phone. {then} Go on with other work meanwhile.",
        ship.what,
        leaves(&ship)
    );
    if let Some(why) = &why {
        said = format!("{said} {why}");
    }
    let allowed_deploy = (place.as_ref())
        .filter(|_| ship.kind == ShipKind::Deploy)
        .map(|name| Deploy {
            head: Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok(),
            agent_id: agent_id.to_owned(),
            project: project.to_owned(),
            environment: name.clone(),
            call: None,
            telling: None,
        });
    let token = uuid::Uuid::new_v4().simple().to_string();
    {
        let mut state = core.ship().state.locked();
        let same =
            |agent: &str, held: &Ship, at: &Path| agent == agent_id && *held == ship && at == cwd;
        if let Some(at) = (state.allowed.iter()).position(|allowed| {
            let Permit::Exactly {
                ship: held,
                cwd: at,
            } = &allowed.permit;
            same(&allowed.agent_id, held, at)
        }) {
            let ran = state.allowed.remove(at).call;
            drop(state);
            if let Some(call) = ran {
                person_calls::set_outcome(core, &call, went(CallResult::Shipped));
            }
            return Ok(allowed_deploy);
        }
        let waiting = (state.holds.iter()).find(|hold| same(&hold.agent_id, &hold.ship, &hold.cwd));
        if let Some(waiting) = waiting {
            if waiting.because.is_none() {
                said = format!("{said} {}", explain(&waiting.token));
            }
            return Err(said);
        }
        let held = (state.holds.iter())
            .filter(|hold| hold.agent_id == agent_id)
            .count();
        if held >= MAX_HOLDS_PER_AGENT {
            if let Some(oldest) = (state.holds.iter()).position(|hold| hold.agent_id == agent_id) {
                state.holds.remove(oldest);
            }
        }
        state.holds.push(Hold {
            token: token.clone(),
            agent_id: agent_id.to_owned(),
            project: project.to_owned(),
            cwd: cwd.to_owned(),
            at: epoch_ms(),
            environment: place,
            why,
            because: None,
            deploy: call.as_ref().and(allowed_deploy),
            call,
            key: None,
            ship,
        });
    }
    core.changed(What::Agents);
    tell_calls(core);
    Err(format!("{said} {}", explain(&token)))
}

fn leaves(ship: &Ship) -> String {
    if let Some(place) = ship.writes_to() {
        return format!("writes to {place} as the person");
    }
    let words = match ship.kind {
        ShipKind::Push => "pushes commits off this machine",
        ShipKind::Merge => "merges a pull request",
        ShipKind::Deploy => "deploys",
        ShipKind::Publish => "publishes a release",
        ShipKind::ProductionWrite => "can change a production database",
    };
    words.to_owned()
}

/// The person allows the held action once: a tool call runs and its agent hears
/// what it answered; a command its agent is told it may run again, until the end
/// of the turn in which it hears this.
pub async fn allow(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    allow_hold(core, hold).await;
    Ok(())
}

async fn allow_hold(core: &Arc<Core>, mut hold: Hold) {
    core.ship().remember(&hold, true);
    if let Some(call) = hold.call.take() {
        let allowed = format!("The person allowed `{}` once", hold.ship.what);
        run_held(core, hold, call, allowed);
        return;
    }
    let text = format!(
        "The person allowed `{}` once. Run exactly that again now; it may run once, until this turn ends.",
        hold.ship.what
    );
    let permit = Permit::Exactly {
        ship: hold.ship,
        cwd: hold.cwd,
    };
    permitted(core, hold.agent_id, permit, text, Some(hold.token)).await;
}

/// Runs a held tool call the person let go, without making them wait on it, and
/// sends its agent what it answered.
fn run_held(core: &Arc<Core>, hold: Hold, call: ToolCall, allowed: String) {
    if let Err(error) = person_calls::started(core, &hold.token) {
        person_calls::set_outcome(core, &hold.token, went(CallResult::NotShipped));
        let text = format!("{allowed}, but {error}. Call it again to be asked again.");
        let core = core.clone();
        tokio::spawn(async move {
            (core.repo_above().messages_for_person_waiting)(&core, hold.agent_id, text).await
        });
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let ran = as_caller(
            call.caller,
            (core.repo_above().plugins_run_agent_tool)(
                &core,
                &call.name,
                call.arguments,
                hold.deploy,
            ),
        )
        .await;
        let result = match ran {
            Ok(_) => CallResult::Shipped,
            Err(_) => CallResult::Failed,
        };
        person_calls::set_outcome(&core, &hold.token, went(result));
        let text = match ran {
            Ok(Value::String(answer)) => {
                format!("{allowed}, and Sikemux ran it for you. It answered:\n{answer}")
            }
            Ok(answer) => {
                format!("{allowed}, and Sikemux ran it for you. It answered:\n{answer:#}")
            }
            Err(error) => format!("{allowed}, and Sikemux ran it for you, but it failed: {error}"),
        };
        let _ = (core.repo_above().messages_for_person)(&core, hold.agent_id, text).await;
    });
}

/// Keeps what the person allowed and tells the agent, whose hearing of it in a
/// turn starts the time it holds for.
async fn permitted(
    core: &Arc<Core>,
    agent_id: String,
    permit: Permit,
    text: String,
    call: Option<String>,
) {
    core.ship().state.locked().allowed.push(Allowed {
        agent_id: agent_id.clone(),
        permit,
        call,
        heard: false,
    });
    let told = (core.repo_above().messages_for_person)(core, agent_id.clone(), text).await;
    let heard = told.is_ok_and(|told| {
        matches!(
            told.get("delivery").and_then(Value::as_str),
            Some("steered" | "newTurn")
        )
    });
    if heard {
        core.ship().turn_started(&agent_id);
    }
}

/// The person refuses the held action, and the agent is told not to take it.
pub async fn refuse(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    core.ship().remember(&hold, false);
    let text = format!("The person did not allow `{}`.", hold.ship.what);
    let _ = (core.repo_above().messages_for_person)(core, hold.agent_id, text).await;
    Ok(())
}

fn take(core: &Core, id: &str) -> CoreResult<Hold> {
    let hold = {
        let mut state = core.ship().state.locked();
        let at = (state.holds.iter())
            .position(|hold| hold.token == id)
            .ok_or("that action no longer waits on you")?;
        state.holds.remove(at)
    };
    core.changed(What::Agents);
    tell_calls(core);
    Ok(hold)
}

/// Puts each held action to the person as a production call, and closes the calls of those no
/// longer held.
pub(super) fn tell_calls(core: &Core) {
    let wanted = {
        let state = core.ship().state.locked();
        (state.holds.iter())
            .filter(|hold| hold.key.is_none())
            .map(|hold| (hold.token.clone(), held_call(core, hold)))
            .collect()
    };
    person_calls::sync_holds(core, wanted);
}

/// What came of a held action the person allowed: it ran, and how it ended where Sikemux saw.
fn went(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

/// A place's id made from its name: lowercase letters and digits, words joined by `-`.
fn slug(name: &str) -> String {
    let lower = name.to_lowercase();
    let words = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty());
    words.collect::<Vec<_>>().join("-")
}

fn held_call(core: &Core, hold: &Hold) -> PersonCall {
    let name = core.names().name_or_id(&hold.agent_id);
    let question = match (hold.ship.kind, hold.ship.writes_to(), &hold.environment) {
        (_, Some(place), _) => format!("Let @{name} write to {place} as you?"),
        (ShipKind::Push, ..) => format!("Let @{name} push its commits?"),
        (ShipKind::Merge, ..) => format!("Let @{name} merge its pull request?"),
        (ShipKind::Deploy, _, Some(environment)) => format!("Let @{name} deploy to {environment}?"),
        (ShipKind::Deploy, ..) => format!("Let @{name} deploy?"),
        (ShipKind::Publish, ..) => format!("Let @{name} publish a release?"),
        (ShipKind::ProductionWrite, ..) => {
            format!("Let @{name} change the production database?")
        }
    };
    let question = match &hold.because {
        Some(reason) => with_reason(&question, reason),
        None => question,
    };
    let option = |id: &str, label: String, detail: Option<String>| CallOption {
        id: id.to_owned(),
        label,
        detail,
        picture: None,
    };
    let options = vec![
        option(person_calls::HOLD_ALLOW, "Allow it once".to_owned(), None),
        option(person_calls::HOLD_REFUSE, "Not now".to_owned(), None),
    ];
    PersonCall {
        id: hold.token.clone(),
        project: hold.project.clone(),
        kind: CallKind::Production,
        title: None,
        asker: CallAsker {
            agent_id: hold.agent_id.clone(),
            cli: (core.repo_above().questions_cli_of)(core, &hold.agent_id),
            name,
            chief: false,
        },
        joined: Vec::new(),
        about: None,
        page: None,
        more_pages: Vec::new(),
        shown_in: Vec::new(),
        link: None,
        question,
        headline: String::new(),
        context: hold.why.clone(),
        assumption: None,
        options,
        pick: None,
        against: None,
        evidence: Vec::new(),
        opened: Vec::new(),
        action: Some(hold.ship.what.clone()),
        brief: None,
        place: (hold.environment.clone()).map(|name| CallPlace {
            id: slug(&name),
            name,
            repository: None,
        }),
        can_hand_back: false,
        multi: false,
        secret: false,
        raised_at: hold.at,
        state: CallState::Open,
        answer: None,
        outcome: None,
    }
}
