//! Actions agents take that leave this machine, held for the person. An agent
//! pushing, merging, deploying, publishing or writing to production is stopped
//! there, and the person is asked in the inbox. Allowing one lets exactly that
//! action run once, until the end of the turn in which the agent hears of it.
//! Where the person lets agents land work, pushes and merges go through. A
//! deploy to an environment the project marked safe goes without asking, and
//! one that goes is followed to its end and kept as evidence on the slice.

use crate::server::changed::What;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::{ShipHold, ShipKind};
use sikemux_view::slice::Verdict;

use super::board::{self, auto::Landing, environments::Goes, DeployRan};
use super::plugins::Provided;
use super::repo_events::RepoEvent;
use super::{messages, plugins, Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::git::Git;
use crate::ship::{Ship, Target, ToolShip};

/// More than an agent waits on at once; past this its oldest hold goes.
const MAX_HOLDS_PER_AGENT: usize = 16;
/// The person's answers remembered, across every agent, while the core runs.
const MAX_ANSWERED: usize = 256;
const DATABASE: &str = "sikemux.database";
const RUNDECK: &str = "sikemux.rundeck";
const VERCEL: &str = "sikemux.vercel";
/// Deploys let go whose end the core is still to hear of.
const MAX_GOING: usize = 64;
/// How often a long deploy is looked at, and how long a look can take, before
/// the core stops following it and says it did not see the end.
const FOLLOW_LOOKS: usize = 60;
const FOLLOW_EVERY: Duration = Duration::from_secs(15);
const HEALTH_TRIES: usize = 6;
const HEALTH_EVERY: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
/// The end of what a deploy printed that its evidence keeps.
const MAX_OUTPUT_CHARS: usize = 4000;

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
    going: VecDeque<(u64, Deploy)>,
}

/// A deploy let go toward one of the project's environments.
#[derive(Clone)]
pub(crate) struct Deploy {
    agent_id: String,
    project: String,
    environment: String,
    what: String,
    /// Why it went or how it was let go, as its evidence opens.
    why: String,
    by_person: bool,
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
    /// The environment it lands in and why it waits, in a project with environments.
    environment: Option<String>,
    why: Option<String>,
}

/// An action the person allowed the agent to take once.
struct Allowed {
    agent_id: String,
    permit: Permit,
    /// The agent has heard it may, so the approval ends with its turn.
    heard: bool,
}

/// What the person allowed.
enum Permit {
    /// Exactly this, from there.
    Exactly { ship: Ship, cwd: PathBuf },
    /// One deploy to this environment of the project, by passing its gate.
    Environment { project: String, name: String },
}

/// What waits on the person, as the inbox lists it.
pub(crate) struct Waiting {
    pub hold: ShipHold,
    pub agent_id: String,
    pub project: String,
    pub at: u64,
}

impl Ships {
    /// Whether an action of the agent's waits on the person.
    pub(crate) fn holds(&self, agent_id: &str) -> bool {
        (self.state.locked().holds.iter()).any(|hold| hold.agent_id == agent_id)
    }

    /// The actions waiting on the person, oldest first.
    pub(crate) fn waiting(&self) -> Vec<Waiting> {
        (self.state.locked().holds.iter())
            .map(|hold| Waiting {
                hold: ShipHold {
                    id: hold.id.to_string(),
                    kind: hold.ship.kind,
                    what: hold.ship.what.clone(),
                    environment: hold.environment.clone(),
                    why: hold.why.clone(),
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

/// Whether the agent `agent_id` may take `ship` from `cwd` now: as the
/// project's environments say for a deploy; once the person allowed exactly
/// that; or where they let agents land work, for a push or a merge. Otherwise
/// it is held, and the agent is told why. What goes toward an environment comes
/// back to be followed to its end.
pub(crate) fn check(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
) -> Result<Option<Deploy>, String> {
    let project = core.workspaces.project_of(cwd);
    let lands = matches!(ship.kind, ShipKind::Push | ShipKind::Merge)
        && board::landing(core, &project) == Landing::Core;
    if lands {
        return Ok(None);
    }
    let judged = board::judge_ship(core, &project, &ship);
    let environment = judged.environment.clone();
    let why = judged.why.clone();
    let deploy = |by_person: bool| {
        environment.as_ref().map(|name| Deploy {
            agent_id: agent_id.to_owned(),
            project: project.clone(),
            environment: name.clone(),
            what: ship.what.clone(),
            why: match by_person {
                true => "You allowed it.".to_owned(),
                false => why.clone().unwrap_or_default(),
            },
            by_person,
        })
    };
    match judged.goes {
        Goes::Freely => return Ok(deploy(false)),
        Goes::OnceChecked => {
            let head = Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok();
            return match board::check_stands(core, &project, agent_id, head.as_deref()) {
                Ok(()) => Ok(deploy(false)),
                Err(why) => Err(format!(
                    "Not yet: `{}` lands in {}, which goes once the project's check passes, and {why}. Run it again once it has; the person needn't be asked.",
                    ship.what,
                    environment.as_deref().unwrap_or_default()
                )),
            };
        }
        Goes::Holds => {}
    }
    let mut said = format!(
        "Held for the person: `{}` {}, so Sikemux asked them first. Go on with other work; you will hear when they allow it, and then run exactly this again.",
        ship.what,
        leaves(ship.kind)
    );
    if let Some(why) = &judged.why {
        said = format!("{said} {why}");
    }
    let let_go = deploy(true);
    {
        let mut state = core.ship.state.locked();
        let same =
            |agent: &str, held: &Ship, at: &Path| agent == agent_id && *held == ship && at == cwd;
        let permits = |allowed: &Allowed| match &allowed.permit {
            Permit::Exactly {
                ship: held,
                cwd: at,
            } => same(&allowed.agent_id, held, at),
            Permit::Environment {
                project: there,
                name,
            } => {
                allowed.agent_id == agent_id
                    && *there == project
                    && environment.as_ref() == Some(name)
            }
        };
        if let Some(at) = state.allowed.iter().position(permits) {
            state.allowed.remove(at);
            return Ok(let_go);
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
            cwd: cwd.to_owned(),
            at: epoch_ms(),
            environment: judged.environment,
            why: judged.why,
            ship,
        });
    }
    core.changed(What::Agents);
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
/// this core runs, as the person's own shell does, is never held. Where the asker
/// `follows` the program to its end it hears a `went` to answer with; otherwise
/// what went is kept at once, as not seen to end.
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let Some(agent_id) = request.agent_id.clone() else {
        return Ok(json!({ "allowed": true }));
    };
    let kind: ShipKind = serde_json::from_value(request.params["kind"].clone())
        .map_err(|_| "kind must be what the action does")?;
    let what = (request.params["what"].as_str())
        .ok_or("what must be the action's command")?
        .to_owned();
    let target = serde_json::from_value::<Option<Target>>(request.params["target"].clone())
        .ok()
        .flatten();
    let follows = request.params["follows"] == true;
    let cwd = PathBuf::from(&request.project);
    let ship = Ship { kind, what, target };
    let checking = core.clone();
    let checked = tokio::task::spawn_blocking(move || check(&checking, &agent_id, ship, &cwd))
        .await
        .map_err(|error| error.to_string())?;
    Ok(match checked {
        Ok(None) => json!({ "allowed": true }),
        Ok(Some(deploy)) if follows => {
            let mut state = core.ship.state.locked();
            state.next += 1;
            let went = state.next;
            if state.going.len() >= MAX_GOING {
                state.going.pop_front();
            }
            state.going.push_back((went, deploy));
            json!({ "allowed": true, "went": went.to_string() })
        }
        Ok(Some(deploy)) => {
            let core = core.clone();
            tokio::spawn(async move { ended(core, deploy, None, String::new()).await });
            json!({ "allowed": true })
        }
        Err(said) => json!({ "allowed": false, "said": said }),
    })
}

/// How a deploy the shim followed ended, by the `went` the core gave it.
pub(crate) fn ran_call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let went: u64 = (request.params["went"].as_str())
        .and_then(|went| went.parse().ok())
        .ok_or("went must be what the check answered with")?;
    let deploy = {
        let mut state = core.ship.state.locked();
        let at = (state.going.iter())
            .position(|(id, deploy)| {
                *id == went && Some(&deploy.agent_id) == request.agent_id.as_ref()
            })
            .ok_or("no such deploy is going")?;
        state.going.remove(at).map(|(_, deploy)| deploy)
    };
    let exit = request.params["exit"].as_i64();
    let verdict = match exit {
        Some(0) => Verdict::Passed,
        _ => Verdict::Failed,
    };
    let output = exit.map_or_else(
        || "It was stopped before it ended.".to_owned(),
        |code| format!("It exited with {code}."),
    );
    if let Some(deploy) = deploy {
        let core = core.clone();
        tokio::spawn(async move { ended(core, deploy, Some(verdict), output).await });
    }
    Ok(json!({}))
}

/// Where the call of Sikemux tool `name` lands, from what the tool's own plugin
/// says. None when it can't say.
async fn tool_target(core: &Arc<Core>, name: &str, arguments: &Value) -> Option<Target> {
    match name {
        "rundeck_run" => {
            let place = plugins::call(core, RUNDECK, "agentJobPlace", arguments.clone())
                .await
                .ok()?;
            Some(Target::RundeckJob {
                path: place["path"].as_str()?.to_owned(),
                production: place["production"].as_bool()?,
            })
        }
        "vercel_promote" => Some(Target::Vercel { production: true }),
        "vercel_redeploy" => {
            let target = match arguments["target"].as_str() {
                Some(target) => target.to_owned(),
                None => {
                    let read = plugins::call(core, VERCEL, "deployment", arguments.clone())
                        .await
                        .ok()?;
                    read["target"].as_str()?.to_owned()
                }
            };
            match target.as_str() {
                "production" => Some(Target::Vercel { production: true }),
                "preview" => Some(Target::Vercel { production: false }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether an agent's call to the Sikemux tool `name` may run now, as
/// [`check`] says for what it does.
pub(crate) async fn check_tool(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    arguments: &Value,
) -> Result<Option<Deploy>, String> {
    let (Some(agent_id), Some(when)) = (&request.agent_id, crate::ship::of_tool(name)) else {
        return Ok(None);
    };
    let kind = match when {
        ToolShip::Always(kind) => kind,
        ToolShip::ProductionDatabase => {
            let asked = plugins::call(core, DATABASE, "agentWritesProduction", arguments.clone())
                .await
                .map_err(|failure| plugins::explain(&failure))?;
            if asked != true {
                return Ok(None);
            }
            ShipKind::ProductionWrite
        }
    };
    let what = format!("{name} {arguments}");
    let target = tool_target(core, name, arguments).await;
    let (checking, agent_id, cwd) = (core.clone(), agent_id.clone(), request.project.clone());
    tokio::task::spawn_blocking(move || {
        check(
            &checking,
            &agent_id,
            Ship { kind, what, target },
            Path::new(&cwd),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// How far a deploy tool's answer says it got.
enum Reached {
    Ended(Verdict),
    Going,
    /// The answer doesn't say.
    Unknown,
}

/// What a look at a deploy tool's run says of how far it got. Rundeck's status
/// can be the job's own word, so only one the core knows settles a verdict.
fn reached(tool: &str, answer: &Value) -> Reached {
    if tool.starts_with("rundeck_") {
        if answer["ended"].is_null() {
            return Reached::Going;
        }
        return match answer["status"].as_str() {
            Some("succeeded") => Reached::Ended(Verdict::Passed),
            Some("failed" | "aborted" | "timedout" | "failed-with-retry") => {
                Reached::Ended(Verdict::Failed)
            }
            _ => Reached::Unknown,
        };
    }
    match answer["state"]
        .as_str()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ready") => Reached::Ended(Verdict::Passed),
        Some("error" | "canceled") => Reached::Ended(Verdict::Failed),
        Some(_) => Reached::Going,
        None => Reached::Unknown,
    }
}

/// What to read again while a deploy tool's run is going.
fn read_again(tool: &str, answer: &Value) -> Option<(&'static str, Value)> {
    if tool.starts_with("rundeck_") {
        return Some((
            "rundeck_execution",
            json!({ "executionId": answer["id"].as_u64()?, "wait": true }),
        ));
    }
    Some((
        "vercel_deployment",
        json!({ "id": answer["id"].as_str()?, "team": answer["team"] }),
    ))
}

/// What a deploy tool's answer says happened, in a few lines.
fn answer_words(answer: &Value) -> String {
    let said = |field: &str| answer[field].as_str().map(str::to_owned);
    let mut lines: Vec<String> = [said("status"), said("state"), said("url"), said("error")]
        .into_iter()
        .flatten()
        .collect();
    if let Some(log) = answer["log"].as_array() {
        lines.extend(log.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    lines.join("\n")
}

/// A deploy tool let go toward an environment answered: it is followed to its
/// end and kept as evidence, without holding up the agent.
pub(crate) fn tool_ran(core: &Arc<Core>, deploy: Deploy, tool: &str, answer: Result<&Value, &str>) {
    let (core, tool) = (core.clone(), tool.to_owned());
    let answer = answer.cloned().map_err(str::to_owned);
    tokio::spawn(async move {
        let (verdict, output) = match answer {
            Err(error) => (Some(Verdict::Failed), error),
            Ok(answer) => followed(&core, &tool, answer).await,
        };
        ended(core, deploy, verdict, output).await;
    });
}

/// Reads a deploy tool's run until it ends, within `FOLLOW_LOOKS`. No verdict
/// when the core did not see the end.
async fn followed(core: &Arc<Core>, tool: &str, mut answer: Value) -> (Option<Verdict>, String) {
    let Some(Provided(plugins)) = core.config.plugins else {
        return (None, answer_words(&answer));
    };
    let mut reading = tool.to_owned();
    for _ in 0..FOLLOW_LOOKS {
        match reached(&reading, &answer) {
            Reached::Ended(verdict) => return (Some(verdict), answer_words(&answer)),
            Reached::Unknown => return (None, answer_words(&answer)),
            Reached::Going => {}
        }
        let Some((next, arguments)) = read_again(&reading, &answer) else {
            return (None, answer_words(&answer));
        };
        tokio::time::sleep(FOLLOW_EVERY).await;
        if let Ok(read) = plugins.call_agent_tool(next, arguments).await {
            answer = read;
            reading = next.to_owned();
        }
    }
    (None, answer_words(&answer))
}

/// Whether the project's health check for the environment answers, in a line.
async fn health_of(address: &str) -> (Verdict, String) {
    let client = crate::http::client();
    let mut said = format!("{address} did not answer");
    for attempt in 0..HEALTH_TRIES {
        if attempt > 0 {
            tokio::time::sleep(HEALTH_EVERY).await;
        }
        let Ok(client) = client else {
            break;
        };
        match client.get(address).timeout(HEALTH_TIMEOUT).send().await {
            Ok(response) if response.status().is_success() => {
                return (
                    Verdict::Passed,
                    format!("{address} answered {}", response.status()),
                );
            }
            Ok(response) => said = format!("{address} answered {}", response.status()),
            Err(error) => said = format!("{address} did not answer: {error}"),
        }
    }
    (Verdict::Failed, said)
}

/// A deploy reached its end: what ran, where and its result, with the project's
/// health check on one that went well, is kept on the agent's slice.
async fn ended(core: Arc<Core>, deploy: Deploy, verdict: Option<Verdict>, output: String) {
    let mut verdict = verdict;
    let mut lines = vec![deploy.why.clone(), output];
    if verdict.is_none() {
        lines.push("Sikemux did not see how it ended.".to_owned());
    }
    if verdict == Some(Verdict::Passed) {
        if let Some(address) = board::health_of(&core, &deploy.project, &deploy.environment) {
            let (health, said) = health_of(&address).await;
            verdict = Some(health);
            lines.push(said);
        }
    }
    let text: String = lines
        .iter()
        .filter(|line| !line.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let start = text.chars().count().saturating_sub(MAX_OUTPUT_CHARS);
    let ran = DeployRan {
        environment: deploy.environment,
        what: deploy.what,
        verdict,
        output: text.chars().skip(start).collect(),
        by_person: deploy.by_person,
    };
    core.repo_events.tell(RepoEvent::Deployed {
        project: deploy.project,
        agent: deploy.agent_id,
        ran,
    });
}

#[derive(Deserialize, Serialize)]
pub struct HoldParams {
    pub hold: String,
}

/// The person allows the held action once: the agent is told it may run it
/// again, which then runs until the end of the turn in which it hears this.
pub(crate) async fn allow(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    allow_hold(core, hold).await;
    Ok(())
}

async fn allow_hold(core: &Arc<Core>, hold: Hold) {
    core.ship.remember(&hold, true);
    let text = format!(
        "The person allowed `{}` once. Run exactly that again now; it may run once, until this turn ends.",
        hold.ship.what
    );
    let permit = Permit::Exactly {
        ship: hold.ship,
        cwd: hold.cwd,
    };
    permitted(core, hold.agent_id, permit, text).await;
}

/// Keeps what the person allowed and tells the agent, whose hearing of it in a
/// turn starts the time it holds for.
async fn permitted(core: &Arc<Core>, agent_id: String, permit: Permit, text: String) {
    core.ship.state.locked().allowed.push(Allowed {
        agent_id: agent_id.clone(),
        permit,
        heard: false,
    });
    let told = messages::for_person(core, agent_id.clone(), text).await;
    let heard =
        told.is_ok_and(|told| matches!(told["delivery"].as_str(), Some("steered" | "newTurn")));
    if heard {
        core.ship.turn_started(&agent_id);
    }
}

/// The person passed the gate of `environment` while `agent` waits on it: its
/// deploys held for there go once, and the gate passes when one has gone. False
/// when none is held, so the gate is the person's to pass by itself.
pub(crate) async fn allow_held(
    core: &Arc<Core>,
    agent: &str,
    project: &str,
    environment: &str,
) -> bool {
    let ids: Vec<u64> = (core.ship.state.locked().holds.iter())
        .filter(|hold| {
            hold.agent_id == agent
                && hold.project == project
                && hold.environment.as_deref() == Some(environment)
        })
        .map(|hold| hold.id)
        .collect();
    for id in &ids {
        if let Ok(hold) = take(core, &id.to_string()) {
            allow_hold(core, hold).await;
        }
    }
    !ids.is_empty()
}

/// The person passed the gate of `environment` with nothing held: the agent may
/// deploy there once.
pub(crate) async fn permit_environment(
    core: &Arc<Core>,
    agent: &str,
    project: &str,
    environment: &str,
) {
    let permit = Permit::Environment {
        project: project.to_owned(),
        name: environment.to_owned(),
    };
    let text = format!(
        "The person passed {environment}. Deploy there now if you haven't; one deploy may run, until this turn ends."
    );
    permitted(core, agent.to_owned(), permit, text).await;
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
    core.changed(What::Agents);
    Ok(hold)
}
